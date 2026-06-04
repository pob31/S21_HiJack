//! Centralized, localization-ready help-bubble (tooltip) catalog.
//!
//! Every help string the UI shows on hover is keyed by a [`HelpKey`] and
//! authored in English here — the canonical reference. [`meta`] is an
//! exhaustive match, so the build fails if a key ever lacks English text:
//! the reference language can never go missing.
//!
//! Call sites pass only the key:
//! ```ignore
//! some_response.on_hover_text(help(HelpKey::CueUndo));
//! ```
//!
//! [`help`] returns the active language's translation, falling back to the
//! English reference per-key when a translation is absent. English is
//! allocation-free (`Cow::Borrowed`); only a translated tooltip clones a
//! short string.
//!
//! Translations live in **external JSON files loaded at runtime** — the UI
//! itself stays English-only, only these bubbles localize, and a new
//! language is added by dropping a file (no recompile). The loader and the
//! active-language plumbing are added in the localization phase; until a
//! locale is active, every bubble renders in English.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use tracing::{info, warn};

/// Stable identifier for one help bubble. Adding a tooltip means adding a
/// variant here plus its English text in [`meta`]. The *variant* is the
/// stable key as far as call sites are concerned — renaming it is safe; only
/// the [`HelpMeta::id`] string (the translator-facing JSON key) must stay
/// fixed once shipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpKey {
    // ── Generic ──
    Dismiss,
    PickColor,

    // ── Cue transport / cue list ──
    CueUndo,
    CuePrev,
    CueGo,
    CueSkip,
    CueListOpen,
    CueListJump,

    // ── Top bar / banners (app.rs) ──
    OfflineToggle,
    SnapshotReRecall,

    // ── Advanced settings ──
    AdvancedDiagnostics,
    AdvancedPacing,
    AdvancedScale,

    // ── Main tab bar ──
    TabSetup,
    TabMacros,
    TabGangs,
    TabPanLink,
    TabSnapshots,
    TabMonitor,
    TabOscLog,
    TabInspector,

    // ── Shared widgets ──
    LongPressConfirm,

    // ── Macros ──
    MacroLearnReady,
    MacroLearnNeedsConn,
    MacroClearSelection,
    MacroDedupSelected,
    MacroDeleteSelected,
    MacroZeroDelaySelected,
    MacroStepSelectHint,
    MacroKeepOnlyStep,
    MacroDeleteStep,
    MacroResetStepDelay,
    MacroMirrorInbound,
    MacroQlabCueNumber,
    MacroAddSwatch,

    // ── Stream Deck ──
    StreamDeckClose,
    StreamDeckEnable,
    StreamDeckDisable,
    StreamDeckOpenPanel,

    // ── Pan Link ──
    PanLinkRevert,
    PanLinkApply,

    // ── Setup ──
    SetupConsoleIp,
    SetupConsoleIpWarning,
    SetupConsolePort,
    SetupLocalPort,
    SetupIpadConsolePort,
    SetupIpadLocalPort,
    SetupIpadReplyPort,
    SetupIpadListenPort,
    SetupIpadIp,
    SetupQlabHost,
    SetupQlabPort,
    SetupTriggerPort,
    SetupMonitorPort,
    SetupOpenShow,
    SetupSaveShow,
    SetupSaveShowAs,
    SetupAdvanced,

    // ── Snapshots ──
    SnapshotAutoUpdate,
    SnapshotConsoleFollowReq,
    SnapshotRecallNoScopeReq,
    SnapshotBulkShift,

    // ── Scope editor ──
    ScopeNoLiveParam,
    ScopeConsoleConflict,

    // ── Monitor ──
    MonitorAddProfile,
    MonitorEditProfile,
    MonitorProfilePin,
    MonitorReorder,
    MonitorReorderDone,

    // ── Diagnostics: OSC Log ──
    OscLogFilter,
    OscLogShowIn,
    OscLogShowOut,
    OscLogShowIpadToConsole,
    OscLogShowIpadFromConsole,
    OscLogPause,
    OscLogClear,

    // ── Diagnostics: Inspector ──
    InspectorFilter,
    InspectorTypeAll,
    InspectorTypeFilter,
    InspectorSortHeader,

    // ── Gang parameter sections ──
    GangFaderMutePan,
    GangName,
    GangInputGain,
    GangTrim,
    GangPolarity,
    GangBalanceWidth,
    GangDelay,
    GangDigitube,
    GangEq,
    GangDyn1,
    GangDyn2,
    GangSends,
    GangGroupRouting,
    GangInserts,
    GangCgMembership,
    GangGraphicEq,
    GangMatrixSends,

    // ── Appearance ──
    ThemeDark,
    ThemeLight,

    // ── Palettes ──
    PaletteCapture,
    PaletteRecapture,
    PaletteDelete,
    PaletteLinkedOther,

    // ── Snapshot / cue actions ──
    SnapshotCaptureNow,
    SnapshotRecall,
    SnapshotRecapture,
    SnapshotUndo,
    SnapshotApplyScope,
    SnapshotQlabCreateTrigger,
    SnapshotQlabExportFull,
    CueAdd,
    CueSaveChanges,

    // ── Scope-editor templates ──
    ScopeTemplateLoad,
    ScopeTemplateSave,

    // ── Monitor channel picker ──
    MonitorPickerCancel,
    MonitorPickerSave,

    // ── Gang controls ──
    GangEnableToggle,
    GangPause,
    GangNeedsEnable,
    GangRelative,
    GangAbsolute,
    GangPanOff,
    GangPanOn,
    GangPanReversed,
    GangEdit,

    // ── Scope-editor modes / actions ──
    ScopeModeScope,
    ScopeModePreWait,
    ScopeModeFade,
    ScopeClearAll,
    ScopeClearChanges,
    ScopeClearChangesDisabled,

    // ── Setup modes / network ──
    UiModeFull,
    UiModeLiveMusic,
    UiModeTheatre,
    ConnModeMode1,
    ConnModeMode2,
    ConnModeMode3,
    ConnModeDisabled,
    SetupNetwork,
    SetupCoverage,
    SetupNewShow,

    // ── Palettes (capture form) ──
    PaletteChannelType,
    PaletteChannelNumber,
    PaletteName,
    PaletteInclude,
    PaletteRename,

    // ── Monitor channel picker ──
    MonitorRipple,
    MonitorRippleCancel,
    MonitorRippleConfirm,
    MonitorDeselectAll,
    MonitorSelectAll,

    // ── Show recovery dialog ──
    RecoveryLoad,
    RecoveryLoadInvalid,
    RecoverySaveOriginal,
    RecoveryCancel,

    // ── Pan Link ──
    PanLinkClearSelection,

    // ── Recall-scope popup ──
    RecallScopePrevChannel,
    RecallScopeNextChannel,

    // ── Snapshots / cues (secondary) ──
    SnapshotEditScope,
    SnapshotClearScope,
    CueNumber,
    CueName,
    CueConsoleRow,
    SnapshotScopeOverride,
    SnapshotScopeOverrideTemplate,
    SnapshotPicker,
    ShiftApply,
    ShiftClose,
    ShiftFromRow,
    ShiftDelta,

    // ── More combos ──
    ScopeTemplateCombo,
    GangChannelType,

    // ── Gang editor form ──
    GangNewName,
    GangNewMembers,
    GangSave,
    GangCancelEdit,

    // ── Macros: step builder ──
    MacroNewName,
    MacroCreate,
    MacroRun,
    MacroLearnStopSave,
    MacroLearnDiscard,
    MacroRename,
    MacroTrackDirty,
    MacroAddStepKind,
    MacroAddStepCommit,
    MacroStepMode,
    MacroStepValue,
    MacroStepDelay,
    MacroWizardChannelType,
    MacroWizardChannelNumber,
    MacroWizardSection,
    MacroWizardParameter,
    MacroTargetMacro,
    MacroTargetSnapshot,
    MacroTargetPalette,
    MacroPaletteChannelType,
    MacroPaletteChannelNumber,

    // ── Stream Deck: setup panel ──
    StreamDeckDevice,
    StreamDeckButtonMacro,
    StreamDeckAddStep,
}

