//! Bounded async job queue.
//!
//! A [`JobQueue`] owns the send half of a bounded mpsc buffer plus a
//! [`Semaphore`] limiting concurrent execution. Enqueuing is non-blocking
//! (via `try_send`): when the buffer is full the caller gets
//! [`EnqueueError::Full`] immediately, which the HTTP layer maps to a 429 with
//! a retry hint — no caller ever parks waiting for a slot.
//!
//! Queue depth ([`JobQueue::depth`]) counts jobs still sitting in the buffer,
//! reported cheaply by `/healthz` and `/metrics`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Semaphore, mpsc};

use crate::jobs::QueuedJob;

/// Default max jobs waiting in the buffer.
pub(crate) const DEFAULT_QUEUE_CAPACITY: usize = 256;
/// Default max jobs executing concurrently.
pub(crate) const DEFAULT_MAX_CONCURRENT: usize = 4;

/// Why an [`enqueue`](JobQueue::enqueue) failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnqueueError {
    /// The buffer is full; the caller should answer 429 with a retry hint.
    Full,
    /// The worker end was dropped (shutdown); the queue is closed.
    Closed,
}

/// Bounded async job queue.
#[derive(Debug)]
pub(crate) struct JobQueue {
    tx: mpsc::Sender<QueuedJob>,
    permits: Arc<Semaphore>,
    depth: Arc<AtomicUsize>,
}

impl JobQueue {
    /// Create a queue with `capacity` buffer slots and at most
    /// `max_concurrent` jobs running at once, returning the receive half for
    /// the caller to hand to [`spawn_worker`](Self::spawn_worker).
    #[must_use]
    pub(crate) fn new(capacity: usize, max_concurrent: usize) -> (Self, mpsc::Receiver<QueuedJob>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Self {
                tx,
                permits: Arc::new(Semaphore::new(max_concurrent)),
                depth: Arc::new(AtomicUsize::new(0)),
            },
            rx,
        )
    }

    /// Reserve a buffer slot and hand `job` to the worker.
    ///
    /// Non-blocking: returns [`EnqueueError::Full`] when the buffer is full
    /// and [`EnqueueError::Closed`] when the worker has shut down.
    pub(crate) async fn enqueue(&self, job: QueuedJob) -> Result<(), EnqueueError> {
        self.depth.fetch_add(1, Ordering::SeqCst);
        match self.tx.try_send(job) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.depth.fetch_sub(1, Ordering::SeqCst);
                Err(EnqueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.depth.fetch_sub(1, Ordering::SeqCst);
                Err(EnqueueError::Closed)
            }
        }
    }

    /// Number of jobs waiting in the buffer (not yet started).
    #[must_use]
    pub(crate) fn depth(&self) -> usize {
        self.depth.load(Ordering::SeqCst)
    }

    /// Drive the queue: gate each slot on the concurrency semaphore first,
    /// then take the next job from the buffer, and run `handler` in a spawned
    /// task. Acquiring the permit *before* `recv` keeps every
    /// accepted-but-not-started job in the buffer, so [`depth`](Self::depth)
    /// stays an accurate measure of pending work.
    ///
    /// Returns the worker's [`JoinHandle`](tokio::task::JoinHandle). Dropping
    /// the queue's send half ends the loop once the buffer drains.
    #[must_use]
    pub(crate) fn spawn_worker<F, Fut>(
        &self,
        rx: mpsc::Receiver<QueuedJob>,
        handler: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(QueuedJob) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let permits = Arc::clone(&self.permits);
        let depth = Arc::clone(&self.depth);
        tokio::spawn(async move {
            let mut rx = rx;
            while let Ok(permit) = permits.clone().acquire_owned().await {
                let Some(job) = rx.recv().await else { break };
                // The job has left the buffer and now has a concurrency slot.
                depth.fetch_sub(1, Ordering::SeqCst);
                let fut = handler(job);
                tokio::spawn(async move {
                    fut.await;
                    drop(permit);
                });
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::mpsc;

    use super::{EnqueueError, JobQueue};
    use crate::jobs::args::JobArgs;
    use crate::jobs::{Op, QueuedJob};

    fn job(id: usize) -> QueuedJob {
        QueuedJob {
            op: Op::Simple,
            args: JobArgs::parse(Op::Simple, &json!({"query": format!("q{id}")})).unwrap(),
            timeout_ms: None,
            trace_id: None,
        }
    }

    #[tokio::test]
    async fn enqueue_beyond_capacity_returns_full() {
        let (queue, _rx) = JobQueue::new(2, 1);
        queue.enqueue(job(1)).await.unwrap();
        queue.enqueue(job(2)).await.unwrap();
        assert_eq!(queue.depth(), 2);

        let err = queue.enqueue(job(3)).await.unwrap_err();
        assert_eq!(err, EnqueueError::Full);
        // The failed enqueue must not have bumped depth.
        assert_eq!(queue.depth(), 2);
    }

    #[tokio::test]
    async fn queue_depth_tracks_pending_until_worker_drains() {
        let (queue, rx) = JobQueue::new(4, 2);
        queue.enqueue(job(1)).await.unwrap();
        queue.enqueue(job(2)).await.unwrap();
        assert_eq!(queue.depth(), 2);

        let handle = queue.spawn_worker(rx, |_job| async {});
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.depth(), 0, "buffer drains once the worker runs");
        handle.abort();
    }

    #[tokio::test]
    async fn queue_limits_concurrency_via_semaphore() {
        let (queue, rx) = JobQueue::new(8, 1);
        let release = Arc::new(tokio::sync::Notify::new());
        let release_for_handler = Arc::clone(&release);
        let (done_tx, mut done_rx) = mpsc::channel::<()>(8);

        queue.enqueue(job(1)).await.unwrap();
        queue.enqueue(job(2)).await.unwrap();

        let handle = queue.spawn_worker(rx, move |_job| {
            let release = Arc::clone(&release_for_handler);
            let done_tx = done_tx.clone();
            async move {
                done_tx.send(()).await.unwrap();
                release.notified().await; // park until released
            }
        });

        // First job starts; the second waits on the semaphore.
        done_rx.recv().await.unwrap();
        assert_eq!(queue.depth(), 1, "second job still pending");

        // Release the first; the second begins.
        release.notify_one();
        done_rx.recv().await.unwrap();
        assert_eq!(queue.depth(), 0);

        release.notify_one();
        handle.abort();
    }

    #[tokio::test]
    async fn enqueue_after_worker_dropped_returns_closed() {
        let (queue, rx) = JobQueue::new(4, 2);
        drop(rx);
        let err = queue.enqueue(job(1)).await.unwrap_err();
        assert_eq!(err, EnqueueError::Closed);
    }
}
