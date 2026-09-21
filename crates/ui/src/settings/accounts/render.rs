use super::*;

impl AccountsPage {
    // ---- render pieces ----

    /// One usage window (zeron settings.agents.tsx `UsageMeter`): label ·
    /// 5px rounded-full bar (indigo → amber ≥80% → red ≥95%) · "NN% used" ·
    /// quiet reset time.
    fn render_usage_meter(
        &self,
        window: &zeron_proto::AgentUsageWindow,
        theme: &Theme,
        now: DateTime<Utc>,
    ) -> AnyElement {
        let fraction = window.used_fraction.clamp(0.0, 1.0);
        let level = usage_level(fraction);
        let fill = usage_color(level, theme).opacity(match level {
            UsageLevel::Normal => 0.8,
            _ => 0.85,
        });
        let reset = format_reset(window.resets_at, now);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .text_size(crate::typography::ui_rems(11.5))
            .text_color(theme.text_muted.opacity(0.7))
            .child(
                div()
                    .w(px(48.0))
                    .flex_none()
                    .truncate()
                    .child(SharedString::from(window.label.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(56.0))
                    .max_w(px(230.0))
                    .h(px(5.0))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(crate::theme::ink(0.07))
                    .when(fraction > 0.0, |el| {
                        el.child(
                            div()
                                .h_full()
                                // A 1.5% floor keeps tiny non-zero usage
                                // visible (zeron `max(used, 1.5)%`).
                                .w(gpui::relative(fraction.max(0.015)))
                                .rounded_full()
                                .bg(fill),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(64.0))
                    .flex_none()
                    .text_right()
                    .child(SharedString::from(format!(
                        "{}% used",
                        (fraction * 100.0).round() as u32
                    ))),
            )
            .when_some(reset, |el, reset| {
                el.child(
                    div()
                        .flex_none()
                        .truncate()
                        .text_color(theme.text_muted.opacity(0.45))
                        .child(SharedString::from(reset)),
                )
            })
            .into_any_element()
    }

    /// One account row (zeron settings.agents.tsx `AccountRow`): initial
    /// avatar, email + usage meters left; badges over the Switch/Forget
    /// actions right-anchored.
    fn render_account_row(
        &self,
        account: &AgentAccount,
        ix: usize,
        first: bool,
        theme: &Theme,
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_busy = self.busy_account.as_deref() == Some(account.id.as_str());
        let email: SharedString = account
            .email
            .clone()
            .or_else(|| account.display_name.clone())
            .unwrap_or_else(|| "Unknown account".into())
            .into();
        let initial: SharedString = email
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into())
            .into();
        let switch_account = account.clone();
        let forget_account = account.clone();

        let badges = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .when(account.active, |el| {
                el.child(widgets::badge_active(theme, "Active"))
            })
            .when_some(account.plan_label.clone(), |el, plan| {
                el.child(widgets::badge(theme, plan))
            });

        // Actions only on INACTIVE accounts (zeron `{!account.active && …}`):
        // an icon-only Forget (trash, hover → foreground) then Switch, which
        // reads "Switching…" while the activate round-trips.
        let actions: Option<gpui::Div> = (!account.active).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .id(("account-forget", ix))
                        .rounded(px(6.0))
                        .px(px(6.0))
                        .py(px(4.0))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .when(is_busy, |el| el.opacity(0.5))
                        .hover(|s| s.bg(crate::theme::ink(0.06)).text_color(theme.text))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_action(methods::FORGET_AGENT_ACCOUNT, &forget_account, cx);
                        }))
                        .child(
                            crate::icons::icon(crate::icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                        ),
                )
                .when(account.switchable, |el| {
                    el.child(
                        crate::popover::btn_primary(
                            theme,
                            if is_busy { "Switching…" } else { "Switch" },
                        )
                        .id(("account-switch", ix))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .text_size(crate::typography::ui_rems(11.5))
                        .when(is_busy, |el| el.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_action(
                                methods::ACTIVATE_AGENT_ACCOUNT,
                                &switch_account,
                                cx,
                            );
                        })),
                    )
                })
        });

        div()
            .px(px(20.0))
            .py(px(14.0))
            .when(!first, |el| el.border_t_1().border_color(theme.border))
            .flex()
            .flex_row()
            .items_stretch()
            .gap(px(12.0))
            .child(
                // Initial avatar: size-8 rounded-full border bg-white/[0.03].
                div()
                    .flex_none()
                    .self_center()
                    .size(px(32.0))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .bg(crate::theme::ink(0.03))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(initial),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, email))
                    .map(|el| {
                        // Meters XOR the quiet fallback line — never both
                        // (zeron: `usage ? meters : "Usage unavailable"…`).
                        if account.usage_windows.is_empty() {
                            el.child(
                                div()
                                    .mt(px(6.0))
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_muted.opacity(0.6))
                                    .child(SharedString::from(if account.switchable {
                                        "Usage unavailable"
                                    } else {
                                        "Credentials unavailable"
                                    })),
                            )
                        } else {
                            el.child(
                                div().mt(px(6.0)).flex().flex_col().gap(px(4.0)).children(
                                    account
                                        .usage_windows
                                        .iter()
                                        .map(|w| self.render_usage_meter(w, theme, now)),
                                ),
                            )
                        }
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_end()
                    .justify_between()
                    .gap(px(8.0))
                    .child(badges)
                    .children(actions),
            )
            .into_any_element()
    }

    fn render_login_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let red_text = theme.danger_muted.opacity(0.9); // red-300
        let login = self.login.as_ref()?;
        let title = login.title();
        let url_link =
            |id: &'static str, label: &'static str, url: &str, cx: &mut Context<Self>| {
                let open_url = url.to_string();
                // "Reopen the …" text link (zeron: `text-[12px]
                // text-muted-foreground/60 hover:underline`).
                div()
                    .id(id)
                    .mt(px(6.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .truncate()
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.open_url(&open_url);
                    }))
                    .child(SharedString::from(label))
            };
        let body: AnyElement = match login {
            LoginFlow::Starting { .. } => div()
                .mt(px(8.0))
                .child(popover::skeleton_rows(
                    "login-starting",
                    &theme,
                    2,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            LoginFlow::PasteCode {
                start,
                submitting,
                error,
                ..
            } => {
                let submitting = *submitting;
                div()
                    .flex()
                    .flex_col()
                    .child(div().mt(px(8.0)).child(popover::dialog_body(
                        &theme,
                        "A browser window opened. Sign in to the account you want to add, \
                         approve access, then paste the code Anthropic shows you below. Your \
                         current login is untouched until you switch.",
                    )))
                    .child(url_link(
                        "login-open-url",
                        "Reopen the authorization page",
                        &start.url,
                        cx,
                    ))
                    .child(
                        div().mt(px(12.0)).child(
                            popover::dialog_field(self.code_input.clone().into_any_element())
                                .font_family(theme.font_mono.clone())
                                .text_size(crate::typography::ui_rems(13.0)),
                        ),
                    )
                    .when_some(error.clone(), |el, message| {
                        el.child(
                            div()
                                .mt(px(8.0))
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(red_text)
                                .child(message),
                        )
                    })
                    .child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                popover::btn_ghost(&theme, "Cancel", "login-cancel")
                                    .id("login-cancel")
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx))),
                            )
                            .child(
                                popover::btn_primary(
                                    &theme,
                                    if submitting {
                                        "Verifying…"
                                    } else {
                                        "Add account"
                                    },
                                )
                                .id("login-submit-code")
                                .when(submitting, |el| el.opacity(0.5))
                                .on_click(cx.listener(|this, _, _, cx| this.submit_code(cx))),
                            ),
                    )
                    .into_any_element()
            }
            LoginFlow::Browser {
                harness,
                start,
                message,
                error,
            } => {
                let has_error = error.is_some();
                let body = match harness {
                    HarnessId::Cursor => {
                        "Finish signing in to Cursor in your browser. This mints a \
                         zeron-named API key you can revoke any time from Cursor's \
                         dashboard — it is separate from `cursor-agent login`."
                    }
                    _ => {
                        "Finish signing in to OpenAI in your browser. The new login is \
                         captured in an isolated profile — your current session is untouched \
                         until you switch."
                    }
                };
                div()
                    .flex()
                    .flex_col()
                    .child(div().mt(px(8.0)).child(popover::dialog_body(&theme, body)))
                    .child(url_link(
                        "login-open-url-browser",
                        "Reopen the sign-in page",
                        &start.url,
                        cx,
                    ))
                    .when(!has_error, |el| {
                        el.child(
                            div()
                                .mt(px(16.0))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(crate::loaders::gradient_spinner(
                                    "login-poll",
                                    &theme,
                                    3.0,
                                    cx.entity_id(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(12.5))
                                        .text_color(theme.text_muted)
                                        .child(message.clone().unwrap_or_else(|| {
                                            SharedString::from("Waiting for the browser…")
                                        })),
                                ),
                        )
                    })
                    .when_some(error.clone(), |el, message| {
                        el.child(
                            div()
                                .mt(px(12.0))
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(red_text)
                                .child(message),
                        )
                    })
                    .child(
                        div().mt(px(16.0)).flex().flex_row().justify_end().child(
                            popover::btn_ghost(
                                &theme,
                                if has_error { "Close" } else { "Cancel" },
                                "login-cancel",
                            )
                            .id("login-cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx))),
                        ),
                    )
                    .into_any_element()
            }
        };
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(&theme, title))
            .child(body)
            .into_any_element();
        Some(popover::modal("add-account-dialog", viewport, card))
    }

    /// A ghost account row (zeron settings.agents.tsx `SkeletonRow`): avatar,
    /// email line, two usage-meter ghosts, a badge — same geometry as the real
    /// row so loaded data lands without a layout jump. `dim` fades row two.
    fn render_skeleton_row(
        &self,
        _id: (&'static str, usize),
        dim: bool,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::motion;
        let delta = motion::pulse_delta(&motion::ZERON_PULSE, cx.entity_id(), cx);
        let ghost = |w: gpui::Length, h: f32, round_full: bool| {
            div()
                .w(w)
                .h(px(h))
                .flex_none()
                .map(|el| {
                    if round_full {
                        el.rounded_full()
                    } else {
                        el.rounded(px(4.0))
                    }
                })
                .bg(crate::theme::ink(0.05))
        };
        let meters = div()
            .mt(px(8.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .children((0..2).map(|_| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(ghost(px(48.0).into(), 9.0, false))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(56.0))
                            .max_w(px(230.0))
                            .h(px(5.0))
                            .rounded_full()
                            .bg(crate::theme::ink(0.04)),
                    )
                    .child(ghost(px(64.0).into(), 9.0, false))
            }));
        let inner = div()
            .flex()
            .flex_row()
            .items_stretch()
            .gap(px(12.0))
            .child(
                div()
                    .flex_none()
                    .self_center()
                    .size(px(32.0))
                    .rounded_full()
                    .bg(crate::theme::ink(0.05)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(ghost(px(176.0).into(), 13.0, false).max_w(gpui::relative(0.6)))
                    .child(meters),
            )
            .child(div().flex_none().flex().flex_col().items_end().child(ghost(
                px(64.0).into(),
                21.0,
                true,
            )));
        div()
            .px(px(20.0))
            .py(px(14.0))
            .when(!first, |el| el.border_t_1().border_color(theme.border))
            .when(dim, |el| el.opacity(0.6))
            .child(inner.opacity(0.55 + 0.35 * motion::pulse_wave(delta)))
            .into_any_element()
    }
}