/// One catalog entry: the stable JSON key id plus the canonical English text.
pub struct HelpMeta {
    /// Stable key used in translation JSON files. **Never rename once
    /// shipped** — it is the contract with translator-maintained files.
    pub id: &'static str,
    /// Canonical English text — the reference shown when no translation
    /// exists for the active language.
    pub english: &'static str,
}

/// The catalog: single source of truth mapping each [`HelpKey`] to its JSON
/// id and English text. Exhaustive, so the compiler guarantees no key is
/// ever missing its English reference.
fn meta(key: HelpKey) -> HelpMeta {
    use HelpKey::*;
    let (id, english): (&'static str, &'static str) = match key {
        // ── Generic ──
        Dismiss => ("app.dismiss", "Dismiss"),
        PickColor => ("color.pick", "Pick a color"),

        // ── Cue transport / cue list ──
        CueUndo => ("cue.undo", "Undo the last cue / snapshot recall."),
        CuePrev => ("cue.prev", "Recall the previous cue."),
        CueGo => ("cue.go", "Recall the next cue."),
        CueSkip => (
            "cue.skip",
            "Skip — advance to the next cue without recalling it.",
        ),
        CueListOpen => ("cue.list_open", "Open the cue list."),
        CueListJump => ("cue.list_jump", "Jump to this cue and recall it."),

        // ── Top bar / banners ──
        OfflineToggle => (
            "app.offline_toggle",
            "Offline mode: drops every inbound and outbound OSC message. \
             Lets you edit show data without affecting the desk. \
             Toggle back to Online to resume — the state mirror will be \
             stale until you click Refresh on the Setup tab.",
        ),
        SnapshotReRecall => (
            "snapshot.re_recall",
            "Re-recall this snapshot now to confirm nothing drifted",
        ),

        // ── Advanced settings ──
        AdvancedDiagnostics => (
            "advanced.diagnostics",
            "Adds OSC Log and Inspector tabs to the main tab bar.",
        ),
        AdvancedPacing => (
            "advanced.pacing",
            "Delay inserted between consecutive OSC messages so a long sequence \
             doesn't flood the console. Applies to snapshot recall and macros. \
             0 = no pacing.",
        ),
        AdvancedScale => (
            "advanced.scale",
            "Manual UI size multiplier, applied on top of the automatic display \
             scaling. 100% = automatic scaling only.",
        ),

        // ── Main tab bar ──
        TabSetup => (
            "tab.setup",
            "Connection, display mode, and show-file load/save.",
        ),
        TabMacros => ("tab.macros", "Record, edit and fire macros."),
        TabGangs => (
            "tab.gangs",
            "Link channels so a parameter change on one propagates to the others.",
        ),
        TabPanLink => (
            "tab.pan_link",
            "Link an input channel's pan to its aux send pans.",
        ),
        TabSnapshots => (
            "tab.snapshots",
            "Capture and recall console state as snapshots and cues.",
        ),
        TabMonitor => (
            "tab.monitor",
            "Live channel meters and remote monitoring clients.",
        ),
        TabOscLog => (
            "tab.osc_log",
            "Live log of OSC traffic to and from the console.",
        ),
        TabInspector => ("tab.inspector", "Inspect the live parameter state mirror."),

        // ── Shared widgets ──
        LongPressConfirm => ("widget.long_press_confirm", "Hold {ms} ms to confirm."),

        // ── Macros ──
        MacroLearnReady => (
            "macro.learn_ready",
            "Record every parameter change made on the console into a new macro.",
        ),
        MacroLearnNeedsConn => (
            "macro.learn_needs_conn",
            "Connect to the console first — Learn captures inbound parameter changes.",
        ),
        MacroClearSelection => ("macro.clear_selection", "Clear the multi-selection"),
        MacroDedupSelected => (
            "macro.dedup_selected",
            "For every selected step, drop other steps with \
             the same (channel, parameter)",
        ),
        MacroDeleteSelected => ("macro.delete_selected", "Remove every selected step"),
        MacroZeroDelaySelected => (
            "macro.zero_delay_selected",
            "Set delay to 0 ms on every selected step",
        ),
        MacroStepSelectHint => (
            "macro.step_select_hint",
            "Click to select; Ctrl/Cmd-click to toggle; \
             Shift-click to range-select.",
        ),
        MacroKeepOnlyStep => (
            "macro.keep_only_step",
            "Keep only this step for its (channel, parameter); \
             remove the rest",
        ),
        MacroDeleteStep => ("macro.delete_step", "Delete this step"),
        MacroResetStepDelay => ("macro.reset_step_delay", "Reset this step's delay to 0 ms"),
        MacroMirrorInbound => (
            "macro.mirror_inbound",
            "Mirror the most-recent inbound parameter from the console into \
             the form below so you can hit Add Step without retyping.",
        ),
        MacroQlabCueNumber => (
            "macro.qlab_cue_number",
            "QLab cue number — free-form string (numbers, letters, dots). \
             Sent as `/cue/<number>/start`.",
        ),
        MacroAddSwatch => (
            "macro.add_swatch",
            "Add the current color to your saved swatches (per show)",
        ),

        // ── Stream Deck ──
        StreamDeckClose => (
            "streamdeck.close",
            "Close Stream Deck setup and go back to the \
             macro step editor.",
        ),
        StreamDeckEnable => (
            "streamdeck.enable",
            "Enable the Stream Deck integration. Auto-connects to \
             the previously-selected device if it's plugged in; \
             otherwise open Setup… to pick one.",
        ),
        StreamDeckDisable => (
            "streamdeck.disable",
            "Disable the Stream Deck integration. Disconnects the \
             device but preserves your button maps.",
        ),
        StreamDeckOpenPanel => (
            "streamdeck.open_panel",
            "Open the Stream Deck panel — device selection, \
             button grid, per-button macro sequences.",
        ),

        // ── Pan Link ──
        PanLinkRevert => (
            "panlink.revert",
            "Discard staged changes and resync with the live bindings.",
        ),
        PanLinkApply => (
            "panlink.apply",
            "Commit staged links. Aux send pans don't move until \
             the input main pan next changes.",
        ),

        // ── Setup ──
        SetupConsoleIp => ("setup.console_ip", "S21 console IP address."),
        SetupConsoleIpWarning => (
            "setup.console_ip_warning",
            "Console IP isn't on the selected NIC's network \
             (first two octets differ — unreachable even with /16). \
             Click to dismiss.",
        ),
        SetupConsolePort => (
            "setup.console_port",
            "Console port — daemon sends GP OSC here.",
        ),
        SetupLocalPort => (
            "setup.local_port",
            "Local port the daemon listens on for GP OSC from the console.",
        ),
        SetupIpadConsolePort => (
            "setup.ipad_console_port",
            "Console iPad-protocol port — daemon sends here.",
        ),
        SetupIpadLocalPort => (
            "setup.ipad_local_port",
            "Local port the daemon listens on for iPad-protocol traffic.",
        ),
        SetupIpadReplyPort => (
            "setup.ipad_reply_port",
            "Port on the iPad — daemon sends to it here.",
        ),
        SetupIpadListenPort => (
            "setup.ipad_listen_port",
            "Local port the daemon listens on for iPad→daemon traffic.",
        ),
        SetupQlabPort => (
            "setup.qlab_port",
            "QLab's OSC listen port — daemon sends cue-build commands here.",
        ),
        SetupTriggerPort => (
            "setup.trigger_port",
            "Local port the daemon listens on for cue triggers from QLab.",
        ),
        SetupMonitorPort => (
            "setup.monitor_port",
            "Local port for the Flutter monitor app — leave blank to disable.",
        ),
        SetupOpenShow => (
            "setup.open_show",
            "Pick a show file and load it. Save As… to save to \
             a new path.",
        ),
        SetupSaveShow => (
            "setup.save_show",
            "Save to the path shown above. If empty, prompts for a \
             location.",
        ),
        SetupSaveShowAs => (
            "setup.save_show_as",
            "Pick a new location and save there. Useful when the \
             path field already points at a different file.",
        ),
        SetupIpadIp => (
            "setup.ipad_ip",
            "iPad device IP (required for iPad Proxy mode) — read it off the DiGiCo iPad app.",
        ),
        SetupQlabHost => (
            "setup.qlab_host",
            "QLab host — usually 127.0.0.1 if QLab runs on this machine.",
        ),
        SetupAdvanced => (
            "setup.advanced",
            "Pacing, diagnostics, and other application preferences.",
        ),

        // ── Snapshots ──
        SnapshotAutoUpdate => (
            "snapshot.auto_update",
            "When recalling a snapshot, write any dirty parameters \
             within the previous snapshot's scope back into it. \
             Captures mid-show tweaks automatically.",
        ),
        SnapshotConsoleFollowReq => (
            "snapshot.console_follow_req",
            "Requires Mode 2 or Mode 3 (iPad protocol) — the console \
             snapshot list is only reachable via that protocol.",
        ),
        SnapshotRecallNoScopeReq => (
            "snapshot.recall_no_scope_req",
            "Only available for snapshots captured with 'Apply scope on recall' \
             — ApplyOnSave snapshots already filtered at capture time, so there \
             is nothing outside the saved scope to recall.",
        ),
        SnapshotBulkShift => (
            "snapshot.bulk_shift",
            "Bulk-shift every cue's console snapshot when you've \
             inserted or removed snapshots on the console.",
        ),

        // ── Scope editor ──
        ScopeNoLiveParam => ("scope.no_live_param", "no live parameter"),
        ScopeConsoleConflict => (
            "scope.console_conflict",
            "In console session scope, not safed for this channel",
        ),

        // ── Monitor ──
        MonitorAddProfile => (
            "monitor.add_profile",
            "Create a monitoring profile — choose which auxes and inputs a \
             musician's monitor app may control.",
        ),
        MonitorEditProfile => (
            "monitor.edit_profile",
            "Edit this profile's name and permitted channels.",
        ),
        MonitorProfilePin => (
            "monitor.profile_pin",
            "Optional PIN for the web (browser) login. Leave it blank for \
             name-only login; the native monitor app ignores it.",
        ),
        MonitorReorder => (
            "monitor.reorder",
            "Rearrange this profile's channel order — drag the badges, then \
             click Done.",
        ),
        MonitorReorderDone => (
            "monitor.reorder_done",
            "Finish reordering and save the new channel order.",
        ),

        // ── Diagnostics: OSC Log ──
        OscLogFilter => (
            "osc_log.filter",
            "Show only messages whose OSC path contains this text.",
        ),
        OscLogShowIn => (
            "osc_log.show_in",
            "Show inbound GP OSC messages (console → daemon).",
        ),
        OscLogShowOut => (
            "osc_log.show_out",
            "Show outbound GP OSC messages (daemon → console).",
        ),
        OscLogShowIpadToConsole => (
            "osc_log.show_ipad_to_console",
            "Show iPad-protocol messages heading to the console (iPad → Console).",
        ),
        OscLogShowIpadFromConsole => (
            "osc_log.show_ipad_from_console",
            "Show iPad-protocol messages heading to the iPad (Console → iPad).",
        ),
        OscLogPause => (
            "osc_log.pause",
            "Pause or resume live capture. While paused the list stays frozen \
             for inspection; traffic still flows.",
        ),
        OscLogClear => ("osc_log.clear", "Discard every captured message."),

        // ── Diagnostics: Inspector ──
        InspectorFilter => (
            "inspector.filter",
            "Filter rows whose channel, parameter, or value contains this text.",
        ),
        InspectorTypeAll => (
            "inspector.type_all",
            "Show parameters for every channel type.",
        ),
        InspectorTypeFilter => (
            "inspector.type_filter",
            "Show only parameters for this channel type.",
        ),
        InspectorSortHeader => (
            "inspector.sort_header",
            "Click to sort by this column; click again to reverse.",
        ),

        // ── Gang parameter sections ──
        GangFaderMutePan => (
            "gang.fader_mute_pan",
            "Channel fader level, mute and solo. Pan has its own OFF/ON/REVERSED \
             control on each gang and is no longer linked by this section.",
        ),
        GangName => ("gang.name", "Channel name string."),
        GangInputGain => (
            "gang.input_gain",
            "Head-amp gain, total/computed gain, gain tracking, phantom \
             power, main/alt input switch, stereo mode (input channels only).",
        ),
        GangTrim => (
            "gang.trim",
            "Channel trim level (-40 dB to +40 dB). Available on input, \
             aux, group and matrix channels.",
        ),
        GangPolarity => (
            "gang.polarity",
            "Phase / polarity invert. Available on input, aux, group and \
             matrix channels.",
        ),
        GangBalanceWidth => (
            "gang.balance_width",
            "Stereo balance (-1 to +1) and width — applies to stereo \
             input channels only (GP OSC, input-only per the S21 chart).",
        ),
        GangDelay => ("gang.delay", "Channel delay time and enable."),
        GangDigitube => ("gang.digitube", "Digitube drive, bias and enable."),
        GangEq => (
            "gang.eq",
            "Parametric EQ — band gains, Q, freq, dynamic-EQ \
             settings, EQ on/off.",
        ),
        GangDyn1 => (
            "gang.dyn1",
            "Dynamics 1 — compressor / gate parameters and on/off.",
        ),
        GangDyn2 => (
            "gang.dyn2",
            "Dynamics 2 — second processor parameters and on/off.",
        ),
        GangSends => (
            "gang.sends",
            "Aux send levels and on/off across all aux buses. \
             Only propagates between members of the same channel type.",
        ),
        GangGroupRouting => (
            "gang.group_routing",
            "Group routing — which group buses the channel feeds. \
             Only propagates between same-type members.",
        ),
        GangInserts => ("gang.inserts", "Insert A / B send / return enable."),
        GangCgMembership => (
            "gang.cg_membership",
            "Control Group membership — which CGs the channel belongs \
             to. Settable via the iPad protocol only (Mode 2 / Mode 3) \
             using CGs_level / CGs_mute bitmasks; GP OSC has no \
             CG-membership writer. Only propagates between same-type \
             members.",
        ),
        GangGraphicEq => (
            "gang.graphic_eq",
            "Graphic EQ band gains. Only meaningful when ganging \
             Graphic EQ channels (GEQ1, GEQ2, …).",
        ),
        GangMatrixSends => (
            "gang.matrix_sends",
            "Matrix-send levels and on/off. Only meaningful when \
             ganging Matrix Input channels (MI1, MI2, …).",
        ),

        // ── Appearance ──
        ThemeDark => ("theme.dark", "Use the dark colour theme."),
        ThemeLight => ("theme.light", "Use the light colour theme."),

        // ── Palettes ──
        PaletteCapture => (
            "palette.capture",
            "Capture the selected channel's chosen processing into a new palette.",
        ),
        PaletteRecapture => (
            "palette.recapture",
            "Overwrite this palette with the current state of its channel.",
        ),
        PaletteDelete => (
            "palette.delete",
            "Delete this palette and unlink it from any snapshots.",
        ),
        PaletteLinkedOther => ("palette.linked_other", "Currently linked to: \"{name}\"."),

        // ── Snapshot / cue actions ──
        SnapshotCaptureNow => (
            "snapshot.capture_now",
            "Capture the current console state into a new snapshot, filtered by the \
             scope above.",
        ),
        SnapshotRecall => (
            "snapshot.recall",
            "Recall the selected snapshot to the console.",
        ),
        SnapshotRecapture => (
            "snapshot.recapture",
            "Overwrite the selected snapshot with the current console state.",
        ),
        SnapshotUndo => (
            "snapshot.undo",
            "Undo the last snapshot recall, restoring the previous values.",
        ),
        SnapshotApplyScope => (
            "snapshot.apply_scope",
            "When the scope is applied: 'On save' stores only in-scope parameters; \
             'On recall' captures the whole console state and filters it on recall \
             (allowing 'Recall without scope').",
        ),
        SnapshotQlabCreateTrigger => (
            "snapshot.qlab_create_trigger",
            "Create a single network cue in QLab whose customString fires \
             /snapshot/recall <name>. Sent to {target}.",
        ),
        SnapshotQlabExportFull => (
            "snapshot.qlab_export_full",
            "Export one network cue per parameter to QLab (grouped). Sent to {target}.",
        ),
        CueAdd => (
            "cue.add",
            "Add a new cue to the list from the fields above.",
        ),
        CueSaveChanges => ("cue.save_changes", "Save the edits to the selected cue."),

        // ── Scope-editor templates ──
        ScopeTemplateLoad => (
            "scope.template_load",
            "Replace the current selection with the chosen scope template.",
        ),
        ScopeTemplateSave => (
            "scope.template_save",
            "Save the current selection as a new scope template named on the left.",
        ),

        // ── Monitor channel picker ──
        MonitorPickerCancel => (
            "monitor.picker_cancel",
            "Discard changes and close without saving the profile.",
        ),
        MonitorPickerSave => (
            "monitor.picker_save",
            "Save this monitoring profile and close.",
        ),

        // ── Gang controls ──
        GangEnableToggle => (
            "gang.enable_toggle",
            "Enable or disable this gang. While off, moving a member doesn't \
             propagate to the others.",
        ),
        GangPause => (
            "gang.pause",
            "Temporarily suspend propagation without unlinking — members keep \
             their grouping; moves stop mirroring until you resume.",
        ),
        GangNeedsEnable => (
            "gang.needs_enable",
            "Enable the gang first to change its mode or pause it.",
        ),
        GangRelative => (
            "gang.relative",
            "Relative mode: moving a member shifts the others by the same delta, \
             preserving their offsets.",
        ),
        GangAbsolute => (
            "gang.absolute",
            "Absolute mode: moving a member sets the others to the same value.",
        ),
        GangPanOff => (
            "gang.pan_off",
            "Pan OFF: channel pan is not linked across this gang — only the \
             selected sections (fader/mute, EQ, etc.) propagate.",
        ),
        GangPanOn => (
            "gang.pan_on",
            "Pan ON: channel pan is linked and follows the gang's Relative / \
             Absolute mode like any other parameter.",
        ),
        GangPanReversed => (
            "gang.pan_reversed",
            "Pan REVERSED: the pair pans away from centre symmetrically — move \
             one channel right and its partner mirrors left. Needs exactly 2 members.",
        ),
        GangEdit => (
            "gang.edit",
            "Load this gang into the editor below to change its members or linked \
             sections.",
        ),

        // ── Scope-editor modes / actions ──
        ScopeModeScope => (
            "scope.mode_scope",
            "Edit which parameters each channel contributes to the scope.",
        ),
        ScopeModePreWait => (
            "scope.mode_pre_wait",
            "Edit each parameter's pre-wait — the delay before it fires on recall.",
        ),
        ScopeModeFade => (
            "scope.mode_fade",
            "Edit each parameter's fade time on recall.",
        ),
        ScopeClearAll => ("scope.clear_all", "Clear the entire scope selection."),
        ScopeClearChanges => (
            "scope.clear_changes",
            "Discard the highlighted live changes — clears the dirty markers only; \
             nothing is sent to the console.",
        ),
        ScopeClearChangesDisabled => (
            "scope.clear_changes_disabled",
            "No unsent changes to clear.",
        ),

        // ── Setup modes / network ──
        UiModeFull => (
            "ui_mode.full",
            "Full mode: every tab is shown — Snapshots, Macros, Gangs, Pan Link, \
             Monitor, Palettes.",
        ),
        UiModeLiveMusic => (
            "ui_mode.live_music",
            "Live Music mode: hides Snapshots and the cue transport — for bands \
             working without a cue list.",
        ),
        UiModeTheatre => (
            "ui_mode.theatre",
            "Theatre mode: cue-driven workflow; hides the Monitor tab.",
        ),
        ConnModeMode1 => (
            "conn_mode.mode1",
            "Mode 1: GP OSC only — the open protocol, no iPad.",
        ),
        ConnModeMode2 => (
            "conn_mode.mode2",
            "Mode 2: direct iPad protocol — the daemon talks to the console as an iPad.",
        ),
        ConnModeMode3 => (
            "conn_mode.mode3",
            "Mode 3: iPad proxy — the daemon sits between a real iPad and the console.",
        ),
        ConnModeDisabled => (
            "conn_mode.disabled",
            "Disconnect to change the connection mode.",
        ),
        SetupNetwork => (
            "setup.network",
            "Local network interface the daemon binds to for OSC — pick the NIC on \
             the console's network, or All interfaces.",
        ),
        SetupCoverage => (
            "setup.coverage",
            "Show which console parameters the daemon currently mirrors.",
        ),
        SetupNewShow => (
            "setup.new_show",
            "Start a new, empty show — clears the current snapshots, macros, gangs \
             and palettes.",
        ),

        // ── Palettes (capture form) ──
        PaletteChannelType => (
            "palette.channel_type",
            "Channel type to capture the palette from.",
        ),
        PaletteChannelNumber => (
            "palette.channel_number",
            "Channel number to capture the palette from.",
        ),
        PaletteName => ("palette.name", "Name for the new palette."),
        PaletteInclude => (
            "palette.include",
            "Include this processing block (EQ / dynamics) when capturing the palette.",
        ),
        PaletteRename => (
            "palette.rename",
            "Rename this palette — press Enter to apply, Escape to cancel.",
        ),

        // ── Monitor channel picker ──
        MonitorRipple => (
            "monitor.ripple",
            "Range-select inputs: click the first, then the last, to select \
             everything between them.",
        ),
        MonitorRippleCancel => ("monitor.ripple_cancel", "Cancel the range selection."),
        MonitorRippleConfirm => (
            "monitor.ripple_confirm",
            "Select every input between the two you picked.",
        ),
        MonitorDeselectAll => ("monitor.deselect_all", "Clear the input selection."),
        MonitorSelectAll => ("monitor.select_all", "Select every input."),

        // ── Show recovery dialog ──
        RecoveryLoad => (
            "recovery.load",
            "Load this backup / autosave copy into the show.",
        ),
        RecoveryLoadInvalid => (
            "recovery.load_invalid",
            "This copy is corrupt and can't be loaded.",
        ),
        RecoverySaveOriginal => (
            "recovery.save_original",
            "Write the recovered show back to the original file path.",
        ),
        RecoveryCancel => ("recovery.cancel", "Close without recovering."),

        // ── Pan Link ──
        PanLinkClearSelection => (
            "panlink.clear_selection",
            "Clear the selected input channels.",
        ),

        // ── Recall-scope popup ──
        RecallScopePrevChannel => ("recall_scope.prev_channel", "Previous channel."),
        RecallScopeNextChannel => ("recall_scope.next_channel", "Next channel."),

        // ── Snapshots / cues (secondary) ──
        SnapshotEditScope => (
            "snapshot.edit_scope",
            "Open the scope editor to choose which parameters this snapshot \
             captures or recalls.",
        ),
        SnapshotClearScope => ("snapshot.clear_scope", "Clear the scope selection."),
        CueNumber => (
            "cue.number",
            "Cue number — orders the cue list (e.g. 1, 2.5).",
        ),
        CueName => ("cue.name", "Optional name for the cue."),
        CueConsoleRow => (
            "cue.console_row",
            "Console snapshot row to recall on the desk (iPad-protocol modes).",
        ),
        SnapshotScopeOverride => (
            "snapshot.scope_override",
            "Recall this cue with a different scope than the snapshot's own.",
        ),
        SnapshotScopeOverrideTemplate => (
            "snapshot.scope_override_template",
            "Scope template to apply when recalling this cue.",
        ),
        SnapshotPicker => (
            "snapshot.picker",
            "Choose which app snapshot this cue recalls (type to filter).",
        ),
        ShiftApply => ("shift.apply", "Apply the row shift to every matching cue."),
        ShiftClose => ("shift.close", "Close without shifting."),
        ShiftFromRow => (
            "shift.from_row",
            "Shift cues whose console snapshot row is at or above this number.",
        ),
        ShiftDelta => (
            "shift.delta",
            "Amount to add to each matching row — negative shifts down; rows \
             reaching 0 are cleared.",
        ),

        // ── More combos ──
        ScopeTemplateCombo => (
            "scope.template_combo",
            "Pick a saved scope template to load, update, or delete.",
        ),
        GangChannelType => ("gang.channel_type", "Channel type for the gang's members."),

        // ── Gang editor form ──
        GangNewName => ("gang.new_name", "Name for the new gang."),
        GangNewMembers => (
            "gang.new_members",
            "Channels to include — range notation like 1-4,7,12 (or I1-4,A1-2,G5 \
             for mixed types).",
        ),
        GangSave => (
            "gang.save",
            "Create the gang from the fields above — saves edits when editing an \
             existing gang.",
        ),
        GangCancelEdit => ("gang.cancel_edit", "Cancel editing and clear the form."),

        // ── Macros: step builder ──
        MacroNewName => ("macro.new_name", "Name for the new macro."),
        MacroCreate => (
            "macro.create",
            "Create a new empty macro with the name on the left.",
        ),
        MacroRun => (
            "macro.run",
            "Run the selected macro now — fires its steps in order.",
        ),
        MacroLearnStopSave => (
            "macro.learn_stop_save",
            "Stop recording and save the captured steps as a new macro.",
        ),
        MacroLearnDiscard => (
            "macro.learn_discard",
            "Stop recording and discard the captured steps.",
        ),
        MacroRename => (
            "macro.rename",
            "Rename this macro — edits apply immediately to the list and any Stream Deck labels.",
        ),
        MacroTrackDirty => (
            "macro.track_dirty",
            "Treat this macro's parameters as modified — its writes mark the affected \
             parameters dirty so auto-update can capture them into snapshots.",
        ),
        MacroAddStepKind => (
            "macro.add_step_kind",
            "Choose what this step does — write a parameter, drive a cue / connection, \
             run another macro, recall a snapshot or palette, or send a QLab command.",
        ),
        MacroAddStepCommit => (
            "macro.add_step_commit",
            "Append a new step to the macro from the fields on this row.",
        ),
        MacroStepMode => (
            "macro.step_mode",
            "How the value is applied: Toggle flips on/off, Fixed sets an absolute value, \
             Relative adds an offset to the live value.",
        ),
        MacroStepValue => (
            "macro.step_value",
            "Value written by this step — an absolute value in Fixed mode, an offset in Relative mode.",
        ),
        MacroStepDelay => (
            "macro.step_delay",
            "Delay in milliseconds before this step fires during playback.",
        ),
        MacroWizardChannelType => (
            "macro.wizard_channel_type",
            "Channel type this parameter step targets.",
        ),
        MacroWizardChannelNumber => (
            "macro.wizard_channel_number",
            "Channel number this parameter step targets.",
        ),
        MacroWizardSection => (
            "macro.wizard_section",
            "Processing section (fader, EQ, dynamics, sends, …) the parameter lives in.",
        ),
        MacroWizardParameter => (
            "macro.wizard_parameter",
            "Specific parameter within the chosen section that this step writes.",
        ),
        MacroTargetMacro => ("macro.target_macro", "Macro this step runs when fired."),
        MacroTargetSnapshot => (
            "macro.target_snapshot",
            "Snapshot this step recalls when fired.",
        ),
        MacroTargetPalette => (
            "macro.target_palette",
            "Palette this step recalls when fired.",
        ),
        MacroPaletteChannelType => (
            "macro.palette_channel_type",
            "Channel type to apply the recalled palette to.",
        ),
        MacroPaletteChannelNumber => (
            "macro.palette_channel_number",
            "Channel number to apply the recalled palette to.",
        ),

        // ── Stream Deck: setup panel ──
        StreamDeckDevice => (
            "streamdeck.device",
            "Pick the Stream Deck to control, or a template to edit a button map offline.",
        ),
        StreamDeckButtonMacro => (
            "streamdeck.button_macro",
            "Macro to add as the next step for the selected button.",
        ),
        StreamDeckAddStep => (
            "streamdeck.add_step",
            "Add the chosen macro as a step on the selected button.",
        ),
    };
    HelpMeta { id, english }
}

