//! Crash-safe file replacement shared by show saves and backup copies.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replace `path` with `bytes`: write `<path>.tmp`, fsync it, then rename it
/// over `path`. A crash or failure part-way leaves the previous file intact,
/// and the orphan `.tmp` is removed (best effort) on error.
///
/// Runs on a blocking thread with `std::fs` on purpose. `tokio::fs::File`
/// buffers the last write and completes it inside `sync_all()`, which drops
/// that write's error, so a full disk produced a truncated `.tmp`, reported
/// success, and was renamed over the good show (audit C2).
pub async fn write_atomically(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || replace_file(&path, &bytes))
        .await
        .map_err(io::Error::other)?
}

fn replace_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp_path = PathBuf::from(tmp_os);

    if let Err(e) = write_and_sync(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s21_hijack_atomic_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn replaces_the_file_and_leaves_no_tmp() {
        let dir = scratch_dir("replace");
        let path = dir.join("show.s21show");
        fs::write(&path, b"old").unwrap();

        write_atomically(&path, b"new".to_vec()).await.unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(!dir.join("show.s21show.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failure_is_reported_and_keeps_the_previous_file() {
        // A directory squatting on the `.tmp` name makes the write fail.
        let dir = scratch_dir("failure");
        let path = dir.join("show.s21show");
        fs::write(&path, b"good show").unwrap();
        fs::create_dir(dir.join("show.s21show.tmp")).unwrap();

        assert!(write_atomically(&path, b"new".to_vec()).await.is_err());

        assert_eq!(fs::read(&path).unwrap(), b"good show");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The C2 case itself: the disk fills part-way through the write.
    /// `/dev/full` accepts the open and fails every write with ENOSPC; the
    /// old `tokio::fs` path returned `Ok` here.
    #[cfg(target_os = "linux")]
    #[test]
    fn full_disk_is_an_error() {
        let err = write_and_sync(Path::new("/dev/full"), &[0u8; 64 * 1024]).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(28), "ENOSPC, got {err:?}");
    }
}
