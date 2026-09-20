//! GPUI input handlers and the custom measured text element.

use super::*;

impl EventEmitter<ComposerInputEvent> for ComposerInput {}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self
            .projection
            .normalize_range(self.range_from_utf16(&range_utf16));
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content.get(range)?.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.selected_range = self.projection.normalize_range(self.selected_range.clone());
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let single_line_text;
        let new_text = if self.single_line {
            single_line_text = new_text.replace(['\r', '\n'], " ");
            single_line_text.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.projection.normalize_range(range);
        self.invalidate_mention_tooltip();
        // An IME commit is the tail of a composition whose pre-composition
        // snapshot was already taken (`replace_and_mark_text_in_range`);
        // recording here would pin undo to the half-composed text instead.
        if self.marked_range.is_none() {
            self.record_edit(&range, new_text);
        }
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.refresh_projection();
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.marked_range.take();
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let single_line_text;
        let new_text = if self.single_line {
            single_line_text = new_text.replace(['\r', '\n'], " ");
            single_line_text.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.projection.normalize_range(range);
        self.invalidate_mention_tooltip();
        // First keystroke of a composition: snapshot the text as it stood
        // before any of it existed, so one undo drops the whole composition.
        if self.marked_range.is_none() {
            self.undo_stack.push(self.snapshot());
            if self.undo_stack.len() > UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.last_edit = None;
        }
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.refresh_projection();
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self
            .projection
            .normalize_range(self.range_from_utf16(&range_utf16));
        let start = self.point_for_index(range.start)?;
        let origin = point(
            bounds.left() + start.x - px(self.scroll_left),
            bounds.top() + start.y - px(self.scroll_top),
        );
        Some(Bounds::new(origin, size(px(2.0), self.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_for_mouse_position(point_in_window);
        Some(self.offset_to_utf16(index))
    }
}

/// The custom element: measured auto-grow layout + shaped-line painting.
struct ComposerTextElement {
    input: Entity<ComposerInput>,
    /// Max content height before internal scrolling kicks in.
    max_content_height: f32,
}

pub(super) struct MentionPathTooltip {
    pub(super) path: SharedString,
    /// Stable for one `Waiting → Visible` promotion; a later activation gets
    /// a new key and therefore exactly one fresh fade-in.
    pub(super) activation: u64,
}

impl Render for MentionPathTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        motion::fade_quick(
            ("file-mention-path-tooltip", self.activation),
            div()
                .h(px(MENTION_TOOLTIP_HEIGHT))
                .max_w(px(480.0))
                .flex()
                .items_center()
                .px(px(8.0))
                .rounded(px(5.0))
                .border_1()
                .border_color(theme.border_strong)
                .bg(theme.surface_raised)
                .font_family(theme.font_mono.clone())
                .text_size(px(11.0))
                .text_color(theme.text_muted)
                .child(self.path.clone()),
        )
    }
}

struct ComposerTextPrepaint {
    cursor: Option<PaintQuad>,
    mention_quads: Vec<PaintQuad>,
    mention_hits: Vec<MentionHit>,
    selection_quads: Vec<PaintQuad>,
    /// Completion preview: window-space origin of the end-of-text caret plus
    /// the suffix to paint there (shaped at paint time — it never joins the
    /// content's own layout, so hit-testing and the caret ignore it).
    ghost: Option<(Point<Pixels>, SharedString)>,
}

impl IntoElement for ComposerTextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for ComposerTextElement {
    type RequestLayoutState = ();
    type PrepaintState = ComposerTextPrepaint;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        let input = self.input.clone();
        let text_style = window.text_style();
        let max_content = self.max_content_height;
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                let width = known.width.unwrap_or(match available.width {
                    gpui::AvailableSpace::Definite(width) => width,
                    _ => px(320.0),
                });
                let content_height = input.update(cx, |input, cx| {
                    input.layout_text(width, &text_style, window, cx)
                });
                size(width, px(content_height.min(max_content)))
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        self.input.update(cx, |input, cx| {
            // Intrinsic measurement may try several widths in one layout.
            // Only publish the resolved geometry, once: notifying for each
            // provisional width starts an endless measure/notify loop.
            input.layout_text(bounds.size.width, &text_style, window, cx);
            let layout = (bounds.size.width, input.content_height);
            let layout_changed = input.last_notified_layout != Some(layout);
            input.last_notified_layout = Some(layout);
            let scrolled = input.clamp_scroll(f32::from(bounds.size.height));
            input.last_bounds = Some(bounds);
            if scrolled || layout_changed {
                cx.emit(ComposerInputEvent::ViewportChanged);
            }
        });
        let input = self.input.read(cx);
        let paint_bounds = input.paint_bounds(bounds);
        let scroll = px(input.scroll_top);
        let origin = point(bounds.left() - px(input.scroll_left), bounds.top() - scroll);
        let selection_color = Theme::of(cx).selection;
        let caret_color = Theme::of(cx).caret;
        // The inline-code recipe: chips use the spectrum wash like `code` spans.
        let mention_color = Theme::of(cx).code_wash;

        let mut mention_quads = Vec::new();
        let mut mention_hits = Vec::new();
        for (mention, display) in &input.projection.mentions {
            let target = MentionTooltipTarget {
                range: mention.range.clone(),
                path: SharedString::from(format!(
                    "{}{}",
                    mention.path,
                    if mention.is_dir { "/" } else { "" }
                )),
            };
            for local_bounds in input.bounds_for_display_range(display.clone()) {
                let chip_bounds = Bounds::new(
                    point(
                        origin.x + local_bounds.origin.x,
                        origin.y + local_bounds.origin.y + px(2.0),
                    ),
                    size(local_bounds.size.width, local_bounds.size.height - px(4.0)),
                );
                mention_quads.push(quad(
                    chip_bounds,
                    px(5.0),
                    mention_color,
                    px(0.0),
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
                let above_anchor = chip_bounds.top() - px(MENTION_TOOLTIP_HEIGHT) - px(1.0);
                let anchor_y = if above_anchor >= px(0.0) {
                    above_anchor
                } else {
                    // GPUI positions at anchor + 1px; subtracting one keeps the
                    // below fallback flush so the pointer can enter the popup.
                    chip_bounds.bottom() - px(1.0)
                };
                let visible_bounds = chip_bounds.intersect(&paint_bounds);
                if visible_bounds.size.width == px(0.0) || visible_bounds.size.height == px(0.0) {
                    continue;
                }
                mention_hits.push(MentionHit {
                    target: target.clone(),
                    bounds: visible_bounds,
                    // The fixed-height popup starts at anchor + 1px. Moving
                    // the anchor above the chip therefore yields conventional
                    // above-target placement without cursor tracking.
                    anchor: point(chip_bounds.left(), anchor_y),
                });
            }
        }
        let mut selection_quads = Vec::new();
        let mut cursor = None;
        if input.selected_range.is_empty() || input.display_is_placeholder {
            if let Some(p) = input.point_for_index(input.cursor_offset()) {
                cursor = Some(fill(
                    Bounds::new(
                        point(origin.x + p.x, origin.y + p.y),
                        size(px(2.0), input.line_height),
                    ),
                    caret_color,
                ));
            } else if input.display_is_placeholder {
                cursor = Some(fill(
                    Bounds::new(origin, size(px(2.0), input.line_height)),
                    caret_color,
                ));
            }
        } else if let (Some(start), Some(end)) = (
            input.point_for_index(input.selected_range.start),
            input.point_for_index(input.selected_range.end),
        ) {
            let lh = input.line_height;
            if start.y == end.y {
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(origin.x + end.x, origin.y + start.y + lh),
                    ),
                    selection_color,
                ));
            } else {
                // First visual row, full middle rows, last visual row.
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(bounds.right(), origin.y + start.y + lh),
                    ),
                    selection_color,
                ));
                if end.y > start.y + lh {
                    selection_quads.push(fill(
                        Bounds::from_corners(
                            point(origin.x, origin.y + start.y + lh),
                            point(bounds.right(), origin.y + end.y),
                        ),
                        selection_color,
                    ));
                }
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x, origin.y + end.y),
                        point(origin.x + end.x, origin.y + end.y + lh),
                    ),
                    selection_color,
                ));
            }
        }
        let tooltip = input.visible_mention_tooltip();
        if let Some((_target, anchor, _activation, view)) = tooltip {
            let view = view.into();
            let input = self.input.clone();
            window.set_tooltip(AnyTooltip {
                view,
                mouse_position: anchor,
                check_visible_and_update: Rc::new(move |popup, window, cx| {
                    input.update(cx, |input, _| {
                        input.check_mention_tooltip_visibility(popup, window.mouse_position())
                    })
                }),
            });
        }
        // The ghost only shows where accepting it would insert: a collapsed
        // caret at the end of real (non-placeholder, non-IME) text.
        let ghost = input
            .ghost
            .clone()
            .filter(|g| {
                !g.is_empty()
                    && !input.display_is_placeholder
                    && input.marked_range.is_none()
                    && input.selected_range.is_empty()
                    && input.cursor_offset() == input.content.len()
            })
            .and_then(|g| {
                input
                    .point_for_index(input.content.len())
                    .map(|p| (point(origin.x + p.x, origin.y + p.y), g))
            });
        ComposerTextPrepaint {
            cursor,
            mention_quads,
            mention_hits,
            selection_quads,
            ghost,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        self.input.update(cx, |input, _| {
            input.set_mention_hits(prepaint.mention_hits.clone())
        });
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let input = self.input.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                input.update(cx, |input, cx| input.on_mouse_move(event, cx));
            }
        });

        // WrappedLine isn't Clone — temporarily take the shaped lines out of the
        // entity for painting, then put them back for mouse mapping.
        let (lines, line_height, scroll, scroll_left) = self.input.update(cx, |input, _| {
            (
                std::mem::take(&mut input.last_lines),
                input.line_height,
                input.scroll_top,
                input.scroll_left,
            )
        });

        let paint_bounds = self.input.read(cx).paint_bounds(bounds);
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: paint_bounds,
            }),
            |window| {
                for quad in prepaint.mention_quads.drain(..) {
                    window.paint_quad(quad);
                }
                for quad in prepaint.selection_quads.drain(..) {
                    window.paint_quad(quad);
                }
                let mut y = bounds.top() - px(scroll);
                for line in &lines {
                    let height = line.size(line_height).height;
                    let _ = line.paint(
                        point(bounds.left() - px(scroll_left), y),
                        line_height,
                        gpui::TextAlign::Left,
                        Some(bounds),
                        window,
                        cx,
                    );
                    y += height;
                }
                if let Some((ghost_origin, ghost)) = prepaint.ghost.take() {
                    let style = window.text_style();
                    let font_size = style.font_size.to_pixels(window.rem_size());
                    let run = TextRun {
                        len: ghost.len(),
                        font: style.font(),
                        color: Theme::of(cx).text_faint,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(ghost, font_size, &[run], None);
                    // (Clipping comes from the surrounding content mask.)
                    let _ = line.paint(
                        ghost_origin,
                        line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                // Caret only when this input is actually focused in an active
                // window (Electron hides it on window deactivation too), and only
                // in the "on" blink phase — solid while typing, ~500ms blink idle.
                if self
                    .input
                    .update(cx, |input, cx| input.caret_shown(window, cx))
                    && let Some(cursor) = prepaint.cursor.take()
                {
                    window.paint_quad(cursor);
                }
            },
        );
        self.input.update(cx, |input, _| {
            input.last_lines = lines;
        });
    }
}