/// Every key, in declaration order — drives the translator-template dump and
/// the per-locale completeness test.
pub const ALL_KEYS: &[HelpKey] = &[
    HelpKey::Dismiss,
    HelpKey::PickColor,
    HelpKey::CueUndo,
    HelpKey::CuePrev,
    HelpKey::CueGo,
    HelpKey::CueSkip,
    HelpKey::CueListOpen,
    HelpKey::CueListJump,
    HelpKey::OfflineToggle,
    HelpKey::SnapshotReRecall,
    HelpKey::AdvancedDiagnostics,
    HelpKey::AdvancedPacing,
    HelpKey::AdvancedScale,
    HelpKey::TabSetup,
    HelpKey::TabMacros,
    HelpKey::TabGangs,
    HelpKey::TabPanLink,
    HelpKey::TabSnapshots,
    HelpKey::TabMonitor,
    HelpKey::TabOscLog,
    HelpKey::TabInspector,
    HelpKey::LongPressConfirm,
    HelpKey::MacroLearnReady,
    HelpKey::MacroLearnNeedsConn,
    HelpKey::MacroClearSelection,
    HelpKey::MacroDedupSelected,
    HelpKey::MacroDeleteSelected,
    HelpKey::MacroZeroDelaySelected,
    HelpKey::MacroStepSelectHint,
    HelpKey::MacroKeepOnlyStep,
    HelpKey::MacroDeleteStep,
    HelpKey::MacroResetStepDelay,
    HelpKey::MacroMirrorInbound,
    HelpKey::MacroQlabCueNumber,
    HelpKey::MacroAddSwatch,
    HelpKey::StreamDeckClose,
    HelpKey::StreamDeckEnable,
    HelpKey::StreamDeckDisable,
    HelpKey::StreamDeckOpenPanel,
    HelpKey::PanLinkRevert,
    HelpKey::PanLinkApply,
    HelpKey::SetupConsoleIp,
    HelpKey::SetupConsoleIpWarning,
    HelpKey::SetupConsolePort,
    HelpKey::SetupLocalPort,
    HelpKey::SetupIpadConsolePort,
    HelpKey::SetupIpadLocalPort,
    HelpKey::SetupIpadReplyPort,
    HelpKey::SetupIpadListenPort,
    HelpKey::SetupIpadIp,
    HelpKey::SetupQlabHost,
    HelpKey::SetupQlabPort,
    HelpKey::SetupTriggerPort,
    HelpKey::SetupMonitorPort,
    HelpKey::SetupOpenShow,
    HelpKey::SetupSaveShow,
    HelpKey::SetupSaveShowAs,
    HelpKey::SetupAdvanced,
    HelpKey::SnapshotAutoUpdate,
    HelpKey::SnapshotConsoleFollowReq,
    HelpKey::SnapshotRecallNoScopeReq,
    HelpKey::SnapshotBulkShift,
    HelpKey::ScopeNoLiveParam,
    HelpKey::ScopeConsoleConflict,
    HelpKey::MonitorAddProfile,
    HelpKey::MonitorEditProfile,
    HelpKey::MonitorProfilePin,
    HelpKey::MonitorReorder,
    HelpKey::MonitorReorderDone,
    HelpKey::OscLogFilter,
    HelpKey::OscLogShowIn,
    HelpKey::OscLogShowOut,
    HelpKey::OscLogShowIpadToConsole,
    HelpKey::OscLogShowIpadFromConsole,
    HelpKey::OscLogPause,
    HelpKey::OscLogClear,
    HelpKey::InspectorFilter,
    HelpKey::InspectorTypeAll,
    HelpKey::InspectorTypeFilter,
    HelpKey::InspectorSortHeader,
    HelpKey::GangFaderMutePan,
    HelpKey::GangName,
    HelpKey::GangInputGain,
    HelpKey::GangTrim,
    HelpKey::GangPolarity,
    HelpKey::GangBalanceWidth,
    HelpKey::GangDelay,
    HelpKey::GangDigitube,
    HelpKey::GangEq,
    HelpKey::GangDyn1,
    HelpKey::GangDyn2,
    HelpKey::GangSends,
    HelpKey::GangGroupRouting,
    HelpKey::GangInserts,
    HelpKey::GangCgMembership,
    HelpKey::GangGraphicEq,
    HelpKey::GangMatrixSends,
    HelpKey::ThemeDark,
    HelpKey::ThemeLight,
    HelpKey::PaletteCapture,
    HelpKey::PaletteRecapture,
    HelpKey::PaletteDelete,
    HelpKey::PaletteLinkedOther,
    HelpKey::SnapshotCaptureNow,
    HelpKey::SnapshotRecall,
    HelpKey::SnapshotRecapture,
    HelpKey::SnapshotUndo,
    HelpKey::SnapshotApplyScope,
    HelpKey::SnapshotQlabCreateTrigger,
    HelpKey::SnapshotQlabExportFull,
    HelpKey::CueAdd,
    HelpKey::CueSaveChanges,
    HelpKey::ScopeTemplateLoad,
    HelpKey::ScopeTemplateSave,
    HelpKey::MonitorPickerCancel,
    HelpKey::MonitorPickerSave,
    HelpKey::GangEnableToggle,
    HelpKey::GangPause,
    HelpKey::GangNeedsEnable,
    HelpKey::GangRelative,
    HelpKey::GangAbsolute,
    HelpKey::GangPanOff,
    HelpKey::GangPanOn,
    HelpKey::GangPanReversed,
    HelpKey::GangEdit,
    HelpKey::ScopeModeScope,
    HelpKey::ScopeModePreWait,
    HelpKey::ScopeModeFade,
    HelpKey::ScopeClearAll,
    HelpKey::ScopeClearChanges,
    HelpKey::ScopeClearChangesDisabled,
    HelpKey::UiModeFull,
    HelpKey::UiModeLiveMusic,
    HelpKey::UiModeTheatre,
    HelpKey::ConnModeMode1,
    HelpKey::ConnModeMode2,
    HelpKey::ConnModeMode3,
    HelpKey::ConnModeDisabled,
    HelpKey::SetupNetwork,
    HelpKey::SetupCoverage,
    HelpKey::SetupNewShow,
    HelpKey::PaletteChannelType,
    HelpKey::PaletteChannelNumber,
    HelpKey::PaletteName,
    HelpKey::PaletteInclude,
    HelpKey::PaletteRename,
    HelpKey::MonitorRipple,
    HelpKey::MonitorRippleCancel,
    HelpKey::MonitorRippleConfirm,
    HelpKey::MonitorDeselectAll,
    HelpKey::MonitorSelectAll,
    HelpKey::RecoveryLoad,
    HelpKey::RecoveryLoadInvalid,
    HelpKey::RecoverySaveOriginal,
    HelpKey::RecoveryCancel,
    HelpKey::PanLinkClearSelection,
    HelpKey::RecallScopePrevChannel,
    HelpKey::RecallScopeNextChannel,
    HelpKey::SnapshotEditScope,
    HelpKey::SnapshotClearScope,
    HelpKey::CueNumber,
    HelpKey::CueName,
    HelpKey::CueConsoleRow,
    HelpKey::SnapshotScopeOverride,
    HelpKey::SnapshotScopeOverrideTemplate,
    HelpKey::SnapshotPicker,
    HelpKey::ShiftApply,
    HelpKey::ShiftClose,
    HelpKey::ShiftFromRow,
    HelpKey::ShiftDelta,
    HelpKey::ScopeTemplateCombo,
    HelpKey::GangChannelType,
    HelpKey::GangNewName,
    HelpKey::GangNewMembers,
    HelpKey::GangSave,
    HelpKey::GangCancelEdit,
    HelpKey::MacroNewName,
    HelpKey::MacroCreate,
    HelpKey::MacroRun,
    HelpKey::MacroLearnStopSave,
    HelpKey::MacroLearnDiscard,
    HelpKey::MacroRename,
    HelpKey::MacroTrackDirty,
    HelpKey::MacroAddStepKind,
    HelpKey::MacroAddStepCommit,
    HelpKey::MacroStepMode,
    HelpKey::MacroStepValue,
    HelpKey::MacroStepDelay,
    HelpKey::MacroWizardChannelType,
    HelpKey::MacroWizardChannelNumber,
    HelpKey::MacroWizardSection,
    HelpKey::MacroWizardParameter,
    HelpKey::MacroTargetMacro,
    HelpKey::MacroTargetSnapshot,
    HelpKey::MacroTargetPalette,
    HelpKey::MacroPaletteChannelType,
    HelpKey::MacroPaletteChannelNumber,
    HelpKey::StreamDeckDevice,
    HelpKey::StreamDeckButtonMacro,
    HelpKey::StreamDeckAddStep,
];

