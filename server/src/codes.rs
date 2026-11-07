//! 固定控制码的绑定表（见 `airhand_proto::signal` 模块说明的"固定控制码"），存在 SQLite 数据库里。
//!
//! 每个绑定记录控制码、密钥的哈希（不是密钥本身：数据库泄露了也取不回控制码）和最后一次在线的时间。
//! 数据库默认和证书放在一起（`/data/airhand.db`），服务器重启后照样有效。每次修改立即写入（WAL 模式），
//! 服务器被强行结束也不会丢。
//!
//! 数据库打不开时（例如目录没有写权限）退回到只在内存里记，服务器照常工作，只是重启后忘掉这些绑定；
//! 运行中读写出错时按"没有绑定"处理（和没有这个功能时一样：控制码空闲就还给来取的被控端）。

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

/// 这么久没有上线的绑定会被清理，控制码可以再分给别人
pub const BINDING_TTL: Duration = Duration::from_secs(180 * 24 * 3600);

/// 数据库结构的版本（`PRAGMA user_version`）。修改结构时加 1，并在 [`migrate`] 里补上升级步骤
const SCHEMA_VERSION: i64 = 1;

pub struct Bindings {
    db: Connection,
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
        std::fs::create_dir_all(dir).ok();
    }
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    // 更新的服务器建的数据库：什么都不改
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        bail!("the database was created by a newer airhand-server (schema version {version})");
    }
    // WAL：写入不阻塞读取，也不用每次提交都刷盘（NORMAL 下断电最多丢最后几次提交，数据库不会损坏）
    let mode: String = db.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        warn!("{}: journal mode is {mode}, not WAL", path.display());
    }
    db.execute_batch("PRAGMA synchronous = NORMAL;")?;
    migrate(&db)?;
    Ok(db)
}

