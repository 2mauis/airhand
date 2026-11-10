//! Phase 0 spike bench (do not merge, do not reuse in Phase 1).
//!
//! `p2p-bench host ...` and `p2p-bench ctrl ...` run in two network
//! namespaces. A Unix socket stands in for the relay's session control
//! stream for the DirectRequest / DirectOffer exchange; the real airhand
//! relay server (`--relay`) carries the baseline session.

mod ctrl;
mod helper;
mod host;
mod lane;
mod pipesock;
mod quic;

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use serde_json::Value;

pub fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
}

/// JSONL event log. Never receives the tailcat address or node keys.
#[derive(Clone)]
pub struct Log(Option<Arc<Mutex<std::fs::File>>>);

impl Log {
    pub fn open(path: Option<&String>) -> Result<Self> {
        Ok(Self(match path {
            Some(p) => Some(Arc::new(Mutex::new(std::fs::File::create(p)?))),
            None => None,
        }))
    }
    pub fn write(&self, mut v: Value) {
        if let Some(o) = v.as_object_mut() {
            o.insert("t_log_us".into(), now_us().into());
        }
        if let Some(f) = &self.0 {
            let mut f = f.lock().unwrap();
            let _ = writeln!(f, "{v}");
        }
    }
}

/// CPU ticks (utime + stime) consumed so far by a process, all threads.
pub fn cpu_ticks(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
}

pub fn clk_tck() -> u64 {
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as u64 }
}

pub struct Args {
    pub cmd: String,
    pub kv: HashMap<String, String>,
}

impl Args {
    pub fn get(&self, k: &str) -> Option<&String> {
        self.kv.get(k)
    }
    pub fn req(&self, k: &str) -> Result<&String> {
        match self.kv.get(k) {
            Some(v) => Ok(v),
            None => bail!("missing --{k}"),
        }
    }
    pub fn num<T: std::str::FromStr>(&self, k: &str, d: T) -> T {
        self.kv.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

fn parse_args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let Some(cmd) = it.next() else {
        bail!("usage: p2p-bench host|ctrl --key value ...")
    };
    let mut kv = HashMap::new();
    while let Some(k) = it.next() {
        let Some(k) = k.strip_prefix("--") else {
            bail!("unexpected {k}")
        };
        let v = it.next().unwrap_or_default();
        kv.insert(k.to_string(), v);
    }
    Ok(Args { cmd, kv })
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = parse_args()?;
    match args.cmd.as_str() {
        "host" => host::run(args).await,
        "ctrl" => ctrl::run(args).await,
        c => bail!("unknown command {c}"),
    }
}
