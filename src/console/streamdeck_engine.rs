//! Stream Deck device engine.
//!
//! Owns a dedicated background thread that talks to an Elgato Stream
//! Deck (or compatible device) over HID via the `elgato-streamdeck`
//! crate. The HID API is synchronous, so a `std::thread` rather than a
//! tokio task is the natural fit — the thread blocks on
//! `StreamDeck::read_input(timeout=…)` between command dispatches.
//!
//! Two channels bridge the thread to the rest of the app:
//!
//! - **Inbound** (UI → thread): `mpsc::Sender<DeviceCmd>` carries
//!   connect / disconnect / refresh requests.
//! - **Outbound** (thread → UI): `mpsc::Sender<UiEvent>` carries
//!   button-press events and connection-state changes; the existing
//!   `HiJackApp::drain_events` then dispatches them.
//!
//! While idle, the thread re-enumerates devices every 2 s so the UI's
//! "available devices" combo stays fresh. While connected, it polls
//! the device with a short read timeout (50 ms) so commands queued
//! between reads land on the device promptly.
//!
//! LCD label rendering: the engine bakes a 72×72 (or device-specific
//! size) RGB image with the next-to-fire macro name centred,
//! word-wrapped to fit two lines, using the embedded NotoSans font.
//! `StreamDeck::set_button_image` accepts a `DynamicImage` and
//! handles the per-model BMP/JPEG conversion internally.

use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{StreamDeck, StreamDeckInput, list_devices, new_hidapi};
use image::{DynamicImage, Rgb, RgbImage};
use tracing::{debug, info, warn};

use crate::model::streamdeck::StepColor;
use crate::ui::UiEvent;

/// Embedded NotoSans Regular for LCD text. Loaded once on engine
/// startup; if the bytes can't be parsed (shouldn't happen in
/// practice), the engine logs a warning and proceeds without text
/// rendering — buttons render as black squares.
const NOTO_SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");

/// One device entry in the "available" list (returned by
/// `available_devices()` and shown in the UI combo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub kind: Kind,
    pub serial: String,
    pub label: String, // e.g. "Stream Deck (Original) — 15 buttons"
}

/// Snapshot of a connected device used by the UI to render the
/// button-grid layout.
#[derive(Clone, Debug)]
pub struct ConnectedDevice {
    pub kind: Kind,
    pub serial: String,
    pub label: String,
    pub key_count: u8,
    pub row_count: u8,
    pub column_count: u8,
}

/// Cached state the UI reads each frame via `try_read()`.
#[derive(Default)]
pub struct EngineState {
    pub available: Vec<DeviceInfo>,
    pub connected: Option<ConnectedDevice>,
    pub last_error: Option<String>,
}

/// Commands sent from the UI to the device thread.
#[allow(clippy::large_enum_variant)] // RefreshAll's Vec<…> is fine
enum DeviceCmd {
    /// Connect to the device with the given HID serial.
    Connect(String),
    /// Disconnect (if connected) and return to idle enumeration.
    Disconnect,
    /// Re-render and push a single button's LCD: macro label on top of
    /// `bg` background color.
    RefreshButton {
        idx: u8,
        label: String,
        bg: StepColor,
    },
    /// Re-render every button's LCD. Indexed by button index, length
    /// matched to the connected device's key count.
    RefreshAll(Vec<(String, StepColor)>),
    /// Tell the worker thread to exit cleanly.
    Shutdown,
}

