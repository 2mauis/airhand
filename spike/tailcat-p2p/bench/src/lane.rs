//! JSON-line messaging on a QUIC stream, and a paced bulk writer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use quinn::{RecvStream, SendStream};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, mpsc};

use crate::now_us;

#[derive(Clone)]
pub struct Tx(pub Arc<Mutex<SendStream>>);

impl Tx {
    pub fn new(s: SendStream) -> Self {
        Self(Arc::new(Mutex::new(s)))
    }
    pub async fn send(&self, v: &Value) -> Result<()> {
        let mut b = serde_json::to_vec(v)?;
        b.push(b'\n');
        self.0.lock().await.write_all(&b).await?;
        Ok(())
    }
}

/// Reads JSON lines; each message is stamped with its arrival time (µs).
pub fn reader(recv: RecvStream) -> mpsc::Receiver<(i64, Value)> {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(async move {
        let mut lines = BufReader::new(recv).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let t = now_us();
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                if tx.send((t, v)).await.is_err() {
                    break;
                }
            }
        }
    });
    rx
}

/// Writes `mbps` for `secs` in 16 KiB chunks, paced on a 2 ms tick. A
/// backlog larger than 100 ms is dropped (as a real-time encoder would skip
/// frames) and counted. Returns (bytes written, bytes skipped).
pub async fn paced_write(s: &mut SendStream, mbps: f64, secs: f64) -> Result<(u64, u64)> {
    const CHUNK: usize = 16 * 1024;
    let chunk = bytes::Bytes::from(vec![0xa5u8; CHUNK]);
    let rate = mbps * 1e6 / 8.0;
    let start = Instant::now();
    let end = start + Duration::from_secs_f64(secs);
    let mut sent: u64 = 0;
    let mut skipped: u64 = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while Instant::now() < end {
        tick.tick().await;
        let target = (rate * start.elapsed().as_secs_f64()) as u64;
        let backlog = target.saturating_sub(sent + skipped);
        let cap = (rate * 0.1) as u64;
        if backlog > cap {
            skipped += backlog - cap;
        }
        while sent + skipped + (CHUNK as u64) <= target {
            s.write_chunk(chunk.clone()).await?;
            sent += CHUNK as u64;
        }
    }
    Ok((sent, skipped))
}
