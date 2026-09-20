//! Input field management, text layout, selection, scrolling, and file mention linking.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, KeyBinding, KeyDownEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, Role, ScrollWheelEvent, SharedString, Task, TextStyle, UTF16Selection, Window,
    WrappedLine, px,
};

use crate::settings::{ComposerSendBehavior, platform_combo};
use crate::theme::Theme;

use super::*;

#[path = "input/mentions.rs"]
mod mentions;
use mentions::*;
pub(crate) use mentions::{
    FILE_MENTION_SCHEME, FileMentionLink, MentionHit, MentionTooltipPhase, MentionTooltipTarget,
    TextProjection, display_row_segments, dropped_file_mention, file_mention_links,
    local_file_link, mention_display_labels, mention_tooltip_contains, mention_tooltip_promote,
    mention_tooltip_reduce,
};
pub use mentions::{SentMentionSpan, sent_mention_display};

#[path = "input/element.rs"]
mod element;
use element::MentionPathTooltip;

// ---------------------------------------------------------------------------
// Scroll & press helpers
// ---------------------------------------------------------------------------

pub(crate) fn input_max_scroll(content_height: f32, viewport_height: f32) -> f32 {
    (content_height - viewport_height).max(0.0)
}

/// Only settled overflow gets a scroll fade. The animated viewport can be
/// smaller for a few frames while an otherwise fitting draft grows into it.
pub(crate) fn input_overflow_edges(
    content_height: f32,
    settled_height: f32,
    visible_height: f32,
    scroll_top: f32,
) -> (bool, bool) {
    if input_max_scroll(content_height, settled_height) <= 1.0 {
        return (false, false);
    }
    let max_scroll = input_max_scroll(content_height, visible_height);
    (scroll_top > 1.0, scroll_top < max_scroll - 1.0)
}

/// During the reveal, stop at a complete row boundary instead of slicing
/// glyphs with a moving clip. Scrolling offsets the row grid inside the box.
pub(crate) fn input_reveal_height(
    visible: f32,
    scroll: f32,
    line_height: f32,
    resizing: bool,
) -> f32 {
    if !resizing {
        return visible;
    }
    let row_end = ((scroll + visible + 0.001) / line_height).floor() * line_height;
    (row_end - scroll).clamp(0.0, visible)
}

/// Apply GPUI's wheel delta to a top-origin input offset. Positive deltas mean
/// scrolling toward the start, matching gpui's built-in list/div behavior.
pub(crate) fn input_scroll_offset(
    current: f32,
    delta_y: f32,
    content_height: f32,
    viewport_height: f32,
) -> f32 {
    (current - delta_y).clamp(0.0, input_max_scroll(content_height, viewport_height))
}

/// Minimally adjust the viewport so the caret row is fully visible.
pub(crate) fn input_scroll_offset_for_cursor(
    current: f32,
    cursor_top: f32,
    cursor_height: f32,
    content_height: f32,
    viewport_height: f32,
    settled_height: Option<f32>,
) -> f32 {
    // Resize the reveal, not the scroll position: existing text stays fixed
    // relative to the input origin throughout the height animation.
    let viewport_height = settled_height.unwrap_or(viewport_height);
    let mut next = current;
    if cursor_top < next {
        next = cursor_top;
    } else if cursor_top + cursor_height > next + viewport_height {
        next = cursor_top + cursor_height - viewport_height;
    }
    next.clamp(0.0, input_max_scroll(content_height, viewport_height))
}

/// What a mouse press in a text field asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PressIntent {
    /// Take the whole field.
    SelectAll,
    /// Grow the current selection to the pressed position.
    ExtendSelection,
    /// Put the caret at the pressed position.
    PlaceCaret,
}

impl PressIntent {
    /// Whether the press starts a drag selection. A select-all must not, or
    /// the next mouse move shrinks it back to a drag from the press position.
    pub(crate) fn arms_drag(self) -> bool {
        !matches!(self, Self::SelectAll)
    }
}

/// Read the intent from the press. Two clicks or more take the whole field,
/// and every further click keeps it, so holding the button through a third
/// click does not change what is selected.
pub(crate) fn press_intent(click_count: usize, shift: bool) -> PressIntent {
    if click_count >= 2 {
        PressIntent::SelectAll
    } else if shift {
        PressIntent::ExtendSelection
    } else {
        PressIntent::PlaceCaret
    }
}