/// Public handle. Cheap to clone via `Arc`. Spawns the device thread
/// on construction and never blocks the UI thread.
pub struct StreamDeckEngine {
    cmd_tx: mpsc::Sender<DeviceCmd>,
    state: Arc<RwLock<EngineState>>,
    /// Held so we can `take().join()` on Drop. Wrapped in Mutex so the
    /// engine itself stays Send + Sync (useful for `Arc<StreamDeckEngine>`).
    join: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl StreamDeckEngine {
    /// Construct the engine, spawning the device thread immediately.
    /// The thread starts in idle (no device) and polls available HID
    /// devices every ~2 s so the UI can populate its combo.
    pub fn new(ui_tx: mpsc::Sender<UiEvent>) -> Arc<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<DeviceCmd>();
        let state = Arc::new(RwLock::new(EngineState::default()));
        let state_thread = state.clone();
        let join = std::thread::Builder::new()
            .name("streamdeck".into())
            .spawn(move || {
                run_device_thread(cmd_rx, ui_tx, state_thread);
            })
            .expect("spawn streamdeck worker thread");
        Arc::new(Self {
            cmd_tx,
            state,
            join: std::sync::Mutex::new(Some(join)),
        })
    }

    /// Snapshot of the available devices (re-enumerated by the
    /// worker every ~2 s). Returns an empty vec if the worker hasn't
    /// run its first scan yet.
    pub fn available_devices(&self) -> Vec<DeviceInfo> {
        self.state
            .read()
            .map(|s| s.available.clone())
            .unwrap_or_default()
    }

    /// Currently-connected device, if any.
    pub fn connected_device(&self) -> Option<ConnectedDevice> {
        self.state.read().ok().and_then(|s| s.connected.clone())
    }

    /// True iff a device is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected_device().is_some()
    }

    /// Request a connect by HID serial. Idempotent if already connected
    /// to the same serial. Best-effort: failures land in `last_error`
    /// and surface as `UiEvent::StreamDeckError` from the worker.
    pub fn connect(&self, serial: String) {
        let _ = self.cmd_tx.send(DeviceCmd::Connect(serial));
    }

    pub fn disconnect(&self) {
        let _ = self.cmd_tx.send(DeviceCmd::Disconnect);
    }

    pub fn refresh_button(&self, idx: u8, label: String, bg: StepColor) {
        let _ = self
            .cmd_tx
            .send(DeviceCmd::RefreshButton { idx, label, bg });
    }

    pub fn refresh_all(&self, labels: Vec<(String, StepColor)>) {
        let _ = self.cmd_tx.send(DeviceCmd::RefreshAll(labels));
    }
}

impl Drop for StreamDeckEngine {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(DeviceCmd::Shutdown);
        if let Ok(mut guard) = self.join.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

// ─── Worker thread ───────────────────────────────────────────────────

fn run_device_thread(
    cmd_rx: mpsc::Receiver<DeviceCmd>,
    ui_tx: mpsc::Sender<UiEvent>,
    state: Arc<RwLock<EngineState>>,
) {
    let font = FontRef::try_from_slice(NOTO_SANS_REGULAR).ok();
    if font.is_none() {
        warn!("Stream Deck: failed to parse embedded NotoSans font; LCDs will render blank");
    }

    let mut hidapi = match new_hidapi() {
        Ok(h) => h,
        Err(e) => {
            warn!("Stream Deck: hidapi init failed: {e}");
            if let Ok(mut s) = state.write() {
                s.last_error = Some(format!("hidapi init failed: {e}"));
            }
            return;
        }
    };

    let mut connected: Option<StreamDeck> = None;
    let mut connected_meta: Option<ConnectedDevice> = None;
    // Pending labels; populated by RefreshAll while disconnected so we
    // can apply them after a successful connect.
    let mut pending_labels: Option<Vec<(String, StepColor)>> = None;
    let mut last_enum = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);

    let enum_interval = Duration::from_secs(2);
    let read_timeout = Some(Duration::from_millis(50));

