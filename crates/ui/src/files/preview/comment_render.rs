use super::*;

impl FilesSurface {
    pub(super) fn render_editor_comment_overlays(
        &mut self,
        path: &str,
        editor: &Entity<super::super::editor::FileEditorState>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(layout) = editor.read_with(cx, |state, _| editor_overlay_layout(state)) else {
            return Vec::new();
        };
        let comments = self.staged_file_comments(path, cx);
        let comments_by_line = comments
            .iter()
            .map(|comment| (comment.line, comment.clone()))
            .collect::<HashMap<_, _>>();
        let (card_left, card_width) = editor_comment_overlay_horizontal(&layout);
        let mut overlays = Vec::with_capacity(layout.rows.len() + 1);
        for row in &layout.rows {
            let group: SharedString = format!("file-comment-gutter-{}-{}", path, row.line).into();
            let cell = div()
                .id(("file-comment-gutter", row.line as usize))
                .absolute()
                .left(px(0.0))
                .top(px(row.top))
                .w(px(layout.gutter_width))
                .h(px(layout.line_height))
                .flex()
                .items_center()
                .justify_center();
            if let Some(comment) = comments_by_line.get(&row.line) {
                let id = comment.id.clone();
                overlays.push(
                    cell.bg(theme.surface_card)
                        .cursor_pointer()
                        .role(gpui::Role::Button)
                        .aria_label(format!("Open comment on line {}", row.line))
                        .hover(|style| style.bg(crate::theme::wash(0.08)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_editor_comment(id.clone(), cx)
                        }))
                        .child(
                            icon(icons::CHAT_ROUND_LINE)
                                .size(px(10.5))
                                .text_color(theme.text_muted),
                        )
                        .into_any_element(),
                );
            } else {
                let target = path.to_string();
                let line = row.line;
                overlays.push(
                    cell.group(group.clone())
                        .cursor_pointer()
                        .role(gpui::Role::Button)
                        .aria_label(format!("Comment on line {line}"))
                        .hover(|style| style.bg(theme.surface_card))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_editor_comment_draft(target.clone(), line, window, cx)
                        }))
                        .child(
                            div()
                                .size(px(comments::COMMENT_ADDER_SIZE))
                                .opacity(0.0)
                                .group_hover(group, |style| style.opacity(1.0))
                                .rounded(px(4.0))
                                .bg(theme.solid)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(icons::PLUS).size(px(11.0)).text_color(theme.on_solid)),
                        )
                        .into_any_element(),
                );
            }
        }

        let active_comment = self
            .preview
            .active_comment
            .as_deref()
            .and_then(|id| comments.iter().find(|comment| comment.id == id))
            .cloned();
        if let Some(comment) = active_comment
            && let Some(top) = editor_comment_overlay_top(
                &layout,
                comment.line,
                comments::card_height(&comment.body),
            )
        {
            overlays.push(
                self.render_editor_comment_card(comment, card_left, card_width, top, theme, cx),
            );
        } else if let Some(draft) = self
            .preview
            .comment_draft
            .as_ref()
            .filter(|draft| draft.path == path)
            && let Some(top) =
                editor_comment_overlay_top(&layout, draft.line, EDITOR_COMMENT_DRAFT_HEIGHT)
        {
            overlays.push(self.render_editor_comment_draft(
                draft.input.clone(),
                card_left,
                card_width,
                top,
                theme,
                cx,
            ));
        }
        overlays
    }

    fn render_editor_comment_card(
        &self,
        comment: ReviewComment,
        left: f32,
        width: f32,
        top: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let group: SharedString = format!("file-comment-card-{}", comment.id).into();
        let id = comment.id.clone();
        let card = crate::popover::popover_card_flush(theme)
            .group(group.clone())
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(comments::card_height(&comment.body)))
            .flex()
            .flex_col()
            .font_family(theme.font_sans.clone())
            .px(px(Theme::SPACE_LG))
            .py(px(comments::CARD_PAD_V / 2.0))
            .child(
                div()
                    .h(px(comments::CARD_HEADER_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        icon(icons::CHAT_ROUND_LINE)
                            .size(px(12.0))
                            .text_color(theme.text_faint),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(comment.location())),
                    )
                    .child(crate::comment_ui::render_comment_edit(
                        &comment,
                        group.clone(),
                        theme,
                        cx,
                        Self::edit_editor_comment,
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "file-comment-remove-{}",
                                comment.id
                            )))
                            .size(px(16.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover(group, |style| style.opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_editor_comment(&id, cx)
                            }))
                            .child(
                                icon(icons::CLOSE_CIRCLE)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .line_height(px(comments::CARD_LINE_HEIGHT))
                    .text_color(theme.text_dim)
                    .child(SharedString::from(comment.body)),
            );
        crate::frost::frosted(crate::popover::CARD_RADIUS, crate::frost::MENU_BLUR, card)
            .into_any_element()
    }

    fn render_editor_comment_draft(
        &self,
        input: Entity<ComposerInput>,
        left: f32,
        width: f32,
        top: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let card = crate::popover::popover_card_flush(theme)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_editor_comment(cx);
                }
            }))
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(EDITOR_COMMENT_DRAFT_HEIGHT))
            .flex()
            .flex_col()
            .font_family(theme.font_sans.clone())
            .px(px(Theme::SPACE_LG))
            .py(px(8.0))
            .child(
                div()
                    .h(px(48.0))
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.input_glass_bg())
                    .px(px(8.0))
                    .py(px(5.0))
                    .text_size(px(12.0))
                    .child(input.into_any_element()),
            )
            .child(
                div()
                    .h(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.0))
                    .child(
                        editor_comment_action("file-comment-cancel", "Cancel", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_editor_comment(cx))),
                    )
                    .child(
                        editor_comment_action(
                            "file-comment-commit",
                            if self
                                .preview
                                .comment_draft
                                .as_ref()
                                .is_some_and(|draft| draft.editing_id.is_some())
                            {
                                "Save"
                            } else {
                                "Comment"
                            },
                            true,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.commit_editor_comment(cx))),
                    ),
            );
        crate::frost::frosted(crate::popover::CARD_RADIUS, crate::frost::MENU_BLUR, card)
            .into_any_element()
    }
}