/// Per-frame drag-selection scroll. Distance increases speed, capped at one
/// text row per frame so crossing the input boundary never causes a jump.
pub(crate) fn input_drag_scroll_delta(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
    line_height: f32,
) -> f32 {
    let distance = if pointer_y < viewport_top {
        pointer_y - viewport_top
    } else if pointer_y > viewport_bottom {
        pointer_y - viewport_bottom
    } else {
        return 0.0;
    };
    distance.signum() * (distance.abs() * 0.2).clamp(1.0, line_height)
}

/// How long a run of single-character edits keeps merging into one undo step.
/// A pause longer than this starts a fresh step, so undo rewinds in the
/// bursts the user actually typed rather than one character at a time.
const UNDO_COALESCE: Duration = Duration::from_millis(700);

/// Cap on retained undo steps — a long-lived composer must not grow forever.
const UNDO_LIMIT: usize = 200;

/// Direction of the last edit — a run only merges with edits of its own kind.
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Insert,
    Delete,
}

pub(crate) const GENERIC_COMPOSER_CONTEXT: &str = "Composer";
pub(crate) const MESSAGE_COMPOSER_CONTEXT: &str = "MessageComposer";
pub(crate) const PALETTE_SEARCH_CONTEXT: &str = "PaletteSearch";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageEnterBindingAction {
    Submit,
    ModifiedSubmit,
    NewlineOrAccept,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageEnterBinding {
    pub(crate) keystroke: String,
    pub(crate) action: MessageEnterBindingAction,
}

pub(crate) fn message_enter_bindings(
    behavior: ComposerSendBehavior,
    modifier_combo: &str,
) -> Vec<MessageEnterBinding> {
    match behavior {
        ComposerSendBehavior::Enter => vec![
            MessageEnterBinding {
                keystroke: "enter".into(),
                action: MessageEnterBindingAction::Submit,
            },
            MessageEnterBinding {
                keystroke: modifier_combo.into(),
                action: MessageEnterBindingAction::ModifiedSubmit,
            },
        ],
        ComposerSendBehavior::ModEnter => vec![
            MessageEnterBinding {
                keystroke: "enter".into(),
                action: MessageEnterBindingAction::NewlineOrAccept,
            },
            MessageEnterBinding {
                keystroke: modifier_combo.into(),
                action: MessageEnterBindingAction::ModifiedSubmit,
            },
        ],
    }
}

pub(crate) fn message_input_context(wizard_active: bool) -> &'static str {
    if wizard_active {
        GENERIC_COMPOSER_CONTEXT
    } else {
        MESSAGE_COMPOSER_CONTEXT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnterOutcome {
    AcceptCompletion,
    Submit,
    Newline,
}

pub(crate) fn enter_outcome(has_completion: bool, fallback: EnterOutcome) -> EnterOutcome {
    if has_completion {
        EnterOutcome::AcceptCompletion
    } else {
        fallback
    }
}

fn input_bindings(context: &'static str) -> Vec<KeyBinding> {
    let ctx = Some(context);
    let mut bindings = vec![
        KeyBinding::new("tab", MentionTab, ctx),
        KeyBinding::new("shift-enter", Newline, ctx),
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("shift-home", SelectHome, ctx),
        KeyBinding::new("shift-end", SelectEnd, ctx),
        // macOS line/document motion — a laptop keyboard has no home/end keys,
        // so Cmd+arrow is the only way users reach either edge.
        KeyBinding::new("cmd-left", Home, ctx),
        KeyBinding::new("cmd-right", End, ctx),
        KeyBinding::new("cmd-up", DocStart, ctx),
        KeyBinding::new("cmd-down", DocEnd, ctx),
        KeyBinding::new("shift-cmd-left", SelectHome, ctx),
        KeyBinding::new("shift-cmd-right", SelectEnd, ctx),
        KeyBinding::new("shift-cmd-up", SelectDocStart, ctx),
        KeyBinding::new("shift-cmd-down", SelectDocEnd, ctx),
        // Line-edge deletion (Cmd+Delete on macOS).
        KeyBinding::new("cmd-backspace", DeleteToLineStart, ctx),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, ctx),
    ];
    for prefix in ["cmd", "ctrl"] {
        bindings.push(KeyBinding::new(&format!("{prefix}-z"), Undo, ctx));
        bindings.push(KeyBinding::new(&format!("shift-{prefix}-z"), Redo, ctx));
    }
    // Word-level editing: Option on macOS, Ctrl on Windows/Linux.
    let word_edit_prefix = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-backspace"),
        DeleteWordLeft,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-delete"),
        DeleteWordRight,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-left"),
        WordLeft,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-right"),
        WordRight,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-left"),
        SelectWordLeft,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-right"),
        SelectWordRight,
        ctx,
    ));
    for prefix in ["cmd", "ctrl"] {
        bindings.push(KeyBinding::new(&format!("{prefix}-a"), SelectAll, ctx));
        bindings.push(KeyBinding::new(&format!("{prefix}-c"), Copy, ctx));
        bindings.push(KeyBinding::new(&format!("{prefix}-x"), Cut, ctx));
        bindings.push(KeyBinding::new(&format!("{prefix}-v"), Paste, ctx));
    }
    bindings
}