impl popover::ScrollRailHost for AccountsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for AccountsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let now = Utc::now();
        let dialog = self.render_login_dialog(window.viewport_size(), cx);
        let refreshing = matches!(self.snapshot, Loadable::Loading);
        let account_count = self
            .snapshot
            .ready()
            .map(|s| s.accounts.len())
            .filter(|&n| n > 0);

        let provider_icon = |harness: HarnessId| match harness {
            HarnessId::Codex => (crate::icons::OPENAI_MARK, None),
            HarnessId::Cursor => (crate::icons::CURSOR_MARK, None),
            HarnessId::Devin => (crate::icons::DEVIN_MARK, None),
            HarnessId::Grok => (crate::icons::GROK_MARK, None),
            HarnessId::Hermes => (crate::icons::HERMES_MARK, None),
            HarnessId::Pi => (crate::icons::PI_MARK, None),
            HarnessId::Opencode => (crate::icons::OPENCODE_MARK, None),
            HarnessId::Antigravity => (crate::icons::ANTIGRAVITY_MARK, None),
            _ => (
                crate::icons::CLAUDE_MARK,
                Some(crate::icons::claude_brand()),
            ),
        };
        // Brand mark inside a 24px centered box (zeron: `grid size-6
        // place-items-center [&_svg]:size-4`).
        let provider_mark = |harness: HarnessId, theme: &Theme| {
            let (mark, tint) = provider_icon(harness);
            div()
                .flex_none()
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    crate::icons::icon(mark)
                        .size(px(16.0))
                        .text_color(tint.unwrap_or(theme.text_muted)),
                )
        };

        // One section per provider (zeron settings.agents.tsx `ProviderSection`):
        // brand header + Add account, then the account rows card.
        let sections: Vec<AnyElement> = match &self.snapshot {
            Loadable::Idle | Loadable::Loading => PROVIDERS
                .into_iter()
                .map(|(harness, name, _cli)| {
                    let skeleton_id = match harness {
                        HarnessId::Codex => "accounts-skeleton-codex",
                        HarnessId::Cursor => "accounts-skeleton-cursor",
                        _ => "accounts-skeleton-claude",
                    };
                    div()
                        .mt(px(24.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(provider_mark(harness, &theme))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(14.0))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.text)
                                        .child(SharedString::from(name)),
                                ),
                        )
                        .child(
                            // Ghost rows shaped like real ones (row two dimmed)
                            // so the card keeps its size while data develops.
                            widgets::section_card(&theme)
                                .mt(px(8.0))
                                .child(self.render_skeleton_row(
                                    (skeleton_id, 0),
                                    false,
                                    true,
                                    &theme,
                                    cx,
                                ))
                                .child(self.render_skeleton_row(
                                    (skeleton_id, 1),
                                    true,
                                    false,
                                    &theme,
                                    cx,
                                )),
                        )
                        .into_any_element()
                })
                .collect(),
            Loadable::Error(message) => {
                let message = message.clone();
                vec![
                    widgets::error_strip(&theme, message)
                        .id("accounts-load-error")
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            // Retry IS the visit's first successful list — force usage.
                            this.load(force_usage_for(LoadTrigger::Retry), cx)
                        }))
                        .child(
                            div()
                                .mt(px(4.0))
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from("Click to retry")),
                        )
                        .into_any_element(),
                ]
            }
            Loadable::Ready(snapshot) => {
                let snapshot = snapshot.clone();
                PROVIDERS
                    .into_iter()
                    .map(|(harness, name, cli)| {
                        let accounts = provider_accounts(&snapshot, harness);
                        // EVERY warning renders its own strip (zeron maps them).
                        let warnings: Vec<String> = snapshot
                            .warnings
                            .iter()
                            .filter(|w| w.harness == harness)
                            .map(|w| w.message.clone())
                            .collect();
                        let rows: Vec<AnyElement> = accounts
                            .iter()
                            .enumerate()
                            .map(|(ix, account)| {
                                self.render_account_row(account, ix, ix == 0, &theme, now, cx)
                            })
                            .collect();
                        let add_id: SharedString = format!("add-account-{name}").into();
                        let card = widgets::section_card(&theme).mt(px(8.0));
                        let empty_copy = match harness {
                            // Cursor's app login is SEPARATE from `cursor-agent
                            // login` — pointing at the CLI would send users to a
                            // sign-in that does not light this up.
                            HarnessId::Cursor => format!(
                                "{name} isn't connected on this device — connect it to run \
                                 Cursor sessions."
                            ),
                            _ => format!(
                                "No {name} login detected on this device — sign in \
                                 with \u{201C}{cli}\u{201D} or add an account."
                            ),
                        };
                        let card = if rows.is_empty() {
                            card.child(
                                div()
                                    .px(px(20.0))
                                    .py(px(32.0))
                                    .text_center()
                                    .text_size(crate::typography::ui_rems(14.0))
                                    .text_color(theme.text_muted.opacity(0.6))
                                    .child(SharedString::from(empty_copy)),
                            )
                        } else {
                            card.children(rows)
                        };
                        div()
                            .mt(px(24.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(provider_mark(harness, &theme))
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(14.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(SharedString::from(name)),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        widgets::ghost_action(&theme)
                                            .id(add_id)
                                            .hover(|s| widgets::ghost_hover(&theme, s))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.start_login(harness, cx);
                                            }))
                                            .child(
                                                crate::icons::icon(crate::icons::ADD_CIRCLE)
                                                    .size(px(16.0))
                                                    .text_color(theme.text_muted),
                                            )
                                            .child(SharedString::from("Add account")),
                                    ),
                            )
                            .children(
                                warnings
                                    .into_iter()
                                    .map(|warning| widgets::warning_strip(&theme, warning)),
                            )
                            .child(card)
                            .into_any_element()
                    })
                    .collect()
            }
        };

        let scrollbar = popover::rail(self, "accounts-page-scrollbar", &theme, cx);
        div()
            .id("accounts-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("accounts-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(widgets::page_header(&theme, "Accounts", account_count))
                                    .child(div().flex_1())
                                    .child(
                                        // `text-[12.5px]` + leading 16px Refresh icon,
                                        // dimmed while a refresh is in flight (zeron
                                        // `disabled:opacity-50`).
                                        widgets::ghost_action(&theme)
                                            .id("accounts-refresh")
                                            .flex_none()
                                            .text_size(crate::typography::ui_rems(12.5))
                                            .hover(|s| widgets::ghost_hover(&theme, s))
                                            .when(refreshing, |el| el.opacity(0.5))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.load(
                                                    force_usage_for(LoadTrigger::Refresh),
                                                    cx,
                                                )
                                            }))
                                            .child(
                                                crate::icons::icon(crate::icons::REFRESH)
                                                    .size(px(16.0))
                                                    .text_color(theme.text_muted),
                                            )
                                            .child(SharedString::from("Refresh")),
                                    )
                                    .child(self.render_device_switcher(&theme, cx)),
                            )
                            .child(widgets::page_subtitle(
                                &theme,
                                "The Claude Code, Codex, and Cursor logins on this device. Zeron \
                                 detects the live session, keeps each account backed up, and can \
                                 swap between them.",
                            ))
                            .when_some(self.error.clone(), |el, message| {
                                el.child(
                                    widgets::error_strip(&theme, message)
                                        .id("accounts-action-error")
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.error = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .children(sections)
                            // Footer note (zeron: `mt-6 text-[12px] leading-relaxed
                            // text-muted-foreground/60`).
                            .child(
                                div()
                                    .mt(px(24.0))
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .line_height(px(19.0))
                                    .text_color(theme.text_muted.opacity(0.6))
                                    .child(SharedString::from(
                                        "Switching rewrites the CLI\u{2019}s stored login, so new \
                                         agent sessions use the selected account immediately. On \
                                         macOS, an already-running Claude Code can hold the previous \
                                         login for up to ~30 seconds (Keychain cache).",
                                    )),
                            ),
                    ),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
    }
}