/// The stable JSON key id for a help bubble (the translator-facing key).
pub fn key_id(key: HelpKey) -> &'static str {
    meta(key).id
}

/// The canonical English text for a help bubble.
pub fn english(key: HelpKey) -> &'static str {
    meta(key).english
}

/// Resolve a help bubble to the active language, falling back to the English
/// reference per-key when no translation exists. Returns a `Cow` so the common
/// English path allocates nothing; only a translated bubble clones a short
/// string.
pub fn help(key: HelpKey) -> Cow<'static, str> {
    let m = meta(key);
    match active_locale() {
        None => Cow::Borrowed(m.english),
        Some(loc) => match loc.get(m.id) {
            Some(translated) => Cow::Owned(translated.to_owned()),
            None => Cow::Borrowed(m.english),
        },
    }
}

// ─── Runtime-loaded translations ────────────────────────────────────────
//
// The UI is English-only; only these help bubbles localize. Translations live
// in external JSON files (`<config>/s21_hijack/locales/<code>.json`), each a
// flat `{ "<key id>": "<text>", "_name": "<display name>" }` object. English
// is never a file — it is the in-binary reference from `meta()`, so the app
// always works even with no locale files present. A new language is added by
// dropping a file; no recompile.

/// Language code for the built-in English reference.
pub const ENGLISH_CODE: &str = "en";
/// Display name for the built-in English reference.
pub const ENGLISH_NAME: &str = "English";
/// Reserved file stem (`template.json`) — the checked-in English reference for
/// translators; ignored at load so it never appears as a selectable language.
const TEMPLATE_STEM: &str = "template";

