//! Owned async task: dropping the owner cancels the task, including on error,
//! timeout and parent cancellation. A bare JoinHandle would detach instead.
use std::ops::{Deref, DerefMut};
use tokio::task::JoinHandle;

pub struct OwnedTask<T>(pub JoinHandle<T>);
impl<T> Drop for OwnedTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl<T> Deref for OwnedTask<T> {
    type Target = JoinHandle<T>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> DerefMut for OwnedTask<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn timeout_cancels_instead_of_detaching() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let mut task = OwnedTask(tokio::spawn(async move {
            let _tx = tx;
            std::future::pending::<()>().await;
        }));
        let _ = tokio::time::timeout(std::time::Duration::from_millis(1), &mut task.0).await;
        drop(task);
        assert!(rx.await.is_err());
    }
}
