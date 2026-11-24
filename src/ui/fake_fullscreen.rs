//! Full screen that stays on the current Space (macOS).
//!
//! winit gives every resizable window the collection behaviour
//! `FullScreenPrimary`, so the green button sends `toggleFullScreen:` and
//! AppKit moves the window to a Space of its own: with QLab video out on
//! another display, the operator loses sight of the desktop. [`install`]
//! replaces `-[NSWindow toggleFullScreen:]` once for the whole app (winit's
//! `WinitWindow` subclass does not override it), which catches the green
//! button and the system full-screen shortcut for every window. A window
//! already in native full screen keeps AppKit's behaviour.
//!
//! Full screen here is: borderless style, frame = the display's whole frame,
//! menu bar and Dock hidden while any such window sits on a display that has
//! a menu bar. Presentation options belong to the app, not to a display, and
//! macOS drops them by itself while another app is frontmost.
//!
//! Port of WFS-DIY's `installFakeFullScreenMac` (`Source/gui/WindowUtils.mm`).
//! Elsewhere there are no Spaces, so the functions are no-ops.

#[cfg(target_os = "macos")]
pub use mac::{install, is_active};

/// No Spaces off macOS: nothing to install.
#[cfg(not(target_os = "macos"))]
pub fn install() {}

/// Never in fake full screen off macOS.
#[cfg(not(target_os = "macos"))]
pub fn is_active() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::{ClassType, MainThreadMarker, Message, sel};
    use objc2_app_kit::{
        NSApp, NSApplicationDidChangeScreenParametersNotification,
        NSApplicationPresentationOptions, NSEvent, NSEventMask, NSEventModifierFlags, NSScreen,
        NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowWillCloseNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSRect, NSString};

    /// NSWindow's own `toggleFullScreen:`, for windows we leave to AppKit.
    static ORIGINAL_TOGGLE: OnceLock<Imp> = OnceLock::new();

    type ToggleFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);

    struct State {
        window: Retained<NSWindow>,
        frame: NSRect,
        style_mask: NSWindowStyleMask,
        hides_menu_bar: bool,
    }

    // AppKit is main-thread only, so the bookkeeping is too. Borrows are never
    // held across an AppKit call: a frame change can re-enter the app (winit
    // delivers resize events, egui's update asks `is_active`).
    thread_local! {
        static WINDOWS: RefCell<HashMap<usize, State>> = RefCell::new(HashMap::new());
        static MENU_BAR_HIDERS: Cell<i32> = const { Cell::new(0) };
    }

    fn key_for(window: &NSWindow) -> usize {
        window as *const NSWindow as usize
    }

    fn is_fake(window: &NSWindow) -> bool {
        WINDOWS.with(|w| w.borrow().contains_key(&key_for(window)))
    }

    /// True while any window is in our full screen.
    pub fn is_active() -> bool {
        WINDOWS.with(|w| !w.borrow().is_empty())
    }

    fn screen_has_menu_bar(screen: &NSScreen, mtm: MainThreadMarker) -> bool {
        NSScreen::screensHaveSeparateSpaces(mtm)
            || NSScreen::screens(mtm)
                .firstObject()
                .is_some_and(|first| std::ptr::eq(&*first, screen))
    }

    fn change_menu_bar_hiders(delta: i32, mtm: MainThreadMarker) {
        let before = MENU_BAR_HIDERS.get();
        let after = (before + delta).max(0);
        MENU_BAR_HIDERS.set(after);

        if (before > 0) != (after > 0) {
            NSApp(mtm).setPresentationOptions(if after > 0 {
                NSApplicationPresentationOptions::HideDock
                    | NSApplicationPresentationOptions::HideMenuBar
            } else {
                NSApplicationPresentationOptions::Default
            });
        }
    }

    fn enter(window: &NSWindow, mtm: MainThreadMarker) {
        let Some(screen) = window.screen().or_else(|| NSScreen::mainScreen(mtm)) else {
            return;
        };
        let target = screen.frame();
        let hides_menu_bar = screen_has_menu_bar(&screen, mtm);

        WINDOWS.with(|w| {
            w.borrow_mut().insert(
                key_for(window),
                State {
                    window: window.retain(),
                    frame: window.frame(),
                    style_mask: window.styleMask(),
                    hides_menu_bar,
                },
            )
        });

        if hides_menu_bar {
            change_menu_bar_hiders(1, mtm);
        }

        window.setStyleMask(NSWindowStyleMask::Borderless);
        window.setFrame_display(target, true);
        window.makeKeyAndOrderFront(None);
    }

    /// `restore = false` when the window is closing: only the bookkeeping goes.
    fn exit(key: usize, restore: bool, mtm: MainThreadMarker) {
        // Out of the map first: the frame change below must already see the
        // window as windowed.
        let Some(s) = WINDOWS.with(|w| w.borrow_mut().remove(&key)) else {
            return;
        };

        if s.hides_menu_bar {
            change_menu_bar_hiders(-1, mtm);
        }

        if !restore {
            return;
        }

        let window = &s.window;
        window.setStyleMask(s.style_mask);

        // AppKit draws no title after a borderless -> titled switch until it changes.
        let title = NSString::from_str(&window.title().to_string());
        window.setTitle(&NSString::from_str(" "));
        window.setTitle(&title);

        window.setFrame_display(s.frame, true);
    }

    fn offers_full_screen(window: &NSWindow) -> bool {
        window
            .collectionBehavior()
            .contains(NSWindowCollectionBehavior::FullScreenPrimary)
    }

    /// Toggles our full screen; false when the window is not one we handle.
    fn toggle(window: &NSWindow, mtm: MainThreadMarker) -> bool {
        if is_fake(window) {
            exit(key_for(window), true, mtm);
            return true;
        }

        if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            return false; // already in native full screen: let AppKit bring it back
        }

        if !offers_full_screen(window) {
            return false;
        }

        enter(window, mtm);
        true
    }

    unsafe extern "C-unwind" fn replacement_toggle(
        this: *mut AnyObject,
        cmd: Sel,
        sender: *mut AnyObject,
    ) {
        if let (Some(mtm), Some(window)) = (
            MainThreadMarker::new(),
            // SAFETY: the method is installed on NSWindow, so `self` is one.
            unsafe { (this as *const NSWindow).as_ref() },
        ) && toggle(window, mtm)
        {
            return;
        }

        if let Some(original) = ORIGINAL_TOGGLE.get() {
            // SAFETY: the IMP we replaced, with its own signature.
            unsafe {
                let original: ToggleFn = std::mem::transmute::<Imp, ToggleFn>(*original);
                original(this, cmd, sender);
            }
        }
    }

    /// A window that moved to another display, or whose display changed size or went away.
    fn refit(mtm: MainThreadMarker) {
        let windows: Vec<(usize, Retained<NSWindow>)> = WINDOWS.with(|w| {
            w.borrow()
                .iter()
                .map(|(k, s)| (*k, s.window.clone()))
                .collect()
        });

        for (key, window) in windows {
            let Some(screen) = window.screen() else {
                exit(key, true, mtm);
                continue;
            };

            let hides = screen_has_menu_bar(&screen, mtm);
            let changed = WINDOWS.with(|w| {
                let mut w = w.borrow_mut();
                match w.get_mut(&key) {
                    Some(s) if s.hides_menu_bar != hides => {
                        s.hides_menu_bar = hides;
                        true
                    }
                    _ => false,
                }
            });
            if changed {
                change_menu_bar_hiders(if hides { 1 } else { -1 }, mtm);
            }

            window.setFrame_display(screen.frame(), true);
        }
    }

    /// Installs the app-wide hook. Main thread only; later calls do nothing.
    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("fake full screen: install() off the main thread, skipped");
            return;
        };
        if ORIGINAL_TOGGLE.get().is_some() {
            return;
        }

        let Some(method) = NSWindow::class().instance_method(sel!(toggleFullScreen:)) else {
            tracing::warn!("fake full screen: NSWindow has no toggleFullScreen:, skipped");
            return;
        };
        // SAFETY: same signature as -[NSWindow toggleFullScreen:] (self, _cmd, id).
        let original = unsafe {
            method.set_implementation(std::mem::transmute::<ToggleFn, Imp>(replacement_toggle))
        };
        let _ = ORIGINAL_TOGGLE.set(original);

        // Ctrl+Cmd+F: the app has no View menu, so AppKit adds no Enter Full
        // Screen item and the shortcut would otherwise do nothing.
        let key_monitor = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a live event.
            let ev = unsafe { event.as_ref() };
            let mods = ev.modifierFlags()
                & (NSEventModifierFlags::Shift
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Option
                    | NSEventModifierFlags::Command);
            let is_f = ev
                .charactersIgnoringModifiers()
                .is_some_and(|c| c.to_string().eq_ignore_ascii_case("f"));

            if mods == (NSEventModifierFlags::Control | NSEventModifierFlags::Command)
                && is_f
                && let Some(window) = ev.window(mtm).or_else(|| NSApp(mtm).keyWindow())
                && (offers_full_screen(&window) || is_fake(&window))
                && toggle(&window, mtm)
            {
                return std::ptr::null_mut();
            }
            event.as_ptr()
        });
        // SAFETY: the handler returns the event it was given, or null.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown,
                &key_monitor,
            )
        };
        // Kept for the app's lifetime.
        std::mem::forget(monitor);

        let centre = NSNotificationCenter::defaultCenter();

        let will_close = RcBlock::new(move |note: NonNull<NSNotification>| {
            // SAFETY: AppKit passes a live notification.
            if let Some(object) = unsafe { note.as_ref() }.object() {
                exit(Retained::as_ptr(&object) as usize, false, mtm);
            }
        });
        let screens_changed = RcBlock::new(move |_note: NonNull<NSNotification>| refit(mtm));

        // SAFETY: both notifications are posted on the main thread (queue nil
        // runs the block there), and the blocks capture only the marker.
        unsafe {
            std::mem::forget(centre.addObserverForName_object_queue_usingBlock(
                Some(NSWindowWillCloseNotification),
                None,
                None,
                &will_close,
            ));
            std::mem::forget(centre.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                None,
                &screens_changed,
            ));
        }
    }
}