/// A loaded translation table for one language.
pub struct Locale {
    /// Language code — the file stem (e.g. `fr`), used for selection.
    code: String,
    /// Human-readable name for the picker (the file's `_name`, else `code`).
    name: String,
    /// key id → translated text.
    map: HashMap<String, String>,
}

impl Locale {
    fn get(&self, id: &str) -> Option<&str> {
        self.map.get(id).map(String::as_str)
    }
}

/// All discovered locales, loaded once at startup. English is implicit and not
/// stored here.
static LOCALES: OnceLock<Vec<Arc<Locale>>> = OnceLock::new();

/// The active language. `None` = English (the reference) — the fast path. A
/// single rare writer (the operator changes the setting) plus many readers
/// (every hovered widget), so a plain `RwLock` is ample.
fn active_cell() -> &'static RwLock<Option<Arc<Locale>>> {
    static ACTIVE: OnceLock<RwLock<Option<Arc<Locale>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| RwLock::new(None))
}

fn active_locale() -> Option<Arc<Locale>> {
    active_cell().read().ok().and_then(|guard| guard.clone())
}

/// Directories scanned for `<code>.json` translation files, in increasing
/// priority — a language present in a later directory overrides the same code
/// from an earlier one:
///
/// 1. `locales/` beside the executable — translations shipped with a build
///    (the distribution location).
/// 2. `assets/locales/` under the working directory — the project tree when
///    running from source (`cargo run`).
/// 3. `<config>/s21_hijack/locales/` beside `preferences.json` — per-machine
///    operator overrides, user-writable without admin rights.
fn locale_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("locales"));
        }
    }
    dirs.push(PathBuf::from("assets").join("locales"));
    if let Some(cfg) = dirs::config_dir() {
        dirs.push(cfg.join("s21_hijack").join("locales"));
    }
    dirs
}

