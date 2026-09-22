use super::*;
use crate::icons::{self, icon};
use crate::motion;
use crate::shell::{
    SIDEBAR_LIST_GAP, SidebarOrganization, SidebarSessionDrag, SidebarSessionDrop,
    SidebarSessionGap, SidebarSessionRows, SidebarSort, format_time_ago, sidebar_row_height,
};
use crate::theme::Theme;
use crate::transcript;
use chrono::Utc;
use gpui::{AnyElement, Context, SharedString, div, px};
use std::collections::HashSet;
use zeron_proto::ChatIndicator;

struct ActiveChatRow {
    status: ChatIndicator,
    chat: zeron_proto::Chat,
    folder: String,
    branch: Option<String>,
    change_request: Option<zeron_proto::ChangeRequestSummary>,
    group: Option<(String, String)>,
}

pub(in crate::shell) fn compare_sidebar_chats(
    sort: SidebarSort,
    left: &zeron_proto::Chat,
    right: &zeron_proto::Chat,
) -> std::cmp::Ordering {
    let primary = match sort {
        SidebarSort::Created => right.created_at.cmp(&left.created_at),
        SidebarSort::LastUpdated => right
            .last_message_at
            .unwrap_or(right.created_at)
            .cmp(&left.last_message_at.unwrap_or(left.created_at)),
    };
    primary.then_with(|| left.id.cmp(&right.id))
}

/// Put this machine's device group first without disturbing the recency-based
/// order of any remote groups. A targeted promotion is more truthful than a
/// full name sort: local context leads, then the user's chosen chat sort wins.
pub(in crate::shell) fn promote_local_device_group<T>(
    groups: &mut Vec<(Option<(String, String)>, Vec<T>)>,
    local_device_id: Option<&str>,
) {
    let Some(local_device_id) = local_device_id else {
        return;
    };
    let Some(index) = groups.iter().position(|(group, _)| {
        group
            .as_ref()
            .is_some_and(|(device_id, _)| device_id == local_device_id)
    }) else {
        return;
    };
    if index > 0 {
        let local = groups.remove(index);
        groups.insert(0, local);
    }
}

/// Shared quiet rule for sidebar groups and palette sections.
pub(in crate::shell) fn sidebar_separator(theme: &Theme) -> gpui::Div {
    div().h(px(1.0)).bg(theme.border.opacity(0.6))
}

fn sidebar_disclosure_header(
    theme: &Theme,
    label: SharedString,
    count: Option<usize>,
    chevron: AnyElement,
    group_icon: Option<&'static str>,
) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(crate::typography::ui_rems(8.0))
        .h(crate::typography::ui_rems(SIDEBAR_DISCLOSURE_HEADER_HEIGHT))
        .px(crate::typography::ui_rems(Theme::SPACE_SM))
        .rounded(crate::typography::ui_rems(8.0))
        .hover(|el| el.bg(theme.glass_hover()).text_color(theme.text))
        .cursor_pointer()
        .when_some(group_icon, |el, icon_path| {
            el.child(
                div()
                    .size(crate::typography::ui_rems(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(icon_path)
                            .size(crate::typography::ui_rems(14.0))
                            .text_color(theme.text_muted.opacity(0.65)),
                    ),
            )
        })
        .child(super::sidebar_faded_label(
            "sidebar-disclosure-label".into(),
            false,
            div()
                .text_size(crate::typography::ui_rems(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted.opacity(0.75))
                .child(label),
        ))
        .child(div().flex_1())
        .when_some(count, |el, count| {
            el.child(
                div()
                    .flex_none()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted.opacity(0.45))
                    .child(SharedString::from(count.to_string())),
            )
        })
        .child(chevron)
}

pub(in crate::shell) fn status_dot_color(status: ChatIndicator, theme: &Theme) -> gpui::Hsla {
    match status {
        // Preset activity tone, not warning amber: running is routine.
        // Non-done statuses sit well below full
        // strength: at full alpha the colored words shouted across the
        // whole sidebar (user request) â€” only Done keeps its pop.
        ChatIndicator::Working => theme.busy.opacity(0.55),
        // Blue: "asking you a question" must read differently from "busy
        // working" at a glance.
        ChatIndicator::AwaitingInput => theme.accent.opacity(0.6),
        ChatIndicator::Errored => theme.danger.opacity(0.65),
        // Green: finished-but-unseen reads as "ready for you".
        ChatIndicator::Completed => {
            theme.success.opacity(0.9) // emerald-400
        }
        ChatIndicator::Idle => crate::theme::ink(0.14),
    }
}

impl Shell {
    fn begin_sidebar_disclosure_motion(
        &mut self,
        key: &str,
        resting_height: f32,
        target_height: f32,
    ) {
        let previous = self.sidebar_disclosure_motion.get(key).copied();
        let from = previous
            .filter(|motion| motion.animating())
            .map(SidebarDisclosureMotion::current)
            .unwrap_or(resting_height);
        let epoch = previous.map_or(1, |motion| motion.epoch + 1);
        self.sidebar_disclosure_motion.insert(
            key.to_owned(),
            SidebarDisclosureMotion::new(epoch, from, target_height),
        );
    }