/// Bind the composer keymap. Call once at app boot.
pub fn init(cx: &mut App, send_behavior: ComposerSendBehavior) {
    let mut generic_bindings = input_bindings(GENERIC_COMPOSER_CONTEXT);
    generic_bindings.push(KeyBinding::new(
        "enter",
        Submit,
        Some(GENERIC_COMPOSER_CONTEXT),
    ));

    let mut message_bindings = input_bindings(MESSAGE_COMPOSER_CONTEXT);
    for binding in message_enter_bindings(send_behavior, &platform_combo("mod-enter")) {
        match binding.action {
            MessageEnterBindingAction::Submit => message_bindings.push(KeyBinding::new(
                &binding.keystroke,
                Submit,
                Some(MESSAGE_COMPOSER_CONTEXT),
            )),
            MessageEnterBindingAction::ModifiedSubmit => message_bindings.push(KeyBinding::new(
                &binding.keystroke,
                ModifiedSubmit,
                Some(MESSAGE_COMPOSER_CONTEXT),
            )),
            MessageEnterBindingAction::NewlineOrAccept => message_bindings.push(KeyBinding::new(
                &binding.keystroke,
                MessageNewlineOrAccept,
                Some(MESSAGE_COMPOSER_CONTEXT),
            )),
        }
    }

    let word_edit_prefix = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    // Palette-search context: TEXT-EDITING keys only. gpui dispatches matched
    // keybindings BEFORE raw key listeners (window.rs `dispatch_key_event`),
    // so anything bound here can never reach a palette's `on_key_down` —
    // navigation keys (up/down/left/right/enter) are deliberately unbound and
    // bubble to the palette frame instead.
    let palette = Some(PALETTE_SEARCH_CONTEXT);
    let mut palette_bindings = vec![
        KeyBinding::new("backspace", Backspace, palette),
        KeyBinding::new("delete", Delete, palette),
        KeyBinding::new("home", Home, palette),
        KeyBinding::new("end", End, palette),
        KeyBinding::new("shift-left", SelectLeft, palette),
        KeyBinding::new("shift-right", SelectRight, palette),
        // Modifier-qualified motion is safe here: the palette's own navigation
        // uses BARE arrows/enter, which stay unbound and bubble to its frame.
        KeyBinding::new("cmd-left", Home, palette),
        KeyBinding::new("cmd-right", End, palette),
        KeyBinding::new("shift-cmd-left", SelectHome, palette),
        KeyBinding::new("shift-cmd-right", SelectEnd, palette),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, palette),
    ];
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-backspace"),
        DeleteWordLeft,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-delete"),
        DeleteWordRight,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-left"),
        WordLeft,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-right"),
        WordRight,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-left"),
        SelectWordLeft,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-right"),
        SelectWordRight,
        palette,
    ));
    for prefix in ["cmd", "ctrl"] {
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-a"), SelectAll, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-c"), Copy, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-x"), Cut, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-v"), Paste, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-z"), Undo, palette));
        palette_bindings.push(KeyBinding::new(&format!("shift-{prefix}-z"), Redo, palette));
    }
    cx.bind_keys(palette_bindings);
    cx.bind_keys(generic_bindings);
    cx.bind_keys(message_bindings);
}

