//! Autosave + backup + corruption-recovery support for show files.
//!
//! Two kinds of safety copies live in a `.s21backups/` subfolder next to the
//! show file:
//!
//! * **Autosave** — written periodically during quiet periods (no recall/cue
//!   burst in flight, console state settled). Keeps the last
//!   [`KEEP_AUTOSAVES`].
//! * **Backup** — a copy of the on-disk file taken right after a successful
//!   load, so the state as it was when opened survives any later corruption.
//!   Keeps the last [`KEEP_BACKUPS`].
//!
//! Everything is keyed by the show's sanitized file **stem**, so several shows
//! sharing one folder each keep their own independent series, and recovery for
//! one show never offers another show's files.
//!
//! Filename scheme (UTC timestamp, type clearly differentiated):
//! ```text
//! autosave__<stem>__<YYYYMMDD-HHMMSS>.s21show
//! backup__<stem>__<YYYYMMDD-HHMMSS>.s21show
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::persistence::show_file::ShowFile;

/// How often an autosave may be written, at most.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(90);
/// How long the console state must be settled (no inbound parameter changes)
/// before an autosave is considered safe to take.
pub const QUIET_SETTLE: Duration = Duration::from_secs(5);
/// Number of autosaves retained per show.
pub const KEEP_AUTOSAVES: usize = 20;
/// Number of load-time backups retained per show.
pub const KEEP_BACKUPS: usize = 5;
/// Subfolder (next to the show file) that holds autosaves + backups.
pub const BACKUP_SUBDIR: &str = ".s21backups";
/// Extension used for safety copies (matches the primary show file).
const SHOW_EXT: &str = "s21show";
/// Timestamp format embedded in safety-copy filenames.
const TS_FMT: &str = "%Y%m%d-%H%M%S";

/// The two kinds of safety copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupKind {
    Autosave,
    Backup,
}

impl BackupKind {
    /// Leading filename token that differentiates the kind.
    pub fn prefix(self) -> &'static str {
        match self {
            BackupKind::Autosave => "autosave",
            BackupKind::Backup => "backup",
        }
    }

    /// How many of this kind to retain.
    pub fn keep(self) -> usize {
        match self {
            BackupKind::Autosave => KEEP_AUTOSAVES,
            BackupKind::Backup => KEEP_BACKUPS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BackupKind::Autosave => "Autosave",
            BackupKind::Backup => "Backup",
        }
    }
}

/// A safety copy found in the backup folder, eligible for recovery.
#[derive(Clone, Debug)]
pub struct RecoveryCandidate {
    pub path: PathBuf,
    pub kind: BackupKind,
    /// Parsed from the filename timestamp.
    pub timestamp: DateTime<Utc>,
    pub size: u64,
    /// `true` if the file successfully parses as a `ShowFile`.
    pub valid: bool,
    /// Show-file version, when the file parsed.
    pub version: Option<u32>,
}

impl RecoveryCandidate {
    /// Human-readable one-line label for the recovery dialog, e.g.
    /// `"Autosave — 2026-05-27 14:03:11 UTC (12.4 KB, v15)"`.
    pub fn describe(&self) -> String {
        let size_kb = self.size as f64 / 1024.0;
        let ver = match self.version {
            Some(v) => format!(", v{v}"),
            None => String::new(),
        };
        let validity = if self.valid { "" } else { " — UNREADABLE" };
        format!(
            "{} — {} ({size_kb:.1} KB{ver}){validity}",
            self.kind.label(),
            self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
        )
    }
}

/// Sanitize a show file's stem to a filesystem- and parse-safe token.
/// Keeps `[A-Za-z0-9._-]`, replaces everything else with `_`.
fn sanitized_stem(show_path: &Path) -> String {
    let raw = show_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".to_string());
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "untitled".to_string()
    } else {
        cleaned
    }
}

/// The `.s21backups/` folder next to `show_path`. `None` only if the show path
/// has no parent component.
pub fn backup_dir(show_path: &Path) -> Option<PathBuf> {
    show_path.parent().map(|p| p.join(BACKUP_SUBDIR))
}

/// Full target path for a new safety copy of `show_path`, stamped now (UTC).
pub fn backup_target(show_path: &Path, kind: BackupKind) -> Option<PathBuf> {
    let dir = backup_dir(show_path)?;
    let stem = sanitized_stem(show_path);
    let ts = Utc::now().format(TS_FMT);
    Some(dir.join(format!("{}__{stem}__{ts}.{SHOW_EXT}", kind.prefix())))
}

