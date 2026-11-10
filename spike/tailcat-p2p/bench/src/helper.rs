//! Supervises one helper process (one epoch): JSON-line control on
//! stdin/stdout, and for candidate (ii) two extra inherited pipes on fds 3/4.

use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, Mutex as AMutex};

use crate::Log;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chan {
    Udp,
    Pipe,
}

pub struct Helper {
    pub child: Child,
    pub pid: u32,
    stdin: AMutex<ChildStdin>,
    pub events: broadcast::Sender<Value>,
    /// Every event so far (secrets stripped).
    pub history: Arc<Mutex<Vec<Value>>>,
    /// Candidate (ii): our ends of the datagram pipes.
    pub pipes: Option<(File, File)>,
    /// Candidate (i): the helper's loopback port.
    pub chan_port: Option<u16>,
    pub offer: Option<String>,
    pub client_key: Option<String>,
    first: Option<tokio::sync::mpsc::UnboundedReceiver<Value>>,
}

fn helper_path() -> std::path::PathBuf {
    std::env::var_os("P2P_HELPER")
        .map(Into::into)
        .unwrap_or_else(|| "airhand-p2p-spike".into())
}

impl Helper {
    pub async fn spawn(role: &str, chan: Chan, log: Log) -> Result<Self> {
        let mut cmd = Command::new(helper_path());
        cmd.arg(role)
            .arg(match chan {
                Chan::Udp => "udp",
                Chan::Pipe => "pipe",
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(match std::env::var_os("P2P_HELPER_STDERR_DIR") {
                Some(d) => Stdio::from(std::fs::File::create(
                    std::path::Path::new(&d).join(format!("helper-{role}-{}.stderr", crate::now_us())),
                )?),
                None => Stdio::null(),
            })
            .kill_on_drop(true);
        let mut ours = None;
        if chan == Chan::Pipe {
            let (to_r, to_w) = std::io::pipe()?;
            let (from_r, from_w) = std::io::pipe()?;
            let child_in: OwnedFd = to_r.into();
            let child_out: OwnedFd = from_w.into();
            let (ci, co) = (child_in.as_raw_fd(), child_out.as_raw_fd());
            unsafe {
                cmd.pre_exec(move || {
                    if libc::dup2(ci, 3) < 0 || libc::dup2(co, 4) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            ours = Some((File::from(OwnedFd::from(to_w)), File::from(OwnedFd::from(from_r)), child_in, child_out));
        }
        let mut child = cmd.spawn().context("spawning helper")?;
        let pid = child.id().unwrap_or(0);
        let pipes = ours.map(|(w, r, ci, co)| {
            drop(ci);
            drop(co);
            (w, r)
        });
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, _) = broadcast::channel(1024);
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut h = Self {
            child,
            pid,
            stdin: AMutex::new(stdin),
            events: tx.clone(),
            history: history.clone(),
            pipes,
            chan_port: None,
            offer: None,
            client_key: None,
            first: None,
        };
        let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let Ok(mut v) = serde_json::from_str::<Value>(&l) else { continue };
                let ev = v["ev"].as_str().unwrap_or("").to_string();
                if ev == "offer" || ev == "key" {
                    let _ = first_tx.send(v.clone());
                    if let Some(o) = v.as_object_mut() {
                        o.remove("addr");
                        o.remove("client_key");
                    }
                }
                if ev == "chan" {
                    let _ = first_tx.send(v.clone());
                }
                log.write(json!({"src": "helper", "event": v.clone()}));
                history.lock().unwrap().push(v.clone());
                let _ = tx.send(v);
            }
        });
        if chan == Chan::Udp {
            let v = tokio::time::timeout(Duration::from_secs(5), first_rx.recv())
                .await?
                .ok_or_else(|| anyhow!("helper exited"))?;
            h.chan_port = v["port"].as_u64().map(|p| p as u16);
        }
        h.first = Some(first_rx);
        Ok(h)
    }

    pub async fn send(&self, v: Value) -> Result<()> {
        let mut s = self.stdin.lock().await;
        let mut b = serde_json::to_vec(&v)?;
        b.push(b'\n');
        s.write_all(&b).await?;
        s.flush().await?;
        Ok(())
    }

    async fn next_secret(&mut self, want: &str) -> Result<Value> {
        let rx = self.first.as_mut().unwrap();
        loop {
            let v = tokio::time::timeout(Duration::from_secs(30), rx.recv())
                .await
                .context("timed out waiting for helper")?
                .ok_or_else(|| anyhow!("helper exited"))?;
            if v["ev"] == want {
                return Ok(v);
            }
        }
    }

    /// Host: start a tailcat Server allowing only `client_key`.
    pub async fn serve(&mut self, client_key: &str, port: u16, region: &Value, poll_ms: u64) -> Result<String> {
        let mut errs = self.events.subscribe();
        self.send(json!({"op": "serve", "client_key": client_key, "port": port, "region": region, "poll_ms": poll_ms}))
            .await?;
        tokio::select! {
            v = self.next_secret("offer") => {
                let addr = v?["addr"].as_str().unwrap_or_default().to_string();
                self.offer = Some(addr.clone());
                Ok(addr)
            }
            e = wait_error(&mut errs) => bail!("helper: {e}"),
        }
    }

    /// Controller: create the client key ("key first").
    pub async fn init(&mut self, port: u16, ping_ms: u64, misses: u32) -> Result<String> {
        self.send(json!({"op": "init", "port": port, "ping_ms": ping_ms, "misses": misses}))
            .await?;
        let v = self.next_secret("key").await?;
        let k = v["client_key"].as_str().unwrap_or_default().to_string();
        self.client_key = Some(k.clone());
        Ok(k)
    }

    pub async fn dial(&self, addr: &str) -> Result<()> {
        self.send(json!({"op": "dial", "addr": addr})).await
    }

    pub async fn set_peer(&self, port: u16) -> Result<()> {
        self.send(json!({"op": "peer", "peer_port": port})).await
    }

    /// Waits until the helper reports a direct path; returns the event.
    pub async fn wait_direct(&self, within: Duration) -> Result<Value> {
        if let Some(v) = self.history.lock().unwrap().iter().rev().find(|v| v["ev"] == "path") {
            if v["path"] == "direct" {
                return Ok(v.clone());
            }
        }
        let mut rx = self.events.subscribe();
        tokio::time::timeout(within, async {
            loop {
                let v = rx.recv().await?;
                if v["ev"] == "path" && v["path"] == "direct" {
                    return Ok(v);
                }
                if v["ev"] == "dial_failed" || v["ev"] == "error" {
                    bail!("helper: {}", v["error"]);
                }
            }
        })
        .await
        .context("no direct path in time")?
    }

    pub async fn stats(&self) -> Result<Value> {
        let mut rx = self.events.subscribe();
        self.send(json!({"op": "stats"})).await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let v = rx.recv().await?;
                if v["ev"] == "stats" {
                    return Ok(v);
                }
            }
        })
        .await?
    }

    pub async fn close(mut self) {
        let _ = self.send(json!({"op": "close"})).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        let _ = self.child.kill().await;
    }
}

async fn wait_error(rx: &mut broadcast::Receiver<Value>) -> String {
    loop {
        match rx.recv().await {
            Ok(v) if v["ev"] == "error" => return v["error"].to_string(),
            Ok(_) => {}
            Err(_) => std::future::pending::<()>().await,
        }
    }
}