    loop {
        // ── Drain queued commands without blocking ──
        loop {
            match cmd_rx.try_recv() {
                Ok(DeviceCmd::Shutdown) => return,
                Ok(DeviceCmd::Connect(serial)) => {
                    if connected_meta.as_ref().map(|c| c.serial.as_str()) == Some(serial.as_str()) {
                        // Already on the requested device.
                        continue;
                    }
                    if connected.is_some() {
                        connected = None;
                        connected_meta = None;
                        if let Ok(mut s) = state.write() {
                            s.connected = None;
                        }
                        let _ = ui_tx.send(UiEvent::StreamDeckDisconnected);
                    }
                    match try_connect(&hidapi, &serial) {
                        Ok((deck, meta)) => {
                            info!(
                                serial = %meta.serial,
                                kind = ?meta.kind,
                                key_count = meta.key_count,
                                "Stream Deck connected"
                            );
                            // Reset the device so any leftover button images
                            // from a previous app session are cleared before
                            // we paint our own.
                            let _ = deck.reset();
                            let _ = deck.set_brightness(80);
                            let _ = ui_tx.send(UiEvent::StreamDeckConnected {
                                device_name: meta.label.clone(),
                                button_count: meta.key_count,
                            });
                            if let Ok(mut s) = state.write() {
                                s.connected = Some(meta.clone());
                                s.last_error = None;
                            }
                            connected = Some(deck);
                            connected_meta = Some(meta);
                            // Apply any labels queued while disconnected.
                            if let Some(labels) = pending_labels.take() {
                                push_all_labels(
                                    connected.as_ref().unwrap(),
                                    connected_meta.as_ref().unwrap(),
                                    font.as_ref(),
                                    &labels,
                                );
                            }
                        }
                        Err(msg) => {
                            warn!("Stream Deck connect failed: {msg}");
                            if let Ok(mut s) = state.write() {
                                s.last_error = Some(msg.clone());
                            }
                            let _ = ui_tx.send(UiEvent::StreamDeckError { message: msg });
                        }
                    }
                }
                Ok(DeviceCmd::Disconnect) => {
                    if connected.is_some() {
                        connected = None;
                        connected_meta = None;
                        if let Ok(mut s) = state.write() {
                            s.connected = None;
                        }
                        let _ = ui_tx.send(UiEvent::StreamDeckDisconnected);
                    }
                }
                Ok(DeviceCmd::RefreshButton { idx, label, bg }) => {
                    if let (Some(deck), Some(meta)) = (&connected, &connected_meta) {
                        push_button_label(deck, meta, font.as_ref(), idx, &label, bg);
                    }
                }
                Ok(DeviceCmd::RefreshAll(labels)) => {
                    if let (Some(deck), Some(meta)) = (&connected, &connected_meta) {
                        push_all_labels(deck, meta, font.as_ref(), &labels);
                    } else {
                        // Stash for after the next connect.
                        pending_labels = Some(labels);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // ── Periodically re-enumerate devices ──
        if last_enum.elapsed() >= enum_interval {
            last_enum = Instant::now();
            if let Err(e) = elgato_streamdeck::refresh_device_list(&mut hidapi) {
                debug!("Stream Deck: refresh_device_list err: {e}");
            }
            let listed = list_devices(&hidapi);
            let mut available = Vec::with_capacity(listed.len());
            for (kind, serial) in listed {
                let label = device_label(kind);
                available.push(DeviceInfo {
                    kind,
                    serial,
                    label,
                });
            }
            if let Ok(mut s) = state.write() {
                s.available = available;
            }
        }

        // ── Read input (or brief sleep when idle) ──
        if let Some(deck) = &connected {
            match deck.read_input(read_timeout) {
                Ok(StreamDeckInput::NoData) => {}
                Ok(StreamDeckInput::ButtonStateChange(states)) => {
                    // `states` is Vec<bool>: true means pressed. We only
                    // emit on press transitions (not release).
                    for (idx, pressed) in states.iter().enumerate() {
                        if *pressed {
                            let _ =
                                ui_tx.send(UiEvent::StreamDeckButtonPressed { button_idx: idx });
                        }
                    }
                }
                Ok(_other) => {
                    // Encoder / touch events from Plus / Pedal are
                    // ignored for now — the MK1 Original doesn't have
                    // them and the rest of the model surface treats
                    // every input as a button press.
                }
                Err(e) => {
                    warn!("Stream Deck read error, dropping device: {e}");
                    connected = None;
                    connected_meta = None;
                    if let Ok(mut s) = state.write() {
                        s.connected = None;
                        s.last_error = Some(format!("read error: {e}"));
                    }
                    let _ = ui_tx.send(UiEvent::StreamDeckDisconnected);
                }
            }
        } else {
            // Idle: brief sleep so we don't busy-loop the enum logic.
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// ─── Connection helpers ──────────────────────────────────────────────

fn try_connect(
    hidapi: &hidapi::HidApi,
    serial: &str,
) -> Result<(StreamDeck, ConnectedDevice), String> {
    let listed = list_devices(hidapi);
    let (kind, _) = listed
        .into_iter()
        .find(|(_, s)| s == serial)
        .ok_or_else(|| format!("device {serial} not found"))?;
    let deck =
        StreamDeck::connect(hidapi, kind, serial).map_err(|e| format!("connect failed: {e}"))?;
    let meta = ConnectedDevice {
        kind,
        serial: serial.to_string(),
        label: device_label(kind),
        key_count: kind.key_count(),
        row_count: kind.row_count(),
        column_count: kind.column_count(),
    };
    Ok((deck, meta))
}

fn device_label(kind: Kind) -> String {
    let model = match kind {
        Kind::Original => "Stream Deck (Original)",
        Kind::OriginalV2 => "Stream Deck (Original V2)",
        Kind::Mini => "Stream Deck Mini",
        Kind::MiniMk2 => "Stream Deck Mini Mk2",
        Kind::Mk2 => "Stream Deck Mk2",
        Kind::Xl => "Stream Deck XL",
        Kind::XlV2 => "Stream Deck XL V2",
        Kind::Neo => "Stream Deck Neo",
        Kind::Pedal => "Stream Deck Pedal",
        Kind::Plus => "Stream Deck Plus",
        _ => "Stream Deck",
    };
    format!("{} — {} buttons", model, kind.key_count())
}

// ─── LCD rendering ───────────────────────────────────────────────────

fn push_all_labels(
    deck: &StreamDeck,
    meta: &ConnectedDevice,
    font: Option<&FontRef<'_>>,
    labels: &[(String, StepColor)],
) {
    let count = meta.key_count as usize;
    let empty = (String::new(), StepColor::BLACK);
    for idx in 0..count {
        let (label, bg) = labels.get(idx).unwrap_or(&empty);
        push_button_label(deck, meta, font, idx as u8, label, *bg);
    }
    let _ = deck.flush();
}

fn push_button_label(
    deck: &StreamDeck,
    meta: &ConnectedDevice,
    font: Option<&FontRef<'_>>,
    idx: u8,
    label: &str,
    bg: StepColor,
) {
    let format = meta.kind.key_image_format();
    let (w, h) = format.size;
    let img = render_label(w as u32, h as u32, label, font, bg);
    if let Err(e) = deck.set_button_image(idx, DynamicImage::ImageRgb8(img)) {
        warn!("Stream Deck: set_button_image({idx}) failed: {e}");
        return;
    }
    let _ = deck.flush();
}

/// Render a Stream Deck button face: `bg` background, contrast-chosen
/// text wrapped to fit the box. `label.is_empty()` → solid `bg`.
fn render_label(
    w: u32,
    h: u32,
    label: &str,
    font: Option<&FontRef<'_>>,
    bg: StepColor,
) -> RgbImage {
    let mut img = RgbImage::from_pixel(w, h, Rgb([bg.r, bg.g, bg.b]));
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return img;
    }
    let Some(font) = font else { return img };

    let text = bg.contrast_text();
    let text_rgb = Rgb([text.r, text.g, text.b]);

    let scale = PxScale::from(18.0);
    let lines = wrap_text(trimmed, font, scale, w);
    let max_lines = ((h as f32 / scale.y) as usize).max(1);
    let lines: Vec<&str> = lines.iter().take(max_lines).map(|s| s.as_str()).collect();
    let line_h = scale.y;
    let total_h = line_h * lines.len() as f32;
    let mut y = ((h as f32 - total_h) / 2.0).max(0.0);

    let scaled_font = font.as_scaled(scale);

    for line in &lines {
        let (text_w, _) = imageproc::drawing::text_size(scale, font, line);
        let x = ((w as f32 - text_w as f32) / 2.0).max(0.0) as i32;
        // imageproc draws glyphs at y = top of ascent; we shift down
        // by the font's ascent so the visual baseline lines up with
        // our `y`.
        let baseline_y = (y + scaled_font.ascent()).round() as i32;
        let draw_y = baseline_y - scaled_font.ascent() as i32;
        imageproc::drawing::draw_text_mut(&mut img, text_rgb, x, draw_y, scale, font, line);
        y += line_h;
    }
    img
}

/// Word-wrap `text` to fit `max_width` in pixels. Falls back to
/// character-level wrapping for tokens that don't fit on a single line.
fn wrap_text(text: &str, font: &FontRef<'_>, scale: PxScale, max_width: u32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let max_w = max_width as i32;

    let measure = |s: &str| -> i32 { imageproc::drawing::text_size(scale, font, s).0 as i32 };

    for word in text.split_whitespace() {
        if current.is_empty() {
            // First word — may itself need char-wrapping.
            if measure(word) <= max_w {
                current.push_str(word);
            } else {
                let pieces = char_wrap(word, font, scale, max_w);
                for (i, piece) in pieces.iter().enumerate() {
                    if i + 1 < pieces.len() {
                        lines.push(piece.clone());
                    } else {
                        current.push_str(piece);
                    }
                }
            }
        } else {
            let candidate = format!("{current} {word}");
            if measure(&candidate) <= max_w {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                if measure(word) <= max_w {
                    current.push_str(word);
                } else {
                    let pieces = char_wrap(word, font, scale, max_w);
                    for (i, piece) in pieces.iter().enumerate() {
                        if i + 1 < pieces.len() {
                            lines.push(piece.clone());
                        } else {
                            current.push_str(piece);
                        }
                    }
                }
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn char_wrap(word: &str, font: &FontRef<'_>, scale: PxScale, max_w: i32) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        let candidate = format!("{current}{ch}");
        if imageproc::drawing::text_size(scale, font, &candidate).0 as i32 <= max_w {
            current = candidate;
        } else {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_label_returns_correct_dimensions() {
        let font = FontRef::try_from_slice(NOTO_SANS_REGULAR).ok();
        let img = render_label(72, 72, "Mute Toggle", font.as_ref(), StepColor::BLACK);
        assert_eq!(img.width(), 72);
        assert_eq!(img.height(), 72);
    }

    #[test]
    fn render_label_handles_empty_string() {
        let font = FontRef::try_from_slice(NOTO_SANS_REGULAR).ok();
        let img = render_label(72, 72, "", font.as_ref(), StepColor::BLACK);
        // All black (default bg).
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(img.get_pixel(35, 35).0, [0, 0, 0]);
    }

    #[test]
    fn render_label_uses_supplied_background_color() {
        let font = FontRef::try_from_slice(NOTO_SANS_REGULAR).ok();
        let red = StepColor::new(255, 0, 0);
        // Empty label → solid fill of the background color.
        let img = render_label(72, 72, "", font.as_ref(), red);
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(35, 35).0, [255, 0, 0]);
    }

    #[test]
    fn wrap_text_breaks_long_phrase_into_multiple_lines() {
        let font = FontRef::try_from_slice(NOTO_SANS_REGULAR).expect("font parses");
        let lines = wrap_text(
            "This is a fairly long button label",
            &font,
            PxScale::from(18.0),
            72,
        );
        assert!(lines.len() >= 2, "expected wrap, got {lines:?}");
    }

    #[test]
    fn device_label_formats_known_kind() {
        let label = device_label(Kind::Original);
        assert!(label.contains("Original"));
        assert!(label.contains("15"));
    }
}
