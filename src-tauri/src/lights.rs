// macOS traffic lights in the mini player: AppKit puts them in the top-left corner of a title bar
// the mini does not have, so the UI asks for them over the cover, in line with the search pill
// (core/window.ts placeLights). Tauri only sets their position when the window is built,
// so they are moved here, as tao does it, and again after every resize (AppKit lays them out anew).

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSButton, NSView, NSWindow, NSWindowButton};
    use objc2_foundation::NSPoint;
    use std::sync::Mutex;

    /// (left, vertical centre) in points from the window's top-left; None = where AppKit puts them.
    static WANTED: Mutex<Option<(f64, f64)>> = Mutex::new(None);
    /// AppKit's own layout (title bar height, each button's origin), taken before the first move.
    static DEFAULT: Mutex<Option<(f64, [(f64, f64); 3])>> = Mutex::new(None);

    const BUTTONS: [NSWindowButton; 3] = [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton];

    /// The three buttons and the title bar view that holds them.
    unsafe fn parts(w: &NSWindow) -> Option<([Retained<NSButton>; 3], Retained<NSView>)> {
        let [Some(close), Some(min), Some(zoom)] = BUTTONS.map(|b| w.standardWindowButton(b)) else { return None };
        let bar = close.superview()?.superview()?;
        Some(([close, min, zoom], bar))
    }

    /// Title bar `height` points tall at the top of the window, buttons at `origins`.
    unsafe fn lay(w: &NSWindow, bar: &NSView, buttons: &[Retained<NSButton>; 3], height: f64, origins: [(f64, f64); 3]) {
        let mut r = bar.frame();
        r.size.height = height;
        r.origin.y = w.frame().size.height - height;
        bar.setFrame(r);
        for (b, (x, y)) in buttons.iter().zip(origins) {
            b.setFrameOrigin(NSPoint::new(x, y));
        }
    }

    unsafe fn ns(win: &tauri::Window) -> Option<&NSWindow> {
        win.ns_window().ok().and_then(|p| (p as *const NSWindow).as_ref())
    }

    /// Some((x, centre)): there from now on; None: back to AppKit's place.
    pub fn place(win: &tauri::Window, at: Option<(f64, f64)>) {
        // SAFETY: main thread (sync command / window event), on the live window of `win`.
        unsafe {
            let Some(w) = ns(win) else { return };
            let Some((buttons, bar)) = parts(w) else { return };
            let mut def = DEFAULT.lock().unwrap();
            let mut wanted = WANTED.lock().unwrap();
            if wanted.is_none() && def.is_none() {
                let o = buttons.each_ref().map(|b| (b.frame().origin.x, b.frame().origin.y));
                *def = Some((bar.frame().size.height, o));
            }
            let was = wanted.is_some();
            *wanted = at;
            match (at, *def) {
                // As tao does it: a bar as tall as the buttons' bottom, AppKit's spacing.
                (Some((x, mid)), Some((_, o))) => {
                    let h = buttons[0].frame().size.height;
                    let step = o[1].0 - o[0].0;
                    lay(w, &bar, &buttons, mid + h / 2.0, [0, 1, 2].map(|i| (x + i as f64 * step, o[i].1)));
                }
                (None, Some((height, o))) if was => lay(w, &bar, &buttons, height, o),
                _ => {}
            }
        }
    }

    /// After a resize AppKit has put them back: there again, if the UI wants them elsewhere.
    pub fn reapply(win: &tauri::Window) {
        let at = *WANTED.lock().unwrap();
        if at.is_some() {
            place(win, at);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{place, reapply};

#[cfg(not(target_os = "macos"))]
pub fn place(_win: &tauri::Window, _at: Option<(f64, f64)>) {}

#[cfg(not(target_os = "macos"))]
pub fn reapply(_win: &tauri::Window) {}
