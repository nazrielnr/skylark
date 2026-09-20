//! The composer: a hand-rolled multiline text input (adapted from gpui's
//! `examples/input.rs`), the compact↔expanded flip, the Send/Queue/Stop morph,
//! optimistic send with failure recovery, per-chat drafts, and the question
//! wizard that replaces the composer while a run awaits input.
//!
//! Pure decision logic (flip, auto-grow math, button morph, wizard reducer,
//! pending-input detection) lives in free functions/structs with unit tests;
//! the gpui element only feeds them measurements.

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyTooltip, App, BorderStyle, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle,
    DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, PaintQuad, Pixels, SharedString, Style, StyledImage as _, Subscription, Task,
    TextRun, UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, quad, relative,
    size,
};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(test)]
use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_proto::{HarnessId, SlashCommand};
#[cfg(test)]
use zeron_proto::UserInputQuestion;

use crate::appshots::CapturedAppshot;
#[cfg(test)]
use crate::appshots;
use crate::attachments::{self, StagedAttachment};
use crate::motion;
use crate::pickers::Pickers;
use crate::state::AppState;
use crate::theme::Theme;


pub mod decision;
pub use decision::*;

// ---------------------------------------------------------------------------
// Multiline text input (adapted from gpui examples/input.rs)
// ---------------------------------------------------------------------------

actions!(
    composer,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        Copy,
        Cut,
        Paste,
        Newline,
        MessageNewlineOrAccept,
        ModifiedSubmit,
        Submit,
        Undo,
        Redo,
        MentionTab,
    ]
);

pub(crate) mod attachment_strip;
pub use attachment_strip::*;

pub(crate) mod wizard;
pub use wizard::*;

pub mod completions;
pub use completions::*;

pub mod send;

pub mod state_sync;

pub mod render;

pub(crate) mod input;
pub use input::{
    init, sent_mention_display, ComposerInput, ComposerInputEvent, SentMentionSpan,
};
pub(crate) use input::MESSAGE_COMPOSER_CONTEXT;
#[cfg(test)]
pub(crate) use input::message_input_context;

#[cfg(test)]
pub(crate) use input::{
    display_row_segments, dropped_file_mention, enter_outcome, file_mention_links,
    input_drag_scroll_delta, input_max_scroll, input_overflow_edges, input_reveal_height,
    input_scroll_offset, input_scroll_offset_for_cursor, local_file_link, mention_display_labels,
    mention_tooltip_contains, mention_tooltip_promote, mention_tooltip_reduce,
    message_enter_bindings, press_intent, EnterOutcome, FileMentionLink, MentionTooltipPhase,
    MentionTooltipTarget, MessageEnterBinding, MessageEnterBindingAction, PressIntent,
    TextProjection, FILE_MENTION_SCHEME, GENERIC_COMPOSER_CONTEXT,
};

// ---------------------------------------------------------------------------
// Composer wrapper
// ---------------------------------------------------------------------------

/// Events the shell listens for.
#[derive(Debug, Clone)]
pub enum ComposerEvent {
    /// Arm the shared-element transition before the draft route is replaced
    /// by the newly-created session. Emitting this before `select_chat` keeps
    /// the first destination frame on the same timeline as the source frame.
    NewThreadTransitionStarted,
    /// A prompt was sent optimistically — give the transcript its exact row
    /// identity so it can anchor the prompt at the top with the reply's
    /// reserved space below it.
    Sent { chat_id: String, message_id: String },
    /// A new worktree's host-side setup attempt completed after its chat id
    /// was minted. The shell attaches an already-open terminal to that exact
    /// chat, even when the user has selected another chat in the meantime.
    WorktreeSetup {
        chat_id: String,
        setup_action: Option<zeron_proto::ProjectActionRun>,
        setup_error: Option<String>,
        target_device_id: Option<String>,
    },
    /// A locally-authored queue row was accepted. It is not a transcript send
    /// yet: the transcript remembers the stable id and promotes it to an
    /// own-turn anchor only when the host materializes the matching bubble.
    Queued { chat_id: String, message_id: String },
}