    fn render_sidebar_disclosure_body(
        &self,
        key: &str,
        open: bool,
        full_height: f32,
        content: AnyElement,
    ) -> AnyElement {
        let frame = div().w_full().flex_none().overflow_hidden().child(content);
        let scale = self.ui_scale();
        let Some(tween) = self
            .sidebar_disclosure_motion
            .get(key)
            .copied()
            .filter(|motion| motion.animating())
        else {
            if open {
                return frame.into_any_element();
            } else {
                return frame.h(px(0.0)).into_any_element();
            }
        };
        let denominator = (full_height * scale).max(1.0);
        frame
            .with_animation(
                SharedString::from(format!("sidebar-disclosure-{key}-{}", tween.epoch)),
                motion::COLLAPSE.animation(),
                move |el, t| {
                    let height = motion::lerp(tween.from * scale, tween.to * scale, t);
                    let reveal = (height / denominator).clamp(0.0, 1.0);
                    el.h(px(height))
                        .opacity(0.35 + 0.65 * reveal)
                        .relative()
                        .top(px(-3.0 * scale * (1.0 - reveal)))
                },
            )
            .into_any_element()
    }

    fn sidebar_disclosure_chevron(&self, key: &str, open: bool, theme: &Theme) -> AnyElement {
        let resting_reveal = if open { 1.0 } else { 0.0 };
        let chevron = icon(icons::ALT_ARROW_RIGHT)
            .size(crate::typography::ui_rems(12.0))
            .text_color(theme.text_muted.opacity(0.5));
        if let Some(tween) = self
            .sidebar_disclosure_motion
            .get(key)
            .copied()
            .filter(|motion| motion.animating())
        {
            let denominator = tween.from.max(tween.to).max(1.0);
            let from = (tween.from / denominator).clamp(0.0, 1.0);
            let to = (tween.to / denominator).clamp(0.0, 1.0);
            div()
                .flex_none()
                .size(crate::typography::ui_rems(12.0))
                .child(chevron.with_animation(
                    SharedString::from(format!("sidebar-chevron-{key}-{}", tween.epoch)),
                    motion::COLLAPSE.animation(),
                    move |el, t| {
                        let reveal = motion::lerp(from, to, t);
                        el.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                            reveal * 0.25,
                        )))
                    },
                ))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .size(crate::typography::ui_rems(12.0))
                .child(
                    chevron.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                        resting_reveal * 0.25,
                    ))),
                )
                .into_any_element()
        }
    }

    /// Flat top-to-bottom chat ids exactly as [`Self::render_active_rows`]
    /// draws them: pins first, then the user's sort, device grouping,
    /// and local-device promotion. Jump shortcuts and session cycling read
    /// this projection so keyboard order never drifts from the screen.
    pub(in crate::shell) fn sidebar_visible_order(&self, cx: &Context<Self>) -> Vec<String> {
        let filter = self.settings.space_filter.clone();
        let profile_key = self.active_sidebar_pin_profile_key(cx);
        let saved_pins = self.active_sidebar_pins(cx);
        let frozen_pinned = self
            .pinned_session_drag
            .as_ref()
            .filter(|drag| {
                drag.filter == filter && profile_key.as_deref() == Some(&drag.profile_key)
            })
            .map(|drag| drag.visible_ids.clone());
        let pinned_order = frozen_pinned
            .as_ref()
            .map_or(saved_pins.as_slice(), |ids| ids.as_slice());
        let state = self.state.read(cx);
        let mut chats: Vec<zeron_proto::Chat> = state
            .sidebar_chats(Utc::now(), filter.as_deref())
            .into_iter()
            .map(|(_, chat)| chat.clone())
            .collect();
        chats.sort_by(|left, right| compare_sidebar_chats(self.settings.sidebar_sort, left, right));
        let (pinned_chats, chats): (Vec<_>, Vec<_>) = chats
            .into_iter()
            .partition(|chat| pinned_order.contains(&chat.id));
        let ordered = if self.settings.sidebar_organization != SidebarOrganization::InOneList {
            let mut groups: Vec<(Option<(String, String)>, Vec<zeron_proto::Chat>)> = Vec::new();
            for chat in chats {
                let key = Some((
                    if self.settings.sidebar_organization == SidebarOrganization::ByProject {
                        chat.space_id
                            .clone()
                            .unwrap_or_else(|| format!("home:{}", chat.device_id))
                    } else {
                        chat.device_id.clone()
                    },
                    String::new(),
                ));
                if let Some((_, existing)) = groups.iter_mut().find(|(group, _)| group == &key) {
                    existing.push(chat);
                } else {
                    groups.push((key, vec![chat]));
                }
            }
            if self.settings.sidebar_organization == SidebarOrganization::ByDevice {
                promote_local_device_group(&mut groups, state.local_device_id.as_deref());
            }
            groups
                .into_iter()
                .flat_map(|(_, rows)| rows)
                .map(|chat| chat.id)
                .collect::<Vec<_>>()
        } else {
            chats.into_iter().map(|chat| chat.id).collect()
        };
        let ordered = pinned_chats
            .into_iter()
            .map(|chat| chat.id)
            .chain(ordered)
            .collect::<Vec<_>>();
        let mut visible = project_pinned_first(&ordered, pinned_order);
        if !self.sessions_open
            && self.settings.sidebar_organization == SidebarOrganization::InOneList
        {
            visible.retain(|id| pinned_order.contains(id));
        }
        if !self.pinned_open {
            let pins: HashSet<&str> = pinned_order.iter().map(String::as_str).collect();
            visible.retain(|id| !pins.contains(id.as_str()));
        }
        visible
    }

    /// Shared metadata and visibility settings for active and archived sessions.
    fn sidebar_chat_data(
        &self,
        status: ChatIndicator,
        chat: zeron_proto::Chat,
        state: &AppState,
    ) -> ActiveChatRow {
        // Line 1 is "project @ device" (t3code's project row);
        // project-less sessions read as their home-dir cwd `~`.
        let space = state.space_for_chat(&chat);
        let project = match (space, chat.space_id.as_deref()) {
            (Some(space), _) => space.display_name().to_string(),
            (None, None) => "~".to_string(),
            (None, Some(_)) => "?".to_string(),
        };
        let device = state
            .device_name(&chat.device_id)
            .unwrap_or("Unknown device")
            .to_string();
        let folder = crate::shell::sidebar_sessions::sidebar_session_location(
            space.map(|space| space.path.as_str()),
            &project,
        );
        // The branch shows whenever the engine has stamped one â€”
        // main-checkout sessions included, not just worktrees.
        let branch = crate::change_requests::conversation_branch(&chat, &state.spaces)
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_string)
            .filter(|_| self.settings.sidebar_show_branch);
        let change_request = state
            .change_request_for_chat(&chat)
            .cloned()
            .filter(|_| self.settings.sidebar_show_pull_request);
        let group = if self.settings.space_filter.is_some() {
            None
        } else {
            match self.settings.sidebar_organization {
                SidebarOrganization::ByDevice => Some((chat.device_id.clone(), device)),
                SidebarOrganization::ByProject => Some((
                    chat.space_id
                        .clone()
                        .unwrap_or_else(|| format!("home:{}", chat.device_id)),
                    project,
                )),
                SidebarOrganization::InOneList => None,
            }
        };
        ActiveChatRow {
            status,
            chat: chat.clone(),
            folder,
            branch,
            change_request,
            group,
        }
    }

    pub(in crate::shell) fn render_active_rows(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> SidebarSessionRows {
        let now = Utc::now();
        let filter = self.settings.space_filter.clone();
        let profile_key = self.active_sidebar_pin_profile_key(cx);
        let saved_pins = self.active_sidebar_pins(cx);
        let frozen_pinned = self
            .pinned_session_drag
            .as_ref()
            .filter(|drag| {
                drag.filter == filter && profile_key.as_deref() == Some(&drag.profile_key)
            })
            .map(|drag| drag.visible_ids.clone());
        let mut rows: Vec<ActiveChatRow> = {
            let state = self.state.read(cx);
            let mut chats: Vec<_> = state
                .sidebar_chats(now, filter.as_deref())
                .into_iter()
                .map(|(status, chat)| (status, chat.clone()))
                .collect();
            chats.sort_by(|left, right| {
                compare_sidebar_chats(self.settings.sidebar_sort, &left.1, &right.1)
            });
            chats
                .into_iter()
                .map(|(status, chat)| self.sidebar_chat_data(status, chat, state))
                .collect()
        };
        let pinned_order = frozen_pinned
            .as_ref()
            .map_or(saved_pins.as_slice(), |ids| ids.as_slice());
        let base_ids: Vec<String> = rows.iter().map(|row| row.chat.id.clone()).collect();
        let ordered_ids = project_pinned_first(&base_ids, pinned_order);
        let rank: std::collections::HashMap<&str, usize> = ordered_ids
            .iter()
            .enumerate()
            .map(|(ix, id)| (id.as_str(), ix))
            .collect();
        rows.sort_by_key(|row| {
            rank.get(row.chat.id.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        // One selected space already scopes the list. Avoid nested group cards.
        if filter.is_some() {
            for row in &mut rows {
                row.group = None;
            }
        }
        let active: HashSet<&str> = base_ids.iter().map(String::as_str).collect();
        let pinned_count = pinned_order
            .iter()
            .filter(|id| active.contains(id.as_str()))
            .collect::<HashSet<_>>()
            .len();
        let regular_rows = rows.split_off(pinned_count);
        let pinned_rows = rows;
        self.sidebar_pinned_heights = pinned_rows
            .iter()
            .map(|row| {
                sidebar_row_height(
                    false,
                    self.settings.sidebar_show_project_label,
                    row.branch.is_some(),
                    row.change_request.is_some(),
                )
            })
            .collect();
        let visible_pinned_ids = std::sync::Arc::new(
            pinned_rows
                .iter()
                .map(|row| row.chat.id.clone())
                .collect::<Vec<_>>(),
        );

        let mut regular_groups: Vec<(Option<(String, String)>, Vec<ActiveChatRow>)> = Vec::new();
        for row in regular_rows {
            if let Some((_, existing)) = regular_groups
                .iter_mut()
                .find(|(group, _)| group == &row.group)
            {
                existing.push(row);
            } else {
                regular_groups.push((row.group.clone(), vec![row]));
            }
        }
        if self.settings.sidebar_organization == SidebarOrganization::ByDevice {
            let local_device_id = self.state.read(cx).local_device_id.clone();
            promote_local_device_group(&mut regular_groups, local_device_id.as_deref());
        }
        let mut sections = Vec::with_capacity(regular_groups.len() + usize::from(pinned_count > 0));
        if !pinned_rows.is_empty() {
            sections.push((None, pinned_rows));
        }
        sections.extend(regular_groups);

        let returning = self.sidebar_session_transfer.is_none();
        let transfer = self.sidebar_session_transfer.as_mut().or_else(|| {
            self.sidebar_session_return
                .as_mut()
                .map(|state| &mut state.transfer)
        });
        if let Some(drag) = transfer {
            for (section_index, (group, rows)) in sections.iter().enumerate() {
                let Some(index) = rows
                    .iter()
                    .position(|row| row.chat.id == drag.payload.chat_id)
                else {
                    continue;
                };
                drag.source_group = if section_index == 0 && pinned_count > 0 {
                    "pinned".into()
                } else {
                    group
                        .as_ref()
                        .map_or_else(|| "regular".into(), |(key, _)| format!("regular:{key}"))
                };
                drag.source_index = index;
                drag.row_height = sidebar_row_height(
                    false,
                    self.settings.sidebar_show_project_label,
                    rows[index].branch.is_some(),
                    rows[index].change_request.is_some(),
                );
                // Move the vacant slot to the destination instead of keeping two
                // holes. Sample layout and paint with the same reversible easing.
                let collapse = !returning
                    && drag
                        .preview
                        .as_ref()
                        .is_some_and(|gap| gap.group != drag.source_group);
                let target = if collapse {
                    drag.row_height
                        + if rows.len() > 1 {
                            SIDEBAR_LIST_GAP
                        } else {
                            0.0
                        }
                } else {
                    0.0
                };
                drag.source_collapse.retarget(target);
                let removed = if self.reduced_motion {
                    target
                } else {
                    drag.source_collapse.current()
                };
                if let Some(gap) = drag.preview.as_mut() {
                    let origin = f32::from(
                        drag.origin.get().y
                            - self.sidebar_scroll.bounds().top()
                            - self.sidebar_scroll.offset().y,
                    );
                    if gap.group != drag.source_group && gap.top > origin {
                        gap.top -= removed - drag.collapsed_height;
                    }
                }
                drag.collapsed_height = removed;
                let destination = drag
                    .preview
                    .as_ref()
                    .filter(|gap| !returning && gap.group != drag.source_group)
                    .map(|gap| gap.group.clone());
                if let Some(group) = &destination {
                    drag.section_gaps
                        .entry(group.clone())
                        .or_insert_with(|| SidebarSessionSlide {
                            from: 0.0,
                            to: 0.0,
                            epoch: 0,
                            started: std::time::Instant::now(),
                        });
                }
                for (group, gap) in &mut drag.section_gaps {
                    gap.retarget(if destination.as_ref() == Some(group) {
                        drag.row_height
                            + if group == "pinned" && pinned_count == 0 {
                                0.0
                            } else {
                                SIDEBAR_LIST_GAP
                            }
                    } else {
                        0.0
                    });
                }
                break;
            }
        }

        let selected = self.state.read(cx).selected_chat.clone();
        // Re-checked at render so the chips drop the FRAME a popover opens,
        // not on the next modifier event â€” the jumps are suppressed under it.
        let jump_hints = self.jump_hints && !self.overlay_owns_keyboard(cx);
        let keymap = self.settings.keymap.clone();
        // Flat top-to-bottom slot across groups: the same order
        // `sidebar_visible_order` hands the jump shortcuts and cycling, so a
        // chip always names the key that opens its row.
        let mut slot = 0usize;
        let mut rendered = Vec::new();
        let mut moving_row = None;
        for (group, rows) in sections {
            let pinned_group = slot < pinned_count;
            let drag_group = if pinned_group {
                "pinned".to_owned()
            } else {
                group
                    .as_ref()
                    .map_or_else(|| "regular".to_owned(), |(key, _)| format!("regular:{key}"))
            };
            let mut rendered_rows = Vec::with_capacity(rows.len());
            for (group_index, row) in rows.into_iter().enumerate() {
                let ActiveChatRow {
                    status,
                    chat,
                    folder,
                    branch,
                    change_request,
                    group: _,
                } = row;
                let time_ago: SharedString =
                    format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), now).into();
                let is_selected = selected.as_deref() == Some(chat.id.as_str());
                let harness = self
                    .settings
                    .sidebar_show_harness
                    .then(|| chat.config.as_ref().map(|c| c.harness))
                    .flatten();
                let height = sidebar_row_height(
                    false,
                    self.settings.sidebar_show_project_label,
                    branch.is_some(),
                    change_request.is_some(),
                );
                // Only rows a jump slot can reach wear a chip; row 10 onward
                // keeps its time-ago.
                let jump_slot = slot.checked_sub(if self.pinned_open { 0 } else { pinned_count });
                let jump_label: Option<SharedString> = if jump_hints && let Some(slot) = jump_slot {
                    let combo = keymap.get(ShortcutId::JumpSession(slot));
                    (slot < JUMP_SLOTS && !combo.is_empty()).then(|| badge_combo(combo).into())
                } else {
                    None
                };
                let drag = (self.pinned_open || slot >= pinned_count)
                    .then(|| {
                        profile_key.as_ref().map(|profile_key| SidebarSessionDrag {
                            chat_id: chat.id.clone(),
                            visible_ids: visible_pinned_ids.clone(),
                            filter: filter.clone(),
                            profile_key: profile_key.clone(),
                        })
                    })
                    .flatten();
                slot += 1;
                let origin = self
                    .sidebar_session_transfer
                    .as_ref()
                    .filter(|drag| drag.payload.chat_id == chat.id)
                    .map(|drag| drag.origin.clone())
                    .or_else(|| {
                        self.sidebar_session_return
                            .as_ref()
                            .filter(|returning| returning.transfer.payload.chat_id == chat.id)
                            .map(|returning| returning.transfer.origin.clone())
                    });
                let is_moving = origin.is_some();
                let removed = self
                    .sidebar_session_transfer
                    .as_ref()
                    .or_else(|| {
                        self.sidebar_session_return
                            .as_ref()
                            .map(|state| &state.transfer)
                    })
                    .filter(|drag| drag.payload.chat_id == chat.id)
                    .map_or(0.0, |drag| drag.collapsed_height);
                let slot_height = height - removed;
                let element = self.render_chat_row(
                    chat.id.clone(),
                    transcript::single_line(
                        &chat.title.clone().unwrap_or_else(|| "New session".into()),
                    )
                    .into(),
                    time_ago,
                    folder.into(),
                    branch.map(SharedString::from),
                    change_request,
                    harness,
                    status,
                    is_selected,
                    false,
                    is_moving,
                    if is_moving { None } else { drag },
                    jump_label,
                    None,
                    theme,
                    cx,
                );
                // The source slot shrinks when its vacancy moves across sections.
                // Its zero-height anchor still tracks the live activity position.
                let element = if let Some(origin) = origin {
                    moving_row = Some((element, height));
                    div()
                        .child(div().h(crate::typography::ui_rems(slot_height.max(0.0))))
                        .on_children_prepainted(move |bounds, _, _| {
                            if let Some(bounds) = bounds.first() {
                                origin.set(bounds.origin);
                            }
                        })
                        .into_any_element()
                } else {
                    self.render_sidebar_gap_row(element, &chat.id, &drag_group, group_index)
                };
                // The hit region stays at the natural slot while its content slides;
                // preview movement must not move its own insertion thresholds.
                let target_group = drag_group.clone();
                let element = div()
                    .id(SharedString::from(format!("session-slot-{}", chat.id)))
                    .debug_selector({
                        let id = chat.id.clone();
                        move || format!("session-slot-{id}")
                    })
                    .h(crate::typography::ui_rems(slot_height.max(0.0)))
                    .mb(crate::typography::ui_rems(slot_height.min(0.0)))
                    .flex_none()
                    .child(element)
                    .on_drag_move::<SidebarSessionDrag>(cx.listener(
                        move |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, cx| {
                            if !event.bounds.contains(&event.event.position)
                                || !this.sidebar_scroll.bounds().contains(&event.event.position)
                            {
                                return;
                            }
                            let scroll_top = -f32::from(this.sidebar_scroll.offset().y);
                            let viewport_top = this.sidebar_scroll.bounds().top();
                            let scale = this.ui_scale();
                            let Some(drag) = this.sidebar_session_transfer.as_mut() else {
                                return;
                            };
                            let after = event.event.position.y >= event.bounds.center().y;
                            let index = group_index + usize::from(after);
                            let mut top = f32::from(
                                if after {
                                    event.bounds.bottom() + px(SIDEBAR_LIST_GAP * scale)
                                } else {
                                    event.bounds.top()
                                } - viewport_top,
                            ) + scroll_top;
                            if drag.source_group == target_group && index > drag.source_index {
                                top -= drag.row_height + SIDEBAR_LIST_GAP - drag.collapsed_height;
                            }
                            drag.preview = Some(SidebarSessionGap {
                                group: target_group.clone(),
                                index,
                                pinned: pinned_group,
                                top,
                            });
                            cx.notify();
                        },
                    ))
                    .into_any_element();
                rendered_rows.push((format!("c:{}", chat.id), slot_height, element));
            }

            let Some((key, label)) = group else {
                rendered.extend(rendered_rows);
                if !pinned_group {
                    let extra = self.sidebar_transfer_extra_gap(&drag_group);
                    if extra > 0.0 {
                        rendered.push((
                            format!("gap:{drag_group}"),
                            extra - SIDEBAR_LIST_GAP,
                            div()
                                .flex_none()
                                .h(crate::typography::ui_rems(
                                    (extra - SIDEBAR_LIST_GAP).max(0.0),
                                ))
                                .mb(crate::typography::ui_rems(
                                    (extra - SIDEBAR_LIST_GAP).min(0.0),
                                ))
                                .into_any_element(),
                        ));
                    }
                }
                continue;
            };
            let organization = match self.settings.sidebar_organization {
                SidebarOrganization::ByDevice => "device",
                SidebarOrganization::ByProject => "project",
                SidebarOrganization::InOneList => "list",
            };
            let collapse_key = format!("{organization}:{key}");
            let motion_key = format!("group:{collapse_key}");
            let collapsed = self.sidebar_collapsed_groups.contains(&collapse_key);
            let row_count = rendered_rows.len();
            let extra_gap = self.sidebar_transfer_extra_gap(&drag_group);
            let body_height = SIDEBAR_DISCLOSURE_BODY_INSET
                + Theme::SPACE_SM
                + extra_gap
                + rendered_rows
                    .iter()
                    .map(|(_, height, _)| *height)
                    .sum::<f32>()
                + SIDEBAR_LIST_GAP * row_count.saturating_sub(1) as f32;
            let guide_color = if theme.appearance.is_dark() {
                theme.text_faint.opacity(0.28)
            } else {
                theme.border_strong
            };
            let body = div()
                .relative()
                .w_full()
                .flex()
                .flex_col()
                .pt(crate::typography::ui_rems(SIDEBAR_DISCLOSURE_BODY_INSET))
                .pb(crate::typography::ui_rems(Theme::SPACE_SM))
                .pl(crate::typography::ui_rems(24.0))
                .gap(crate::typography::ui_rems(SIDEBAR_LIST_GAP))
                .when(row_count > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom(crate::typography::ui_rems(Theme::SPACE_SM))
                            .left(crate::typography::ui_rems(15.5))
                            .w(px(1.0))
                            .bg(guide_color),
                    )
                })
                .children(rendered_rows.into_iter().map(|(_, _, row)| row))
                .when(extra_gap > 0.0, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .h(crate::typography::ui_rems(
                                (extra_gap - SIDEBAR_LIST_GAP).max(0.0),
                            ))
                            .mb(crate::typography::ui_rems(
                                (extra_gap - SIDEBAR_LIST_GAP).min(0.0),
                            )),
                    )
                });
            let visible_label: SharedString = label.into();
            let chevron = self.sidebar_disclosure_chevron(&motion_key, !collapsed, theme);
            let toggle_key = collapse_key.clone();
            let toggle_motion_key = motion_key.clone();
            let group_icon = match self.settings.sidebar_organization {
                SidebarOrganization::ByProject => Some(icons::FOLDER),
                SidebarOrganization::ByDevice => Some(icons::MONITOR),
                SidebarOrganization::InOneList => None,
            };
            let header = sidebar_disclosure_header(
                theme,
                visible_label,
                Some(row_count),
                chevron,
                group_icon,
            )
            .id(SharedString::from(format!("sidebar-group-{collapse_key}")))
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = !this.sidebar_collapsed_groups.contains(&toggle_key);
                this.begin_sidebar_disclosure_motion(
                    &toggle_motion_key,
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                if was_open {
                    this.sidebar_collapsed_groups.insert(toggle_key.clone());
                } else {
                    this.sidebar_collapsed_groups.remove(&toggle_key);
                }
                cx.notify();
            }));
            let body = self.render_sidebar_disclosure_body(
                &motion_key,
                !collapsed,
                body_height,
                body.into_any_element(),
            );
            let height =
                SIDEBAR_DISCLOSURE_SECTION_HEIGHT + if collapsed { 0.0 } else { body_height };
            let element = div()
                .w_full()
                .flex()
                .flex_col()
                .pt(crate::typography::ui_rems(SIDEBAR_SECTION_GAP))
                .child(header)
                .child(body)
                .into_any_element();
            rendered.push((format!("g:{collapse_key}"), height, element));
        }
        SidebarSessionRows {
            regular_count: base_ids.len().saturating_sub(pinned_count),
            rows: rendered,
            pinned_count,
            moving_row,
        }
    }

    pub(in crate::shell) fn render_pinned_section(
        &mut self,
        items: Vec<AnyElement>,
        body_height: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.pinned_open;
        let label = "Pinned".into();
        let chevron = self.sidebar_disclosure_chevron("pinned", open, theme);
        let header = sidebar_disclosure_header(theme, label, Some(items.len()), chevron, None)
            .id("pinned-toggle")
            .debug_selector(|| "pinned-toggle".into())
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, _| {
                    if !event.bounds.contains(&event.event.position)
                        || !this.sidebar_scroll.bounds().contains(&event.event.position)
                    {
                        return;
                    }
                    let top = f32::from(event.bounds.bottom() - this.sidebar_scroll.bounds().top())
                        + SIDEBAR_DISCLOSURE_BODY_INSET
                        - f32::from(this.sidebar_scroll.offset().y);
                    if let Some(drag) = this.sidebar_session_transfer.as_mut() {
                        drag.preview = Some(SidebarSessionGap {
                            group: "pinned".into(),
                            index: 0,
                            pinned: true,
                            top,
                        });
                    }
                },
            ))
            .on_drop::<SidebarSessionDrag>(cx.listener(|this, payload, _, cx| {
                this.finish_sidebar_session_transfer(payload, SidebarSessionDrop::Pinned(0), cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = this.pinned_open;
                this.cancel_pinned_session_drag(cx);
                this.begin_sidebar_disclosure_motion(
                    "pinned",
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                this.pinned_open = !was_open;
                // The disclosure owns this movement; avoid a second FLIP
                // animation on the regular sessions below it.
                this.sidebar_prev_order.clear();
                this.sidebar_resort.clear();
                this.sidebar_new_keys.clear();
                cx.notify();
            }));
        let content = div()
            .pt(crate::typography::ui_rems(SIDEBAR_DISCLOSURE_BODY_INSET))
            .child(Self::render_pinned_session_group(
                items,
                self.sidebar_transfer_extra_gap("pinned"),
                cx,
            ))
            .into_any_element();
        let body = self.render_sidebar_disclosure_body("pinned", open, body_height, content);
        div()
            .id("sidebar-pinned-section")
            .debug_selector(|| "sidebar-pinned-section".into())
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub(in crate::shell) fn render_sessions_section(
        &mut self,
        content: AnyElement,
        body_height: f32,
        count: usize,
        follows_pinned: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.settings.sidebar_organization != SidebarOrganization::InOneList {
            return content;
        }
        let open = self.sessions_open;
        let label = "Sessions".into();
        let chevron = self.sidebar_disclosure_chevron("sessions", open, theme);
        let header = sidebar_disclosure_header(theme, label, Some(count), chevron, None)
            .id("sessions-toggle")
            .debug_selector(|| "sessions-toggle".into())
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, _| {
                    if event.bounds.contains(&event.event.position) {
                        let top = f32::from(
                            event.bounds.bottom()
                                - this.sidebar_scroll.bounds().top()
                                - this.sidebar_scroll.offset().y,
                        ) + SIDEBAR_DISCLOSURE_BODY_INSET;
                        if let Some(drag) = this.sidebar_session_transfer.as_mut() {
                            drag.preview = Some(SidebarSessionGap {
                                group: "regular".into(),
                                index: 0,
                                pinned: false,
                                top,
                            });
                        }
                    }
                },
            ))
            .on_drop::<SidebarSessionDrag>(cx.listener(|this, payload, _, cx| {
                this.finish_sidebar_session_transfer(payload, SidebarSessionDrop::Regular, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.cancel_sidebar_session_transfer(cx);
                let was_open = this.sessions_open;
                this.begin_sidebar_disclosure_motion(
                    "sessions",
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                this.sessions_open = !was_open;
                this.sidebar_prev_order.clear();
                this.sidebar_resort.clear();
                this.sidebar_new_keys.clear();
                cx.notify();
            }));
        let content = div()
            .pt(crate::typography::ui_rems(SIDEBAR_DISCLOSURE_BODY_INSET))
            .child(content)
            .into_any_element();
        let body = self.render_sidebar_disclosure_body("sessions", open, body_height, content);
        div()
            .id("sidebar-sessions-section")
            .debug_selector(|| "sidebar-sessions-section".into())
            .flex()
            .flex_col()
            .pt(crate::typography::ui_rems(if follows_pinned {
                SIDEBAR_SECTION_GAP
            } else {
                0.0
            }))
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Archived sessions share active-row data and layout, with a restore action.
    /// The shelf starts with ten sessions and pages by 25.
    pub(in crate::shell) fn render_archived_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        const INITIAL: usize = 10;
        const PAGE: usize = 25;
        let now = Utc::now();
        let filter = self.settings.space_filter.clone();
        let mut rows: Vec<zeron_proto::Chat> = {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .filter(|c| c.archived)
                .filter(|chat| match &filter {
                    Some(space_id) => chat.space_id.as_deref() == Some(space_id.as_str()),
                    None => true,
                })
                .cloned()
                .collect()
        };
        rows.sort_by(|left, right| compare_sidebar_chats(self.settings.sidebar_sort, left, right));
        if rows.is_empty() {
            return None;
        }
        let rows: Vec<_> = {
            let state = self.state.read(cx);
            rows.into_iter()
                .map(|chat| {
                    self.sidebar_chat_data(state.display_status_for(&chat, now), chat, state)
                })
                .collect()
        };
        let total = rows.len();
        let open = self.archived_open;
        let shown = self.archived_shown.max(INITIAL);
        let visible_count = total.min(shown);
        let has_more = total > shown;
        let more_height = 36.0;
        let body_height = SIDEBAR_DISCLOSURE_BODY_INSET
            + rows
                .iter()
                .take(shown)
                .map(|row| {
                    sidebar_row_height(
                        false,
                        self.settings.sidebar_show_project_label,
                        row.branch.is_some(),
                        row.change_request.is_some(),
                    )
                })
                .sum::<f32>()
            + visible_count.saturating_sub(1) as f32 * SIDEBAR_LIST_GAP
            + if has_more {
                more_height + SIDEBAR_LIST_GAP
            } else {
                0.0
            };
        // Match Pinned: a muted label with a right-aligned disclosure chevron.
        // The count only shows while collapsed.
        let label: SharedString = "Archived".into();
        let chevron = self.sidebar_disclosure_chevron("archived", open, theme);
        let header = sidebar_disclosure_header(theme, label, Some(total), chevron, None)
            .id("archived-toggle")
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = this.archived_open;
                this.begin_sidebar_disclosure_motion(
                    "archived",
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                this.archived_open = !was_open;
                this.archived_shown = INITIAL;
                cx.notify();
            }));
        let section = div().flex().flex_col().child(header);
        let body = {
            let selected = self.state.read(cx).selected_chat.clone();
            let mut list = div()
                .flex()
                .flex_col()
                .pt(crate::typography::ui_rems(SIDEBAR_DISCLOSURE_BODY_INSET))
                .gap(crate::typography::ui_rems(SIDEBAR_LIST_GAP));
            for row in rows.into_iter().take(shown) {
                let chat = row.chat;
                let is_selected = selected.as_deref() == Some(chat.id.as_str());
                let harness = self
                    .settings
                    .sidebar_show_harness
                    .then(|| chat.config.as_ref().map(|c| c.harness))
                    .flatten();
                list = list.child(
                    self.render_chat_row(
                        chat.id.clone(),
                        transcript::single_line(
                            &chat.title.clone().unwrap_or_else(|| "New session".into()),
                        )
                        .into(),
                        format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), now)
                            .into(),
                        row.folder.into(),
                        row.branch.map(SharedString::from),
                        row.change_request,
                        harness,
                        row.status,
                        is_selected,
                        true,
                        false,
                        None,
                        None,
                        None,
                        theme,
                        cx,
                    ),
                );
            }
            let mut body = div().w_full().flex().flex_col().child(list);
            if has_more {
                let remaining = (total - shown).min(PAGE);
                body = body.child(
                    div()
                        .id("archived-more")
                        // Sits outside the rows' gapped column â€” match the
                        // list's 2px row gap or it fuses with the last row.
                        .mt(crate::typography::ui_rems(2.0))
                        .h(crate::typography::ui_rems(more_height))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(crate::typography::ui_rems(10.0))
                        .px(crate::typography::ui_rems(Theme::SPACE_SM))
                        .rounded(crate::typography::ui_rems(6.0))
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text_muted.opacity(0.55))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.archived_shown = this.archived_shown.max(INITIAL) + PAGE;
                            cx.notify();
                        }))
                        .child(
                            crate::icons::icon(crate::icons::PLUS)
                                .size(crate::typography::ui_rems(14.0))
                                .flex_none(),
                        )
                        .child(SharedString::from(format!("Show {remaining} more"))),
                );
            }
            body.into_any_element()
        };
        let body = self.render_sidebar_disclosure_body("archived", open, body_height, body);
        // The active list already provides the spacing above Archived.
        let section = section.child(body);
        Some(section.into_any_element())
    }
}
