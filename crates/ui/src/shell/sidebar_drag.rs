//! Drag-and-drop state, animations, and render container for the sidebar.

use gpui::{
    AnyElement, Context, Empty, IntoElement, Pixels, Point, Render, Subscription, Window, div,
    prelude::*,
};

use crate::motion::{self, TAB_SLIDE};
use crate::theme::Theme;

use super::{Route, Shell};

/// Ramp height of the sidebar's scroll-edge fade (the gpui
/// [`gpui::EdgeFade`] scope — per-primitive, so text fades per glyph).
pub(super) const SIDEBAR_GLASS_FADE_BAND: f32 = 24.0;

/// Sidebar-only drag payload. Regular sessions never acquire a manual order.
#[derive(Clone)]
pub(super) struct SidebarSessionDrag {
    pub(super) chat_id: String,
    pub(super) visible_ids: std::sync::Arc<Vec<String>>,
    pub(super) filter: Option<String>,
    pub(super) profile_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarSessionDrop {
    Pinned(usize),
    Regular,
}

pub(super) struct SidebarSessionTransfer {
    pub(super) payload: SidebarSessionDrag,
    pub(super) origin: std::rc::Rc<std::cell::Cell<Point<Pixels>>>,
    pub(super) cursor_offset: Point<Pixels>,
    pub(super) pointer: Point<Pixels>,
    pub(super) viewport: Option<gpui::Bounds<Pixels>>,
    pub(super) slide: SidebarSessionSlide,
    pub(super) preview: Option<SidebarSessionGap>,
    pub(super) source_group: String,
    pub(super) source_index: usize,
    pub(super) row_height: f32,
    pub(super) source_collapse: SidebarSessionSlide,
    pub(super) collapsed_height: f32,
    pub(super) section_gaps: std::collections::HashMap<String, SidebarSessionSlide>,
    pub(super) siblings: std::collections::HashMap<String, SidebarSessionSlide>,
}

#[derive(Clone)]
pub(super) struct SidebarSessionGap {
    pub(super) group: String,
    pub(super) index: usize,
    pub(super) pinned: bool,
    pub(super) top: f32,
}

/// The same slot-to-slot easing as pinned reordering, applied to the actual row.
pub(super) struct SidebarSessionSlide {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) epoch: u64,
    pub(super) started: std::time::Instant,
}

impl SidebarSessionSlide {
    pub(super) fn current(&self) -> f32 {
        let progress = TAB_SLIDE
            .progress(self.started.elapsed().as_secs_f32() / TAB_SLIDE.total().as_secs_f32());
        motion::lerp(self.from, self.to, progress)
    }

    pub(super) fn retarget(&mut self, target: f32) {
        if (target - self.to).abs() < 0.5 {
            return;
        }
        self.from = self.current();
        self.to = target;
        self.epoch = self.epoch.wrapping_add(1);
        self.started = std::time::Instant::now();
    }
}

pub(super) struct SidebarSessionReturn {
    pub(super) transfer: SidebarSessionTransfer,
    pub(super) epoch: u64,
    pub(super) started: std::time::Instant,
}

/// Live destination for a pinned-session drag. The real row remains clipped
/// to the sidebar and slides between slots with its pinned siblings.
pub(super) struct PinnedSessionDragState {
    pub(super) chat_id: String,
    pub(super) visible_ids: std::sync::Arc<Vec<String>>,
    pub(super) from: usize,
    pub(super) over: usize,
    pub(super) prev_over: usize,
    pub(super) epoch: usize,
    pub(super) filter: Option<String>,
    pub(super) profile_key: String,
    pub(super) pointer_y: Option<f32>,
    pub(super) viewport_top: f32,
    pub(super) viewport_bottom: f32,
    pub(super) generation: u64,
    pub(super) autoscroll_active: bool,
}

pub(super) type SidebarKeyedRow = (String, f32, AnyElement);

pub(super) struct SidebarSessionRows {
    pub(super) regular_count: usize,
    pub(super) rows: Vec<SidebarKeyedRow>,
    pub(super) pinned_count: usize,
    pub(super) moving_row: Option<(AnyElement, f32)>,
}

/// Invisible drag ghost — resize drags and contained pinned-session reorders
/// render nothing at the cursor.
pub(crate) struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// Sidebar render identity lets transcript/caret frames reuse its GPUI scene.
/// State and event handlers stay on Shell. Explicit Shell notifications still
/// invalidate the sidebar, including selection, menus, theme and navigation.
pub(super) struct SidebarPane {
    pub(super) shell: gpui::WeakEntity<Shell>,
    pub(super) _observation: Subscription,
}

impl Render for SidebarPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::transcript::record_view_frame("sidebar");
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let inner = shell.update(cx, |shell, cx| {
            let theme = Theme::of(cx).clone();
            match shell.route {
                Route::Settings(section) => shell.render_settings_nav(section, &theme, cx),
                Route::Chat => shell.render_chat_sidebar(&theme, cx),
            }
        });
        div().size_full().child(inner).into_any_element()
    }
}

impl Shell {
    pub(super) fn contain_pinned_session_drag(
        &mut self,
        event: &gpui::DragMoveEvent<SidebarSessionDrag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(transfer) = self.sidebar_session_transfer.as_mut() {
            transfer.pointer = event.event.position;
            cx.notify();
        }
        let inside_window = event.bounds.contains(&event.event.position);
        let pointer_x = f32::from(event.event.position.x);
        let sidebar_left = f32::from(event.bounds.left());
        let inside_sidebar =
            pointer_x >= sidebar_left && pointer_x <= sidebar_left + self.settings.sidebar_width;
        if !inside_window || !inside_sidebar {
            if let Some(transfer) = self.sidebar_session_transfer.as_mut() {
                transfer.preview = None;
            }
            self.cancel_pinned_session_drag(cx);
        }
    }
}