impl Render for ComposerInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let popup_theme = theme.for_popup();
        let theme = if self.key_context == "PaletteSearch"
            || self.accessibility_role == gpui::Role::SearchInput
        {
            &popup_theme
        } else {
            theme
        };
        let text_color = if self.content.is_empty() {
            theme.text_faint
        } else {
            theme.text
        };
        div()
            .id(("composer-input", cx.entity_id()))
            .role(self.accessibility_role)
            .aria_label(self.placeholder.clone())
            .aria_placeholder(self.placeholder.clone())
            .key_context(self.key_context)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::mention_tab))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::message_newline_or_accept))
            .on_action(cx.listener(Self::modified_submit))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, window, _| {
                // Capture runs before the clicked control handles the press, so
                // another input can take focus normally during bubbling.
                if event.button == MouseButton::Left && this.focus_handle.is_focused(window) {
                    window.blur();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .w_full()
            .text_size(crate::typography::ui_rems(self.text_size))
            .line_height(crate::typography::ui_rems(self.configured_line_height))
            .text_color(text_color)
            .font_family(theme.font_sans.clone())
            .child({
                let input = cx.entity();
                let ascent = self.max_ascent;
                crate::edge_fade::edge_faded(
                    INPUT_FADE_BAND,
                    true,
                    true,
                    ComposerTextElement {
                        input: input.clone(),
                        max_content_height: self
                            .viewport_height
                            .unwrap_or(TEXTAREA_MAX - TEXTAREA_PAD_V),
                    },
                )
                // Fade through the existing top padding, like the transcript
                // scrolling under its chrome. Account for GPUI's baseline
                // sampling without consuming another inset inside the text box.
                .inset_top(ascent - self.overflow_top_padding)
                .fade_overflow_y_with(move |cx| {
                    let input = input.read(cx);
                    let visible_height = input
                        .last_bounds
                        .map_or(0.0, |bounds| f32::from(bounds.size.height));
                    input_overflow_edges(
                        input.content_height,
                        input
                            .settled_viewport_height
                            .unwrap_or(TEXTAREA_MAX - TEXTAREA_PAD_V),
                        visible_height,
                        input.scroll_top,
                    )
                })
            })
    }
}
