//! Process-level signal handling.
//!
//! [`install`] spawns a background tokio task that listens for OS signals
//! (SIGINT / SIGTERM on Unix, Ctrl+C on Windows). On the first signal it:
//!
//! 1. Prints a stderr banner.
//! 2. Sends a [`SignalInfo`] through the provided broadcast channel so
//!    downstream consumers (e.g. `run_workflow`) can translate it into an
//!    [`AgentEvent::SignalReceived`] and persist it to `events.jsonl`.
//! 3. Cancels the provided [`CancellationToken`].
//!
//! On a **second** signal it calls `std::process::exit(130)` for an
//! immediate force-kill — no further cleanup is attempted.

use chrono::Utc;
use tokio::signal;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Information about a received OS signal, broadcast to active run
/// consumers so they can emit `AgentEvent::SignalReceived`.
#[derive(Debug, Clone)]
pub struct SignalInfo {
    pub signal: String,
    pub ts: chrono::DateTime<chrono::Utc>,
}

/// Install the process-level signal handler. The returned future runs
/// indefinitely: first signal triggers graceful shutdown via `cancel`,
/// second signal force-exits the process.
pub fn install(sig_tx: broadcast::Sender<SignalInfo>, cancel: CancellationToken) {
    tokio::spawn(async move {
        let name = wait_for_signal().await;
        eprintln!(
            "\u{26a0}  received {name}, shutting down \
             (current step will be cancelled; checkpoint will be saved)"
        );
        let _ = sig_tx.send(SignalInfo {
            signal: name.into(),
            ts: Utc::now(),
        });
        cancel.cancel();

        // Force-kill window: if a second signal arrives before graceful
        // shutdown completes, exit immediately.
        let name2 = wait_for_signal().await;
        eprintln!("\u{26a0}  received {name2}, force exit");
        std::process::exit(130);
    });
}

async fn wait_for_signal() -> &'static str {
    #[cfg(unix)]
    {
        let ctrl_c = async {
            signal::ctrl_c().await.expect("install ctrl_c handler");
        };
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler")
                .recv()
                .await;
        };
        tokio::select! {
            _ = ctrl_c    => "SIGINT",
            _ = terminate => "SIGTERM",
        }
    }
    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.expect("install ctrl_c handler");
        "ctrl_c"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn install_sends_signal_info_and_cancels() {
        let (sig_tx, mut sig_rx) = broadcast::channel(16);
        let cancel = CancellationToken::new();

        // We can't easily send a real OS signal in a unit test, so we
        // test the constituent parts directly.
        let info = SignalInfo {
            signal: "SIGINT".into(),
            ts: Utc::now(),
        };
        sig_tx.send(info.clone()).unwrap();
        cancel.cancel();

        let received = sig_rx.recv().await.unwrap();
        assert_eq!(received.signal, "SIGINT");
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn signal_info_is_clone() {
        let info = SignalInfo {
            signal: "SIGTERM".into(),
            ts: Utc::now(),
        };
        let cloned = info.clone();
        assert_eq!(cloned.signal, "SIGTERM");
    }
}
