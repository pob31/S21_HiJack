pub mod client;
pub mod encode;
pub mod ipad_client;
pub mod ipad_encode;
pub mod ipad_parse;
pub mod ipad_values;
pub mod monitor_server;
pub mod parse;
pub mod qlab_client;
pub mod qlab_cue_builder;
pub mod trigger_listener;

// ─── Shared OSC address constants ───────────────────────────────────

/// OSC address that triggers a snapshot recall on the daemon. The Phase D
/// QLab "Create Trigger Cue" feature embeds this in a network cue's
/// customString; the Phase E trigger listener parses incoming OSC for it.
/// Defining the constant once here keeps the two sides in lockstep.
pub const SNAPSHOT_RECALL_ADDR: &str = "/snapshot/recall";

/// Variant that bypasses the snapshot's exclusion scope (only meaningful for
/// `SnapshotKind::ApplyOnRecall` snapshots — see `SnapshotEngine::recall`).
pub const SNAPSHOT_RECALL_FULL_ADDR: &str = "/snapshot/recall_full";
