use serde::{Deserialize, Serialize};

use crate::ui::Tab;

/// Display mode that controls which tabs are visible in the UI.
///
/// `Default = Full` is the serde fallback for show files saved before
/// this field existed — they keep behaving exactly as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiMode {
    #[default]
    Full,
    LiveMusic,
    Theatre,
}

impl UiMode {
    /// Whether the given tab is visible in this mode.
    ///
    /// `show_diagnostics` controls the OSC Log + Inspector tabs and is
    /// independent of the mode (operator preference, not a per-show
    /// setting).
    pub fn tab_visible(self, tab: Tab, show_diagnostics: bool) -> bool {
        match tab {
            Tab::Setup
            | Tab::Macros
            | Tab::Live
            | Tab::Gangs
            | Tab::PanLink => true,
            Tab::Snapshots => self != UiMode::LiveMusic,
            Tab::Monitor => self != UiMode::Theatre,
            Tab::OscLog | Tab::Inspector => show_diagnostics,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            UiMode::Full => "Full",
            UiMode::LiveMusic => "Live music",
            UiMode::Theatre => "Theatre",
        }
    }

    pub const ALL: [UiMode; 3] = [UiMode::Full, UiMode::LiveMusic, UiMode::Theatre];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_shows_every_feature_tab() {
        let m = UiMode::Full;
        assert!(m.tab_visible(Tab::Setup, false));
        assert!(m.tab_visible(Tab::Snapshots, false));
        assert!(m.tab_visible(Tab::Macros, false));
        assert!(m.tab_visible(Tab::Live, false));
        assert!(m.tab_visible(Tab::Gangs, false));
        assert!(m.tab_visible(Tab::PanLink, false));
        assert!(m.tab_visible(Tab::Monitor, false));
    }

    #[test]
    fn live_music_hides_snapshots_only() {
        let m = UiMode::LiveMusic;
        assert!(!m.tab_visible(Tab::Snapshots, false));
        assert!(m.tab_visible(Tab::Monitor, false));
        assert!(m.tab_visible(Tab::Macros, false));
        assert!(m.tab_visible(Tab::Gangs, false));
    }

    #[test]
    fn theatre_hides_monitor_only() {
        let m = UiMode::Theatre;
        assert!(!m.tab_visible(Tab::Monitor, false));
        assert!(m.tab_visible(Tab::Snapshots, false));
        assert!(m.tab_visible(Tab::Macros, false));
        assert!(m.tab_visible(Tab::Gangs, false));
    }

    #[test]
    fn diagnostic_toggle_is_mode_independent() {
        for m in UiMode::ALL {
            assert!(!m.tab_visible(Tab::OscLog, false));
            assert!(!m.tab_visible(Tab::Inspector, false));
            assert!(m.tab_visible(Tab::OscLog, true));
            assert!(m.tab_visible(Tab::Inspector, true));
        }
    }

    #[test]
    fn default_is_full() {
        assert_eq!(UiMode::default(), UiMode::Full);
    }
}