/// Parse a safety-copy filename back into `(kind, stem, timestamp)`.
///
/// The timestamp is fixed-format and taken from the final `__`-separated
/// segment, so stems containing underscores parse unambiguously.
fn parse_name(file_name: &str) -> Option<(BackupKind, String, DateTime<Utc>)> {
    let stem_part = file_name.strip_suffix(&format!(".{SHOW_EXT}"))?;

    let kind = if let Some(rest) = stem_part.strip_prefix("autosave__") {
        (BackupKind::Autosave, rest)
    } else if let Some(rest) = stem_part.strip_prefix("backup__") {
        (BackupKind::Backup, rest)
    } else {
        return None;
    };
    let (kind, rest) = kind;

    // rest == "<stem>__<timestamp>" — split on the LAST "__".
    let (stem, ts_str) = rest.rsplit_once("__")?;
    if stem.is_empty() {
        return None;
    }
    let ts = NaiveDateTime::parse_from_str(ts_str, TS_FMT).ok()?;
    Some((kind, stem.to_string(), ts.and_utc()))
}

/// Write `bytes` to a timestamped safety copy of `show_path`, then prune the
/// series for that kind down to its keep-limit.
///
/// Uses `.tmp` + rename for the same atomicity guarantee as
/// [`ShowFile::save`]. Best-effort rotation: a delete that fails (e.g. a file
/// held open by an AV scanner on Windows) is logged and skipped.
pub async fn write_and_rotate(
    show_path: &Path,
    kind: BackupKind,
    bytes: &[u8],
) -> std::io::Result<PathBuf> {
    use tokio::io::AsyncWriteExt;

    let dir = backup_dir(show_path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "show path has no parent directory",
        )
    })?;
    tokio::fs::create_dir_all(&dir).await?;

    let target = backup_target(show_path, kind).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "could not build backup target path",
        )
    })?;

    let mut tmp_os = target.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp_path = PathBuf::from(tmp_os);

    let write_result: std::io::Result<()> = async {
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        Ok(())
    }
    .await;
    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &target).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    let stem = sanitized_stem(show_path);
    rotate(&dir, kind, &stem, kind.keep()).await;
    Ok(target)
}

/// Keep the `keep` newest safety copies of `kind` for `stem` in `dir`; delete
/// the rest. Files of other shows / other kinds are never touched. Best-effort.
pub async fn rotate(dir: &Path, kind: BackupKind, stem: &str, keep: usize) {
    let mut matches: Vec<(PathBuf, DateTime<Utc>)> = Vec::new();

    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some((k, s, ts)) = parse_name(&name)
            && k == kind
            && s == stem
        {
            matches.push((entry.path(), ts));
        }
    }

    if matches.len() <= keep {
        return;
    }
    // Newest first.
    matches.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, _) in matches.into_iter().skip(keep) {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(path = %path.display(), error = %e, "Failed to prune old backup");
        }
    }
}

/// Scan the backup folder for safety copies of `show_path`'s show, validate
/// each, and return them newest-first. Empty if the folder is missing.
pub async fn list_recovery_candidates(show_path: &Path) -> Vec<RecoveryCandidate> {
    let Some(dir) = backup_dir(show_path) else {
        return Vec::new();
    };
    let stem = sanitized_stem(show_path);

    let mut out: Vec<RecoveryCandidate> = Vec::new();
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        let Some((kind, s, ts)) = parse_name(&name) else {
            continue;
        };
        if s != stem {
            continue;
        }
        let path = entry.path();
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        let (valid, version) = match ShowFile::load(&path).await {
            Ok(show) => (true, Some(show.version)),
            Err(_) => (false, None),
        };
        out.push(RecoveryCandidate {
            path,
            kind,
            timestamp: ts,
            size,
            valid,
            version,
        });
    }

    out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    out
}