/// Events the composer wrapper listens for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerInputEvent {
    Submitted,
    ModifiedSubmitted,
    Edited,
    CursorMoved,
    ViewportChanged,
    MentionNavigate(isize),
    MentionAccept,
    MentionDismiss,
    /// Images pasted from the clipboard (screenshots / copied image data) —
    /// the wrapper stages them as attachments (use-attachments.ts onPaste).
    PastedImages(Vec<gpui::Image>),
    /// File paths pasted from the clipboard (a file manager "Copy").
    PastedPaths(Vec<PathBuf>),
}

/// Shaping inputs excluding mutable viewport and selection geometry.
#[derive(Clone, PartialEq)]
struct InputLayoutKey {
    width: Pixels,
    font: gpui::Font,
    font_size: Pixels,
    color: gpui::Hsla,
    chip_family: SharedString,
    chip_color: gpui::Hsla,
    pub(crate) marked_range: Option<Range<usize>>,
    pub(crate) placeholder: SharedString,
    mentions_enabled: bool,
}

/// Multiline input entity: content + selection + IME marked text + measured
/// layout (wrapped lines) for mouse mapping and auto-grow.
pub struct ComposerInput {
    /// Key context for the binding map ("Composer", or "PaletteSearch" for
    /// palette filters whose navigation keys must bubble).
    key_context: &'static str,
    accessibility_role: Role,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) content: String,
    pub(crate) read_only: bool,
    pub(crate) placeholder: SharedString,
    pub(crate) selected_range: Range<usize>,
    selection_reversed: bool,
    pub(crate) marked_range: Option<Range<usize>>,
    is_selecting: bool,
    drag_position: Option<Point<Pixels>>,
    drag_generation: u64,
    drag_autoscroll_active: bool,
    /// Vertical scroll inside the input once content exceeds the max height.
    pub(crate) scroll_top: f32,
    /// Visible content budget supplied by the animated composer.
    pub(crate) viewport_height: Option<f32>,
    /// Final content budget, excluding temporary overflow during a resize.
    pub(crate) settled_viewport_height: Option<f32>,
    pub(crate) resizing: bool,
    pub(crate) overflow_top_padding: f32,
    pub(crate) needs_measure: bool,
    last_layout_key: Option<InputLayoutKey>,
    pub(crate) last_notified_layout: Option<(Pixels, f32)>,
    max_ascent: f32,
    #[cfg(test)]
    pub(crate) layout_rebuilds: usize,
    /// Normally keeps the caret visible through edits and rewraps. Manual
    /// wheel scrolling pauses it until the next caret move or edit.
    follow_cursor: bool,
    text_size: f32,
    configured_line_height: f32,
    single_line: bool,
    pub(crate) scroll_left: f32,
    // -- measured state (written during layout/paint) --
    pub(crate) last_lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    pub(crate) last_bounds: Option<Bounds<Pixels>>,
    pub(crate) line_height: Pixels,
    pub(crate) content_height: f32,
    max_line_width: f32,
    pub(crate) last_width: f32,
    /// Raw Markdown → chip display projection from the last layout pass.
    projection: TextProjection,
    /// Inline completion preview: painted in faint ink after the text while
    /// the caret sits at the end (palette tab-completion). Owned by the
    /// wrapper — it recomputes and re-sets this on every render pass, so the
    /// input never has to know what the completion means.
    ghost: Option<SharedString>,
    /// File mentions are a composer feature, not a behavior of generic inputs
    /// (picker searches and rename fields also use this type).
    mentions_enabled: bool,
    /// Bumped once per `layout_text` pass — the flip logic uses it to apply at
    /// most one compact↔expanded flip per layout (a flip is only re-evaluated
    /// after the input has been measured in the new mode).
    pub(crate) layout_epoch: u64,
    pub(crate) display_is_placeholder: bool,
    /// Caret blink anchor: reset on every keystroke/caret move so the caret is
    /// solid while typing and blinks at [`CARET_BLINK_MS`] when idle.
    blink_anchor: Instant,
    /// Half-period repaint driver, alive only while the input is focused.
    blink_task: Option<Task<()>>,
    // -- undo history --
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    /// Kind, trailing offset, and time of the last edit — the merge test that
    /// decides whether the next edit extends the current undo step.
    last_edit: Option<(EditKind, usize, Instant)>,
    /// The wrapper owns mention state; this only redirects bound keys while a
    /// mention token is active, keeping input focus and native text editing.
    mention_open: bool,
    pub(crate) mention_has_selection: bool,
    /// Last prepainted chip bounds; the paint-phase pointer listener uses
    /// these instead of attempting to infer text geometry from the cursor.
    mention_hits: Vec<MentionHit>,
    mention_tooltip: MentionTooltipPhase,
    mention_tooltip_generation: u64,
    mention_tooltip_popup: Option<Bounds<Pixels>>,
    mention_tooltip_task: Option<Task<()>>,
    /// Created once when Waiting promotes; retaining this entity preserves
    /// GPUI's global animation state across prepaint frames.
    mention_tooltip_view: Option<Entity<MentionPathTooltip>>,
}

