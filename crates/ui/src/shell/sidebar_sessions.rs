//! Sidebar chat sessions list, session row rendering, and resort FLIP motion.

use super::*;

/// Flex gap between sidebar list items.
pub(crate) const SIDEBAR_LIST_GAP: f32 = 2.0;
/// Fixed vertical slot occupied by one active sidebar card.
#[cfg(test)]
pub(crate) const SIDEBAR_SESSION_SLOT: f32 = 61.0 + SIDEBAR_LIST_GAP;
pub(crate) const SIDEBAR_DRAG_SCROLL_BAND: f32 = 48.0;
pub(crate) const SIDEBAR_DRAG_SCROLL_MAX: f32 = 12.0;
pub(crate) const SIDEBAR_DRAG_SCROLL_FRAME_MS: u64 = 16;
pub(crate) const SIDEBAR_LIST_PAD_TOP: f32 = 4.0;

/// Sidebar resort glide (feature-inventory §1.6): 260ms
/// `cubic-bezier(0.22,1,0.36,1)` per-row translate, the View Transitions
/// equivalent.
pub const RESORT: MotionSpec = MotionSpec::new(260, motion::EASE_RESORT);

/// Active and archived sessions share harness/title geometry.
pub(crate) const SIDEBAR_ACTIVE_HARNESS_ICON_SIZE: f32 = 13.0;
pub(crate) const SIDEBAR_ACTIVE_HARNESS_TITLE_GAP: f32 = Theme::SPACE_SM;

/// FLIP diff for a keyed list: given the previously rendered order and the new
/// order (key + row height), return each surviving key's paint-only start
/// offset old_y - new_y (only keys whose position actually moved). gap is
/// the flex gap between rows. Pure — drives the sidebar resort glide.
pub fn resort_offsets(
    old: &[(String, f32)],
    new: &[(String, f32)],
    gap: f32,
) -> std::collections::HashMap<String, f32> {
    let mut old_y = std::collections::HashMap::new();
    let mut y = 0.0_f32;
    for (key, height) in old {
        old_y.insert(key.as_str(), y);
        y += height + gap;
    }
    let mut offsets = std::collections::HashMap::new();
    let mut y = 0.0_f32;
    for (key, height) in new {
        if let Some(prev) = old_y.get(key.as_str()) {
            let dy = prev - y;
            if dy.abs() > 0.5 {
                offsets.insert(key.clone(), dy);
            }
        }
        y += height + gap;
    }
    offsets
}

/// Height changes do not constitute a list reorder. In particular, sidebar
/// disclosures animate their own height and must not also trigger FLIP offsets
/// on every following keyed section.
pub(crate) fn sidebar_key_order_changed(old: &[(String, f32)], new: &[(String, f32)]) -> bool {
    old.len() != new.len()
        || old
            .iter()
            .zip(new)
            .any(|((old_key, _), (new_key, _))| old_key != new_key)
}

/// Exact active-session row height. Harness identity lives on the title line
/// and the Working glyph lives in the status corner, so neither adds a third
/// line. Compact rows omit the metadata line and its preceding gap entirely;
/// branch / pull-request rows add the exact height of their tallest child.
/// Keeping this calculation beside the renderer's metrics prevents disclosure
/// clips when view options alter the row structure.
pub(crate) fn chat_row_height(shows_branch: bool, shows_pull_request: bool) -> f32 {
    let mut metadata_height: f32 = 0.0;
    if shows_branch {
        metadata_height = metadata_height.max(14.0);
    }
    if shows_pull_request {
        metadata_height = metadata_height.max(16.0);
    }
    if metadata_height == 0.0 {
        45.0
    } else {
        47.0 + metadata_height
    }
}

pub(crate) fn sidebar_row_height(compact: bool, show_label: bool, branch: bool, pr: bool) -> f32 {
    if compact {
        29.0
    } else {
        chat_row_height(branch, pr) - if show_label { 0.0 } else { 16.0 }
    }
}

/// Keep the fade short so only the last few glyphs recede. Tracking clipped
/// content lets the shared paint-time overflow gate leave fitting labels intact.
pub(crate) fn sidebar_faded_label(id: SharedString, fill: bool, label: impl IntoElement) -> impl IntoElement {
    let overflow = gpui::ScrollHandle::new();
    crate::edge_fade::edge_faded(
        20.0,
        false,
        false,
        div()
            .id(id.clone())
            .debug_selector(move || id.to_string())
            .when(fill, |el| el.flex_1())
            .min_w_0()
            .overflow_hidden()
            .track_scroll(&overflow)
            .flex()
            .child(div().flex_none().whitespace_nowrap().child(label)),
    )
    .fade_right(true)
    .fade_label_overflow(&overflow)
}

