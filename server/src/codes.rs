//! Fixed control codes: authoritative in-memory index plus a dedicated SQLite
//! writer. Broker locks never cover disk I/O. Writes are coalesced per code (the
//! namespace has 900,000 entries), retried on failure, and drained on shutdown.
//! A disk error never makes an existing binding appear unowned.
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

pub const BINDING_TTL: Duration = Duration::from_secs(180 * 24 * 3600);
const SCHEMA_VERSION: i64 = 1;
#[derive(Clone)]
struct Binding {
    hash: Vec<u8>,
    last_seen: i64,
}
#[derive(Default)]
struct Writes {
    dirty: HashMap<u32, Option<Binding>>,
    writing: bool,
    stop: bool,
}
#[derive(Default)]
struct Store {
    state: Mutex<Writes>,
    cv: Condvar,
}
pub struct Bindings {
    entries: HashMap<u32, Binding>,
    store: Option<Arc<Store>>,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
fn secret_hash(secret: &[u8; 16]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"airhand code binding v1\0");
    h.update(secret);
    h.finalize().into()
}
fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        bail!("database schema {version} is newer than this server");
    }
    db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    if version < 1 {
        db.execute_batch("BEGIN;
            CREATE TABLE fixed_codes (code INTEGER PRIMARY KEY, secret_hash BLOB NOT NULL, last_seen INTEGER NOT NULL);
            CREATE INDEX fixed_codes_last_seen ON fixed_codes (last_seen);
            PRAGMA user_version = 1; COMMIT;")?;
    }
    Ok(db)
}
fn load(db: &Connection) -> Result<HashMap<u32, Binding>> {
    let mut stmt = db.prepare("SELECT code, secret_hash, last_seen FROM fixed_codes")?;
    Ok(stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                Binding {
                    hash: r.get(1)?,
                    last_seen: r.get(2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?)
}
fn writer(mut db: Connection, store: Arc<Store>) {
    loop {
        let batch = {
            let mut s = store.state.lock().unwrap();
            while s.dirty.is_empty() && !s.stop {
                s = store.cv.wait(s).unwrap();
            }
            if s.dirty.is_empty() && s.stop {
                break;
            }
            s.writing = true;
            std::mem::take(&mut s.dirty)
        };
        let result = (|| -> rusqlite::Result<()> {
            let tx = db.transaction()?;
            for (code, binding) in &batch {
                match binding {
                    Some(b) => {
                        tx.execute(
                        "INSERT INTO fixed_codes (code, secret_hash, last_seen) VALUES (?1, ?2, ?3)
                         ON CONFLICT (code) DO UPDATE SET secret_hash = excluded.secret_hash, last_seen = excluded.last_seen",
                        params![code, &b.hash, b.last_seen])?;
                    }
                    None => {
                        tx.execute("DELETE FROM fixed_codes WHERE code = ?1", [code])?;
                    }
                }
            }
            tx.commit()
        })();
        let failed = result.is_err();
        let mut s = store.state.lock().unwrap();
        s.writing = false;
        if let Err(e) = result {
            warn!("fixed control codes: persistence failed, retaining bindings and retrying: {e}");
            if s.stop {
                store.cv.notify_all();
                break;
            }
            for (code, b) in batch {
                s.dirty.entry(code).or_insert(b);
            }
        }
        store.cv.notify_all();
        drop(s);
        if failed {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}
impl Bindings {
    pub fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self {
                entries: HashMap::new(),
                store: None,
            };
        };
        let loaded = open(path)
            .and_then(|db| Ok((load(&db)?, db)))
            .with_context(|| format!("can't open {}", path.display()));
        match loaded {
            Ok((entries, db)) => {
                info!(
                    "{} fixed control code(s) in {}",
                    entries.len(),
                    path.display()
                );
                let store = Arc::new(Store::default());
                let worker = store.clone();
                std::thread::Builder::new()
                    .name("airhand-codes".into())
                    .spawn(move || writer(db, worker))
                    .expect("spawn code persistence worker");
                Self {
                    entries,
                    store: Some(store),
                }
            }
            Err(e) => {
                error!("{e:#}; fixed codes are only kept in memory until restart");
                Self {
                    entries: HashMap::new(),
                    store: None,
                }
            }
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn contains(&self, code: u32) -> bool {
        self.entries.contains_key(&code)
    }
    pub fn check(&self, code: u32, secret: &[u8; 16]) -> Option<bool> {
        self.entries
            .get(&code)
            .map(|b| b.hash.as_slice() == secret_hash(secret))
    }
    fn persist(&self, code: u32) {
        if let Some(store) = &self.store {
            store
                .state
                .lock()
                .unwrap()
                .dirty
                .insert(code, self.entries.get(&code).cloned());
            store.cv.notify_one();
        }
    }
    pub fn bind(&mut self, code: u32, secret: &[u8; 16]) {
        self.entries.insert(
            code,
            Binding {
                hash: secret_hash(secret).to_vec(),
                last_seen: now(),
            },
        );
        self.persist(code);
    }
    pub fn touch(&mut self, code: u32) {
        if let Some(b) = self.entries.get_mut(&code) {
            b.last_seen = now();
            self.persist(code);
        }
    }
    pub fn remove(&mut self, code: u32) -> bool {
        let removed = self.entries.remove(&code).is_some();
        if removed {
            self.persist(code);
        }
        removed
    }
    pub fn expire(&mut self, online: impl Fn(u32) -> bool) {
        let limit = now() - BINDING_TTL.as_secs() as i64;
        let old: Vec<_> = self
            .entries
            .iter()
            .filter(|(code, b)| b.last_seen < limit && !online(**code))
            .map(|(code, _)| *code)
            .collect();
        for code in old {
            self.remove(code);
        }
    }
    /// Clone a barrier while holding the broker lock, then wait OUTSIDE it on
    /// a blocking worker. Used at orderly shutdown (and persistence tests).
    pub fn flush_barrier(&self) -> impl FnOnce(Duration) -> bool + Send + 'static {
        let store = self.store.clone();
        move |timeout| {
            let Some(store) = store else {
                return true;
            };
            let deadline = Instant::now() + timeout;
            let mut s = store.state.lock().unwrap();
            while s.writing || !s.dirty.is_empty() {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return false;
                };
                s = store.cv.wait_timeout(s, left).unwrap().0;
            }
            true
        }
    }
}
impl Drop for Bindings {
    fn drop(&mut self) {
        if let Some(store) = &self.store {
            store.state.lock().unwrap().stop = true;
            store.cv.notify_one();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("airhand-codes-test-{:016x}", rand::random::<u64>()))
            .join("airhand.db")
    }
    #[test]
    fn roundtrip_and_expiry() {
        let path = temp();
        let mut b = Bindings::open(Some(&path));
        b.bind(123456, &[1; 16]);
        b.bind(654321, &[2; 16]);
        b.entries.get_mut(&654321).unwrap().last_seen = 1;
        b.persist(654321);
        assert!(b.flush_barrier()(Duration::from_secs(5)));
        drop(b);
        let mut b = Bindings::open(Some(&path));
        assert_eq!(b.len(), 2);
        assert_eq!(b.check(123456, &[1; 16]), Some(true));
        assert_eq!(b.check(123456, &[2; 16]), Some(false));
        b.expire(|_| false);
        assert!(!b.contains(654321));
        b.remove(123456);
        b.bind(123456, &[3; 16]); // latest write wins, including remove/rebind
        assert!(b.flush_barrier()(Duration::from_secs(5)));
        let again = Bindings::open(Some(&path));
        assert_eq!(again.check(123456, &[3; 16]), Some(true));
        assert_eq!(again.len(), 1);
    }
    #[test]
    fn locked_database_does_not_block_broker_or_lose_ownership() {
        let path = temp();
        let mut b = Bindings::open(Some(&path));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        let start = Instant::now();
        b.bind(123456, &[1; 16]);
        assert_eq!(b.check(123456, &[2; 16]), Some(false));
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(!b.flush_barrier()(Duration::from_millis(20)));
        db.execute_batch("ROLLBACK").unwrap();
        assert!(b.flush_barrier()(Duration::from_secs(6)));
    }
    #[test]
    fn newer_schema_is_left_alone() {
        let path = temp();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch("PRAGMA user_version = 99")
            .unwrap();
        let mut b = Bindings::open(Some(&path));
        b.bind(123456, &[1; 16]);
        assert!(b.contains(123456));
        let v: i64 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 99);
    }
}
