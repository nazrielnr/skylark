//! Queue panel rendering, row controls, and attachment previews.

use super::*;

impl Composer {
    /// The queue panel, or `None` when nothing is waiting. Like the composer,
    /// it is one frosted surface; rows use spacing and hover wash rather than
    /// nesting raised cards inside it.
    pub(crate) fn render_queue_panel(
        &mut self,
        show_latest_shortcut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // A drop outside the panel ends GPUI's active drag without invoking our
        // `on_drop`. Never leave the source row replaced by a stale gap.
        if self.queue_drag.is_some() && !cx.has_active_drag() {
            self.queue_drag = None;
        }
        let (items, chat_id, host_supports_actions) = {
            let state = self.state.read(cx);
            let chat_id = state.selected_chat.clone()?;
            let host_supports_actions = state.chat_host_supports(
                &chat_id,
                skylark_proto::capabilities::MESSAGE_QUEUE_ACTIONS_V1,
            );
            (state.queue.clone(), chat_id, host_supports_actions)
        };
        self.prepare_queue_previews(&items, window, cx);
        if items.is_empty() {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let count = items.len();
        let drag = self
            .queue_drag
            .as_ref()
            .map(|d| (d.from, d.over, d.prev_over, d.epoch));
        let editing = self.editing_queued.clone();

        let list_chat = chat_id.clone();
        let drop_chat = chat_id.clone();
        let rows = queue_rows(
            &self.queue_scroll,
            window.viewport_size().height * 0.3,
            items.iter().enumerate().map(|(ix, item)| {
                self.queue_row(
                    &chat_id,
                    ix,
                    count,
                    item,
                    drag,
                    &editing,
                    host_supports_actions,
                    show_latest_shortcut,
                    &theme,
                    cx,
                )
            }),
        );

        let panel = queue_panel_surface(&theme)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            // The complete glass surface is a drop target, including its
            // padding.
            .on_drag_move::<QueueDragPayload>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<QueueDragPayload>, _, cx| {
                    let payload = event.drag(cx);
                    if payload.chat != list_chat {
                        return;
                    }
                    let from = payload.from;
                    let rel_y = f32::from(event.event.position.y)
                        - f32::from(event.bounds.top())
                        - f32::from(this.queue_scroll.offset().y);
                    let over = queue_drop_index(rel_y, count);
                    this.update_queue_drag_over(from, over, cx);
                },
            ))
            .on_drop::<QueueDragPayload>(cx.listener(
                move |this, payload: &QueueDragPayload, _, cx| {
                    if payload.chat != drop_chat {
                        this.queue_drag = None;
                        cx.notify();
                        return;
                    }
                    let to = this
                        .queue_drag
                        .as_ref()
                        .map(|d| d.over)
                        .unwrap_or(payload.from);
                    this.queue_drag = None;
                    this.move_queued(payload.from, to, cx);
                },
            ))
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_queue_drag(cx)),
            )
            .child(rows);
        Some(crate::frost::frosted(PANEL_RADIUS, crate::frost::MENU_BLUR, panel).into_any_element())
    }

    /// One queued message: a quiet queue marker, the text, edit controls, and
    /// one explicit primary delivery action.
    #[allow(clippy::too_many_arguments)]
    fn queue_row(
        &self,
        chat_id: &str,
        ix: usize,
        count: usize,
        item: &QueuedMessage,
        drag: Option<(usize, usize, usize, usize)>,
        editing: &Option<String>,
        host_supports_actions: bool,
        show_latest_shortcut: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = SharedString::from(format!("queue-{}", item.id));
        let being_edited = editing.as_deref() == Some(item.id.as_str());
        let being_removed = self.queue_removing.contains(&item.id);
        let delivery_blocked = item.delivery_gate.is_some();
        let interaction_blocked = delivery_blocked || being_removed;
        let text = match &item.delivery_gate {
            Some(QueueDeliveryGate::Editing {
                owner_device_id, ..
            }) if !being_edited => SharedString::from(format!("Editing on {owner_device_id}")),
            Some(QueueDeliveryGate::ReviewRequired { .. }) if !being_edited => {
                SharedString::from("Needs review")
            }
            _ => one_line(&queue_visible_text(&item.text, &item.attachments)),
        };

        let edit_id = item.id.clone();
        let edit = self.queue_action(
            &key,
            "edit",
            "Edit",
            icons::PEN,
            !being_removed,
            theme,
            cx.listener(move |this, _, _, cx| {
                this.begin_queue_edit(edit_id.clone(), cx);
            }),
        );
        let drop_id = item.id.clone();
        let discard = self.queue_action(
            &key,
            "drop",
            if being_removed {
                "Removing…"
            } else {
                "Remove"
            },
            icons::TRASH_BIN_MINIMALISTIC,
            !being_removed,
            theme,
            cx.listener(move |this, _, _, cx| {
                this.remove_queued(drop_id.clone(), cx);
            }),
        );
        let resolved_primary =
            available_queue_primary_action(interaction_blocked, host_supports_actions);
        let primary_action = resolved_primary.unwrap_or(QueuePrimaryAction::SendNow);
        let primary_id = item.id.clone();
        let primary = self.queue_primary_action_button(
            &key,
            primary_action,
            resolved_primary.is_some(),
            queue_latest_shortcut_visible(
                ix,
                count,
                show_latest_shortcut,
                resolved_primary.is_some() && !being_removed,
            ),
            theme,
            cx.listener(move |this, _, _, cx| {
                this.activate_queued_primary(primary_id.clone(), primary_action, cx);
            }),
        );
        let save = self.queue_action(
            &key,
            "save",
            "Save to queue",
            icons::QUEUE_CHECK,
            !self.queue_edit_finishing,
            theme,
            cx.listener(|this, _, _, cx| {
                this.commit_queue_edit(cx);
            }),
        );
        let cancel = self.queue_action(
            &key,
            "cancel",
            "Cancel",
            icons::QUEUE_CLOSE,
            !self.queue_edit_finishing,
            theme,
            cx.listener(|this, _, _, cx| {
                this.cancel_queue_edit(cx);
            }),
        );

        let drag_chat = chat_id.to_string();
        let queue_marker = div()
            .id(SharedString::from(format!("{key}-drag")))
            .w(px(14.0))
            .h(px(22.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .when(interaction_blocked, |el| {
                el.cursor(gpui::CursorStyle::Arrow).opacity(0.35)
            })
            .child(
                icon(icons::QUEUE_DRAG_HANDLE)
                    .size(px(QUEUE_ICON_SIZE))
                    .text_color(theme.text_muted.opacity(0.5)),
            );

        let row = div()
            .id(SharedString::from(format!("{key}-row")))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .px(px(ROW_PAD_X))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(if self.queue_preview_limit() == 1 {
                4.0
            } else {
                8.0
            }))
            .rounded(px(ROW_RADIUS))
            .when(being_edited, |el| el.bg(crate::theme::ink(0.06)))
            .when(!being_edited && !being_removed, |el| {
                el.hover(|s| s.bg(crate::theme::ink(0.04)))
            })
            .when(being_removed, |el| el.opacity(0.55))
            .cursor(gpui::CursorStyle::Arrow)
            // The marker hints that the row belongs to the queue, while the
            // proven full-row drag hitbox keeps reordering easy. Editing
            // disables it so selection cannot become a reorder gesture.
            .when(!being_edited && !interaction_blocked, |el| {
                el.on_drag(
                    QueueDragPayload {
                        chat: drag_chat,
                        from: ix,
                    },
                    move |_payload, _point, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| QueueGhost)
                    },
                )
            })
            .when(!being_edited, |el| el.child(queue_marker))
            // Preserve text alignment while removing the disabled drag glyph
            // from the editing state.
            .when(being_edited, |el| el.child(div().w(px(14.0)).flex_none()))
            .when(!being_edited, |el| {
                let labels = queue_attachment_labels(&item.text, &item.attachments);
                let summary = if labels.len() > 1 {
                    format!("{} attachments · {}", labels.len(), labels.join(" · "))
                } else {
                    labels.join(" · ")
                };
                let only_images = text.as_ref() == crate::attachments::ATTACHMENT_ONLY_TEXT;
                let title = if only_images {
                    summary.clone().into()
                } else {
                    text
                };
                let mut content = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(QUEUE_TEXT_SIZE))
                            .line_height(px(16.0))
                            .text_color(theme.text.opacity(0.9))
                            .child(title),
                    );
                if !labels.is_empty() && !only_images {
                    content = content.child(
                        div()
                            .truncate()
                            .text_size(px(11.0))
                            .line_height(px(13.0))
                            .text_color(theme.text_muted)
                            .child(summary),
                    );
                }
                el.children(
                    item.attachments
                        .iter()
                        .take(self.queue_preview_limit())
                        .enumerate()
                        .map(|(index, path)| self.queue_thumbnail(&key, index, path, cx)),
                )
                .when(item.attachments.len() > self.queue_preview_limit(), |el| {
                    let remaining = item.attachments.len() - self.queue_preview_limit();
                    el.child(
                        div()
                            .id(SharedString::from(format!("{key}-more-attachments")))
                            .w(px(28.0))
                            .h(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .bg(crate::theme::ink(0.06))
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .aria_label(format!(
                                "{remaining} more attachments; edit message to view all"
                            ))
                            .child(format!("+{remaining}")),
                    )
                })
                .child(content)
            })
            .when(being_edited, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(QUEUE_TEXT_SIZE))
                        .text_color(theme.text_muted)
                        .child(if self.queue_edit_finishing {
                            "Saving…"
                        } else {
                            "Editing in composer"
                        }),
                )
            })
            .when(being_edited, |el| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(3.0))
                        .child(save)
                        .child(cancel),
                )
            })
            .when(!being_edited, |el| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(3.0))
                        .child(discard)
                        .child(edit)
                        .child(primary),
                )
            });

        let Some((from, over, prev_over, epoch)) = drag else {
            return row.into_any_element();
        };
        let (start, target) = queue_drag_offsets(ix, from, prev_over, over);
        if cx.reduce_motion() {
            return div()
                .relative()
                .top(px(target))
                .child(row)
                .into_any_element();
        }
        div()
            .child(row)
            .with_animation(
                ("queue-row-slide", (ix as u64) | ((epoch as u64) << 32)),
                TAB_SLIDE.animation(),
                move |el, t| el.relative().top(px(motion::lerp(start, target, t))),
            )
            .into_any_element()
    }

    pub(crate) fn release_queue_previews(&mut self, cx: &mut gpui::App) {
        for (_, preview) in self.queue_previews.drain() {
            if let Some(image) = preview.image {
                gpui::ImageSource::Image(image.image).evict(None, cx);
            }
        }
    }

    fn prepare_queue_previews(
        &mut self,
        items: &[QueuedMessage],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::attachments;
        let state = self.state.read(cx);
        let device = state
            .selected_chat_row()
            .map(|chat| chat.device_id.clone())
            .unwrap_or_default();
        let engine = state.engine().cloned();
        let target =
            (state.local_device_id.as_deref() != Some(device.as_str())).then(|| device.clone());
        let visible = visible_queue_rows(
            f32::from(self.queue_scroll.offset().y),
            f32::from(window.viewport_size().height) * 0.3,
            items.len(),
        );
        let keys: std::collections::HashSet<_> = items[visible]
            .iter()
            .flat_map(|item| item.attachments.iter().take(self.queue_preview_limit()))
            .map(|path| (device.clone(), path.clone()))
            .take(64)
            .collect();
        self.queue_previews.retain(|key, preview| {
            if keys.contains(key) {
                return true;
            }
            if let Some(image) = &preview.image {
                gpui::ImageSource::Image(image.image.clone()).evict(Some(window), cx);
            }
            false
        });
        let Some(engine) = engine else { return };
        for key in keys {
            if self.queue_previews.contains_key(&key) {
                continue;
            }
            let engine = engine.clone();
            let target = target.clone();
            let task_key = key.clone();
            let task = cx.spawn(async move |this, cx| {
                let _permit = preview_load_gate().lock().await;
                let source = match attachments::attachment_snapshot(&task_key.0, &task_key.1) {
                    attachments::AttachmentSnapshot::Loaded(image) => {
                        Some(attachments::LoadedAttachmentImage {
                            name: image.name.to_string(),
                            image: image.image,
                        })
                    }
                    _ => {
                        attachments::read_attachment_image(
                            &engine,
                            cx.background_executor(),
                            target.as_deref(),
                            &task_key.1,
                            None,
                        )
                        .await
                    }
                };
                let image = if let Some(source) = source {
                    cx.background_executor()
                        .spawn(async move {
                            attachments::queue_thumbnail_image(&source.image).map(|image| {
                                attachments::CachedAttachmentImage {
                                    name: source.name.into(),
                                    image,
                                }
                            })
                        })
                        .await
                } else {
                    None
                };
                this.update(cx, |this, cx| {
                    if let Some(preview) = this.queue_previews.get_mut(&task_key) {
                        preview.image = image;
                        preview.finished = true;
                    }
                    cx.notify();
                })
                .ok();
            });
            self.queue_previews.insert(
                key,
                QueuePreview {
                    image: None,
                    finished: false,
                    _task: task,
                },
            );
        }
    }

    fn load_queue_full_preview(&mut self, device: String, path: String, cx: &mut Context<Self>) {
        use crate::attachments;
        if let attachments::AttachmentSnapshot::Loaded(image) =
            attachments::attachment_snapshot(&device, &path)
        {
            self.queue_full_preview = None;
            self.show_queue_image(
                crate::attachments::PreviewImage::new(image.name, image.image),
                cx,
            );
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let target = (state.local_device_id.as_deref() != Some(device.as_str())).then_some(device);
        let chat = state.selected_chat.clone();
        self.queue_full_preview = Some(cx.spawn(async move |this, cx| {
            let image = attachments::read_attachment_image(
                &engine,
                cx.background_executor(),
                target.as_deref(),
                &path,
                None,
            )
            .await;
            this.update(cx, |this, cx| {
                this.queue_full_preview = None;
                if this.state.read(cx).selected_chat != chat {
                    return;
                }
                if let Some(image) = image {
                    this.show_queue_image(
                        crate::attachments::PreviewImage::new(image.name, image.image),
                        cx,
                    );
                } else {
                    this.show_appshot_error(
                        "Could not load the image. Try opening it again.".into(),
                        cx,
                    );
                }
            })
            .ok();
        }));
    }

    fn queue_thumbnail(
        &self,
        key: &SharedString,
        index: usize,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::attachments;
        let device = self
            .state
            .read(cx)
            .selected_chat_row()
            .map(|chat| chat.device_id.clone())
            .unwrap_or_default();
        let cache_key = (device.clone(), path.to_string());
        let failed = self
            .queue_previews
            .get(&cache_key)
            .is_some_and(|preview| preview.finished && preview.image.is_none());
        let snapshot = self
            .queue_previews
            .get(&cache_key)
            .and_then(|preview| preview.image.clone());
        let frame = div()
            .id(SharedString::from(format!("{key}-image-{index}")))
            .w(px(40.0))
            .h(px(28.0))
            .flex_none()
            .rounded(px(5.0))
            .border_1()
            .border_color(crate::theme::hairline(0.1))
            .bg(crate::theme::ink(0.035))
            .overflow_hidden();
        match snapshot {
            Some(image) => {
                let label = image.name.clone();
                let path = path.to_owned();
                let accent = Theme::of(cx).accent;
                frame
                    .role(gpui::Role::Button)
                    .aria_label(format!("Preview {}", label))
                    .tab_index(0)
                    .focus_visible(move |style| style.border_color(accent))
                    .hover(move |style| style.border_color(accent))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.load_queue_full_preview(device.clone(), path.clone(), cx);
                    }))
                    .child(
                        gpui::img(image.image)
                            .w(px(38.0))
                            .h(px(26.0))
                            .rounded(px(4.0))
                            .object_fit(gpui::ObjectFit::Cover),
                    )
                    .into_any_element()
            }
            _ => frame
                .when(failed, |frame| {
                    let accent = Theme::of(cx).accent;
                    frame
                        .role(gpui::Role::Button)
                        .aria_label("Open attachment preview")
                        .tab_index(0)
                        .focus_visible(move |style| style.border_color(accent))
                        .cursor_pointer()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.queue_previews.remove(&cache_key);
                            this.load_queue_full_preview(
                                cache_key.0.clone(),
                                cache_key.1.clone(),
                                cx,
                            );
                            cx.notify();
                        }))
                })
                .flex()
                .items_center()
                .justify_center()
                .child(icon(icons::QUEUE_PAPERCLIP).size(px(14.0)))
                .into_any_element(),
        }
    }

    /// A permanently-visible trailing glyph button. The queue reference keeps
    /// edit and remove present instead of revealing them only on hover.
    fn queue_action(
        &self,
        key: &SharedString,
        slot: &str,
        label: &'static str,
        glyph: &'static str,
        enabled: bool,
        theme: &Theme,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> AnyElement {
        let own = SharedString::from(format!("{key}-{slot}-grp"));
        let accent = theme.accent;
        div()
            .id(SharedString::from(format!("{key}-{slot}")))
            .group(own.clone())
            .role(gpui::Role::Button)
            .aria_label(label)
            .size(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .opacity(0.72)
            .when(enabled, |el| {
                el.cursor_pointer()
                    .hover(|s| s.opacity(1.0).bg(crate::theme::ink(0.07)))
                    .tab_index(0)
                    .focus_visible(move |s| s.bg(accent.opacity(0.18)).text_color(accent))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        on_click(event, window, cx);
                    })
            })
            .when(!enabled, |el| {
                el.cursor(gpui::CursorStyle::Arrow).opacity(0.45)
            })
            .tooltip(move |_, cx| {
                cx.new(|_| QueueActionTooltip {
                    label: label.into(),
                })
                .into()
            })
            .tooltip_show_delay(std::time::Duration::from_millis(350))
            .child(
                icon(glyph)
                    .size(px(QUEUE_ICON_SIZE))
                    .text_color(theme.text_muted.opacity(0.8))
                    .group_hover(own, |s| s.text_color(theme.text)),
            )
            .into_any_element()
    }

    /// Send now interrupts the current response before delivering the row.
    fn queue_primary_action_button(
        &self,
        key: &SharedString,
        action: QueuePrimaryAction,
        enabled: bool,
        show_shortcut: bool,
        theme: &Theme,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> AnyElement {
        let tooltip = if enabled {
            action.tooltip()
        } else {
            "Waiting for provider capabilities"
        };
        let accent = theme.accent;
        let compact = self.queue_preview_limit() == 1;
        div()
            .id(SharedString::from(format!("{key}-primary")))
            .role(gpui::Role::Button)
            .aria_label(tooltip)
            // Both labels occupy the same slot; modifier previews never move
            // the message text, thumbnails, or adjacent actions.
            .w(px(if compact { 28.0 } else { 72.0 }))
            .h(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .text_size(px(11.5))
            .text_color(theme.text_muted)
            .when(enabled, |el| {
                el.cursor_pointer()
                    .tab_index(0)
                    .hover(|s| s.bg(crate::theme::ink(0.07)).text_color(theme.text))
                    .focus_visible(move |s| s.bg(accent.opacity(0.18)).text_color(accent))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        on_click(event, window, cx);
                    })
            })
            .when(!enabled, |el| el.opacity(0.45))
            .tooltip(move |_, cx| {
                cx.new(|_| QueueActionTooltip {
                    label: tooltip.into(),
                })
                .into()
            })
            .tooltip_show_delay(std::time::Duration::from_millis(350))
            .child(if show_shortcut {
                div()
                    .child(if compact {
                        if cfg!(target_os = "macos") {
                            "⌘↵"
                        } else {
                            "⌃↵"
                        }
                    } else {
                        modifier_send_label(cfg!(target_os = "macos"))
                    })
                    .into_any_element()
            } else if compact {
                icon(icons::QUEUE_SEND)
                    .size(px(QUEUE_ICON_SIZE))
                    .text_color(theme.text_muted)
                    .into_any_element()
            } else {
                div().child("Send now").into_any_element()
            })
            .into_any_element()
    }
}
