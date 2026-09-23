//! The pending-message queue, docked above the composer.
//!
//! Everything you typed while the agent was busy, in the order it will be sent.
//! The rows live on the session doc ([`skylark_doc::QueuedMessage`]), so the phone
//! shows the same queue and either device can reorder it.
//!
//! Each row exposes a `Send now` control that interrupts the active response.
//! Editing moves the message into the composer while its leased row reserves
//! its position.

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, prelude::*, px,
};

use skylark_doc::{QueueDeliveryGate, QueuedMessage};
use skylark_rpc::methods;

use crate::composer::{Composer, QUEUE_COMPOSER_OVERLAP};
use crate::icons::{self, icon};
use crate::motion::{self, AnimationExt as _, TAB_SLIDE};
use crate::settings::shortcuts::modifier_send_label;
use crate::terminal::panel::{drop_index, slide_offset};
use crate::theme::Theme;

/// Queue rows are replicated CRDT state, so ordinary mutations deliberately
/// land on the local engine. Delivery and cancellation must execute on the
/// chat's owning device because they race over the same row.
fn queue_action_needs_host(method: &str) -> bool {
    matches!(
        method,
        methods::SEND_QUEUED_MESSAGE_NOW
            | methods::STEER_QUEUED_MESSAGE_NOW
            | methods::REMOVE_QUEUED_MESSAGE
            | methods::BEGIN_QUEUED_MESSAGE_EDIT
            | methods::RENEW_QUEUED_MESSAGE_EDIT
            | methods::FINISH_QUEUED_MESSAGE_EDIT
    )
}

/// Queue mutation replies are deliberately explicit. A false or malformed
/// acknowledgement means an optimistic local edit may not match the document
/// (for example, another device removed the same row first).
fn queue_mutation_acknowledged(method: &str, reply: &serde_json::Value) -> bool {
    let field = match method {
        methods::UPDATE_QUEUED_MESSAGE | methods::MOVE_QUEUED_MESSAGE => "changed",
        methods::REMOVE_QUEUED_MESSAGE => "removed",
        methods::SEND_QUEUED_MESSAGE_NOW | methods::STEER_QUEUED_MESSAGE_NOW => "sent",
        _ => return true,
    };
    reply.get(field).and_then(serde_json::Value::as_bool) == Some(true)
}

struct QueueActionTooltip {
    label: SharedString,
}

impl Render for QueueActionTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let card = div()
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(theme))
            .text_size(px(10.5))
            .text_color(theme.text_muted)
            .child(self.label.clone());
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}

/// Compact, borderless rows inside the queue's single glass surface.
const ROW_HEIGHT: f32 = 36.0;
const QUEUE_TEXT_SIZE: f32 = 12.5;
const ROW_GAP: f32 = 0.0;
const ROW_SLOT: f32 = ROW_HEIGHT + ROW_GAP;
const ROW_PAD_X: f32 = 8.0;
const ROW_RADIUS: f32 = 8.0;
const PANEL_RADIUS: f32 = 16.0;
const PANEL_PAD_TOP: f32 = 0.0;
/// The custom 24px queue glyphs have quieter geometry than the legacy set, so
/// render them slightly larger to preserve the previous optical weight.
const QUEUE_ICON_SIZE: f32 = 13.0;

/// The single trailing action a queue row advertises and executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueuePrimaryAction {
    SendNow,
}

impl QueuePrimaryAction {
    fn tooltip(self) -> &'static str {
        match self {
            Self::SendNow => "Send now (interrupt)",
        }
    }
}

/// All providers use Send now. Only host support and edit/review gates
/// determine whether the action is available.
fn available_queue_primary_action(
    delivery_blocked: bool,
    host_supports_actions: bool,
) -> Option<QueuePrimaryAction> {
    (!delivery_blocked && host_supports_actions).then_some(QueuePrimaryAction::SendNow)
}

fn queue_latest_shortcut_visible(
    index: usize,
    count: usize,
    reveal_requested: bool,
    action_available: bool,
) -> bool {
    index.checked_add(1) == Some(count) && reveal_requested && action_available
}