/// 建表 / 升级到 [`SCHEMA_VERSION`]
fn migrate(db: &Connection) -> Result<()> {
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        db.execute_batch(
            "BEGIN;
             CREATE TABLE fixed_codes (
                 code        INTEGER PRIMARY KEY,  -- 6 位控制码
                 secret_hash BLOB    NOT NULL,     -- SHA-256(\"airhand code binding v1\\0\" || 密钥)
                 last_seen   INTEGER NOT NULL      -- 最后在线的 Unix 时间（秒）
             );
             CREATE INDEX fixed_codes_last_seen ON fixed_codes (last_seen);
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    Ok(())
}

fn memory() -> Connection {
    let db = Connection::open_in_memory().expect("SQLite in-memory database");
    migrate(&db).expect("in-memory schema");
    db
}

/// 运行中的数据库错误：记日志，按 `fallback` 继续
fn or_log<T>(r: rusqlite::Result<T>, what: &str, fallback: T) -> T {
    r.unwrap_or_else(|e| {
        warn!("fixed control codes: can't {what}: {e}");
        fallback
    })
}

impl Bindings {
    /// 打开（不存在时创建）数据库；`None` 表示只在内存里（测试用）
    pub fn open(path: Option<&Path>) -> Self {
        let db = match path {
            Some(p) => match open(p).with_context(|| format!("can't open {}", p.display())) {
                Ok(db) => {
                    let b = Self { db };
                    info!("{} fixed control code(s) in {}", b.len(), p.display());
                    return b;
                }
                Err(e) => {
                    error!(
                        "{e:#}; fixed control codes are only kept in memory until the server restarts"
                    );
                    memory()
                }
            },
            None => memory(),
        };
        Self { db }
    }

    pub fn len(&self) -> usize {
        let r = self
            .db
            .query_row("SELECT COUNT(*) FROM fixed_codes", [], |r| {
                r.get::<_, i64>(0)
            });
        or_log(r, "count", 0) as usize
    }

    fn hash(&self, code: u32) -> Option<Vec<u8>> {
        let r = self
            .db
            .prepare_cached("SELECT secret_hash FROM fixed_codes WHERE code = ?1")
            .and_then(|mut s| s.query_row([code], |r| r.get(0)).optional());
        or_log(r, "look up a code", None)
    }

    pub fn contains(&self, code: u32) -> bool {
        self.hash(code).is_some()
    }

    /// `Some(true)`：绑定给了持有这个密钥的被控端；`Some(false)`：绑定给了别人；`None`：没有绑定
    pub fn check(&self, code: u32, secret: &[u8; 16]) -> Option<bool> {
        self.hash(code)
            .map(|h| h.as_slice() == secret_hash(secret).as_slice())
    }

    /// 绑定（或者刷新最后在线时间）
    pub fn bind(&mut self, code: u32, secret: &[u8; 16]) {
        let r = self
            .db
            .prepare_cached(
                "INSERT INTO fixed_codes (code, secret_hash, last_seen) VALUES (?1, ?2, ?3)
                 ON CONFLICT (code) DO UPDATE SET secret_hash = excluded.secret_hash, last_seen = excluded.last_seen",
            )
            .and_then(|mut s| s.execute(params![code, secret_hash(secret).as_slice(), now()]));
        or_log(r, "save a code", 0);
    }

    /// 刷新最后在线时间（被控端断开时）
    pub fn touch(&mut self, code: u32) {
        let r = self
            .db
            .prepare_cached("UPDATE fixed_codes SET last_seen = ?2 WHERE code = ?1")
            .and_then(|mut s| s.execute(params![code, now()]));
        or_log(r, "update a code", 0);
    }

    pub fn remove(&mut self, code: u32) -> bool {
        let r = self
            .db
            .prepare_cached("DELETE FROM fixed_codes WHERE code = ?1")
            .and_then(|mut s| s.execute([code]));
        or_log(r, "remove a code", 0) > 0
    }

    /// 清理很久没有上线的绑定（`online` 的控制码正在线上，跳过）
    pub fn expire(&mut self, online: impl Fn(u32) -> bool) {
        let limit = now() - BINDING_TTL.as_secs() as i64;
        let r = (|| -> rusqlite::Result<usize> {
            let tx = self.db.transaction()?;
            let old: Vec<u32> = tx
                .prepare_cached("SELECT code FROM fixed_codes WHERE last_seen < ?1")?
                .query_map([limit], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut n = 0;
            for code in old.into_iter().filter(|c| !online(*c)) {
                n += tx.execute("DELETE FROM fixed_codes WHERE code = ?1", [code])?;
            }
            tx.commit()?;
            Ok(n)
        })();
        let n = or_log(r, "expire old codes", 0);
        if n > 0 {
            info!("{n} fixed control code(s) expired");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_expiry() {
        let dir =
            std::env::temp_dir().join(format!("airhand-codes-test-{:016x}", rand::random::<u64>()));
        let path = dir.join("airhand.db");
        let mut b = Bindings::open(Some(&path));
        b.bind(123_456, &[1; 16]);
        b.bind(654_321, &[2; 16]);
        b.db.execute(
            "UPDATE fixed_codes SET last_seen = 1 WHERE code = 654321",
            [],
        )
        .unwrap();
        drop(b);

        // 重新打开（服务器重启）
        let mut again = Bindings::open(Some(&path));
        assert_eq!(again.len(), 2);
        assert_eq!(again.check(123_456, &[1; 16]), Some(true));
        assert_eq!(again.check(123_456, &[2; 16]), Some(false));
        assert_eq!(again.check(111_111, &[1; 16]), None);

        // 过期的绑定被清理，正在线上的不清理
        again.expire(|c| c == 999_999);
        assert!(again.contains(123_456) && !again.contains(654_321));
        again.bind(999_999, &[3; 16]);
        again
            .db
            .execute(
                "UPDATE fixed_codes SET last_seen = 1 WHERE code = 999999",
                [],
            )
            .unwrap();
        again.expire(|c| c == 999_999);
        assert!(again.contains(999_999));

        // 重新绑定（换了密钥）、删除
        again.bind(999_999, &[4; 16]);
        assert_eq!(again.check(999_999, &[4; 16]), Some(true));
        assert!(again.remove(123_456));
        assert!(!again.remove(123_456));
        drop(again);
        let last = Bindings::open(Some(&path));
        assert!(!last.contains(123_456) && last.contains(999_999));
        drop(last);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 更新的服务器建的数据库：不碰它，只在内存里记
    #[test]
    fn newer_schema_is_left_alone() {
        let dir =
            std::env::temp_dir().join(format!("airhand-codes-test-{:016x}", rand::random::<u64>()));
        let path = dir.join("airhand.db");
        std::fs::create_dir_all(&dir).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch("PRAGMA user_version = 99;")
            .unwrap();
        let mut b = Bindings::open(Some(&path));
        b.bind(123_456, &[1; 16]);
        assert!(b.contains(123_456));
        drop(b);
        let v: i64 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 99);
        let _ = std::fs::remove_dir_all(dir);
    }
}
