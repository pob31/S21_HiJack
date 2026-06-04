//! A transient status-line message that can optionally carry a localized
//! help bubble.
//!
//! Several tabs keep a single `Option<StatusMessage>` and render it once via
//! `ui.colored_label(theme::TEXT_WARNING, …)`. The visible text stays English
//! (set at the call site), but a warning/validation message can also attach a
//! [`HelpKey`] so hovering the status line surfaces a translated explanation —
//! the same localization path as every other help bubble.
//!
//! Ephemeral progress/success messages ("Connecting…", "Saved") carry no key:
//! the `From<&str>` / `From<String>` conversions default `help` to `None`, so
//! existing `Some("…".into())` / `Some(format!(…))` call sites keep compiling
//! unchanged. Only the messages we want translatable use
//! [`StatusMessage::with_help`].

use super::help::HelpKey;

/// A status-line message plus an optional help-bubble key for translation.
pub struct StatusMessage {
    /// The text shown on screen (English, set at the call site).
    pub text: String,
    /// Optional help key — when present, hovering the status line shows the
    /// active language's translation of this message.
    pub help: Option<HelpKey>,
}

impl StatusMessage {
    /// A message with a translatable help bubble. Use for warnings and
    /// validation errors a limited-English operator may need explained.
    pub fn with_help(text: impl Into<String>, key: HelpKey) -> Self {
        StatusMessage {
            text: text.into(),
            help: Some(key),
        }
    }
}

impl From<&str> for StatusMessage {
    fn from(s: &str) -> Self {
        StatusMessage {
            text: s.to_owned(),
            help: None,
        }
    }
}

impl From<String> for StatusMessage {
    fn from(s: String) -> Self {
        StatusMessage {
            text: s,
            help: None,
        }
    }
}