fn latest_queued_message(items: &[QueuedMessage]) -> Option<&QueuedMessage> {
    items.last()
}

/// Translate a pointer inside the whole panel into a row slot. The top pad
/// belongs to slot zero; the bottom pad clamps to the final row.
fn queue_drop_index(panel_y: f32, count: usize) -> usize {
    drop_index(panel_y - PANEL_PAD_TOP, ROW_SLOT, count)
}

/// Paint-only start and target positions for the PR #90 reorder treatment.
/// The dragged row travels to the hovered slot while every row in its path
/// slides into the space it leaves behind.
fn queue_drag_offsets(ix: usize, from: usize, prev_over: usize, over: usize) -> (f32, f32) {
    if ix == from {
        (
            (prev_over as f32 - from as f32) * ROW_SLOT,
            (over as f32 - from as f32) * ROW_SLOT,
        )
    } else {
        (
            slide_offset(ix, from, prev_over) * ROW_SLOT,
            slide_offset(ix, from, over) * ROW_SLOT,
        )
    }
}

/// A queue row being dragged (gpui drag-and-drop). Scoped to its chat so a
/// drag can't land in a queue it didn't come from.
pub struct QueueDragPayload {
    chat: String,
    from: usize,
}

/// Where the dragged row would land, including the previous slot needed to
/// restart the short PR #90-style slide from its current visual position.
pub struct QueueDragState {
    pub from: usize,
    pub over: usize,
    pub prev_over: usize,
    pub epoch: usize,
}

/// Invisible cursor ghost: the real row stays in the queue and moves between
/// slots, instead of following the pointer as a detached tooltip.
struct QueueGhost;

impl Render for QueueGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// One line of a queued message: the newlines that make it a paragraph in the
/// composer make it three rows here, and the row is one line tall.
fn one_line(text: &str) -> SharedString {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    SharedString::from(flat)
}

/// New queue rows contain only editable user text. During a rolling upgrade,
/// an older client may still have stored the attachment trailer in `text`.
/// Hide it only when the parsed paths exactly match the row's attachment field.
fn queue_visible_text(text: &str, attachments: &[String]) -> String {
    let text = crate::appshots::strip_context_for_display(text);
    if text.trim().is_empty() && !attachments.is_empty() {
        return crate::attachments::ATTACHMENT_ONLY_TEXT.to_string();
    }
    if attachments.is_empty() {
        return text.to_string();
    }
    let parsed = crate::attachments::parse_user_message_images(text);
    let paths_match = parsed.attachments.len() == attachments.len()
        && parsed
            .attachments
            .iter()
            .zip(attachments)
            .all(|(parsed, stored)| parsed.path == *stored);
    if !paths_match {
        return text.to_string();
    }
    if parsed.text.trim().is_empty() {
        crate::attachments::ATTACHMENT_ONLY_TEXT.to_string()
    } else {
        parsed.text
    }
}

/// Presentation-only metadata. Never expose the observed accessibility payload.
fn queue_attachment_labels(text: &str, paths: &[String]) -> Vec<String> {
    let presentations = crate::appshots::presentations(text);
    paths
        .iter()
        .map(|path| match presentations.get(path) {
            Some(appshot) => format!("{} Appshot", appshot.app_name),
            None => std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Image")
                .to_owned(),
        })
        .collect()
}

fn queue_panel_surface(theme: &Theme) -> gpui::Div {
    div()
        .occlude()
        .rounded_t(px(PANEL_RADIUS))
        .bg(crate::popover::surface_bg(theme))
        .border_1()
        .border_color(theme.border)
        .when(!theme.is_frost(), |el| el.shadow_lg())
        // Keep visible rows flush with the tray; only the portion tucked behind
        // the composer needs padding.
        .pb(px(QUEUE_COMPOSER_OVERLAP))
        .flex()
        .flex_col()
}