pub struct Composer {
    pub(crate) state: Entity<AppState>,
    pub(crate) input: Entity<ComposerInput>,
    /// Draft displaced while a queued message occupies the composer.
    pub(crate) queue_edit_draft: Option<(String, Vec<StagedAttachment>, Vec<CapturedAppshot>)>,
    /// Composer actions row plus the new-session floating target tab
    /// ([`Pickers::render_new_thread_target_selectors`]).
    pub(super) pickers: Entity<Pickers>,
    /// Draft text per chat key ("" = new-chat canvas), surviving navigation.
    pub(super) drafts: HashMap<String, String>,
    /// Staged-but-unsent attachments per chat key (use-attachments.ts `stash`):
    /// navigating away and back restores them; memory-only, like the original.
    pub(crate) attachments: HashMap<String, Vec<StagedAttachment>>,
    /// Rich window captures keyed exactly like drafts and ordinary staged
    /// attachments. Each owns one screenshot that joins the existing upload
    /// path only at send time.
    pub(crate) appshots: HashMap<String, Vec<CapturedAppshot>>,
    pub(super) appshot_entrances: HashMap<String, Instant>,
    /// The staged attachment being viewed full-size (click a thumbnail).
    pub(super) preview: Option<attachments::PreviewImage>,
    /// Focused while the lightbox is open so Escape reaches it; the input
    /// gets focus back on close.
    pub(super) preview_focus: FocusHandle,
    /// Focus grab deferred to the next render (open sites don't all have a
    /// `Window` — the `ZERON_ATTACH_PREVIEW` boot knob opens in `new`).
    pub(super) preview_focus_pending: bool,
    /// In-flight file-picker prompt (paperclip).
    pub(super) picker_task: Option<Task<()>>,
    pub(super) mention_task: Option<Task<()>>,
    pub(super) mention: FileMentionState,
    pub(super) slash_task: Option<Task<()>>,
    pub(super) slash: SlashState,
    /// Advertised commands per harness (one `ListCommands` per harness per
    /// composer lifetime; the engine caches discovery on its side too).
    pub(super) slash_cache: HashMap<HarnessId, Vec<SlashCommand>>,
    /// Slash-popup row scroll — the stack overflows into a wheel/keyboard-
    /// scrollable list once it outgrows the card.
    pub(super) slash_scroll: gpui::ScrollHandle,
    /// File-mention popup row scroll (same treatment).
    pub(super) mention_scroll: gpui::ScrollHandle,
    /// Shared scrollbar hover/drag state for both popups' floating rails —
    /// they never show at once (mutually exclusive by token shape).
    pub(super) popup_bar: crate::popover::MenuScrollbarState,
    pub(crate) current_key: String,
    pub(super) sending: bool,
    /// Armed immediately before a blank-canvas send selects its minted chat.
    /// The state observer consumes it to distinguish that handoff from normal
    /// session navigation, which must continue to snap.
    pub(super) launching_new_chat: bool,
    pub(crate) failure: Option<SharedString>,
    /// The chat key `failure` belongs to (`None` = global, e.g. "Engine not
    /// connected"). Chat-scoped failures survive navigation and render only
    /// under their own chat — a blanket clear-on-switch erased the one
    /// visible trace of a failed send (2026-08-19).
    pub(crate) failure_key: Option<String>,
    pub(crate) wizard: Option<Wizard>,
    pub(crate) wizard_focus: FocusHandle,
    /// Requests already answered locally (suppresses the panel until the doc
    /// frame marks them resolved).
    pub(crate) answered_requests: HashSet<String>,
    pub(crate) advance_task: Option<Task<()>>,
    pub(super) send_task: Option<Task<()>>,
    /// The queued message being edited in the composer (see
    /// [`Composer::begin_queue_edit`]).
    pub(crate) editing_queued: Option<String>,
    /// Host-issued generation protecting `editing_queued` from automatic
    /// delivery. The text buffer is kept until Finish receives an ACK.
    pub(crate) queue_edit_lease_id: Option<String>,
    pub(crate) queue_edit_base_text_hash: Option<String>,
    pub(crate) queue_edit_chat_id: Option<String>,
    pub(crate) queue_edit_host_device_id: Option<String>,
    pub(crate) queue_edit_instance_id: String,
    pub(crate) queue_edit_pending_id: Option<String>,
    pub(crate) queue_edit_finishing: bool,
    pub(crate) queue_edit_task: Option<Task<()>>,
    pub(crate) queue_edit_renew_task: Option<Task<()>>,
    /// Focus once on mount, navigation, or after opening/closing a queue edit.
    pub(crate) focus_pending: bool,
    /// Live drag over the queue panel: which row, and where it would land.
    pub(crate) queue_drag: Option<crate::queue::QueueDragState>,
    pub(crate) queue_scroll: gpui::ScrollHandle,
    pub(crate) queue_full_preview: Option<Task<()>>,
    pub(crate) queue_previews: HashMap<(String, String), crate::queue::QueuePreview>,
    /// Rows awaiting a host-authoritative removal acknowledgement. They stay
    /// visible but inert until the host wins the race against queue delivery.
    pub(crate) queue_removing: HashSet<String>,
    /// Whether the modifier overlay should currently reveal the queue hint.
    /// The shell owns modifier tracking and clears this on window deactivation.
    pub(super) queue_shortcut_revealed: bool,
    /// Interrupt/answer commands get their own slot: assigning `send_task`
    /// DROPPED an in-flight send future mid-upload — no banner, no cleanup,
    /// `sending` stuck true forever (2026-08-19 incident, "press Stop while
    /// a send grinds" shape).
    pub(crate) action_task: Option<Task<()>>,
    /// Chats whose durable Interrupt command has been accepted or is still
    /// being queued. Kept independently so stopping one chat cannot replace
    /// another chat's request when the user navigates quickly.
    pub(super) interrupting: HashSet<String>,
    pub(super) interrupt_tasks: HashMap<String, Task<()>>,
    // -- compact/expanded flip state (hysteresis; see `composer_flip`) --
    /// Current layout mode (persisted across frames — never derived fresh).
    pub(super) expanded_mode: bool,
    /// `layout_epoch` of the measurement that caused the last flip: the flip is
    /// re-evaluated only after the input has been laid out in the new mode, so
    /// at most one flip can happen per layout pass.
    pub(super) flip_epoch: u64,
    /// Compact-mode input capacity, learned while compact (layout-stable).
    pub(super) compact_capacity: f32,
    /// Input width first measured after expanding — container-width deltas
    /// while expanded shift `compact_capacity` by the same amount.
    pub(super) expanded_anchor: f32,
    /// Last input width seen in the current mode (resize detection).
    pub(super) last_seen_width: f32,
    /// Stable outer composer width supplied by the shell. Unlike Taffy's
    /// provisional input measurements, this changes only when the actual
    /// conversation column changes and can safely drive a follow-up render.
    pub(super) last_available_width: Option<f32>,
    /// Set while an interactive resize is in flight; collapse is deferred
    /// until widths have settled for [`RESIZE_SETTLE_MS`].
    pub(super) width_changed_at: Option<Instant>,
    pub(super) settle_task: Option<Task<()>>,
    /// In-flight compact↔expanded morph (one per committed flip; manual
    /// drive — see [`FlipMorph`]).
    pub(super) flip_morph: Option<FlipMorph>,
    /// Pill height actually rendered last frame — a committed flip morphs
    /// from here, so mid-flight reversals hand off without a jump.
    pub(super) last_rendered_height: f32,
    pub(super) model_handoff_position: f32,
    pub(super) model_handoff_from: f32,
    pub(super) model_handoff_morph: Option<FlipMorph>,
    pub(super) model_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    pub(super) dock_frame: Option<crate::composer_dock::DockFrame>,
    /// The shared clock owns this frame's height, including its final step.
    pub(super) dock_height_changed: bool,
    pub(super) dock_clearance_correction: f32,
    pub(super) surface_bounds: crate::new_thread_background_mask::SurfaceBounds,
    pub(super) last_target_height: f32,
    pub(super) height_morph: Option<FlipMorph>,
    /// Monotonic clock anchor for the morph timeline.
    pub(super) morph_clock: Instant,
    /// Set on every session/route change: flips committed before this instant
    /// SNAP instead of morphing (see [`ROUTE_SNAP_MS`]).
    pub(super) route_snap_until: Option<Instant>,
    _observe: Subscription,
    _pickers_observe: Subscription,
    _picker_focus: Subscription,
    _input_events: Subscription,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub(crate) fn set_dock_frame(
        &mut self,
        frame: crate::composer_dock::DockFrame,
        cx: &mut Context<Self>,
    ) {
        let changed = self.dock_frame != Some(frame);
        self.dock_height_changed |= self
            .dock_frame
            .is_none_or(|previous| previous.amount != frame.amount);
        self.dock_frame = Some(frame);
        if frame.active {
            self.flip_morph = None;
            self.height_morph = None;
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn dock_clearance_correction(&self) -> f32 {
        self.dock_clearance_correction
    }

    pub(crate) fn surface_bounds(&self) -> crate::new_thread_background_mask::SurfaceBounds {
        self.surface_bounds.clone()
    }

    /// The picker entity, for the shell's canvas target selectors.
    pub fn pickers(&self) -> &Entity<Pickers> {
        &self.pickers
    }

    /// Feed the stable conversation-column width into responsive composer
    /// controls.
    pub fn set_available_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let composer_width = width.clamp(0.0, COMPOSER_MAX_WIDTH);
        if composer_width_changed(self.last_available_width, composer_width) {
            self.last_available_width = Some(composer_width);
            // The shell renders before this child, so this queues one more
            // pass after the input has been laid out at its final width. That
            // pass can consume the completed measurement without emitting an
            // event from inside Taffy's multi-pass measurement callback.
            cx.notify();
        }
    }

    pub(crate) fn set_queue_shortcut_revealed(&mut self, revealed: bool, cx: &mut Context<Self>) {
        if self.queue_shortcut_revealed != revealed {
            self.queue_shortcut_revealed = revealed;
            cx.notify();
        }
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.release_queue_previews(cx))
            .detach();
        let input = cx.new(|cx| {
            let mut input =
                ComposerInput::with_context("Do anything…", MESSAGE_COMPOSER_CONTEXT, cx);
            input.enable_mentions();
            input
        });
        let pickers = cx.new(|cx| Pickers::new(state.clone(), cx));
        // The footer toolbar (checkout kind + ref picker) is rendered INLINE
        // by the composer from picker state — a pickers-side notify (refs
        // loaded, popover toggled, pick made) must repaint the composer too.
        let pickers_observe = cx.observe(&pickers, |_, _, cx| cx.notify());
        let picker_focus = cx.subscribe(
            &pickers,
            |this: &mut Self, _, _: &crate::pickers::ReturnComposerFocus, cx| {
                this.focus_pending = true;
                cx.notify();
            },
        );
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.on_state_changed(cx));
        let input_events = cx.subscribe(&input, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Submitted => this.on_submit(cx),
            ComposerInputEvent::ModifiedSubmitted => this.on_modified_submit(cx),
            ComposerInputEvent::Edited | ComposerInputEvent::CursorMoved => {
                this.on_input_edited(cx)
            }
            ComposerInputEvent::ViewportChanged => cx.notify(),
            // The slash popup and the mention popup share the input's
            // completion key routing; they are mutually exclusive by token
            // shape (`/` at offset 0 vs `@` at a token boundary).
            ComposerInputEvent::MentionNavigate(delta) => {
                if this.slash.token.is_some() {
                    this.move_slash(*delta, cx)
                } else {
                    this.move_mention(*delta, cx)
                }
            }
            ComposerInputEvent::MentionAccept => {
                if this.slash.token.is_some() {
                    this.accept_slash(cx)
                } else {
                    this.accept_mention(cx)
                }
            }
            ComposerInputEvent::MentionDismiss => {
                if this.slash.token.is_some() {
                    this.dismiss_slash(cx)
                } else {
                    this.dismiss_mention(cx)
                }
            }
            ComposerInputEvent::PastedImages(images) => {
                let staged = images
                    .iter()
                    .map(|image| attachments::stage_clipboard_image(image.clone()))
                    .collect();
                this.add_staged(staged, cx);
            }
            ComposerInputEvent::PastedPaths(paths) => this.add_paths(paths.clone(), cx),
        });
        let current_key = state.read(cx).selected_chat.clone().unwrap_or_default();
        let mut composer = Self {
            state,
            input,
            queue_edit_draft: None,
            pickers,
            drafts: HashMap::new(),
            attachments: HashMap::new(),
            appshots: HashMap::new(),
            appshot_entrances: HashMap::new(),
            preview: None,
            preview_focus: cx.focus_handle(),
            preview_focus_pending: false,
            picker_task: None,
            mention_task: None,
            mention: FileMentionState::default(),
            slash_task: None,
            slash: SlashState::default(),
            slash_cache: HashMap::new(),
            slash_scroll: gpui::ScrollHandle::new(),
            mention_scroll: gpui::ScrollHandle::new(),
            popup_bar: crate::popover::MenuScrollbarState::default(),
            current_key,
            sending: false,
            launching_new_chat: false,
            failure: None,
            wizard: None,
            wizard_focus: cx.focus_handle(),
            answered_requests: HashSet::new(),
            failure_key: None,
            action_task: None,
            advance_task: None,
            send_task: None,
            interrupting: HashSet::new(),
            interrupt_tasks: HashMap::new(),
            editing_queued: None,
            queue_edit_lease_id: None,
            queue_edit_base_text_hash: None,
            queue_edit_chat_id: None,
            queue_edit_host_device_id: None,
            queue_edit_instance_id: uuid::Uuid::new_v4().to_string(),
            queue_edit_pending_id: None,
            queue_edit_finishing: false,
            queue_edit_task: None,
            queue_edit_renew_task: None,
            focus_pending: true,
            queue_drag: None,
            queue_scroll: gpui::ScrollHandle::new(),
            queue_full_preview: None,
            queue_previews: HashMap::new(),
            queue_removing: HashSet::new(),
            queue_shortcut_revealed: false,
            expanded_mode: false,
            flip_epoch: 0,
            compact_capacity: 0.0,
            expanded_anchor: 0.0,
            last_seen_width: 0.0,
            last_available_width: None,
            width_changed_at: None,
            settle_task: None,
            flip_morph: None,
            last_rendered_height: 0.0,
            model_handoff_position: 1.0,
            model_handoff_from: 1.0,
            model_handoff_morph: None,
            model_bounds: Default::default(),
            dock_frame: None,
            dock_height_changed: false,
            dock_clearance_correction: 0.0,
            surface_bounds: Default::default(),
            last_target_height: 0.0,
            height_morph: None,
            morph_clock: Instant::now(),
            route_snap_until: None,
            _observe: observe,
            _pickers_observe: pickers_observe,
            _picker_focus: picker_focus,
            _input_events: input_events,
        };
        // Dev knob: pre-stage attachments (drop/paste can't be synthesized on
        // a rig) — `ZERON_ATTACH=/path/a.png[,/path/b.png]`, and
        // `ZERON_ATTACH_PREVIEW=1` boots with the first one's lightbox open.
        if let Ok(spec) = std::env::var("ZERON_ATTACH") {
            let staged: Vec<StagedAttachment> = spec
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .filter_map(|path| {
                    match attachments::stage_file(std::path::Path::new(path.trim())) {
                        Ok(att) => Some(att),
                        Err(err) => {
                            tracing::warn!(%path, error = %err, "ZERON_ATTACH stage failed");
                            None
                        }
                    }
                })
                .collect();
            if std::env::var("ZERON_ATTACH_PREVIEW").is_ok_and(|v| v == "1")
                && let Some(first) = staged.first()
            {
                composer.preview = Some(attachments::PreviewImage::new(
                    first.name.clone(),
                    first.image.clone(),
                ));
                composer.preview_focus_pending = true;
            }
            if !staged.is_empty() {
                composer
                    .attachments
                    .entry(composer.current_key.clone())
                    .or_default()
                    .extend(staged);
            }
        }
        composer
    }

    /// Capture-knob passthrough (`ZERON_OPEN_DIALOG=model`): open the
    /// combined harness/model menu.
    pub fn debug_open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pickers
            .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
    }
}

/// Focus lands on the prompt input (window-level focus fallbacks — e.g. after
/// the focused terminal panel is hidden — route here).
impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}


#[cfg(test)]
mod tests;
