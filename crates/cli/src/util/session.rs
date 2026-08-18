//! CDP session lifecycle helpers.

use std::sync::Arc;

use crate::util::flags::UniversalFlags;

/// Connect, wrap in `Arc<Session>`, run the async function, then disconnect.
/// Returns the exit code from `f`, or the connection error code.
pub(crate) async fn with_session<F, Fut>(flags: &UniversalFlags, f: F) -> i32
where
    F: FnOnce(Arc<gthings_cdp::Session>) -> Fut,
    Fut: std::future::Future<Output = i32>,
{
    let session = match crate::util::connect(flags).await {
        Ok(s) => s,
        Err(c) => return c,
    };
    let arc_session = Arc::new(session);
    let code = f(Arc::clone(&arc_session)).await;
    // Dropping the `Arc` aborts the dialog handler and the I/O task, so a clean
    // disconnect happens automatically when the last reference goes away.
    drop(arc_session);
    code
}