fn queue_rows(
    scroll: &gpui::ScrollHandle,
    max_height: gpui::Pixels,
    rows: impl IntoIterator<Item = AnyElement>,
) -> crate::edge_fade::EdgeFaded {
    crate::edge_fade::edge_faded(
        Theme::TRANSCRIPT_FADE_BAND,
        true,
        true,
        div()
            .id("message-queue-rows")
            .max_h(max_height)
            .overflow_y_scroll()
            .track_scroll(scroll)
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .children(rows),
    )
    .fade_overflow_y(scroll)
    // GPUI samples glyph fades at baseline + font size.
    .outset_bottom(QUEUE_TEXT_SIZE)
}

fn preview_load_gate() -> &'static futures::lock::Mutex<()> {
    static GATE: std::sync::OnceLock<futures::lock::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| futures::lock::Mutex::new(()))
}

pub(crate) struct QueuePreview {
    image: Option<crate::attachments::CachedAttachmentImage>,
    finished: bool,
    // Keeping the task here cancels offscreen transfers on eviction.
    _task: gpui::Task<()>,
}

fn visible_queue_rows(offset: f32, height: f32, count: usize) -> std::ops::Range<usize> {
    let first = ((-offset).max(0.0) / ROW_SLOT).floor() as usize;
    let last = first.saturating_add((height.max(0.0) / ROW_SLOT).ceil() as usize + 1);
    first.min(count)..last.min(count)
}

#[path = "queue/actions.rs"]
mod actions;
#[path = "queue/render.rs"]
mod render;

#[cfg(test)]
mod tests {
    use skylark_rpc::methods;

    use super::{
        PANEL_PAD_TOP, QueuePrimaryAction, ROW_SLOT, available_queue_primary_action,
        latest_queued_message, one_line, queue_action_needs_host, queue_drag_offsets,
        queue_drop_index, queue_latest_shortcut_visible, queue_mutation_acknowledged,
        queue_visible_text, visible_queue_rows,
    };

    #[test]
    fn queue_preview_work_follows_the_visible_rows() {
        assert_eq!(visible_queue_rows(0.0, ROW_SLOT * 3.0, 1000), 0..4);
        assert_eq!(
            visible_queue_rows(-ROW_SLOT * 20.0, ROW_SLOT * 3.0, 1000),
            20..24
        );
        assert_eq!(
            visible_queue_rows(-ROW_SLOT * 20.0, ROW_SLOT * 3.0, 0),
            0..0
        );
    }

    #[test]
    fn available_primary_action_obeys_row_and_host_gates() {
        assert_eq!(
            available_queue_primary_action(false, true),
            Some(QueuePrimaryAction::SendNow)
        );
        assert_eq!(available_queue_primary_action(true, true), None);
        assert_eq!(available_queue_primary_action(false, false), None);
    }

    #[test]
    fn queue_shortcut_only_appears_on_the_actionable_latest_row_when_revealed() {
        assert!(!queue_latest_shortcut_visible(0, 2, true, true));
        assert!(queue_latest_shortcut_visible(1, 2, true, true));
        assert!(!queue_latest_shortcut_visible(1, 2, false, true));
        assert!(!queue_latest_shortcut_visible(1, 2, true, false));
        assert!(!queue_latest_shortcut_visible(0, 0, true, true));
    }

    #[test]
    fn queue_shortcut_targets_the_most_recently_added_row() {
        let items = vec![
            skylark_doc::QueuedMessage::new("older", "first", "device"),
            skylark_doc::QueuedMessage::new("newer", "second", "device"),
        ];
        assert_eq!(latest_queued_message(&items).unwrap().id, "newer");
        assert!(latest_queued_message(&[]).is_none());
    }

    #[test]
    fn host_authoritative_queue_actions_route_to_the_host() {
        assert!(queue_action_needs_host(methods::SEND_QUEUED_MESSAGE_NOW));
        assert!(queue_action_needs_host(methods::STEER_QUEUED_MESSAGE_NOW));
        assert!(queue_action_needs_host(methods::REMOVE_QUEUED_MESSAGE));
        assert!(queue_action_needs_host(methods::BEGIN_QUEUED_MESSAGE_EDIT));
        assert!(queue_action_needs_host(methods::RENEW_QUEUED_MESSAGE_EDIT));
        assert!(queue_action_needs_host(methods::FINISH_QUEUED_MESSAGE_EDIT));
        assert!(!queue_action_needs_host(methods::QUEUE_MESSAGE));
        assert!(!queue_action_needs_host(methods::UPDATE_QUEUED_MESSAGE));
        assert!(!queue_action_needs_host(methods::MOVE_QUEUED_MESSAGE));
    }

