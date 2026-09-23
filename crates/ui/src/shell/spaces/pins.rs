use std::collections::HashSet;

use gpui::{AnyElement, Context, SharedString, div, px};

use super::super::*;
use crate::motion;
use crate::theme::Theme;

// Keep disclosure sections separated from their rows, not detached from
// adjacent sections. All sidebar section boundaries share this 4px rhythm.
pub(in crate::shell) const SIDEBAR_SECTION_GAP: f32 = 4.0;
pub(in crate::shell) const SIDEBAR_DISCLOSURE_HEADER_HEIGHT: f32 = 30.0;
pub(in crate::shell) const SIDEBAR_DISCLOSURE_BODY_INSET: f32 = 4.0;
pub(in crate::shell) const SIDEBAR_DISCLOSURE_SECTION_HEIGHT: f32 =
    SIDEBAR_SECTION_GAP + SIDEBAR_DISCLOSURE_HEADER_HEIGHT;
pub(in crate::shell) const SIDEBAR_DISCLOSURE_TWEEN_GRACE: std::time::Duration =
    std::time::Duration::from_millis(120);

/// Promote the user's ordered pins above the untouched activity projection.
/// Every unpinned id keeps exactly the relative order supplied by recency.
pub(in crate::shell) fn project_pinned_first(
    recency_ids: &[String],
    pinned_ids: &[String],
) -> Vec<String> {
    let active: HashSet<&str> = recency_ids.iter().map(String::as_str).collect();
    let pinned: HashSet<&str> = pinned_ids.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    pinned_ids
        .iter()
        .filter(|id| active.contains(id.as_str()))
        .chain(
            recency_ids
                .iter()
                .filter(|id| !pinned.contains(id.as_str())),
        )
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// Move only the dragged pin. Every other pin, including hidden/archived pins,
/// keeps its relative order; no other position needs to be written.
pub(in crate::shell) fn reorder_visible_pins(
    pinned_ids: &[String],
    visible_ids: &[String],
    from: usize,
    to: usize,
) -> Vec<String> {
    if from >= visible_ids.len() || to >= visible_ids.len() || from == to {
        return pinned_ids.to_vec();
    }

    let moved = &visible_ids[from];
    let anchor = &visible_ids[to];
    let mut result: Vec<_> = pinned_ids
        .iter()
        .filter(|id| *id != moved)
        .cloned()
        .collect();
    let Some(index) = result.iter().position(|id| id == anchor) else {
        return pinned_ids.to_vec();
    };
    result.insert(index + usize::from(from < to), moved.clone());
    result
}

/// Interruptible height tween for the sidebar's device/archive disclosures.
/// The rendered element owns the frame clock; this state preserves the current
/// interpolated height when a second click reverses an in-flight transition.
#[derive(Clone, Copy)]
pub struct SidebarDisclosureMotion {
    pub epoch: u64,
    pub from: f32,
    pub to: f32,
    pub started: std::time::Instant,
}

impl SidebarDisclosureMotion {
    pub fn new(epoch: u64, from: f32, to: f32) -> Self {
        Self {
            epoch,
            from,
            to,
            started: std::time::Instant::now(),
        }
    }

    pub fn current(self) -> f32 {
        let total = motion::COLLAPSE.total().as_secs_f32();
        let raw = if total > 0.0 {
            self.started.elapsed().as_secs_f32() / total
        } else {
            1.0
        };
        motion::lerp(self.from, self.to, motion::COLLAPSE.progress(raw))
    }

    pub fn animating(self) -> bool {
        self.started.elapsed() < motion::COLLAPSE.total() + SIDEBAR_DISCLOSURE_TWEEN_GRACE
    }
}

/// Remove only ids absent from the workspace. Archived sessions remain known
/// so unarchiving restores their local pin and position.
pub(in crate::shell) fn retain_known_pins(
    pinned_ids: &mut Vec<String>,
    known_chat_ids: &HashSet<String>,
) -> bool {
    let before = pinned_ids.len();
    let mut seen = HashSet::new();
    pinned_ids.retain(|id| known_chat_ids.contains(id) && seen.insert(id.clone()));
    pinned_ids.len() != before
}

/// Convert viewport coordinates to the first pinned row, below its disclosure.
pub(in crate::shell) fn pinned_session_pointer_y(
    pointer_y: f32,
    viewport_top: f32,
    scroll_top: f32,
) -> f32 {
    pointer_y - viewport_top + scroll_top
        - super::SIDEBAR_LIST_PAD_TOP
        - SIDEBAR_DISCLOSURE_HEADER_HEIGHT
        - SIDEBAR_DISCLOSURE_BODY_INSET
}

#[cfg(test)]
pub(in crate::shell) fn pinned_session_drop_index(rel_y: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let height = count as f32 * super::SIDEBAR_SESSION_SLOT - super::SIDEBAR_LIST_GAP;
    (rel_y >= 0.0 && rel_y <= height)
        .then(|| ((rel_y / super::SIDEBAR_SESSION_SLOT).floor() as usize).min(count - 1))
}

/// Hit testing shares the exact row metrics used by layout, including mixed PR rows.
pub(in crate::shell) fn row_drop_index(y: f32, heights: &[f32], clamp: bool) -> Option<usize> {
    if heights.is_empty() {
        return None;
    }
    let total =
        heights.iter().sum::<f32>() + heights.len().saturating_sub(1) as f32 * SIDEBAR_LIST_GAP;
    if !clamp && !(0.0..=total).contains(&y) {
        return None;
    }
    let mut bottom = 0.0;
    for (index, height) in heights.iter().enumerate() {
        bottom += height + SIDEBAR_LIST_GAP;
        if y < bottom {
            return Some(index);
        }
    }
    Some(heights.len() - 1)
}

/// Keep a sidebar-wide drag physically bounded to the pinned section. The
/// strict drop helper above still identifies whether the pointer is actually
/// inside the section; this helper supplies the nearest valid pinned slot
/// while the pointer is over regular sessions.
#[cfg(test)]
pub(in crate::shell) fn pinned_session_clamped_index(rel_y: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(((rel_y.max(0.0) / super::SIDEBAR_SESSION_SLOT).floor() as usize).min(count - 1))
}

/// A drop can change pin membership or pinned order, never activity ordering.
pub(in crate::shell) fn sidebar_session_drop_pins(
    saved: &[String],
    visible: &[String],
    chat_id: &str,
    target: SidebarSessionDrop,
) -> Vec<String> {
    let mut next = saved.to_vec();
    match target {
        SidebarSessionDrop::Regular => next.retain(|id| id != chat_id),
        SidebarSessionDrop::Pinned(index) => {
            if let Some(from) = visible.iter().position(|id| id == chat_id) {
                return reorder_visible_pins(saved, visible, from, index.min(visible.len() - 1));
            }
            if saved.iter().any(|id| id == chat_id) {
                return next;
            }
            let insertion = visible
                .get(index)
                .and_then(|anchor| next.iter().position(|id| id == anchor))
                .or_else(|| {
                    visible
                        .last()
                        .and_then(|anchor| next.iter().position(|id| id == anchor))
                        .map(|ix| ix + 1)
                })
                .unwrap_or(next.len());
            next.insert(insertion, chat_id.to_owned());
        }
    }
    next
}

/// Preview geometry only: regular ordering is never persisted by a drag.
pub(in crate::shell) fn sidebar_gap_offset(
    row: usize,
    source: Option<usize>,
    boundary: usize,
    height: f32,
) -> f32 {
    match source {
        Some(source) if row == source => 0.0,
        Some(source) if row < source && row >= boundary => height,
        Some(source) if row > source && row < boundary => -height,
        None if row >= boundary => height,
        _ => 0.0,
    }
}

pub(in crate::shell) fn pinned_drag_scroll_delta(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
) -> f32 {
    if viewport_bottom <= viewport_top {
        return 0.0;
    }
    if pointer_y < viewport_top + super::SIDEBAR_DRAG_SCROLL_BAND {
        let penetration = ((viewport_top + super::SIDEBAR_DRAG_SCROLL_BAND - pointer_y)
            / super::SIDEBAR_DRAG_SCROLL_BAND)
            .clamp(0.0, 1.0);
        -super::SIDEBAR_DRAG_SCROLL_MAX * penetration
    } else if pointer_y > viewport_bottom - super::SIDEBAR_DRAG_SCROLL_BAND {
        let penetration = ((pointer_y - (viewport_bottom - super::SIDEBAR_DRAG_SCROLL_BAND))
            / super::SIDEBAR_DRAG_SCROLL_BAND)
            .clamp(0.0, 1.0);
        super::SIDEBAR_DRAG_SCROLL_MAX * penetration
    } else {
        0.0
    }
}

pub(in crate::shell) fn pinned_drag_scroll_step(
    drag_active: bool,
    loop_generation: u64,
    drag_generation: u64,
    current: f32,
    max: f32,
    delta: f32,
) -> Option<f32> {
    if !drag_active || loop_generation != drag_generation || delta == 0.0 {
        return None;
    }
    let next = (current + delta).clamp(0.0, max.max(0.0));
    (next != current).then_some(next)
}

pub(in crate::shell) fn pinned_drag_snapshot_is_valid(
    dragged_id: &str,
    snapshot_ids: &[String],
    current_ids: &[String],
) -> bool {
    snapshot_ids.iter().any(|id| id == dragged_id) && snapshot_ids == current_ids
}

impl Shell {
    pub(in crate::shell) fn sidebar_transfer_extra_gap(&self, group: &str) -> f32 {
        self.sidebar_session_transfer
            .as_ref()
            .or_else(|| {
                self.sidebar_session_return
                    .as_ref()
                    .map(|state| &state.transfer)
            })
            .and_then(|drag| drag.section_gaps.get(group))
            .map_or(0.0, |gap| {
                if self.reduced_motion {
                    gap.to
                } else {
                    gap.current()
                }
            })
    }

    pub(in crate::shell) fn render_sidebar_gap_row(
        &mut self,
        row: AnyElement,
        id: &str,
        group: &str,
        index: usize,
    ) -> AnyElement {
        let returning = self.sidebar_session_transfer.is_none();
        let transfer = if let Some(drag) = self.sidebar_session_transfer.as_mut() {
            Some(drag)
        } else {
            self.sidebar_session_return
                .as_mut()
                .map(|returning| &mut returning.transfer)
        };
        let Some(drag) = transfer else {
            return row;
        };
        // Pin-to-pin keeps the existing sibling-slide implementation.
        let target = if returning || (group == "pinned" && drag.source_group == "pinned") {
            0.0
        } else {
            drag.preview
                .as_ref()
                .filter(|gap| gap.group == group)
                .map_or(0.0, |gap| {
                    sidebar_gap_offset(
                        index,
                        (drag.source_group == group).then_some(drag.source_index),
                        gap.index,
                        drag.row_height + SIDEBAR_LIST_GAP,
                    )
                })
        };
        let slide = drag
            .siblings
            .entry(id.to_owned())
            .or_insert_with(|| SidebarSessionSlide {
                from: 0.0,
                to: 0.0,
                epoch: drag.slide.epoch,
                started: std::time::Instant::now(),
            });
        slide.retarget(target);
        let (from, to, epoch) = (slide.from, slide.to, slide.epoch);
        let frame = div().relative().child(row);
        if !returning || self.reduced_motion {
            frame.top(px(to)).into_any_element()
        } else {
            frame
                .with_animation(
                    SharedString::from(format!("session-gap-{id}-{epoch}")),
                    TAB_SLIDE.animation(),
                    move |el, t| el.top(px(motion::lerp(from, to, t))),
                )
                .into_any_element()
        }
    }

    pub(in crate::shell) fn begin_sidebar_session_transfer(
        &mut self,
        payload: &SidebarSessionDrag,
        cursor_offset: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.chat_status_hover = None;
        self.cancel_pinned_session_drag(cx);
        self.sidebar_session_return = None;
        self.pinned_session_drag_generation = self.pinned_session_drag_generation.wrapping_add(1);
        let top = f32::from(
            window.mouse_position().y - cursor_offset.y - self.sidebar_scroll.bounds().top(),
        ) - f32::from(self.sidebar_scroll.offset().y);
        self.sidebar_session_transfer = Some(SidebarSessionTransfer {
            payload: payload.clone(),
            origin: std::rc::Rc::new(std::cell::Cell::new(
                window.mouse_position() - cursor_offset,
            )),
            cursor_offset,
            pointer: window.mouse_position(),
            viewport: None,
            preview: None,
            source_group: String::new(),
            source_index: 0,
            row_height: 0.0,
            source_collapse: SidebarSessionSlide {
                from: 0.0,
                to: 0.0,
                epoch: 0,
                started: std::time::Instant::now(),
            },
            collapsed_height: 0.0,
            section_gaps: Default::default(),
            siblings: Default::default(),
            slide: SidebarSessionSlide {
                from: top,
                to: top,
                epoch: self.pinned_session_drag_generation << 32,
                started: std::time::Instant::now(),
            },
        });
        {
            let origin = self
                .sidebar_session_transfer
                .as_ref()
                .unwrap()
                .origin
                .clone();
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(
                            SIDEBAR_DRAG_SCROLL_FRAME_MS,
                        ))
                        .await;
                    let keep_running = this
                        .update(cx, |shell, cx| {
                            let Some(transfer) = shell.sidebar_session_transfer.as_ref() else {
                                return false;
                            };
                            if !cx.has_active_drag()
                                || !std::rc::Rc::ptr_eq(&origin, &transfer.origin)
                            {
                                return false;
                            }
                            // Layout also animates while the pointer is stationary.
                            cx.notify();
                            // Pinned drags already have their own edge-scroll loop.
                            if transfer
                                .payload
                                .visible_ids
                                .contains(&transfer.payload.chat_id)
                            {
                                return true;
                            }
                            let Some(viewport) = transfer.viewport else {
                                return true;
                            };
                            if !viewport.contains(&transfer.pointer) {
                                return true;
                            }
                            let delta = pinned_drag_scroll_delta(
                                f32::from(transfer.pointer.y),
                                f32::from(viewport.top()),
                                f32::from(viewport.bottom()),
                            );
                            let offset = shell.sidebar_scroll.offset();
                            let scroll_top = -f32::from(offset.y);
                            let max_scroll = f32::from(shell.sidebar_scroll.max_offset().y);
                            let next = (scroll_top + delta).clamp(0.0, max_scroll);
                            if next != scroll_top {
                                shell
                                    .sidebar_scroll
                                    .set_offset(gpui::point(offset.x, px(-next)));
                                cx.notify();
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !keep_running {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub(in crate::shell) fn sidebar_session_transfer_is_valid(
        &self,
        payload: &SidebarSessionDrag,
        cx: &App,
    ) -> bool {
        if payload.filter != self.settings.space_filter
            || self.active_sidebar_pin_profile_key(cx).as_deref() != Some(&payload.profile_key)
        {
            return false;
        }
        let state = self.state.read(cx);
        let visible: HashSet<String> = state
            .sidebar_chats(Utc::now(), payload.filter.as_deref())
            .into_iter()
            .map(|(_, chat)| chat.id.clone())
            .collect();
        if !visible.contains(&payload.chat_id) {
            return false;
        }
        let pins = self.sidebar_pins_for_profile(&payload.profile_key, cx);
        let current: Vec<_> = pins.into_iter().filter(|id| visible.contains(id)).collect();
        current == *payload.visible_ids
    }

    pub(in crate::shell) fn cancel_sidebar_session_transfer(&mut self, cx: &mut Context<Self>) {
        self.chat_status_hover = None;
        self.cancel_pinned_session_drag(cx);
        if let Some(mut transfer) = self.sidebar_session_transfer.take() {
            transfer.preview = None;
            if !self.reduced_motion && self.sidebar_session_transfer_is_valid(&transfer.payload, cx)
            {
                self.sidebar_session_return = Some(SidebarSessionReturn {
                    transfer,
                    epoch: self.pinned_session_drag_generation,
                    started: std::time::Instant::now(),
                });
                let epoch = self.pinned_session_drag_generation;
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(
                                SIDEBAR_DRAG_SCROLL_FRAME_MS,
                            ))
                            .await;
                        let keep_running = this
                            .update(cx, |shell, cx| {
                                let Some(returning) = shell.sidebar_session_return.as_ref() else {
                                    return false;
                                };
                                if returning.epoch != epoch {
                                    return false;
                                }
                                if returning.started.elapsed() >= TAB_SLIDE.total()
                                    && returning.transfer.slide.started.elapsed()
                                        >= TAB_SLIDE.total()
                                    && returning.transfer.source_collapse.started.elapsed()
                                        >= TAB_SLIDE.total()
                                    && returning
                                        .transfer
                                        .section_gaps
                                        .values()
                                        .all(|gap| gap.started.elapsed() >= TAB_SLIDE.total())
                                {
                                    shell.sidebar_session_return = None;
                                    cx.notify();
                                    return false;
                                }
                                cx.notify();
                                true
                            })
                            .unwrap_or(false);
                        if !keep_running {
                            break;
                        }
                    }
                })
                .detach();
                self.pinned_session_drag_generation =
                    self.pinned_session_drag_generation.wrapping_add(1);
            }
            cx.notify();
        }
    }

    pub(in crate::shell) fn render_moving_sidebar_session(
        &mut self,
        row: AnyElement,
        height: f32,
        theme: &Theme,
    ) -> AnyElement {
        let viewport = self.sidebar_scroll.bounds();
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let returning = self.sidebar_session_transfer.is_none();
        let transfer = if let Some(transfer) = self.sidebar_session_transfer.as_mut() {
            transfer
        } else {
            &mut self.sidebar_session_return.as_mut().unwrap().transfer
        };
        let origin = f32::from(transfer.origin.get().y - viewport.top()) + scroll_top;
        let target = if returning {
            origin
        } else {
            f32::from(transfer.pointer.y - transfer.cursor_offset.y - viewport.top()) + scroll_top
        };
        if returning {
            transfer.slide.retarget(target);
        } else {
            transfer.slide.from = target;
            transfer.slide.to = target;
            transfer.slide.started = std::time::Instant::now();
        }
        let from = transfer.slide.from;
        let to = transfer.slide.to;
        let epoch = transfer.slide.epoch;
        let frame = div()
            .absolute()
            .left(px(Theme::SPACE_SM))
            .right(px(Theme::SPACE_SM))
            .h(px(height))
            .rounded(px(8.0))
            .when(!returning, |el| el.bg(theme.surface_raised).shadow_md())
            .child(row);
        if self.reduced_motion {
            frame.top(px(to)).into_any_element()
        } else {
            frame
                .with_animation(
                    ("sidebar-session-slide", epoch),
                    TAB_SLIDE.animation(),
                    move |el, t| el.top(px(motion::lerp(from, to, t))),
                )
                .into_any_element()
        }
    }

    pub(in crate::shell) fn finish_sidebar_session_transfer(
        &mut self,
        payload: &SidebarSessionDrag,
        target: SidebarSessionDrop,
        cx: &mut Context<Self>,
    ) {
        self.chat_status_hover = None;
        let matches_drag = self.sidebar_session_transfer.as_ref().is_some_and(|drag| {
            drag.payload.chat_id == payload.chat_id
                && drag.payload.profile_key == payload.profile_key
                && drag.payload.filter == payload.filter
                && drag.payload.visible_ids == payload.visible_ids
        });
        if !matches_drag || !self.sidebar_session_transfer_is_valid(payload, cx) {
            self.cancel_sidebar_session_transfer(cx);
            return;
        }
        let saved = self.sidebar_pins_for_profile(&payload.profile_key, cx);
        let next =
            sidebar_session_drop_pins(&saved, &payload.visible_ids, &payload.chat_id, target);
        // Validate and accept before ending the preview. Rejected drops use
        // the same animated return path as dropping outside a destination.
        let change = if let Some(index) = next.iter().position(|id| id == &payload.chat_id) {
            let after = index.checked_sub(1).and_then(|i| next.get(i)).cloned();
            let before = next.get(index + 1).cloned();
            if saved.contains(&payload.chat_id) {
                skylark_proto::SidebarPinChange::Move {
                    session_id: payload.chat_id.clone(),
                    after,
                    before,
                }
            } else {
                skylark_proto::SidebarPinChange::Pin {
                    session_id: payload.chat_id.clone(),
                    after,
                    before,
                }
            }
        } else {
            skylark_proto::SidebarPinChange::Unpin {
                session_id: payload.chat_id.clone(),
            }
        };
        if !self.apply_sidebar_pin_change(payload.profile_key.clone(), change, cx) {
            self.cancel_sidebar_session_transfer(cx);
            return;
        }
        self.sidebar_session_transfer = None;
        self.cancel_pinned_session_drag(cx);
        if matches!(target, SidebarSessionDrop::Pinned(_)) {
            self.pinned_open = true;
        } else {
            self.sessions_open = true;
        }
        // Only pin preferences change. Regular rows keep their live activity sort.
        // Drag previews already animated this move. Establish a fresh layout
        // baseline so the automatic resort glide does not replay it on release.
        self.sidebar_prev_order.clear();
        self.sidebar_resort.clear();
        self.sidebar_new_keys.clear();
        cx.notify();
    }

    pub(in crate::shell) fn update_pinned_session_drag(
        &mut self,
        payload: &SidebarSessionDrag,
        over: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.pinned_open
            || self.active_sidebar_pin_profile_key(cx).as_deref() != Some(&payload.profile_key)
        {
            self.cancel_pinned_session_drag(cx);
            return;
        }
        let Some(from) = payload
            .visible_ids
            .iter()
            .position(|id| id == &payload.chat_id)
        else {
            self.cancel_pinned_session_drag(cx);
            return;
        };
        if over >= payload.visible_ids.len() {
            self.cancel_pinned_session_drag(cx);
            return;
        }
        match &mut self.pinned_session_drag {
            Some(drag)
                if drag.chat_id == payload.chat_id
                    && drag.filter == payload.filter
                    && drag.profile_key == payload.profile_key
                    && drag.visible_ids.as_ref() == payload.visible_ids.as_ref()
                    && drag.over != over =>
            {
                drag.prev_over = drag.over;
                drag.over = over;
                drag.epoch = drag.epoch.wrapping_add(1);
                cx.notify();
            }
            Some(drag)
                if drag.chat_id == payload.chat_id
                    && drag.filter == payload.filter
                    && drag.profile_key == payload.profile_key
                    && drag.visible_ids.as_ref() == payload.visible_ids.as_ref() => {}
            _ => {
                self.pinned_session_drag_generation =
                    self.pinned_session_drag_generation.wrapping_add(1);
                self.pinned_session_drag = Some(PinnedSessionDragState {
                    chat_id: payload.chat_id.clone(),
                    visible_ids: payload.visible_ids.clone(),
                    from,
                    over,
                    prev_over: from,
                    epoch: 0,
                    filter: payload.filter.clone(),
                    profile_key: payload.profile_key.clone(),
                    pointer_y: None,
                    viewport_top: 0.0,
                    viewport_bottom: 0.0,
                    generation: self.pinned_session_drag_generation,
                    autoscroll_active: false,
                });
                cx.notify();
            }
        }
    }

    pub(in crate::shell) fn track_pinned_session_drag_pointer(
        &mut self,
        payload: SidebarSessionDrag,
        pointer_y: f32,
        viewport_top: f32,
        viewport_bottom: f32,
        cx: &mut Context<Self>,
    ) {
        if !payload.visible_ids.contains(&payload.chat_id) {
            return;
        }
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let rel_y = pinned_session_pointer_y(pointer_y, viewport_top, scroll_top);
        let inside_pinned_section =
            row_drop_index(rel_y, &self.sidebar_pinned_heights, false).is_some();
        let Some(over) = row_drop_index(rel_y, &self.sidebar_pinned_heights, true) else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.pointer_y = None;
                drag.autoscroll_active = false;
            }
            return;
        };

        self.update_pinned_session_drag(&payload, over, cx);
        if !inside_pinned_section {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.pointer_y = None;
                drag.autoscroll_active = false;
            }
            return;
        }
        let delta = pinned_drag_scroll_delta(pointer_y, viewport_top, viewport_bottom);
        let Some(drag) = self.pinned_session_drag.as_mut() else {
            return;
        };
        drag.pointer_y = Some(pointer_y);
        drag.viewport_top = viewport_top;
        drag.viewport_bottom = viewport_bottom;
        let should_start = delta != 0.0 && !drag.autoscroll_active;
        let generation = drag.generation;
        if should_start {
            drag.autoscroll_active = true;
            self.start_pinned_session_autoscroll(generation, cx);
        }
    }

    pub(in crate::shell) fn start_pinned_session_autoscroll(
        &mut self,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(
                        super::SIDEBAR_DRAG_SCROLL_FRAME_MS,
                    ))
                    .await;
                let keep_running = this
                    .update(cx, |shell, cx| {
                        shell.pinned_session_autoscroll_tick(generation, cx)
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::shell) fn pinned_session_autoscroll_tick(
        &mut self,
        generation: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.pinned_session_drag.as_ref() else {
            return false;
        };
        if drag.generation != generation || !cx.has_active_drag() {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        }
        let Some(pointer_y) = drag.pointer_y else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        };
        let viewport_top = drag.viewport_top;
        let viewport_bottom = drag.viewport_bottom;
        let delta = pinned_drag_scroll_delta(pointer_y, viewport_top, viewport_bottom);
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let max_scroll = f32::from(self.sidebar_scroll.max_offset().y);
        let Some(next_scroll) = pinned_drag_scroll_step(
            true,
            generation,
            drag.generation,
            scroll_top,
            max_scroll,
            delta,
        ) else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        };

        let rel_y = pinned_session_pointer_y(pointer_y, viewport_top, next_scroll);
        let Some(over) = row_drop_index(rel_y, &self.sidebar_pinned_heights, false) else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        };

        let offset = self.sidebar_scroll.offset();
        self.sidebar_scroll
            .set_offset(gpui::point(offset.x, px(-next_scroll)));
        if let Some(drag) = self.pinned_session_drag.as_mut()
            && drag.over != over
        {
            drag.prev_over = drag.over;
            drag.over = over;
            drag.epoch = drag.epoch.wrapping_add(1);
        }
        cx.notify();
        true
    }

    pub(in crate::shell) fn pinned_session_drag_is_valid(&self, cx: &App) -> bool {
        let Some(drag) = self.pinned_session_drag.as_ref() else {
            return true;
        };
        if !self.pinned_open
            || drag.filter != self.settings.space_filter
            || self.active_sidebar_pin_profile_key(cx).as_deref() != Some(&drag.profile_key)
        {
            return false;
        }
        let current_pins = self.sidebar_pins_for_profile(&drag.profile_key, cx);
        let visible_ids: HashSet<String> = self
            .state
            .read(cx)
            .overview_chats(Utc::now())
            .into_iter()
            .filter(|(_, chat)| match &drag.filter {
                Some(space_id) => chat.space_id.as_deref() == Some(space_id.as_str()),
                None => true,
            })
            .map(|(_, chat)| chat.id.clone())
            .collect();
        let current_ids = current_pins
            .iter()
            .filter(|id| visible_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        pinned_drag_snapshot_is_valid(&drag.chat_id, drag.visible_ids.as_ref(), &current_ids)
    }

    pub(in crate::shell) fn cancel_pinned_session_drag(&mut self, cx: &mut Context<Self>) {
        if self.pinned_session_drag.take().is_some() {
            self.pinned_session_drag_generation =
                self.pinned_session_drag_generation.wrapping_add(1);
            cx.notify();
        }
    }
}
