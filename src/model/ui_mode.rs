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
            Tab::Setup | Tab::Macros | Tab::Gangs | Tab::PanLink | Tab::Sidecar => true,
            Tab::Snapshots => self != UiMode::LiveMusic,
            Tab::Monitor => self != UiMode::Theatre,
            Tab::OscLog | Tab::Inspector => show_diagnostics,
        }
    }

    /// Whether the top-bar cue-transport strip (Prev / current cue / Go)
    /// should render in this display mode. Currently mirrors the
    /// `Snapshots` tab visibility — Full and Theatre have cueing,
    /// Live music doesn't.
    pub fn cue_transport_visible(self) -> bool {
        self.tab_visible(Tab::Snapshots, false)
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

/// UI colour theme. `Default = Dark` is the serde fallback for preference
/// files saved before this field existed — they keep the original dark look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorTheme {
    #[default]
    Dark,
    Light,
}

impl ColorTheme {
    pub fn label(self) -> &'static str {
        match self {
            ColorTheme::Dark => "Dark",
            ColorTheme::Light => "Light",
        }
    }

    pub const ALL: [ColorTheme; 2] = [ColorTheme::Dark, ColorTheme::Light];
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
        assert!(m.tab_visible(Tab::Gangs, false));
        assert!(m.tab_visible(Tab::PanLink, false));
        assert!(m.tab_visible(Tab::Sidecar, false));
        assert!(m.tab_visible(Tab::Monitor, false));
    }

    #[test]
    fn sidecar_visible_in_every_mode() {
        // The sidecar is an operator surface like Macros — never hidden
        // by a display mode.
        for m in UiMode::ALL {
            assert!(m.tab_visible(Tab::Sidecar, false));
        }
    }

    #[test]
    fn cue_transport_visible_in_cueing_modes() {
        assert!(UiMode::Full.cue_transport_visible());
        assert!(UiMode::Theatre.cue_transport_visible());
        assert!(!UiMode::LiveMusic.cue_transport_visible());
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