/// Classify a load error: `true` when it indicates a corrupt / truncated /
/// bad-header file (so recovery should be offered), `false` for `NotFound`,
/// permission errors, etc. (so the plain error is shown).
pub fn is_corruption_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "s21_backup_test_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sanitized_stem_strips_illegal_chars() {
        let p = PathBuf::from("/shows/My Show #1!.s21show");
        assert_eq!(sanitized_stem(&p), "My_Show__1_");
        let ok = PathBuf::from("/shows/Set_2-Final.v3.s21show");
        assert_eq!(sanitized_stem(&ok), "Set_2-Final.v3");
    }

    #[test]
    fn backup_target_round_trips_through_parse() {
        let p = PathBuf::from("/shows/My_Show.s21show");
        let target = backup_target(&p, BackupKind::Autosave).unwrap();
        assert!(target.starts_with("/shows/.s21backups"));
        let name = target.file_name().unwrap().to_string_lossy();
        let (kind, stem, _ts) = parse_name(&name).expect("parses");
        assert_eq!(kind, BackupKind::Autosave);
        assert_eq!(stem, "My_Show");
    }

    #[test]
    fn parse_name_handles_underscored_stem() {
        let (kind, stem, ts) =
            parse_name("backup__a__b__c__20260527-140311.s21show").expect("parses");
        assert_eq!(kind, BackupKind::Backup);
        assert_eq!(stem, "a__b__c");
        assert_eq!(ts.format(TS_FMT).to_string(), "20260527-140311");
        // Wrong extension / prefix are rejected.
        assert!(parse_name("autosave__x__20260527-140311.json").is_none());
        assert!(parse_name("random__x__20260527-140311.s21show").is_none());
        assert!(parse_name("autosave__x__not-a-date.s21show").is_none());
    }

    #[tokio::test]
    async fn rotate_is_per_show_and_per_kind() {
        let dir = unique_dir("rotate");
        // Two shows interleaved, plus a backup-kind file that must survive.
        let stamps = [
            "20260101-000001",
            "20260101-000002",
            "20260101-000003",
            "20260101-000004",
        ];
        for ts in stamps {
            for stem in ["MyShow", "OtherShow"] {
                let f = dir.join(format!("autosave__{stem}__{ts}.s21show"));
                tokio::fs::write(&f, b"{}").await.unwrap();
            }
        }
        let keep_backup = dir.join("backup__MyShow__20260101-000001.s21show");
        tokio::fs::write(&keep_backup, b"{}").await.unwrap();

        // Keep 2 autosaves of MyShow only.
        rotate(&dir, BackupKind::Autosave, "MyShow", 2).await;

        let mut my_auto = 0;
        let mut other_auto = 0;
        let mut backups = 0;
        let mut rd = tokio::fs::read_dir(&dir).await.unwrap();
        while let Ok(Some(e)) = rd.next_entry().await {
            let n = e.file_name().to_string_lossy().to_string();
            match parse_name(&n) {
                Some((BackupKind::Autosave, s, _)) if s == "MyShow" => my_auto += 1,
                Some((BackupKind::Autosave, s, _)) if s == "OtherShow" => other_auto += 1,
                Some((BackupKind::Backup, _, _)) => backups += 1,
                _ => {}
            }
        }
        assert_eq!(my_auto, 2, "MyShow autosaves pruned to keep-limit");
        assert_eq!(other_auto, 4, "OtherShow autosaves untouched");
        assert_eq!(backups, 1, "backup-kind file untouched");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn write_and_rotate_then_list_and_validate() {
        let dir = unique_dir("write");
        let show_path = dir.join("MyShow.s21show");

        // A valid show, serialized like the app would.
        let show = ShowFile::new(crate::model::config::ConsoleConfig::default());
        let json = serde_json::to_vec(&show).unwrap();
        let written = write_and_rotate(&show_path, BackupKind::Autosave, &json)
            .await
            .unwrap();
        assert!(written.exists());
        // Round-trips back to an equal ShowFile.
        let reloaded = ShowFile::load(&written).await.unwrap();
        assert_eq!(reloaded.version, show.version);

        // Drop in a truncated file under the same stem so listing flags it.
        let bad = dir
            .join(BACKUP_SUBDIR)
            .join("backup__MyShow__20990101-000000.s21show");
        tokio::fs::write(&bad, b"{ truncated").await.unwrap();

        // A different show's file must be excluded.
        let other = dir
            .join(BACKUP_SUBDIR)
            .join("autosave__OtherShow__20990101-000001.s21show");
        tokio::fs::write(&other, &json).await.unwrap();

        let candidates = list_recovery_candidates(&show_path).await;
        assert_eq!(candidates.len(), 2, "only MyShow's two files");
        // Newest first: the 2099 backup precedes the just-written autosave.
        assert_eq!(candidates[0].kind, BackupKind::Backup);
        assert!(!candidates[0].valid, "truncated file flagged invalid");
        assert!(candidates[1].valid);
        assert_eq!(candidates[1].version, Some(show.version));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn corruption_error_classification() {
        let invalid = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad json");
        assert!(is_corruption_error(&invalid));
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        assert!(!is_corruption_error(&not_found));
    }
}
