use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};

use crate::{CdpConnection, CdpError, Session};

/// A pool of CDP sessions that enables tab reuse across parallel CDP operations.
///
/// Maintains an idle queue of recycled sessions and limits total open tabs
/// via a semaphore. All checked-out sessions automatically return to the
/// pool (or are closed) when their `PooledSession` guard is dropped.
pub struct SessionPool {
    conn: Arc<CdpConnection>,
    semaphore: Semaphore,
    max_open: usize,
    idle: Mutex<VecDeque<Session>>,
    /// Maps target_id → session_id for sessions currently checked out.
    live: Mutex<HashMap<String, String>>,
}

/// A checked-out session that automatically returns to the pool when dropped.
///
/// While this guard lives the session is considered "checked out" and will
/// not be handed to another consumer. On `Drop` the session is either
/// recycled into the idle queue or closed, depending on pool capacity.
pub struct PooledSession {
    session: Option<Session>,
    pool: Arc<SessionPool>,
}

impl SessionPool {
    /// Create a new pool.
    ///
    /// `max_open` limits both the semaphore permits and the maximum number
    /// of idle sessions retained for reuse.
    pub fn new(conn: Arc<CdpConnection>, max_open: usize) -> Self {
        Self {
            conn,
            semaphore: Semaphore::new(max_open),
            max_open,
            idle: Mutex::new(VecDeque::new()),
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Check out a session from the pool.
    ///
    /// Acquires a semaphore permit (to limit concurrent session creation),
    /// then either reuses an idle session or creates a fresh one by calling
    /// `Target.createTarget` + `Target.attachToTarget`.
    pub async fn checkout(self: &Arc<Self>) -> Result<PooledSession, CdpError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| CdpError::ConnectionClosed)?;

        let session = match self.idle.lock().await.pop_front() {
            Some(session) => session,
            None => self.create_session().await?,
        };

        let target_id = session.target_id().to_string();
        let sid = session.session_id().to_string();
        self.live.lock().await.insert(target_id, sid);

        Ok(PooledSession {
            session: Some(session),
            pool: self.clone(),
        })
    }

    /// Return a session to the pool (recycle) or close it.
    ///
    /// 1. Best-effort navigation to `about:blank`.
    /// 2. If the idle queue has room, push the session onto it.
    /// 3. Otherwise, close the tab with the Dia browser quirk:
    ///    `window.close()` → 100 ms delay → `Target.closeTarget`
    ///    and remove it from the live map.
    async fn checkin(&self, session: Session) {
        // Best-effort: reset the page to a clean blank state
        if let Err(e) = session
            .call(
                "Page.navigate",
                Some(serde_json::json!({"url": "about:blank"})),
            )
            .await
        {
            tracing::warn!("Failed to navigate session to about:blank: {e}");
        }

        let target_id = session.target_id().to_string();
        let mut idle = self.idle.lock().await;

        if idle.len() < self.max_open {
            // Recycle – keep the tab alive for future reuse
            let sid = session.session_id().to_string();
            self.live.lock().await.insert(target_id, sid);
            idle.push_back(session);
        } else {
            // Idle queue is full – close the tab
            self.live.lock().await.remove(&target_id);

            // Dia browser quirk: ask the page to close itself first
            let _ = session
                .call(
                    "Runtime.evaluate",
                    Some(serde_json::json!({"expression": "window.close()"})),
                )
                .await;

            // Give Dia a moment to process the close
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Force-close via CDP
            let _ = self
                .conn
                .call(
                    "Target.closeTarget",
                    Some(serde_json::json!({"targetId": target_id})),
                )
                .await;
        }
    }

    /// Create a brand-new CDP session.
    ///
    /// 1. `Target.createTarget` with `url: "about:blank"`
    /// 2. `Target.attachToTarget` with `flatten: true` (via `Session::attach`)
    async fn create_session(&self) -> Result<Session, CdpError> {
        let params = serde_json::json!({"url": "about:blank"});
        let result = self
            .conn
            .call("Target.createTarget", Some(params))
            .await?;
        let target_id = result["targetId"]
            .as_str()
            .ok_or_else(|| {
                CdpError::Other("No targetId in Target.createTarget response".to_string())
            })?
            .to_string();
        Session::attach(&self.conn, &target_id).await
    }
}

impl PooledSession {
    /// Access the inner session.
    pub fn session(&self) -> &Session {
        self.session.as_ref().unwrap()
    }

    /// The CDP target ID of the checked-out session.
    pub fn target_id(&self) -> &str {
        self.session.as_ref().unwrap().target_id()
    }

    /// The CDP session ID of the checked-out session.
    pub fn session_id(&self) -> &str {
        self.session.as_ref().unwrap().session_id()
    }
}

impl Drop for PooledSession {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            let pool = self.pool.clone();
            // Spawn checkin in background – cannot block in Drop
            tokio::spawn(async move {
                pool.checkin(session).await;
            });
        }
    }
}