    #[test]
    fn mutation_acknowledgements_detect_conflicts_and_malformed_replies() {
        assert!(queue_mutation_acknowledged(
            methods::MOVE_QUEUED_MESSAGE,
            &serde_json::json!({ "changed": true })
        ));
        assert!(!queue_mutation_acknowledged(
            methods::MOVE_QUEUED_MESSAGE,
            &serde_json::json!({ "changed": false })
        ));
        assert!(!queue_mutation_acknowledged(
            methods::REMOVE_QUEUED_MESSAGE,
            &serde_json::json!({})
        ));
        assert!(queue_mutation_acknowledged(
            methods::SEND_QUEUED_MESSAGE_NOW,
            &serde_json::json!({ "sent": true })
        ));
    }

    #[test]
    fn the_whole_panel_maps_to_a_clamped_queue_drop_slot() {
        assert_eq!(queue_drop_index(0.0, 2), 0, "top pad targets the head");
        assert_eq!(queue_drop_index(PANEL_PAD_TOP + ROW_SLOT - 0.1, 2), 0);
        assert_eq!(queue_drop_index(PANEL_PAD_TOP + ROW_SLOT, 2), 1);
        assert_eq!(queue_drop_index(10_000.0, 2), 1);
    }

    #[test]
    fn drag_offsets_move_the_real_row_and_open_its_destination() {
        assert_eq!(queue_drag_offsets(0, 0, 0, 2), (0.0, 2.0 * ROW_SLOT));
        assert_eq!(queue_drag_offsets(1, 0, 0, 2), (0.0, -ROW_SLOT));
        assert_eq!(queue_drag_offsets(2, 0, 0, 2), (0.0, -ROW_SLOT));

        // Moving the pointer back one slot restarts only the rows whose
        // visual destination actually changed.
        assert_eq!(queue_drag_offsets(0, 0, 2, 1), (2.0 * ROW_SLOT, ROW_SLOT));
        assert_eq!(queue_drag_offsets(1, 0, 2, 1), (-ROW_SLOT, -ROW_SLOT));
        assert_eq!(queue_drag_offsets(2, 0, 2, 1), (-ROW_SLOT, 0.0));
    }

    /// A row is one line tall, so a multi-line message has to read as one line
    /// — otherwise the panel's rows stop lining up.
    #[test]
    fn rows_flatten_multi_line_messages() {
        assert_eq!(
            one_line("fix the test\n\nthen ship it").as_ref(),
            "fix the test then ship it"
        );
        assert_eq!(one_line("  spaced   out  ").as_ref(), "spaced out");
    }

    #[test]
    fn appshot_context_is_hidden_in_clean_and_legacy_queue_rows() {
        let shot = crate::appshots::tests::shot();
        let paths: Vec<String> = vec!["/host/image.png".into()];
        for user_text in ["inspect this", ""] {
            let body = crate::appshots::with_appshots(
                user_text,
                &[shot.clone()],
                &std::collections::HashMap::from([(shot.screenshot.id.clone(), paths[0].clone())]),
            );
            let expected = if user_text.is_empty() {
                crate::attachments::ATTACHMENT_ONLY_TEXT
            } else {
                user_text
            };
            assert_eq!(queue_visible_text(&body, &paths), expected);
            assert_eq!(
                queue_visible_text(&crate::attachments::with_attachments(&body, &paths), &paths),
                expected
            );
        }
    }