impl Shell {
    /// One session row: context + status on line one, harness + title on line
    /// two, and source metadata below. Working uses the live thread glyph in
    /// the status corner. Click selects; right-click opens the context menu.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_chat_row(
        &self,
        id: String,
        title: SharedString,
        time_ago: SharedString,
        space_name: SharedString,
        branch: Option<SharedString>,
        change_request: Option<zeron_proto::ChangeRequestSummary>,
        harness: Option<zeron_proto::HarnessId>,
        status: zeron_proto::ChatIndicator,
        selected: bool,
        archived: bool,
        preview: bool,
        drag: Option<SidebarSessionDrag>,
        // This row's jump combo while the hint overlay is up. It takes the
        // corner outright — above hover and above the status word — so all
        // nine chips appear together instead of leaving a hole on whichever
        // row is busy or under the pointer.
        jump_label: Option<SharedString>,
        search_query: Option<&str>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Activity, not position (t3code Sidebar): status is a small colored
        // word + glyph in the row's top-right corner — Working animates the
        // composer-strip spinner, Done wears a check; Idle rows show the
        // relative time instead. Hovering the ROW swaps the corner for the
        // ARCHIVE button. Compact rows keep status first and elapsed time last;
        // their archive control occupies the remote-icon slot on hover.
        // A chat can appear on both surfaces at once. Namespace every hover
        // key and child id so the palette never animates the sidebar copy.
        let row_id = if search_query.is_some() {
            format!("palette-chat-{id}")
        } else {
            format!("chat-{id}")
        };
        let compact = search_query.is_none() && self.settings.sidebar_compact;
        let show_label = search_query.is_some() || self.settings.sidebar_show_project_label;
        let remote = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == id)
            .is_some_and(|chat| {
                self.state.read(cx).local_device_id.as_deref() != Some(chat.device_id.as_str())
            });
        let project_icon = (search_query.is_none() && self.settings.sidebar_show_project_icon)
            .then(|| self.render_project_icon(&id, SIDEBAR_ACTIVE_HARNESS_ICON_SIZE, selected, cx));
        let corner_hovered = !preview && self.chat_status_hover.as_deref() == Some(row_id.as_str());
        let archived_muted = archived && search_query.is_none() && !selected && !corner_hovered;
        let project_icon = project_icon.map(|icon| {
            div()
                .flex_none()
                .opacity(if archived_muted { 0.4 } else { 1.0 })
                .child(icon)
                .into_any_element()
        });
        let content_id = id.clone();
        // Send-truth overrides: a send unadopted past the grace window is
        // FAILED (explicit, with the transcript's retry affordance); a send
        // whose delivery path is degraded is QUEUED, not Working — the
        // pending pill tells the truth instead of faking a spinner.
        let (queued, undelivered) = {
            let now = Utc::now();
            let state = self.state.read(cx);
            (
                state.send_queued(&id, now),
                state.send_undelivered(&id, now),
            )
        };
        let status_color = if undelivered {
            theme.danger
        } else if queued {
            theme.warning
        } else {
            spaces::status_dot_color(status, theme)
        };
        let status_label: Option<&'static str> = if undelivered {
            Some("Failed")
        } else if queued {
            Some("Queued")
        } else {
            match status {
                zeron_proto::ChatIndicator::Working => Some("Working"),
                zeron_proto::ChatIndicator::AwaitingInput => Some("Input"),
                zeron_proto::ChatIndicator::Errored => Some("Failed"),
                zeron_proto::ChatIndicator::Completed => Some("Done"),
                zeron_proto::ChatIndicator::Idle => None,
            }
        };
        let shows_metadata = branch.is_some() || change_request.is_some();
        let queued = queued && !undelivered;
        let working = status == zeron_proto::ChatIndicator::Working && !queued && !undelivered;
        let compact_status = compact.then(|| {
            let glyph = if working {
                loaders::mini_glyph_spinner(
                    format!("{row_id}-working"),
                    2.0,
                    theme.glyph,
                    self.sidebar_pane.entity_id(),
                    cx,
                )
                .into_any_element()
            } else if status == zeron_proto::ChatIndicator::Completed && !queued && !undelivered {
                icon(icons::CHECK)
                    .size(px(11.0))
                    .text_color(status_color)
                    .into_any_element()
            } else {
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(status_color)
                    .into_any_element()
            };
            div()
                .id(SharedString::from(format!("{row_id}-status")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("chat-status-{id}")
                })
                .size(px(13.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .aria_label(status_label.unwrap_or("Idle"))
                .child(glyph)
                .into_any_element()
        });
        let compact_jump_label = compact.then(|| jump_label.clone()).flatten();
        let corner_body: AnyElement = if let Some(label) = jump_label.filter(|_| !compact) {
            // The jump hint replaces the status/time corner while the modifier
            // is held, cut to the sidebar PR badge's exact cloth
            // (`pull_request_badge`, Sidebar surface): pinned 16px, px 4,
            // rounded 4, borderless 0.08-fill with 0.85 text of one tone —
            // neutral here — and the label in the badge's mono at 10 MEDIUM.
            // Any other geometry reads as a second badge system on the row.
            {
                let tone = theme.text_muted;
                div()
                    .h(px(16.0))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(4.0))
                    .rounded(px(4.0))
                    .bg(tone.opacity(0.08))
                    .text_size(crate::typography::ui_rems(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(tone.opacity(0.85))
                    .font_family(theme.font_mono.clone())
                    .child(label)
                    .into_any_element()
            }
        } else if corner_hovered {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .h(px(18.0))
                .when(!compact, |el| {
                    el.px(px(4.0))
                        .mr(px(-4.0))
                        .rounded(px(5.0))
                        .bg(crate::theme::wash(0.10))
                        .hover(|s| s.bg(crate::theme::wash(0.18)))
                })
                .child(
                    icon(if archived {
                        icons::ARCHIVE_UP_MINIMALISTIC
                    } else {
                        icons::ARCHIVE_MINIMALISTIC
                    })
                    .size(px(if compact {
                        SIDEBAR_ACTIVE_HARNESS_ICON_SIZE
                    } else {
                        11.0
                    }))
                    .flex_none()
                    .text_color(theme.text_muted),
                )
                .when(!compact, |el| {
                    el.child(
                        div()
                            .text_size(crate::typography::ui_rems(10.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(if archived {
                                "Unarchive"
                            } else {
                                "Archive"
                            })),
                    )
                })
                .into_any_element()
        } else if compact {
            if remote {
                icon(icons::REMOTE_SERVER)
                    .size(px(SIDEBAR_ACTIVE_HARNESS_ICON_SIZE))
                    .text_color(theme.text_muted.opacity(0.5))
                    .into_any_element()
            } else {
                div().into_any_element()
            }
        } else {
            match status_label {
                Some(label) => {
                    // Glyph slot: Working wears the preset's animated pixel
                    // glyph beside its label, Done wears the check, and the
                    // remaining statuses use a compact dot.
                    let glyph: AnyElement = if status == zeron_proto::ChatIndicator::Completed {
                        icon(icons::CHECK)
                            .size(px(11.0))
                            .flex_none()
                            .text_color(status_color)
                            .into_any_element()
                    } else if working {
                        loaders::mini_glyph_spinner(
                            format!("{row_id}-working"),
                            2.0,
                            theme.glyph,
                            self.sidebar_pane.entity_id(),
                            cx,
                        )
                        .into_any_element()
                    } else {
                        div()
                            .size(px(6.0))
                            .flex_none()
                            .rounded_full()
                            .bg(status_color)
                            .into_any_element()
                    };
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(glyph)
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(10.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(status_color)
                                .child(SharedString::from(label)),
                        )
                        .into_any_element()
                }
                None => div()
                    .text_size(crate::typography::ui_rems(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(time_ago.clone())
                    .into_any_element(),
            }
        };
        // One stable wrapper across both states (identity keeps the hover
        // from flickering as the content swaps); the swap is driven by the
        // ROW's hover (user request — corner-only felt undiscoverable), but
        // archiving only clicks on the corner itself, so the row's own click
        // stays the selector.
        let corner: AnyElement = {
            let archive_id = id.clone();
            div()
                .id(SharedString::from(format!("{row_id}-corner")))
                .aria_label(if corner_hovered {
                    if archived { "Unarchive" } else { "Archive" }
                } else {
                    if compact {
                        if remote {
                            "Remote session"
                        } else {
                            "Session actions"
                        }
                    } else {
                        status_label.unwrap_or("Idle")
                    }
                })
                .when(compact, |el| el.w(px(18.0)).justify_center())
                .flex_none()
                // Pin the corner to line 1's text height so the archive pill
                // (taller, padded) overflows vertically instead of growing the
                // row — the swap must not shift the card's content.
                // NO occlude: the ROW's hover drives the swap, and an
                // occluding corner un-hovered the row underneath it —
                // pill mounts, steals the pointer, row un-hovers, pill
                // unmounts, repeat (user-reported flicker). The pill's
                // stop_propagation click is separation enough.
                .h(px(14.0))
                .flex()
                .items_center()
                .when(!preview, |el| el.cursor_pointer())
                .when(corner_hovered, |el| {
                    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_chat_archived(archive_id.clone(), !archived, cx);
                        }))
                })
                .child(corner_body)
                .into_any_element()
        };
        let mut corner = Some(corner);
        let (hover, text) = (theme.glass_hover(), theme.text);
        let selected_wash = crate::theme::glass_selected_bg();
        let subline = if search_query.is_some() {
            theme.text_muted
        } else {
            theme.text_muted.opacity(0.5)
        };
        let select_id = id.clone();
        let menu_id = id.clone();
        // Hover fades over transition-colors (zeron session-row.tsx) — both
        // the wash and the title brighten ride the same 150ms blend.
        let fade_key = format!("{row_id}-hover");
        let rest_bg = if selected {
            selected_wash
        } else {
            crate::theme::wash(0.0)
        };
        // A selected row must NOT drift toward the hover wash: in dark the two
        // fills are identical so the blend is a no-op, but light's hover sits
        // below its near-opaque selected fill, and blending toward it visibly
        // dimmed the active row under the pointer (user report).
        let hover_bg = if selected { selected_wash } else { hover };
        let rest_text = if selected || search_query.is_some() {
            text
        } else if archived {
            text.opacity(0.55)
        } else {
            text.opacity(0.8)
        };
        div()
            .id(SharedString::from(row_id.clone()))
            .group("sidebar-session-row")
            .debug_selector({
                let row_id = row_id.clone();
                move || row_id.clone()
            })
            .h(px(sidebar_row_height(
                compact,
                show_label,
                branch.is_some(),
                change_request.is_some(),
            )))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .rounded(px(if search_query.is_some() {
                popover::PALETTE_ITEM_RADIUS
            } else {
                8.0
            }))
            .px(px(Theme::SPACE_SM))
            .py(px(6.0))
            .text_color(motion::hover_blend(&fade_key, rest_text, text))
            .bg(motion::hover_blend(&fade_key, rest_bg, hover_bg))
            // No selection ring (user request) — the wash alone marks the
            // active row.
            // Row hover drives BOTH the wash blend and the corner's
            // status→Archive swap (one listener — gpui allows a single
            // hover listener per element).
            .when(!preview, |el| {
                el.on_hover({
                    let fade_hover = motion::hover_listener(fade_key.clone());
                    let hover_id = row_id.clone();
                    cx.listener(move |this, hovered: &bool, window, cx| {
                        fade_hover(hovered, window, cx);
                        if *hovered {
                            if this.chat_status_hover.as_deref() != Some(hover_id.as_str()) {
                                this.chat_status_hover = Some(hover_id.clone());
                                cx.notify();
                            }
                        } else if this.chat_status_hover.as_deref() == Some(hover_id.as_str()) {
                            this.chat_status_hover = None;
                            cx.notify();
                        }
                    })
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_chat(select_id.clone(), cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.chat_menu.open(ChatMenuState {
                            chat_id: menu_id.clone(),
                            position: event.position,
                            page: ChatMenuPage::Root,
                        });
                        cx.notify();
                    }),
                )
            })
            .when_some(drag, |el, payload| {
                let shell = cx.entity();
                el.on_drag(payload, move |payload, point, window, cx| {
                    shell.update(cx, |shell, cx| {
                        shell.begin_sidebar_session_transfer(payload, point, window, cx);
                    });
                    cx.stop_propagation();
                    cx.new(|_| DragGhost)
                })
            })
            // Line 1: "project @ device", status word / time-ago right.
            .when(!compact && show_label, |el| {
                el.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(Theme::SPACE_SM))
                        .child(sidebar_faded_label(
                            format!("chat-device-{content_id}").into(),
                            true,
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .line_height(px(14.0))
                                .text_color(subline)
                                .child(popover::search_highlight(space_name, search_query, theme)),
                        ))
                        .child(div().text_color(subline).children(corner.take())),
                )
            })
            // Line 2: harness identity belongs directly with the title,
            // instead of floating as unrelated metadata below it.
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(if compact {
                        4.0
                    } else {
                        SIDEBAR_ACTIVE_HARNESS_TITLE_GAP
                    }))
                    .children(compact_status)
                    .when_some(
                        harness.map(crate::pickers::harness_brand_icon),
                        |el, (path, tint)| {
                            el.child(
                                icon(path)
                                    .size(px(SIDEBAR_ACTIVE_HARNESS_ICON_SIZE))
                                    .flex_none()
                                    .text_color(
                                        tint.unwrap_or(subline).opacity(if archived_muted {
                                            0.4
                                        } else {
                                            0.8
                                        }),
                                    ),
                            )
                        },
                    )
                    .children(project_icon)
                    .child(sidebar_faded_label(
                        format!("chat-title-{content_id}").into(),
                        true,
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .line_height(px(17.0))
                            .child(popover::search_highlight(title, search_query, theme)),
                    ))
                    .when(!compact && !show_label && remote, |el| {
                        el.child(
                            icon(icons::REMOTE_SERVER)
                                .size(px(SIDEBAR_ACTIVE_HARNESS_ICON_SIZE))
                                .flex_none()
                                .text_color(subline),
                        )
                    })
                    .when(
                        if compact {
                            remote || corner_hovered
                        } else {
                            !show_label
                        },
                        |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_color(subline)
                                    .children(corner.take()),
                            )
                        },
                    )
                    .when(compact, |el| {
                        el.children(change_request.clone().map(|summary| {
                            if preview {
                                crate::change_requests::pull_request_badge_preview(
                                    format!("{row_id}-compact-pr").into(),
                                    summary,
                                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                                    theme,
                                )
                            } else {
                                crate::change_requests::pull_request_badge(
                                    format!("{row_id}-compact-pr").into(),
                                    summary,
                                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                                    theme,
                                )
                            }
                        }))
                    })
                    .when(compact, |el| {
                        el.child(
                            div()
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("chat-time-{id}")
                                })
                                .w(px(30.0))
                                .flex_none()
                                .text_right()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(subline)
                                .child(compact_jump_label.unwrap_or(time_ago)),
                        )
                    }),
            )
            // Line 3 is structural, not reserved whitespace: compact states
            // omit it completely when both Branch and Pull request are hidden.
            .when(!compact && shows_metadata, |row| {
                row.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .when_some(branch, |el, branch| {
                            el.child(
                                icon(icons::GIT_BRANCH)
                                    .size(px(11.0))
                                    .flex_none()
                                    .text_color(subline),
                            )
                            .child(sidebar_faded_label(
                                format!("chat-branch-{content_id}").into(),
                                false,
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .line_height(px(14.0))
                                    .text_color(subline)
                                    .child(popover::search_highlight(branch, search_query, theme)),
                            ))
                        })
                        // Stable invisible spring keeps the optional PR badge
                        // pinned right without changing no-PR paint.
                        .child(div().flex_1().min_w_0())
                        .when_some(change_request, |el, summary| {
                            el.child(if preview {
                                crate::change_requests::pull_request_badge_preview(
                                    format!("{row_id}-pr").into(),
                                    summary,
                                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                                    theme,
                                )
                            } else {
                                crate::change_requests::pull_request_badge_with_query(
                                    format!("{row_id}-pr").into(),
                                    summary,
                                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                                    search_query,
                                    theme,
                                )
                            })
                        }),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_pinned_session_group(
        items: Vec<AnyElement>,
        extra_gap: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = items.len();
        let row_centers = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Pixels>::new()));
        div()
            .on_children_prepainted({
                let row_centers = row_centers.clone();
                move |bounds, _, _| {
                    *row_centers.borrow_mut() = bounds.iter().map(|row| row.center().y).collect();
                }
            })
            .id("sidebar-pinned-sessions")
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_LIST_GAP))
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, cx| {
                    let payload = event.drag(cx).clone();
                    if !event.bounds.contains(&event.event.position)
                        || !this.sidebar_scroll.bounds().contains(&event.event.position)
                        || payload.visible_ids.len() != count
                    {
                        return;
                    }
                    let rel_y = f32::from(event.event.position.y) - f32::from(event.bounds.top());
                    if !payload.visible_ids.contains(&payload.chat_id) {
                        let top =
                            f32::from(event.bounds.bottom() - this.sidebar_scroll.bounds().top())
                                - extra_gap
                                + SIDEBAR_LIST_GAP
                                - f32::from(this.sidebar_scroll.offset().y);
                        if let Some(drag) = this.sidebar_session_transfer.as_mut() {
                            drag.preview = Some(SidebarSessionGap {
                                group: "pinned".into(),
                                index: count,
                                pinned: true,
                                top,
                            });
                        }
                    }
                    if payload.visible_ids.contains(&payload.chat_id)
                        && let Some(over) =
                            spaces::row_drop_index(rel_y, &this.sidebar_pinned_heights, false)
                    {
                        this.update_pinned_session_drag(&payload, over, cx);
                    }
                },
            ))
            .on_drop::<SidebarSessionDrag>(cx.listener(
                move |this, payload: &SidebarSessionDrag, window, cx| {
                    if payload.visible_ids.len() == count {
                        let index = this
                            .sidebar_session_transfer
                            .as_ref()
                            .filter(|_| !payload.visible_ids.contains(&payload.chat_id))
                            .and_then(|drag| drag.preview.as_ref())
                            .filter(|gap| gap.pinned)
                            .map(|gap| gap.index)
                            .or_else(|| this.pinned_session_drag.as_ref().map(|drag| drag.over))
                            .unwrap_or_else(|| {
                                row_centers
                                    .borrow()
                                    .iter()
                                    .position(|center| window.mouse_position().y < *center)
                                    .unwrap_or(count)
                            });
                        this.finish_sidebar_session_transfer(
                            payload,
                            SidebarSessionDrop::Pinned(index),
                            cx,
                        );
                    } else {
                        this.cancel_sidebar_session_transfer(cx);
                    }
                },
            ))
            .children(items)
            .when(extra_gap > 0.0, |el| {
                let height = extra_gap - if count > 0 { SIDEBAR_LIST_GAP } else { 0.0 };
                el.child(
                    div()
                        .flex_none()
                        .h(px(height.max(0.0)))
                        .mb(px(height.min(0.0))),
                )
            })
            .into_any_element()
    }

    /// Chat-mode sidebar (spaces overhaul): window-control strip, the Spaces
    /// section (folder + device rows, add-space), the global Active sessions
    /// list, the notice strip, and the UserMenu (§1.6).
    /// The global connection line. `None` while healthy (`Connected`) or on
    /// local profiles (`Disabled`) — and the engine's degrade grace means it
    /// only exists during REAL outages, never join/wake blips. No surface,
    /// no border (v0.2.12 feedback): a bare spinner + faint caption while
    /// reconnecting; an amber dot only when the OS says offline. The
    /// transport error belongs in logs, not the sidebar.
    pub(super) fn render_connection_pill(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        use zeron_proto::ConnectivityState as S;
        let conn = self.state.read(cx).connectivity.clone();
        let (label, glyph): (SharedString, AnyElement) = match conn.state {
            S::Disabled | S::Connected => return None,
            S::Offline => (
                "Offline — sends are saved".into(),
                div()
                    .size(px(5.0))
                    .rounded_full()
                    .bg(theme.warning)
                    .into_any_element(),
            ),
            S::Reconnecting => (
                "Reconnecting…".into(),
                loaders::mini_mono_spinner(
                    "connection-spinner",
                    2.0,
                    theme.text_muted,
                    self.sidebar_pane.entity_id(),
                    cx,
                )
                .into_any_element(),
            ),
        };
        Some(
            crate::motion::fade_in(
                "connection-pill",
                div()
                    .id("connection-pill")
                    .mx(px(Theme::SPACE_SM + 4.0))
                    .mb(px(Theme::SPACE_SM))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(glyph)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(label),
                    ),
            )
            .into_any_element(),
        )
    }

    pub(super) fn render_chat_sidebar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_session_transfer.as_ref().is_some_and(|drag| {
            !cx.has_active_drag() || !self.sidebar_session_transfer_is_valid(&drag.payload, cx)
        }) {
            self.cancel_sidebar_session_transfer(cx);
        }
        // A release outside the sidebar ends GPUI's drag without calling the
        // sidebar drop handler. Heal the ephemeral slide state here.
        if self.pinned_session_drag.is_some()
            && (!cx.has_active_drag() || !self.pinned_session_drag_is_valid(cx))
        {
            self.cancel_pinned_session_drag(cx);
        }
        let (user, workspace_scope) = {
            let state = self.state.read(cx);
            (state.auth_user().cloned(), state.workspace_scope)
        };

        // Keyed rows: (stable key, estimated height, element) — the key + height
        // list drives the §1.6 resort FLIP diff below (attention-bucket
        // promotions glide; cleared rows just go).
        let session_rows = self.render_active_rows(theme, cx);
        let moving_row = session_rows
            .moving_row
            .map(|(row, height)| self.render_moving_sidebar_session(row, height, theme));
        let pinned_count = session_rows.pinned_count;
        let keyed = session_rows.rows;
        let regular_count = session_rows.regular_count;
        let ungrouped = self.settings.sidebar_organization == SidebarOrganization::InOneList;
        let regular_body_height = spaces::SIDEBAR_DISCLOSURE_BODY_INSET
            + keyed
                .iter()
                .skip(pinned_count)
                .map(|(_, height, _)| height)
                .sum::<f32>()
            + SIDEBAR_LIST_GAP * keyed.len().saturating_sub(pinned_count + 1) as f32;
        let regular_body_height =
            if keyed.len() == pinned_count && self.sidebar_session_transfer.is_some() {
                spaces::SIDEBAR_DISCLOSURE_BODY_INSET
                    + 48.0
                    + self.sidebar_transfer_extra_gap("regular")
            } else {
                regular_body_height
            };
        let pinned_body_height = spaces::SIDEBAR_DISCLOSURE_BODY_INSET
            + self.sidebar_transfer_extra_gap("pinned")
            + keyed
                .iter()
                .take(pinned_count)
                .map(|(_, height, _)| height)
                .sum::<f32>()
            + SIDEBAR_LIST_GAP * pinned_count.saturating_sub(1) as f32;

        // Resort glide (§1.6 View Transitions parity): when the ORDER of a live
        // list changes (new activity resort, grouping flip), surviving rows
        // glide from their old y to the new one — layout is already at the new
        // position; the offset is a paint-only relative inset animated to 0
        // over 260ms cubic-bezier(0.22,1,0.36,1). New rows fade in; removals
        // just go (matching the original). First fill and chat switches (which
        // don't reorder) never animate.
        let mut order: Vec<(String, f32)> = Vec::with_capacity(keyed.len() + 2);
        let show_pinned_section = pinned_count > 0 || self.sidebar_session_transfer.is_some();
        if show_pinned_section {
            order.push((
                "sidebar-pinned-header".to_string(),
                spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT
                    + if self.pinned_open {
                        spaces::SIDEBAR_DISCLOSURE_BODY_INSET - SIDEBAR_LIST_GAP
                    } else {
                        0.0
                    },
            ));
        }
        for (ix, (key, height, _)) in keyed.iter().enumerate() {
            if ungrouped && ix == pinned_count {
                order.push((
                    "sidebar-sessions-header".into(),
                    spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT
                        + if show_pinned_section { 12.0 } else { 0.0 }
                        + if self.sessions_open {
                            spaces::SIDEBAR_DISCLOSURE_BODY_INSET - SIDEBAR_LIST_GAP
                        } else {
                            0.0
                        },
                ));
            }
            if ungrouped && ix >= pinned_count && !self.sessions_open {
                continue;
            }
            if ix < pinned_count && !self.pinned_open {
                continue;
            }
            order.push((key.clone(), *height));
        }
        if self.pinned_session_drag.is_none()
            && self.sidebar_session_transfer.is_none()
            && self.sidebar_prev_order != order
        {
            let key_order_changed = sidebar_key_order_changed(&self.sidebar_prev_order, &order);
            if !self.sidebar_prev_order.is_empty() {
                // A disclosure already animates its own body height. Applying
                // FLIP offsets when only keyed heights change double-counts
                // that movement, leaving gaps and momentary overlaps between
                // the first group, following groups, and Archived.
                let offsets = if key_order_changed {
                    resort_offsets(&self.sidebar_prev_order, &order, SIDEBAR_LIST_GAP)
                } else {
                    std::collections::HashMap::new()
                };
                let prev_keys: std::collections::HashSet<&str> = self
                    .sidebar_prev_order
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect();
                let new_keys: std::collections::HashSet<String> = order
                    .iter()
                    .filter(|(k, _)| !prev_keys.contains(k.as_str()))
                    .map(|(k, _)| k.clone())
                    .collect();
                if key_order_changed && (!offsets.is_empty() || !new_keys.is_empty()) {
                    self.resort_epoch += 1;
                    self.sidebar_resort = offsets;
                    self.sidebar_new_keys = new_keys;
                }
            }
            self.sidebar_prev_order = order;
        }
        let epoch = self.resort_epoch;
        let pinned_drag = self
            .pinned_session_drag
            .as_ref()
            .filter(|_| {
                self.sidebar_session_transfer
                    .as_ref()
                    .is_none_or(|drag| drag.preview.as_ref().is_none_or(|gap| gap.pinned))
            })
            .map(|drag| (drag.from, drag.over, drag.prev_over, drag.epoch));
        let list_items: Vec<AnyElement> = keyed
            .into_iter()
            .enumerate()
            .map(|(ix, (key, _, element))| {
                if ix < pinned_count
                    && let Some((from, over, prev_over, drag_epoch)) = pinned_drag
                {
                    // Its actual row is drawn once in the sidebar's movement layer.
                    if ix == from {
                        return element;
                    }
                    let start = crate::terminal::panel::slide_offset(ix, from, prev_over)
                        * (self
                            .sidebar_pinned_heights
                            .get(from)
                            .copied()
                            .unwrap_or(61.0)
                            + SIDEBAR_LIST_GAP);
                    let target = crate::terminal::panel::slide_offset(ix, from, over)
                        * (self
                            .sidebar_pinned_heights
                            .get(from)
                            .copied()
                            .unwrap_or(61.0)
                            + SIDEBAR_LIST_GAP);
                    if self.reduced_motion {
                        return div()
                            .relative()
                            .top(px(target))
                            .child(element)
                            .into_any_element();
                    }
                    return div()
                        .child(element)
                        .with_animation(
                            (
                                "pinned-session-slide",
                                (ix as u64) | ((drag_epoch as u64) << 32),
                            ),
                            TAB_SLIDE.animation(),
                            move |el, t| el.relative().top(px(motion::lerp(start, target, t))),
                        )
                        .into_any_element();
                }
                if self.sidebar_session_transfer.is_some() {
                    return element;
                }
                if let Some(dy) = self.sidebar_resort.get(&key).copied() {
                    let id = SharedString::from(format!("resort-{epoch}-{key}"));
                    div()
                        .child(element)
                        .with_animation(id, RESORT.animation(), move |el, t| {
                            el.relative().top(px(dy * (1.0 - t)))
                        })
                        .into_any_element()
                } else if self.sidebar_new_keys.contains(&key) {
                    let id = SharedString::from(format!("row-in-{epoch}-{key}"));
                    motion::fade_quick(id, div().child(element)).into_any_element()
                } else {
                    element
                }
            })
            .collect();

        // t3code's archived accordion, below the active list.
        let archived_section = self.render_archived_section(theme, cx);

        let (user_line, trigger_subline, menu_identity): (
            SharedString,
            Option<SharedString>,
            SharedString,
        ) = match workspace_scope {
            Some(WorkspaceScope::Local) => {
                let line = if matches!(self.sync_flow, SyncFlow::RestartPending { .. }) {
                    "Sync ready after restart"
                } else {
                    "Local only"
                };
                (line.into(), None, "Stored on this device".into())
            }
            Some(WorkspaceScope::Development) => (
                "Development".into(),
                Some("Local development runtime".into()),
                "Authentication disabled".into(),
            ),
            Some(WorkspaceScope::Synced) | None => {
                let line: SharedString = user
                    .as_ref()
                    .map(|u| u.name.clone().unwrap_or_else(|| u.email.clone()).into())
                    .unwrap_or_else(|| SharedString::from("Not signed in"));
                let email = user
                    .as_ref()
                    .map(|u| SharedString::from(u.email.clone()))
                    .unwrap_or_else(|| line.clone());
                (line, Some("Alpha".into()), email)
            }
        };
        let user_menu =
            self.render_user_menu(user_line.clone(), trigger_subline, menu_identity, theme, cx);

        // The space filter lives ABOVE the scroll region (fixed) so its
        // dropdown can float without being clipped by the list's overflow.
        let filter_row = self.render_spaces_filter(theme, cx);
        let active_list = if !list_items.is_empty() {
            let mut pinned_items = list_items;
            let regular_items = pinned_items.split_off(pinned_count);
            let regular_empty = regular_items.is_empty();
            let pinned_group = show_pinned_section
                .then(|| self.render_pinned_section(pinned_items, pinned_body_height, theme, cx));
            div()
                .id("sidebar-active-sessions")
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_LIST_GAP))
                .pb(px(Theme::SPACE_SM))
                .when_some(pinned_group, |el, group| el.child(group))
                .when(
                    !regular_items.is_empty() || self.sidebar_session_transfer.is_some(),
                    |el| {
                        el.child(
                            self.render_sessions_section(
                                div()
                                    .id("sidebar-regular-sessions")
                                    .debug_selector(|| "sidebar-regular-sessions".into())
                                    .on_drag_move::<SidebarSessionDrag>(cx.listener(
                                        move |this,
                                              event: &gpui::DragMoveEvent<SidebarSessionDrag>,
                                              _,
                                              _| {
                                            if regular_empty
                                                && event.bounds.contains(&event.event.position)
                                            {
                                                let top = f32::from(
                                                    event.bounds.top()
                                                        - this.sidebar_scroll.bounds().top()
                                                        - this.sidebar_scroll.offset().y,
                                                );
                                                if let Some(drag) =
                                                    this.sidebar_session_transfer.as_mut()
                                                {
                                                    drag.preview = Some(SidebarSessionGap {
                                                        group: "regular".into(),
                                                        index: 0,
                                                        pinned: false,
                                                        top,
                                                    });
                                                }
                                                return;
                                            }
                                            if event.bounds.contains(&event.event.position)
                                                && let Some(drag) =
                                                    this.sidebar_session_transfer.as_mut()
                                                && drag
                                                    .preview
                                                    .as_ref()
                                                    .is_some_and(|gap| gap.pinned)
                                            {
                                                drag.preview = None;
                                            }
                                        },
                                    ))
                                    .on_drop::<SidebarSessionDrag>(cx.listener(
                                        |this, payload, _, cx| {
                                            this.finish_sidebar_session_transfer(
                                                payload,
                                                SidebarSessionDrop::Regular,
                                                cx,
                                            );
                                        },
                                    ))
                                    .flex()
                                    .flex_col()
                                    .gap(px(SIDEBAR_LIST_GAP))
                                    .when(regular_items.is_empty(), |el| {
                                        el.h(px(48.0 + self.sidebar_transfer_extra_gap("regular")))
                                            .justify_center()
                                            .px(px(10.0))
                                            .text_color(theme.text_muted)
                                            .text_size(crate::typography::ui_rems(12.0))
                                            .child("Drop here to unpin")
                                    })
                                    .children(regular_items)
                                    .into_any_element(),
                                regular_body_height,
                                regular_count,
                                show_pinned_section,
                                theme,
                                cx,
                            ),
                        )
                    },
                )
                .into_any_element()
        } else {
            div()
                .px(px(Theme::SPACE_SM))
                .pb(px(Theme::SPACE_SM))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from("No sessions yet"))
                .into_any_element()
        };

        // The (filtered) Sessions list scrolls inside an EdgeFade scope —
        // a true per-glyph gradient at active overflow edges. Glass-safe
        // (no painted overlay can fade content over see-through blur) and
        // equivalent on opaque themes: alpha→0 reveals the surface tone
        // underneath, same as the gradient overlays it replaced. Overflow
        // is read at PAINT time via the scroll handle — render-time gating
        // rode the previous frame's offset, so the last frame of a content
        // shrink (row archived while scrolled) left a phantom fade stuck
        // over an unscrollable list (user report).
        let sidebar_lists = crate::edge_fade::edge_faded(
            SIDEBAR_GLASS_FADE_BAND,
            true,
            true,
            div().relative().flex_1().min_h_0().child(
                div()
                    .id("sidebar-lists")
                    .relative()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .on_drag_move::<SidebarSessionDrag>(cx.listener(
                        move |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, cx| {
                            if let Some(transfer) = this.sidebar_session_transfer.as_mut() {
                                transfer.viewport = Some(event.bounds);
                            }
                            if !event.bounds.contains(&event.event.position) {
                                if let Some(transfer) = this.sidebar_session_transfer.as_mut() {
                                    transfer.preview = None;
                                }
                                return;
                            }
                            let payload = event.drag(cx).clone();
                            this.track_pinned_session_drag_pointer(
                                payload,
                                f32::from(event.event.position.y),
                                f32::from(event.bounds.top()),
                                f32::from(event.bounds.bottom()),
                                cx,
                            );
                        },
                    ))
                    // Empty space and Archived are not transfer targets.
                    .on_drop::<SidebarSessionDrag>(cx.listener(
                        |this, _: &SidebarSessionDrag, _, cx| {
                            this.cancel_sidebar_session_transfer(cx);
                        },
                    ))
                    .px(px(Theme::SPACE_SM))
                    .flex()
                    .flex_col()
                    // No "Sessions" header (user request) — the list
                    // is the whole column; a little air stands in.
                    .pt(px(SIDEBAR_LIST_PAD_TOP))
                    .child(active_list)
                    .children(archived_section)
                    .children(moving_row),
            ),
        )
        .fade_overflow_y(&self.sidebar_scroll);

        div()
            .w(px(self.settings.sidebar_width))
            .h_full()
            .flex()
            .flex_col()
            // (No titlebar strip: the unified window titlebar spans the whole
            // window above this column.)
            .child(filter_row)
            .child(sidebar_lists)
            // Global connection pill (durable-by-design UI truth): appears
            // whenever the edge posture is degraded; hidden while healthy —
            // appearing IS the signal.
            .when_some(self.render_connection_pill(theme, cx), |el, pill| {
                el.child(pill)
            })
            // Update strip (above the user menu; below the lists).
            .when_some(self.render_update_strip(theme, cx), |el, strip| {
                el.child(strip)
            })
            // Inline mutation-failure notice.
            .when_some(self.sidebar_notice.clone(), |el, notice| {
                el.child(
                    div()
                        .id("sidebar-notice")
                        .mx(px(Theme::SPACE_SM))
                        .mb(px(Theme::SPACE_SM))
                        .px(px(Theme::SPACE_SM))
                        .py(px(4.0))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(theme.danger)
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.danger)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar_notice = None;
                            cx.notify();
                        }))
                        .child(notice),
                )
            })
            .child(div().p(px(Theme::SPACE_SM)).flex_none().child(user_menu))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sidebar resort FLIP diff (§1.6) ----

    fn keys(list: &[(&str, f32)]) -> Vec<(String, f32)> {
        list.iter().map(|(k, h)| (k.to_string(), *h)).collect()
    }

    #[test]
    fn sidebar_chat_height_tracks_visible_metadata() {
        assert_eq!(chat_row_height(false, false), 45.0);
        assert_eq!(chat_row_height(true, false), 61.0);
        assert_eq!(chat_row_height(false, true), 63.0);
        assert_eq!(chat_row_height(true, true), 63.0);
    }

    #[test]
    fn sidebar_harness_geometry_reflects_row_hierarchy() {
        assert_eq!(SIDEBAR_ACTIVE_HARNESS_TITLE_GAP, Theme::SPACE_SM);
    }

    #[test]
    fn sidebar_height_change_is_not_a_reorder() {
        let open = keys(&[("first-group", 105.0), ("second-group", 240.0)]);
        let collapsed = keys(&[("first-group", 40.0), ("second-group", 240.0)]);
        assert!(!sidebar_key_order_changed(&open, &collapsed));

        let reordered = keys(&[("second-group", 240.0), ("first-group", 40.0)]);
        assert!(sidebar_key_order_changed(&collapsed, &reordered));
    }

    #[test]
    fn resort_offsets_empty_when_order_unchanged() {
        let order = keys(&[("a", 29.0), ("b", 29.0), ("c", 45.0)]);
        assert!(resort_offsets(&order, &order, 2.0).is_empty());
    }

    #[test]
    fn resort_offsets_activity_moves_row_to_top() {
        // c (bottom, y=62) jumps to top: c glides down-from-above? No — c's
        // old y is 62, new y is 0 → starts +62 below… offset = old - new = +62,
        // painted at +62 decaying to 0 (a glide UP into place). a and b shift
        // down by c's height + gap (31).
        let old = keys(&[("a", 29.0), ("b", 29.0), ("c", 29.0)]);
        let new = keys(&[("c", 29.0), ("a", 29.0), ("b", 29.0)]);
        let offsets = resort_offsets(&old, &new, 2.0);
        assert_eq!(offsets.get("c"), Some(&62.0));
        assert_eq!(offsets.get("a"), Some(&-31.0));
        assert_eq!(offsets.get("b"), Some(&-31.0));
    }

    #[test]
    fn resort_offsets_respect_heights_and_gap() {
        // Tall row (45px) swaps with a short one (29px).
        let old = keys(&[("tall", 45.0), ("short", 29.0)]);
        let new = keys(&[("short", 29.0), ("tall", 45.0)]);
        let offsets = resort_offsets(&old, &new, 2.0);
        // short: old y 47 → new y 0; tall: old y 0 → new y 31.
        assert_eq!(offsets.get("short"), Some(&47.0));
        assert_eq!(offsets.get("tall"), Some(&-31.0));
    }

    #[test]
    fn resort_offsets_ignore_added_and_removed_keys() {
        let old = keys(&[("a", 29.0), ("gone", 29.0), ("b", 29.0)]);
        let new = keys(&[("new", 29.0), ("a", 29.0), ("b", 29.0)]);
        let offsets = resort_offsets(&old, &new, 2.0);
        // "new" has no old position (fades in instead); "gone" just goes.
        assert!(!offsets.contains_key("new"));
        assert!(!offsets.contains_key("gone"));
        // a: old 0 → new 31 (pushed down by the insert); b: 62 → 62 (gone's
        // slot replaced by "new" of equal height — no move, no entry).
        assert_eq!(offsets.get("a"), Some(&-31.0));
        assert_eq!(offsets.get("b"), None);
    }

    #[test]
    fn resort_glide_spec_matches_original() {
        // §1.6: 260ms cubic-bezier(0.22, 1, 0.36, 1).
        assert_eq!(RESORT.duration_ms, 260);
        assert_eq!(RESORT.curve, motion::EASE_RESORT);
    }
}
