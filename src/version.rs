//! App version metadata + the GitHub "latest release" update check.
//!
//! [`APP_VERSION`] is this build's semantic version, baked in from `Cargo.toml`.
//! [`check_latest_release`] queries the GitHub Releases API for the newest
//! published tag and reports whether a newer build is available. The check is
//! best-effort: any network / parse / API failure degrades to
//! [`UpdateStatus::Failed`] and never disrupts the app. The module is pure (no
//! UI or runtime dependencies) — callers run [`check_latest_release`] on a
//! worker thread and surface the [`UpdateStatus`] however they like.

use std::time::Duration;

/// This build's semantic version, from `Cargo.toml` at compile time.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `owner/repo` the update check queries on GitHub.
pub const GITHUB_REPO: &str = "pob31/S21_HiJack";

/// Overall timeout for the release lookup. Short enough that a slow / blocked
/// network never leaves the "Checking…" state visible for long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

/// Outcome of a GitHub release check, surfaced on the Setup tab.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum UpdateStatus {
    /// No check has run yet this session.
    #[default]
    Unknown,
    /// A check is in flight.
    Checking,
    /// This build is the latest published release (or newer — a dev build).
    UpToDate,
    /// A newer release exists. Carries its version (no leading `v`) and the
    /// release page URL for the operator to open.
    Available { version: String, url: String },
    /// The check could not complete (offline, rate-limited, no releases yet,
    /// malformed response). Carries a short human-readable reason for the
    /// hover tooltip.
    Failed(String),
}

/// Blocking GitHub Releases lookup. Run on a worker thread (e.g.
/// `tokio::runtime::Handle::spawn_blocking`) — it performs a synchronous HTTPS
/// request with an overall [`CHECK_TIMEOUT`]. Never panics; every error maps to
/// [`UpdateStatus::Failed`].
pub fn check_latest_release() -> UpdateStatus {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");

    // Self-contained TLS (bundled rustls roots) so the single binary needs no
    // system OpenSSL on any platform. We read the HTTP status ourselves —
    // 404 ("no releases yet") is an expected, non-fatal outcome — so disable
    // ureq's status-as-error behaviour.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(CHECK_TIMEOUT))
        .http_status_as_error(false)
        .user_agent(concat!("s21_hijack/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();

    let mut resp = match agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Failed(e.to_string()),
    };

    let status = resp.status().as_u16();
    if status == 404 {
        return UpdateStatus::Failed("no releases published yet".to_string());
    }
    if !(200..300).contains(&status) {
        return UpdateStatus::Failed(format!("GitHub returned HTTP {status}"));
    }

    let body = match resp.body_mut().read_to_string() {
        Ok(b) => b,
        Err(e) => return UpdateStatus::Failed(e.to_string()),
    };
    let json: serde_json::Value = match serde_json::from_str(&body) {
        Ok(j) => j,
        Err(e) => return UpdateStatus::Failed(e.to_string()),
    };

    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if tag.is_empty() {
        return UpdateStatus::Failed("release has no tag_name".to_string());
    }
    let page_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if is_newer(tag, APP_VERSION) {
        UpdateStatus::Available {
            version: strip_v(tag).to_string(),
            url: page_url,
        }
    } else {
        UpdateStatus::UpToDate
    }
}

/// Strip a leading `v` / `V` from a tag like `v0.2.0` → `0.2.0`.
fn strip_v(tag: &str) -> &str {
    tag.strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag)
}

/// Parse the numeric `major.minor.patch…` components of a version string,
/// ignoring a leading `v` and any pre-release / build suffix (everything from
/// the first `-` or `+`). Parsing stops at the first non-numeric component, so
/// a garbage tag yields an empty / partial list rather than panicking.
fn numeric_parts(version: &str) -> Vec<u64> {
    let core = strip_v(version.trim());
    let core = core.split(['-', '+']).next().unwrap_or(core);
    core.split('.')
        .map(|p| p.parse::<u64>().ok())
        .take_while(|p| p.is_some())
        .flatten()
        .collect()
}

/// True when `remote` is a strictly newer version than `local`. Compares the
/// numeric `major.minor.patch` components left-to-right, padding the shorter
/// side with zeros. Pre-release suffixes are ignored for the comparison, so
/// `v0.2.0-beta` counts as newer than `0.1.0`, while `0.1.0` does not flag an
/// update against its own `-beta` tag.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let r = numeric_parts(remote);
    let l = numeric_parts(local);
    let n = r.len().max(l.len());
    for i in 0..n {
        let rv = r.get(i).copied().unwrap_or(0);
        let lv = l.get(i).copied().unwrap_or(0);
        if rv != lv {
            return rv > lv;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_detects_bumps() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.1.0", "0.0.9"));
    }

    #[test]
    fn not_newer_when_equal_or_older() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn handles_uneven_component_counts() {
        assert!(is_newer("0.2", "0.1.9"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(is_newer("1", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1"));
    }

    #[test]
    fn prerelease_suffix_ignored_for_core() {
        assert!(is_newer("v0.2.0-beta.1", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0-beta"));
    }

    #[test]
    fn garbage_tag_is_not_newer() {
        assert!(!is_newer("not-a-version", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
    }

    #[test]
    fn strip_v_works() {
        assert_eq!(strip_v("v1.2.3"), "1.2.3");
        assert_eq!(strip_v("V1.2.3"), "1.2.3");
        assert_eq!(strip_v("1.2.3"), "1.2.3");
    }

    /// Live smoke test against the real GitHub API — exercises the full TLS +
    /// HTTP + JSON path. Ignored by default (needs internet); run with
    /// `cargo test -- --ignored live_release_check`.
    #[test]
    #[ignore = "requires network access to api.github.com"]
    fn live_release_check_returns_a_concrete_status() {
        let status = check_latest_release();
        // Any concrete outcome is acceptable; we only assert the call returned
        // (TLS handshake + parse worked) rather than panicked or hung.
        assert!(
            !matches!(status, UpdateStatus::Unknown | UpdateStatus::Checking),
            "got {status:?}"
        );
    }
}
