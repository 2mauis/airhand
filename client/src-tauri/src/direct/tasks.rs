//! Tasks owned by the desktop service, including calls from the tray thread.
use std::future::Future;

pub(super) struct OwnedTask<T>(tauri::async_runtime::JoinHandle<T>);

impl<T: Send + 'static> OwnedTask<T> {
    pub(super) fn spawn(future: impl Future<Output = T> + Send + 'static) -> Self {
        Self(tauri::async_runtime::spawn(future))
    }

    pub(super) fn is_finished(&self) -> bool {
        self.0.inner().is_finished()
    }
}

impl<T> Drop for OwnedTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn tray_thread_can_spawn_and_cancel_without_entering_tokio() {
        std::thread::spawn(|| {
            assert!(tokio::runtime::Handle::try_current().is_err());
            let (started, ready) = mpsc::channel();
            let (dropped, cancelled) = mpsc::channel();
            struct OnDrop(mpsc::Sender<()>);
            impl Drop for OnDrop {
                fn drop(&mut self) {
                    let _ = self.0.send(());
                }
            }
            let task = OwnedTask::spawn(async move {
                let _guard = OnDrop(dropped);
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            });
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(!task.is_finished());
            drop(task);
            cancelled.recv_timeout(Duration::from_secs(2)).unwrap();
        })
        .join()
        .expect("tray callback must not panic");
    }
}
