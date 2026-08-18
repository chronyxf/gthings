use crate::connection::{Connection, call_async};
use crate::session::Session;
use crate::session::navigate::{RecvOutcome, recv_event};
use tokio::task::JoinHandle;

impl Session {
    /// Spawn a background task that auto-accepts JavaScript dialogs
    /// (`alert`, `confirm`, `prompt`, `beforeunload`) by listening for
    /// `Page.javascriptDialogOpening` events and immediately calling
    /// `Page.handleJavaScriptDialog` with `accept: true`.
    pub(crate) fn spawn_dialog_handler(conn: &Connection) -> JoinHandle<()> {
        let mut rx = conn.event_rx();
        let write = conn.write_tx();
        tokio::spawn(async move {
            loop {
                match recv_event(&mut rx).await {
                    RecvOutcome::Event(event) if event.method == "Page.javascriptDialogOpening" => {
                        tracing::debug!(
                            "Auto-accepting dialog: type={:?}, message={:?}",
                            event.params.get("type"),
                            event.params.get("message"),
                        );
                        call_async(
                            &write,
                            "Page.handleJavaScriptDialog",
                            serde_json::json!({"accept": true}),
                            event.session_id,
                        );
                    }
                    RecvOutcome::Event(_) => continue,
                    RecvOutcome::Closed => {
                        tracing::debug!("Dialog handler: event channel closed, stopping");
                        break;
                    }
                    RecvOutcome::Lagged(()) => continue,
                }
            }
        })
    }
}