/// Parse one locale from JSON text: a flat `{ id: text }` object with an
/// optional `_name` display-name field (defaults to `code`). Separate from
/// [`load_locales`] so the parse logic is unit-testable without the filesystem.
fn parse_locale(code: &str, text: &str) -> Result<Locale, serde_json::Error> {
    let mut map: HashMap<String, String> = serde_json::from_str(text)?;
    let name = map.remove("_name").unwrap_or_else(|| code.to_owned());
    Ok(Locale {
        code: code.to_owned(),
        name,
        map,
    })
}

/// Load translation files from every [`locale_dirs`] directory and publish the
/// merged set. Call once at startup, before the UI reads any tooltip.
/// Best-effort: missing directories yield nothing; malformed files are logged
/// and skipped. A language found in a higher-priority directory replaces the
/// same code from a lower one.
pub fn load_locales() {
    // Keyed by lower-cased code so a later directory overrides an earlier one.
    let mut by_code: std::collections::BTreeMap<String, Locale> = std::collections::BTreeMap::new();
    for dir in locale_dirs() {
        for locale in scan_locale_dir(&dir) {
            by_code.insert(locale.code.to_lowercase(), locale);
        }
    }
    let mut locales: Vec<Arc<Locale>> = by_code.into_values().map(Arc::new).collect();
    locales.sort_by_key(|l| l.name.to_lowercase());
    // `set` fails only if called twice; loading once at startup is the contract.
    let _ = LOCALES.set(locales);
}

