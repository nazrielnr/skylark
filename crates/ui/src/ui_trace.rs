//! Opt-in UI tracing for automation and debugging.
//!
//! Enable with `ZERON_UI_TRACE=1`. Every hook below is a no-op (a single
//! atomic load) when disabled, so shipping builds pay nothing but this
//! check. The `tools/ui_debug/` scripts drive the app and assert against
//! these `[trace]` lines — see `docs/ui-tracing.md`.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{div, prelude::*};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn init() {
    let on = std::env::var_os("ZERON_UI_TRACE").is_some_and(|v| v != "0");
    ENABLED.store(on, Ordering::Relaxed);
}

/// One-line structured event. Keep the label stable — scripts parse it.
#[macro_export]
macro_rules! ui_trace {
    ($($arg:tt)*) => {
        if $crate::ui_trace::enabled() {
            eprintln!("[trace] {}", format_args!($($arg)*));
        }
    };
}

/// Bounds probe: an invisible, non-interactive overlay that reports its
/// bounds once per paint. Attach inside any element to expose its
/// on-screen geometry to the automation scripts.
pub fn bounds_probe(id: &'static str) -> gpui::AnyElement {
    let paint =
        move |bounds: gpui::Bounds<gpui::Pixels>, _: &mut gpui::Window, _: &mut gpui::App| {
            if enabled() {
                eprintln!(
                    "[trace] bounds {id} L={:.0} T={:.0} R={:.0} B={:.0}",
                    f32::from(bounds.left()),
                    f32::from(bounds.top()),
                    f32::from(bounds.right()),
                    f32::from(bounds.bottom())
                );
            }
        };
    gpui::canvas(paint, |_, _, _, _| {})
        .absolute()
        .inset_0()
        .into_any_element()
}
