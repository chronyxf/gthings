use std::path::Path;

/// Returns `true` if `path`'s mtime is older than `ttl_secs`.
///
/// Missing files are treated as expired. Files whose metadata is inaccessible
/// are conservatively treated as **not** expired (never delete what we cannot
/// verify).
#[must_use]
pub(crate) fn is_file_expired(path: &Path, ttl_secs: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(elapsed) = modified.elapsed() else {
        return false;
    };
    elapsed.as_secs() >= ttl_secs
}

/// Atomically write `data` to `path` via a temporary sibling file and rename.
///
/// The temporary file is created as `{path}.tmp.{PID}.{nanos}` — the nanosecond
/// component makes the name unique across concurrent writers within a process.
/// If the rename succeeds the write is complete; if the process crashes between
/// write and rename the temp file is harmless garbage left behind.
///
/// # Errors
///
/// Returns [`std::io::Error`] if the temporary file cannot be written or
/// the rename fails.
pub(crate) fn atomic_write(path: &Path, data: &str) -> std::io::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = path.with_extension(format!("tmp.{}.{}", std::process::id(), nanos));
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{atomic_write, is_file_expired};

    #[test]
    fn atomic_write_creates_file_and_is_not_expired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atomic.txt");
        atomic_write(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert!(!is_file_expired(&path, 3600));
    }

    #[test]
    fn is_file_expired_missing_is_expired() {
        assert!(is_file_expired(
            std::path::Path::new("/nonexistent/gthings"),
            0
        ));
    }
}