/// Scan one directory for `<code>.json` translation files. Best-effort: a
/// missing directory yields nothing; malformed files are logged and skipped.
/// `en.json` and `template.json` are ignored — English is the in-binary
/// reference and `template.json` is the translator starting point.
fn scan_locale_dir(dir: &Path) -> Vec<Locale> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            warn!(?dir, error = %e, "Failed to read locales directory");
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(code) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if code.eq_ignore_ascii_case(ENGLISH_CODE) || code.eq_ignore_ascii_case(TEMPLATE_STEM) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse_locale(code, &text) {
                Ok(locale) => {
                    let translated = ALL_KEYS
                        .iter()
                        .filter(|k| locale.get(key_id(**k)).is_some())
                        .count();
                    let total = ALL_KEYS.len();
                    info!(
                        code = %locale.code,
                        name = %locale.name,
                        translated,
                        total,
                        path = %path.display(),
                        "Loaded help-bubble locale"
                    );
                    out.push(locale);
                }
                Err(e) => warn!(?path, error = %e, "Skipping malformed locale file"),
            },
            Err(e) => warn!(?path, error = %e, "Failed to read locale file"),
        }
    }
    out
}

/// `(code, display name)` for every selectable language — English first, then
/// each discovered locale. Safe before [`load_locales`] (yields just English).
pub fn available_languages() -> Vec<(String, String)> {
    let mut out = vec![(ENGLISH_CODE.to_owned(), ENGLISH_NAME.to_owned())];
    if let Some(locales) = LOCALES.get() {
        out.extend(locales.iter().map(|l| (l.code.clone(), l.name.clone())));
    }
    out
}