impl ComposerInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self::with_context(placeholder, GENERIC_COMPOSER_CONTEXT, cx)
    }

    /// An input in a custom KEY context — palettes use `"PaletteSearch"`,
    /// whose keymap binds only text-editing keys so navigation keys bubble to
    /// the surrounding frame (see `init`).
    pub fn with_context(
        placeholder: impl Into<SharedString>,
        key_context: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            key_context,
            accessibility_role: Role::MultilineTextInput,
            focus_handle: cx.focus_handle(),
            content: String::new(),
            read_only: false,
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            is_selecting: false,
            drag_position: None,
            drag_generation: 0,
            drag_autoscroll_active: false,
            scroll_top: 0.0,
            viewport_height: None,
            settled_viewport_height: None,
            resizing: false,
            overflow_top_padding: 0.0,
            needs_measure: true,
            last_layout_key: None,
            last_notified_layout: None,
            max_ascent: INPUT_TEXT_SIZE,
            #[cfg(test)]
            layout_rebuilds: 0,
            follow_cursor: true,
            text_size: INPUT_TEXT_SIZE,
            configured_line_height: INPUT_LINE_HEIGHT,
            single_line: false,
            scroll_left: 0.0,
            last_lines: Vec::new(),
            line_starts: vec![0],
            last_bounds: None,
            line_height: px(INPUT_LINE_HEIGHT),
            content_height: INPUT_LINE_HEIGHT,
            max_line_width: 0.0,
            last_width: 0.0,
            projection: TextProjection::default(),
            ghost: None,
            mentions_enabled: false,
            layout_epoch: 0,
            display_is_placeholder: true,
            blink_anchor: Instant::now(),
            blink_task: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: None,
            mention_open: false,
            mention_has_selection: false,
            mention_hits: Vec::new(),
            mention_tooltip: MentionTooltipPhase::Hidden,
            mention_tooltip_generation: 0,
            mention_tooltip_popup: None,
            mention_tooltip_task: None,
            mention_tooltip_view: None,
        }
    }

    /// Override the text metrics for compact one-line surfaces such as
    /// toolbar searches without changing the main composer typography.
    pub fn with_text_metrics(mut self, text_size: f32, line_height: f32) -> Self {
        self.text_size = text_size;
        self.configured_line_height = line_height;
        self.line_height = px(line_height);
        self.content_height = line_height;
        self
    }

    /// Keep compact fields on one row and reveal the caret horizontally.
    pub fn with_single_line(mut self) -> Self {
        self.single_line = true;
        self
    }

    pub(crate) fn set_key_context(&mut self, key_context: &'static str, cx: &mut Context<Self>) {
        if self.key_context != key_context {
            self.key_context = key_context;
            cx.notify();
        }
    }

    pub fn with_accessibility_role(mut self, role: Role) -> Self {
        self.accessibility_role = role;
        self
    }

    /// Reset the caret blink phase (solid again) — called on every edit and
    /// caret move, matching textarea behavior.
    pub(crate) fn reset_blink(&mut self) {
        self.blink_anchor = Instant::now();
    }

    /// Caret paint gate: focused input in an active window, in the "on" blink
    /// phase. Also (re)arms the half-period repaint driver while focused, and
    /// drops it on blur so an unfocused input schedules no frames.
    pub(crate) fn caret_shown(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let focused = self.focus_handle.is_focused(window);
        if !focused || !window.is_window_active() {
            self.blink_task = None;
            return false;
        }
        if self.blink_task.is_none() {
            self.blink_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(CARET_BLINK_MS))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
        caret_visible(self.blink_anchor.elapsed().as_millis() as u64)
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_mention_controls(
        &mut self,
        open: bool,
        has_selection: bool,
        cx: &mut Context<Self>,
    ) {
        if self.mention_open == open && self.mention_has_selection == has_selection {
            return;
        }
        self.mention_open = open;
        self.mention_has_selection = has_selection;
        cx.notify();
    }

    pub(crate) fn enable_mentions(&mut self) {
        self.mentions_enabled = true;
        self.refresh_projection();
    }

    fn refresh_projection(&mut self) {
        self.projection = if self.mentions_enabled {
            TextProjection::new(&self.content)
        } else {
            TextProjection {
                display: self.content.clone(),
                mentions: Vec::new(),
            }
        };
    }

    /// Replace a completed `@query` token as one non-coalescing undo step.
    pub fn replace_mention(
        &mut self,
        range: Range<usize>,
        path: &str,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.invalidate_mention_tooltip();
        let path = local_file_link(path, is_dir);
        let next = self.content[range.end..].chars().next();
        let existing_separator = next.filter(|ch| ch.is_whitespace() && *ch != '\n' && *ch != '\r');
        let inserted = if existing_separator.is_some() {
            path
        } else {
            format!("{path} ")
        };
        self.record_edit(&range, &inserted);
        self.content =
            self.content[..range.start].to_owned() + &inserted + &self.content[range.end..];
        self.refresh_projection();
        let cursor =
            range.start + inserted.len() + existing_separator.map(char::len_utf8).unwrap_or(0);
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    /// Insert a workspace reference at the current selection. Drag-and-drop
    /// uses the same strict local Markdown transport and projected chip as an
    /// `@` mention selected from completion.
    pub(crate) fn insert_dropped_mention(
        &mut self,
        path: &str,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let range = self.selected_range.clone();
        let Some((inserted, cursor_advance)) =
            dropped_file_mention(&self.content, range.clone(), path, is_dir)
        else {
            return false;
        };
        self.invalidate_mention_tooltip();
        self.record_edit(&range, &inserted);
        self.content =
            self.content[..range.start].to_owned() + &inserted + &self.content[range.end..];
        self.refresh_projection();
        let cursor = range.start + cursor_advance;
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.follow_cursor = true;
        self.reset_blink();
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
        true
    }

    /// Replace a completed plain-text token (slash commands) as one
    /// non-coalescing undo step. Unlike [`Self::replace_mention`], the
    /// replacement is ordinary text — no link, no chip projection.
    pub fn replace_plain_token(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let next = self.content[range.end..].chars().next();
        let existing_separator = next.filter(|ch| ch.is_whitespace() && *ch != '\n' && *ch != '\r');
        let inserted = if existing_separator.is_some() {
            replacement.to_owned()
        } else {
            format!("{replacement} ")
        };
        self.record_edit(&range, &inserted);
        self.content =
            self.content[..range.start].to_owned() + &inserted + &self.content[range.end..];
        self.refresh_projection();
        let cursor =
            range.start + inserted.len() + existing_separator.map(char::len_utf8).unwrap_or(0);
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Set (or clear) the inline completion preview. Only paints while the
    /// caret sits at the end of a non-empty draft — see the prepaint gate.
    pub fn set_ghost(&mut self, ghost: Option<SharedString>, cx: &mut Context<Self>) {
        if self.ghost == ghost {
            return;
        }
        self.ghost = ghost;
        cx.notify();
    }

    pub fn has_newline(&self) -> bool {
        self.content.contains('\n')
    }

    /// Unwrapped width of the widest line — feeds the compact/expanded flip.
    pub fn measured_text_width(&self) -> f32 {
        self.max_line_width
    }

    pub fn measured_content_height(&self) -> f32 {
        self.content_height
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.invalidate_mention_tooltip();
        self.content = text.into();
        if self.single_line {
            self.content = self.content.replace(['\r', '\n'], " ");
        }
        self.refresh_projection();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_top = 0.0;
        self.scroll_left = 0.0;
        self.follow_cursor = true;
        // Programmatic replacement (draft load, clear-on-submit) is a new
        // document, not an edit — undo must not reach back past it.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn invalidate_mention_tooltip(&mut self) {
        self.mention_tooltip_generation = self.mention_tooltip_generation.wrapping_add(1);
        self.mention_tooltip = MentionTooltipPhase::Hidden;
        self.mention_tooltip_popup = None;
        self.mention_tooltip_task = None;
        self.mention_tooltip_view = None;
    }

    pub(crate) fn set_mention_hits(&mut self, hits: Vec<MentionHit>) {
        self.mention_hits = hits;
        let live = self
            .mention_tooltip
            .target()
            .is_none_or(|target| self.mention_hits.iter().any(|hit| &hit.target == target));
        if !live {
            self.invalidate_mention_tooltip();
        }
    }

    fn start_mention_tooltip_wait(&mut self, target: MentionTooltipTarget, cx: &mut Context<Self>) {
        self.mention_tooltip_generation = self.mention_tooltip_generation.wrapping_add(1);
        let generation = self.mention_tooltip_generation;
        self.mention_tooltip = MentionTooltipPhase::Waiting { target, generation };
        self.mention_tooltip_popup = None;
        self.mention_tooltip_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENTION_TOOLTIP_DELAY).await;
            this.update(cx, |input, cx| {
                let live = input.mention_tooltip.target().is_some_and(|target| {
                    input.mention_hits.iter().any(|hit| &hit.target == target)
                });
                let next = mention_tooltip_promote(input.mention_tooltip.clone(), generation, live);
                if next != input.mention_tooltip {
                    input.mention_tooltip = next;
                    input.mention_tooltip_task = None;
                    if let MentionTooltipPhase::Visible { target, generation } =
                        &input.mention_tooltip
                    {
                        input.mention_tooltip_view = Some(cx.new(|_| MentionPathTooltip {
                            path: target.path.clone(),
                            activation: *generation,
                        }));
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn on_mention_pointer_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.invalidate_mention_tooltip();
            return;
        }
        let target = self
            .mention_hits
            .iter()
            .find(|hit| hit.bounds.contains(&position))
            .map(|hit| hit.target.clone());
        let in_popup = self
            .mention_tooltip_popup
            .is_some_and(|popup| popup.contains(&position));
        let next_generation = self.mention_tooltip_generation.wrapping_add(1);
        let next = mention_tooltip_reduce(
            self.mention_tooltip.clone(),
            target.clone(),
            in_popup,
            next_generation,
        );
        if next == self.mention_tooltip {
            return;
        }
        match next {
            MentionTooltipPhase::Waiting { target, .. } => {
                self.start_mention_tooltip_wait(target, cx)
            }
            _ => {
                self.invalidate_mention_tooltip();
                self.mention_tooltip = next;
                cx.notify();
            }
        }
    }

    fn visible_mention_tooltip(
        &self,
    ) -> Option<(
        MentionTooltipTarget,
        Point<Pixels>,
        u64,
        Entity<MentionPathTooltip>,
    )> {
        let MentionTooltipPhase::Visible { target, generation } = &self.mention_tooltip else {
            return None;
        };
        self.mention_hits
            .iter()
            .find(|hit| hit.target == *target)
            .and_then(|hit| {
                let view = self.mention_tooltip_view.clone()?;
                Some((target.clone(), hit.anchor, *generation, view))
            })
    }

    fn check_mention_tooltip_visibility(
        &mut self,
        popup: Bounds<Pixels>,
        pointer: Point<Pixels>,
    ) -> bool {
        let Some((target, _, _, _)) = self.visible_mention_tooltip() else {
            return false;
        };
        let in_chip = self
            .mention_hits
            .iter()
            .any(|hit| hit.target == target && hit.bounds.contains(&pointer));
        if mention_tooltip_contains(in_chip, popup.contains(&pointer)) {
            self.mention_tooltip_popup = Some(popup);
            true
        } else {
            self.invalidate_mention_tooltip();
            false
        }
    }
}

#[path = "input/editing.rs"]
mod editing;
#[path = "input/geometry.rs"]
mod geometry;
