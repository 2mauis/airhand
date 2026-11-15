//! Sidecar ownership: private stdin configuration, bounded category-only events,
//! no inherited stderr. Dropping the owner closes stdin and reaps the process.
use airhand_link::direct::Secret;
use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

#[derive(Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub count: u64,
    pub kind: String,
    #[serde(default)]
    pub public: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub certificate: Vec<u8>,
    #[serde(default)]
    pub path: String,
}

pub type Counters = Arc<Mutex<std::collections::BTreeMap<String, u64>>>;

pub struct Helper {
    pub counters: Counters,
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    pub path: watch::Receiver<bool>,
    replies: mpsc::Receiver<Reply>,
}
impl Helper {
    pub fn spawn(program: &Path) -> Result<Self> {
        let mut child = Command::new(program)
            .arg("--control")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| anyhow!("helper unavailable"))?;
        let counters = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        let observed = counters.clone();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("helper pipe unavailable"))?;
        let stdin = child.stdin.take();
        let child = Arc::new(Mutex::new(child));
        let (path_tx, path) = watch::channel(false);
        let (reply_tx, replies) = mpsc::channel(4);
        let events_at = Arc::new(Mutex::new(Instant::now()));
        let clock = events_at.clone();
        let ended = path_tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                // Read at most 64 KiB, including newline; never log rejected bytes.
                while line.len() < 65536 {
                    match reader.fill_buf() {
                        Ok([]) | Err(_) => break,
                        Ok(buf) => {
                            let n = buf
                                .iter()
                                .position(|b| *b == b'\n')
                                .map_or(buf.len(), |n| n + 1)
                                .min(65536 - line.len());
                            line.extend_from_slice(&buf[..n]);
                            reader.consume(n);
                            if line.last() == Some(&b'\n') {
                                break;
                            }
                        }
                    }
                }
                if line.last() != Some(&b'\n') {
                    break;
                }
                let Ok(reply) = serde_json::from_slice::<Reply>(&line) else {
                    break;
                };
                *clock.lock().unwrap() = Instant::now();
                match reply.kind.as_str() {
                    "prepared" => {
                        if reply_tx.blocking_send(reply).is_err() {
                            break;
                        }
                    }
                    "path" => {
                        if reply.path != "none" && reply.path != "direct" {
                            break;
                        };
                        ended.send_replace(reply.path == "direct");
                    }
                    "gate_closed" | "error" => {
                        ended.send_replace(false);
                        break;
                    }
                    "to_tunnel"
                    | "from_tunnel"
                    | "magicsock_send_data_derp"
                    | "magicsock_recv_data_derp" => {
                        observed.lock().unwrap().insert(reply.kind, reply.count);
                    }
                    _ => break,
                }
            }
            ended.send_replace(false);
        });
        let process = child.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let exited = process
                    .lock()
                    .unwrap()
                    .try_wait()
                    .map_or(true, |s| s.is_some());
                if exited
                    || (*path_tx.borrow()
                        && events_at.lock().unwrap().elapsed() > Duration::from_secs(2))
                {
                    path_tx.send_replace(false);
                    if !exited {
                        let _ = process.lock().unwrap().kill();
                    }
                    break;
                }
            }
        });
        Ok(Self {
            counters,
            child,
            stdin,
            path,
            replies,
        })
    }
    pub fn send(&mut self, value: &Secret<serde_json::Value>) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("helper pipe closed"))?;
        // All callers bound the message size. No JSON or OS error escapes this layer.
        let mut data = serde_json::to_vec(value.expose())
            .map_err(|_| anyhow!("invalid helper configuration"))?;
        data.push(b'\n');
        stdin
            .write_all(&data)
            .map_err(|_| anyhow!("helper configuration failed"))
    }
    pub async fn prepared(&mut self) -> Result<Reply> {
        tokio::time::timeout(Duration::from_secs(20), self.replies.recv())
            .await
            .ok()
            .flatten()
            .ok_or_else(|| anyhow!("helper preparation failed"))
    }
    pub async fn wait_direct(&mut self) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if *self.path.borrow_and_update() {
                    return Ok(());
                };
                if self.path.changed().await.is_err() {
                    bail!("helper ended")
                }
            }
        })
        .await
        .map_err(|_| anyhow!("direct path unavailable"))?
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let process = self.child.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            loop {
                let mut child = process.lock().unwrap();
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                if start.elapsed() >= Duration::from_millis(800) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                drop(child);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires a built helper via AIRHAND_HELPER_TEST_BINARY"]
    async fn supervisor_passes_secrets_only_over_stdin_and_reaps_on_drop() {
        let binary =
            std::env::var_os("AIRHAND_HELPER_TEST_BINARY").expect("select the built helper");
        let mut helper = Helper::spawn(Path::new(&binary)).unwrap();
        let node_seed: [u8; 32] = rand::random();
        let tls_seed: [u8; 32] = rand::random();
        let private = format!(
            "privkey:{}",
            node_seed
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        helper.send(&Secret::new(serde_json::json!({"cmd":"prepare","mode":"controller","node_private":private,"tls_seed":tls_seed}))).unwrap();
        let prepared = helper.prepared().await.unwrap();
        assert_eq!(prepared.kind, "prepared");
        assert!(!prepared.certificate.is_empty());
        let child = helper.child.clone();
        #[cfg(target_os = "linux")]
        {
            let pid = child.lock().unwrap().id();
            let argv = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
            assert!(!argv.windows(private.len()).any(|b| b == private.as_bytes()));
            assert!(argv.windows(9).any(|b| b == b"--control"));
        }
        drop(helper);
        let started = Instant::now();
        loop {
            if child.lock().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "helper ownership exceeded exit bound"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
