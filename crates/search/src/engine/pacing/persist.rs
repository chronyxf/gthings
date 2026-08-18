//! Disk persistence for pacing state.
//!
//! Holds the serializable slice of the pacing store ([`PersistedState`]), the
//! background writer ([`PersistWriter`]) that drains a channel on a dedicated
//! thread, and the atomic temp-file+rename write path ([`persist_to_disk`]).
//!
//! Mutations enqueue a serialized snapshot over an unbounded channel — a
//! non-blocking send — so the async router path never performs file I/O while
//! holding the pacing lock. The single consumer writes each snapshot
//! atomically; the last snapshot always wins on disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use serde::{Deserialize, Serialize};

use super::PACING_FILE;

/// Serializable slice of the pacing store used for disk persistence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct PersistedState {
    /// engine identifier → last-call unix millis.
    #[serde(default)]
    pub(super) last_calls: HashMap<String, u64>,
    /// engine identifier → unix millis until which the engine is blocked.
    #[serde(default)]
    pub(super) cooldowns: HashMap<String, u64>,
    /// Aggregate paid-API quota spend.
    #[serde(default)]
    pub(super) api_quota_spend: u64,
}

/// Background writer that persists pacing snapshots on a dedicated thread.
///
/// Mutations enqueue a serialized snapshot over an unbounded channel — a
/// non-blocking send — so the async router path never performs file I/O while
/// holding the pacing lock. The single consumer writes each snapshot
/// atomically; the last snapshot always wins on disk.
#[derive(Debug)]
pub(super) struct PersistWriter {
    tx: Sender<PersistMsg>,
}

/// Message sent to the persistence thread.
#[derive(Debug)]
enum PersistMsg {
    /// Write this snapshot to disk.
    Write(PersistedState),
    /// Test/sync barrier: the writer signals once all prior writes are done.
    #[cfg(test)]
    Flush(mpsc::SyncSender<()>),
}

impl PersistWriter {
    /// Spawn the writer thread persisting to `dir`.
    pub(super) fn spawn(dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("gthings-pacing-writer".to_string())
            .spawn(move || persist_loop(dir, rx))
            .expect("failed to spawn pacing writer thread");
        Self { tx }
    }

    /// Enqueue a snapshot for the writer thread. Never blocks; a closed
    /// channel (writer gone) silently drops the snapshot — pacing must never
    /// fail a search.
    pub(super) fn enqueue(&self, state: PersistedState) {
        let _ = self.tx.send(PersistMsg::Write(state));
    }

    /// Block until every previously enqueued snapshot has been written.
    #[cfg(test)]
    pub(super) fn flush(&self) {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        if self.tx.send(PersistMsg::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }
}

/// Consume persistence messages until the channel closes.
fn persist_loop(dir: PathBuf, rx: Receiver<PersistMsg>) {
    for msg in rx {
        match msg {
            PersistMsg::Write(state) => persist_to_disk(&dir, &state),
            #[cfg(test)]
            PersistMsg::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Atomically write `state` to `{dir}/pacing.json` (temp file + rename).
/// I/O failures are logged and swallowed — pacing must never fail a search.
fn persist_to_disk(dir: &std::path::Path, state: &PersistedState) {
    let Ok(json) = serde_json::to_string_pretty(state) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("pacing: failed to create {dir:?}: {e}");
        return;
    }
    let tmp = dir.join(format!("{PACING_FILE}.tmp"));
    let path = dir.join(PACING_FILE);
    if let Err(e) = std::fs::write(&tmp, json) {
        tracing::warn!("pacing: failed to write {tmp:?}: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        tracing::warn!("pacing: failed to persist {path:?}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::engine::SearchEngine;
    use crate::engine::pacing::{PACING_FILE, PacingStore, is_tmp_path};

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    #[test]
    fn disk_persistence_round_trip() {
        let dir = std::env::temp_dir().join(format!("gthings-pacing-test-{}", now_ms()));
        let mut store = PacingStore::load_from_dir(dir.clone());
        store.record(SearchEngine::Brave, 1_700_000_000_000);
        store.record_cooldown(SearchEngine::Google, 1_700_000_360_000);
        store.bump_quota();
        store.bump_quota();

        // The background writer persists asynchronously: flush before
        // reloading so the assertions below are deterministic.
        store.flush();

        // A fresh store loaded from the same dir must observe every field.
        let reloaded = PacingStore::load_from_dir(dir.clone());
        assert_eq!(
            reloaded.last_call_ms(SearchEngine::Brave),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            reloaded.cooldown_until_ms(SearchEngine::Google),
            Some(1_700_000_360_000)
        );
        assert_eq!(reloaded.quota_spend(), 2);

        // No stray temp files left behind (atomic write via rename).
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("pacing dir exists")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| is_tmp_path(p))
            .collect();
        assert!(leftovers.is_empty(), "temp files must be renamed away");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_yields_empty_store() {
        let dir = std::env::temp_dir().join(format!("gthings-pacing-missing-{}", now_ms()));
        // No file exists yet: fresh empty store.
        let store = PacingStore::load_from_dir(dir.clone());
        assert!(store.last_calls.is_empty());
        assert_eq!(store.quota_spend(), 0);
        // A corrupt file must degrade to an empty store, not panic.
        std::fs::create_dir_all(&dir).expect("dir created");
        std::fs::write(dir.join(PACING_FILE), "not json").expect("file written");
        let store = PacingStore::load_from_dir(dir.clone());
        assert!(store.last_calls.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
