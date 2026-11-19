//! Revocation is checked before reading another controller message.
use airhand_proto::session::{ByeReason, ControllerMsg};
use tokio::sync::{mpsc, watch};

pub(super) enum SessionEvent {
    Revoked(ByeReason),
    Message(Option<ControllerMsg>),
}

pub(super) async fn next_event(
    kick: &mut watch::Receiver<ByeReason>,
    incoming: &mut mpsc::Receiver<ControllerMsg>,
) -> SessionEvent {
    tokio::select! {
        biased;
        _ = kick.changed() => SessionEvent::Revoked(*kick.borrow_and_update()),
        message = incoming.recv() => SessionEvent::Message(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn revocation_wins_during_continuous_input_within_two_seconds() {
        let (input, mut incoming) = mpsc::channel(128);
        let (revoke, mut kick) = watch::channel(ByeReason::Kicked);
        for _ in 0..128 {
            input.try_send(ControllerMsg::Input(Vec::new())).unwrap();
        }
        // Keep the queue full while the real session selector consumes input.
        for _ in 0..1024 {
            assert!(matches!(
                next_event(&mut kick, &mut incoming).await,
                SessionEvent::Message(Some(ControllerMsg::Input(_)))
            ));
            input.try_send(ControllerMsg::Input(Vec::new())).unwrap();
        }
        revoke.send_replace(ByeReason::Kicked);
        let start = Instant::now();
        let event = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::task::unconstrained(next_event(&mut kick, &mut incoming)),
        )
        .await
        .expect("revocation starved by input");
        assert!(matches!(event, SessionEvent::Revoked(ByeReason::Kicked)));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(incoming.len(), 128, "input ran after local revocation");
    }
}