fn editor_comment_overlay_top(
    layout: &EditorOverlayLayout,
    line: u32,
    card_height: f32,
) -> Option<f32> {
    let row = layout.rows.iter().find(|row| row.line == line)?;
    let preferred = row.top + layout.line_height;
    Some(preferred.clamp(0.0, (layout.viewport_height - card_height).max(0.0)))
}

fn editor_comment_overlay_horizontal(layout: &EditorOverlayLayout) -> (f32, f32) {
    let anchored_left =
        (layout.gutter_width - EDITOR_COMMENT_CARD_MARGIN).max(EDITOR_COMMENT_CARD_MARGIN);
    let anchored_width = (layout.viewport_width - anchored_left - EDITOR_COMMENT_CARD_MARGIN)
        .min(EDITOR_COMMENT_CARD_WIDTH)
        .max(0.0);
    if anchored_width >= EDITOR_COMMENT_CARD_MIN_ANCHORED_WIDTH {
        (anchored_left, anchored_width)
    } else {
        (
            EDITOR_COMMENT_CARD_MARGIN,
            (layout.viewport_width - EDITOR_COMMENT_CARD_MARGIN * 2.0).max(0.0),
        )
    }
}

fn editor_comment_action(
    id: &'static str,
    label: &'static str,
    primary: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(22.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |element| {
            element.bg(theme.solid).text_color(theme.on_solid)
        })
        .when(!primary, |element| {
            element
                .text_color(crate::motion::hover_blend(id, theme.text_muted, theme.text))
                .bg(crate::motion::hover_blend(
                    id,
                    gpui::transparent_black(),
                    theme.element_hover,
                ))
                .on_hover(crate::motion::hover_listener(id))
        })
        .child(SharedString::from(label))
}