/// Display name for a language code (for showing the current selection). Falls
/// back to the code itself for an unknown / removed language.
pub fn language_name(code: &str) -> String {
    if code.is_empty() || code.eq_ignore_ascii_case(ENGLISH_CODE) {
        return ENGLISH_NAME.to_owned();
    }
    LOCALES
        .get()
        .and_then(|locales| locales.iter().find(|l| l.code.eq_ignore_ascii_case(code)))
        .map(|l| l.name.clone())
        .unwrap_or_else(|| code.to_owned())
}

/// Publish the active help-bubble language by code. An empty code, `"en"`, or
/// an unknown / not-yet-loaded code selects the English reference. Cheap; call
/// once at startup from the saved preference and whenever it changes.
pub fn set_active_language(code: &str) {
    let chosen = if code.is_empty() || code.eq_ignore_ascii_case(ENGLISH_CODE) {
        None
    } else {
        LOCALES
            .get()
            .and_then(|locales| locales.iter().find(|l| l.code.eq_ignore_ascii_case(code)))
            .cloned()
    };
    if let Ok(mut active) = active_cell().write() {
        *active = chosen;
    }
}

/// The English reference as a pretty-printed JSON object (`id` → text, plus a
/// `_name`), sorted by id. This is the template a translator copies to start a
/// new `<code>.json`; English itself is always served from the in-binary
/// reference, so the file is a convenience, not a runtime input.
pub fn english_template_json() -> String {
    let mut map: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    map.insert("_name", ENGLISH_NAME);
    for key in ALL_KEYS {
        map.insert(key_id(*key), english(*key));
    }
    serde_json::to_string_pretty(&map).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn key_ids_are_unique_and_nonempty() {
        let mut seen = HashSet::new();
        for key in ALL_KEYS {
            let id = key_id(*key);
            assert!(!id.is_empty(), "{key:?} has an empty id");
            assert!(seen.insert(id), "duplicate help id: {id}");
            assert!(!english(*key).is_empty(), "{key:?} has empty English text");
        }
    }

    #[test]
    fn english_template_is_valid_json_with_every_key() {
        let json = english_template_json();
        let parsed: HashMap<String, String> =
            serde_json::from_str(&json).expect("template must be valid JSON");
        // Every key id, plus the `_name` meta field.
        assert_eq!(parsed.len(), ALL_KEYS.len() + 1);
        for key in ALL_KEYS {
            assert_eq!(
                parsed.get(key_id(*key)).map(String::as_str),
                Some(english(*key)),
                "template missing/mismatched for {key:?}"
            );
        }
    }

    #[test]
    fn active_language_resolution_and_fallback() {
        // Default: nothing active → the English reference.
        set_active_language("");
        assert_eq!(help(HelpKey::CueUndo), english(HelpKey::CueUndo));

        // Publish a partial French locale straight into the active cell. The
        // file-loading path needs the filesystem; this exercises the
        // translate-vs-fallback resolution. `tests` is a child module, so the
        // private `Locale` / `active_cell` are in scope.
        let mut map = HashMap::new();
        map.insert(key_id(HelpKey::CueUndo).to_string(), "Annuler".to_string());
        *active_cell().write().unwrap() = Some(Arc::new(Locale {
            code: "fr".to_string(),
            name: "Français".to_string(),
            map,
        }));

        // Present key → translated; absent key → English fallback.
        assert_eq!(help(HelpKey::CueUndo), "Annuler");
        assert_eq!(help(HelpKey::CueGo), english(HelpKey::CueGo));

        // Unknown and English codes reset to the English reference (no locales
        // are loaded in the test binary).
        set_active_language("zz");
        assert_eq!(help(HelpKey::CueUndo), english(HelpKey::CueUndo));
        set_active_language(ENGLISH_CODE);
        assert_eq!(help(HelpKey::CueUndo), english(HelpKey::CueUndo));
    }

    #[test]
    fn parse_locale_extracts_name_and_translations() {
        let json = r#"{ "_name": "Français", "cue.undo": "Annuler" }"#;
        let loc = parse_locale("fr", json).unwrap();
        assert_eq!(loc.code, "fr");
        assert_eq!(loc.name, "Français");
        assert_eq!(loc.get(key_id(HelpKey::CueUndo)), Some("Annuler"));
        // Absent key → None, so `help` falls back to the English reference.
        assert_eq!(loc.get(key_id(HelpKey::CueGo)), None);
    }

    #[test]
    fn parse_locale_without_name_uses_code() {
        let loc = parse_locale("de", "{}").unwrap();
        assert_eq!(loc.name, "de");
    }

    #[test]
    fn english_template_parses_back_as_a_complete_locale() {
        // Feeding the shipped template back through the loader yields a locale
        // that "translates" every key to its English text — proving the
        // template and the loader agree on the key ids.
        let loc = parse_locale("en-copy", &english_template_json()).unwrap();
        for key in ALL_KEYS {
            assert_eq!(loc.get(key_id(*key)), Some(english(*key)));
        }
    }

    #[test]
    fn project_locales_dir_loads_committed_translations() {
        // Tests run with the crate root as the working directory, so the
        // in-repo `assets/locales` is the project source. The committed French
        // file must parse and expose its translations.
        let locales = scan_locale_dir(Path::new("assets/locales"));
        let fr = locales
            .iter()
            .find(|l| l.code == "fr")
            .expect("assets/locales/fr.json should load");
        assert_eq!(fr.name, "Français");
        assert_eq!(
            fr.get(key_id(HelpKey::CueGo)),
            Some("Rappeler le cue suivant.")
        );
    }

    #[test]
    fn committed_template_matches_catalog() {
        // `assets/locales/template.json` is the checked-in English reference
        // for translators. It must stay in sync with the catalog — regenerate
        // it with `--dump-help-template` when keys change.
        let on_disk = std::fs::read_to_string("assets/locales/template.json")
            .expect("assets/locales/template.json should exist");
        let on_disk: HashMap<String, String> = serde_json::from_str(&on_disk).unwrap();
        let generated: HashMap<String, String> =
            serde_json::from_str(&english_template_json()).unwrap();
        assert_eq!(
            on_disk, generated,
            "template.json is stale — regenerate with `--dump-help-template`"
        );
    }

    #[test]
    fn reference_files_are_not_loaded_as_languages() {
        // `template.json` / `en.json` are references, not selectable languages.
        let locales = scan_locale_dir(Path::new("assets/locales"));
        assert!(
            !locales
                .iter()
                .any(|l| l.code.eq_ignore_ascii_case("template"))
        );
        assert!(!locales.iter().any(|l| l.code.eq_ignore_ascii_case("en")));
    }
}
