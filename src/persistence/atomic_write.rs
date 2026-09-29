//! Crash-safe file replacement shared by show saves and backup copies.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Replace `path` with `bytes`: write a temp file next to it, fsync it, then
/// rename it over `path`. A crash or failure part-way leaves the previous file
/// intact, and the orphan temp file is removed (best effort) on error.
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
    let tmp_path = tmp_path_for(path);

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

/// `<path>.<pid>-<n>.tmp`, unique to this write. Two saves of one show can
/// be in flight at once (a Save still running when Close → Save starts
/// another); with one shared `<path>.tmp` they truncated and wrote the same
/// file, and could rename a mix of both, reported as saved (audit R8).
fn tmp_path_for(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    PathBuf::from(tmp)
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

    /// Names of the temp files left in `dir`.
    fn leftover_tmp(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[tokio::test]
    async fn replaces_the_file_and_leaves_no_tmp() {
        let dir = scratch_dir("replace");
        let path = dir.join("show.s21show");
        fs::write(&path, b"old").unwrap();

        write_atomically(&path, b"new".to_vec()).await.unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(leftover_tmp(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failure_is_reported_and_keeps_the_previous_file() {
        // The rename can't replace a directory that holds a file.
        let dir = scratch_dir("failure");
        let path = dir.join("show.s21show");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside"), b"good show").unwrap();

        assert!(write_atomically(&path, b"new".to_vec()).await.is_err());

        assert_eq!(fs::read(path.join("inside")).unwrap(), b"good show");
        assert!(leftover_tmp(&dir).is_empty(), "the temp file is removed");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Audit R8: saves of one file that overlap each land whole. Sharing one
    /// temp name, they interleaved in it, and a rename could fail when
    /// another writer had already moved the file away.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn overlapping_saves_each_land_whole() {
        let dir = scratch_dir("overlap");
        let path = dir.join("show.s21show");
        let writes: Vec<_> = (0u8..8)
            .map(|i| {
                let path = path.clone();
                tokio::spawn(async move { write_atomically(&path, vec![b'a' + i; 1 << 20]).await })
            })
            .collect();
        for w in writes {
            w.await.unwrap().expect("every save succeeds");
        }

        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 1 << 20);
        assert!(
            bytes.iter().all(|&b| b == bytes[0]),
            "the file is one save's content, not a mix"
        );
        assert!(leftover_tmp(&dir).is_empty());
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
