//! New-data acknowledgement signal, using only Quinn's public stream API.
//! No watchdog, helper IPC, or fallback policy lives here.

use quinn::Connection;
use std::{
    future::{Future, poll_fn},
    sync::{Arc, Mutex},
    task::Poll,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinSet};

pub const PERIOD: Duration = Duration::from_millis(250);
pub const MAX_IN_FLIGHT: usize = 16;
pub const PRIORITY: i32 = i32::MAX;

#[derive(Debug, Clone, Default)]
pub struct ProbeStats {
    pub sent: usize,
    pub skipped: usize,
    pub max_in_flight: usize,
    pub accepted: usize,
}

/// Dropping the source cancels its sender, accept loop, and acknowledgement waiters.
pub struct ProbeSource {
    pub progress: mpsc::Receiver<Instant>,
    pub sent: mpsc::Receiver<Instant>,
    pub stats: Arc<Mutex<ProbeStats>>,
    _tasks: JoinSet<()>,
}

impl ProbeSource {
    pub fn start(connection: Connection) -> Self {
        Self::with_priority(connection, PRIORITY)
    }

    /// Also permits the default-priority control used by the PV experiment.
    pub fn with_priority(connection: Connection, priority: i32) -> Self {
        let (tx, progress) = mpsc::channel(16);
        let (sent_tx, sent) = mpsc::channel(16);
        let stats = Arc::new(Mutex::new(ProbeStats::default()));
        let mut tasks = JoinSet::new();
        let rx_conn = connection.clone();
        let rx_stats = stats.clone();
        tasks.spawn(async move {
            while let Ok(mut stream) = rx_conn.accept_uni().await {
                if stream
                    .read_to_end(1)
                    .await
                    .is_ok_and(|data| data.len() == 1)
                {
                    rx_stats.lock().unwrap().accepted += 1;
                }
            }
        });
        let send_stats = stats.clone();
        tasks.spawn(async move {
            let mut pending = JoinSet::new();
            let mut ticks = tokio::time::interval(PERIOD);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = connection.closed() => break,
                    Some(result) = pending.join_next(), if !pending.is_empty() => {
                        let _ = result; // waiters publish progress independently of the sender
                    }
                    _ = ticks.tick() => {
                        if pending.len() >= MAX_IN_FLIGHT {
                            send_stats.lock().unwrap().skipped += 1;
                            continue;
                        }
                        let mut open = std::pin::pin!(connection.open_uni());
                        // Poll exactly once: exhausted stream credit must not block the cadence
                        // or prevent existing stopped() futures from reporting progress.
                        let ready = poll_fn(|cx| Poll::Ready(open.as_mut().poll(cx))).await;
                        let mut stream = match ready {
                            Poll::Ready(Ok(stream)) => stream,
                            Poll::Ready(Err(_)) => break,
                            Poll::Pending => {
                                send_stats.lock().unwrap().skipped += 1;
                                continue;
                            }
                        };
                        if stream.set_priority(priority).is_err() { break; }
                        let tx = tx.clone();
                        let stats = send_stats.clone();
                        let sent_tx = sent_tx.clone();
                        pending.spawn(async move {
                            if stream.write_all(&[0]).await.is_err() || stream.finish().is_err() { return; }
                            stats.lock().unwrap().sent += 1;
                            let _ = sent_tx.try_send(tokio::time::Instant::now().into_std());
                            if let Ok(None) = stream.stopped().await {
                                let _ = tx.try_send(tokio::time::Instant::now().into_std());
                            }
                        });
                        let mut stats = send_stats.lock().unwrap();
                        stats.max_in_flight = stats.max_in_flight.max(pending.len());
                    }
                }
            }
        });
        Self {
            progress,
            sent,
            stats,
            _tasks: tasks,
        }
    }
}