    #[test]
    fn attachment_labels_decode_app_names_and_preserve_ordinary_images() {
        let shot = crate::appshots::tests::shot();
        let paths = vec![
            "/tmp/shot & detail.png".to_owned(),
            "/tmp/reference.png".to_owned(),
        ];
        let mut shot = shot;
        shot.app_name = "Notes & Ideas".into();
        let body = crate::appshots::with_appshots(
            "look",
            &[shot.clone()],
            &[(shot.screenshot.id.clone(), paths[0].clone())]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            super::queue_attachment_labels(&body, &paths),
            vec!["Notes & Ideas Appshot", "reference.png"]
        );
        assert_eq!(
            super::queue_attachment_labels(&body, &["/tmp/other.png".into()]),
            vec!["other.png"]
        );
        let malformed = format!("\n\n{}\n<appshot", crate::appshots::CONTEXT_MARKER);
        assert_eq!(
            super::queue_attachment_labels(&malformed, &paths),
            vec!["shot & detail.png", "reference.png"]
        );
    }

    #[test]
    fn legacy_attachment_trailers_are_hidden_from_queue_text() {
        let paths = vec!["/tmp/image.png".to_string()];
        let legacy = crate::attachments::with_attachments("inspect this", &paths);
        assert_eq!(queue_visible_text(&legacy, &paths), "inspect this");

        let image_only = crate::attachments::with_attachments("", &paths);
        assert_eq!(
            queue_visible_text(&image_only, &paths),
            crate::attachments::ATTACHMENT_ONLY_TEXT
        );
        assert_eq!(
            queue_visible_text("literal user text", &paths),
            "literal user text"
        );
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use gpui::{AppContext, ScrollHandle, TestAppContext, point};

    struct QueueScrollTestView {
        queue: ScrollHandle,
        transcript: ScrollHandle,
        count: usize,
    }

    impl Render for QueueScrollTestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(
                    div()
                        .id("transcript-underlay")
                        .absolute()
                        .inset_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.transcript)
                        .child(div().h(px(2000.0))),
                )
                .child(
                    div().absolute().bottom_0().w_full().child(
                        queue_panel_surface(Theme::of(cx)).child(queue_rows(
                            &self.queue,
                            px(180.0),
                            (0..self.count)
                                .map(|_| div().h(px(ROW_HEIGHT)).flex_none().into_any_element()),
                        )),
                    ),
                )
        }
    }

    #[gpui::test]
    fn queue_wheel_does_not_scroll_the_transcript_even_at_its_boundaries(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (view, cx) = cx.add_window_view(|_, _| QueueScrollTestView {
            queue: ScrollHandle::new(),
            transcript: ScrollHandle::new(),
            count: 24,
        });
        cx.simulate_resize(gpui::size(px(400.0), px(400.0)));
        cx.run_until_parked();
        let (queue, transcript) =
            view.read_with(cx, |view, _| (view.queue.clone(), view.transcript.clone()));
        for delta in [-80.0, -10_000.0, -80.0, 10_000.0, 80.0] {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: point(px(200.0), px(300.0)),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(delta))),
                ..Default::default()
            });
            cx.run_until_parked();
            assert_eq!(
                transcript.offset().y,
                px(0.0),
                "queue wheel leaked to transcript"
            );
            if delta == -80.0 {
                assert!(queue.offset().y < px(0.0), "the queue must still scroll");
            }
        }
        view.update(cx, |view, cx| {
            view.count = 2;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: point(px(200.0), px(370.0)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(transcript.offset().y, px(0.0));
        assert_eq!(queue.max_offset().y, px(0.0));
    }
}

#[cfg(test)]
mod appshot_edit_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn restoring_displaced_draft_keeps_appshots_and_new_captures(cx: &mut TestAppContext) {
        let state = cx.new(|_| crate::state::AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let original = crate::appshots::tests::shot();
            let mut during_save = original.clone();
            during_save.id = "during-save".into();
            composer.queue_edit_draft = Some(("draft".into(), vec![], vec![original]));
            composer.editing_queued = Some("row".into());
            composer.queue_edit_finishing = true;
            composer.stage_appshot(during_save, cx);
            assert!(composer.staged_appshots().is_empty());
            composer.clear_queue_edit_local(cx);
            assert_eq!(composer.input.read(cx).text(), "draft");
            assert_eq!(composer.staged_appshots().len(), 2);
            assert_eq!(composer.staged_appshots()[1].id, "during-save");
            assert!(composer.editing_queued.is_none());
        });
    }
}
