use super::*;

impl HarnessesPage {
    pub(super) fn render_titles(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut card = widgets::section_card(theme).mt(px(20.0)).p(px(16.0))
            .child(widgets::row_title(theme, "Session titles"))
            .child(widgets::page_subtitle(theme, "Choose the agent and model for automatic titles on this device. Claude Code and Codex support restricted title generation."));
        let Loadable::Ready(settings) = &self.title_settings else {
            let message = match &self.title_settings {
                Loadable::Error(error) => error.clone(),
                _ => "Loading title settings…".into(),
            };
            return card
                .child(div().mt(px(8.0)).child(message))
                .into_any_element();
        };
        for is_model in [false, true] {
            let label = if is_model {
                settings
                    .model
                    .as_ref()
                    .map(|id| {
                        if let Loadable::Ready(models) = &self.title_models {
                            models
                                .iter()
                                .find(|m| &m.id == id)
                                .map(|m| m.label.clone())
                                .unwrap_or_else(|| id.clone())
                        } else {
                            id.clone()
                        }
                    })
                    .unwrap_or_else(|| "Automatic (cheapest model)".into())
            } else {
                settings
                    .harness
                    .map(|id| match id {
                        HarnessId::ClaudeCode => "Claude Code".to_string(),
                        HarnessId::Codex => "Codex".to_string(),
                        _ => format!("{id:?}"),
                    })
                    .unwrap_or_else(|| "Automatic (session agent when supported)".into())
            };
            let interactive = !self.title_saving && (!is_model || settings.harness.is_some());
            let mut row = div()
                .mt(px(12.0))
                .child(widgets::row_title(
                    theme,
                    if is_model {
                        "Title model"
                    } else {
                        "Title harness"
                    },
                ))
                .child(
                    widgets::ghost_action(theme)
                        .id(if is_model {
                            "title-model"
                        } else {
                            "title-harness"
                        })
                        .when(interactive, |el| {
                            el.cursor_pointer()
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.title_menu = if page.title_menu == Some(is_model) {
                                        None
                                    } else {
                                        Some(is_model)
                                    };
                                    cx.notify();
                                }))
                        })
                        .when(!interactive, |el| el.opacity(0.5))
                        .child(label),
                );
            if self.title_menu == Some(is_model) {
                let mut choices = vec![(
                    "Automatic".to_string(),
                    TitleSettings {
                        harness: if is_model { settings.harness } else { None },
                        model: None,
                    },
                )];
                if is_model {
                    if let Loadable::Ready(models) = &self.title_models {
                        choices.extend(models.iter().map(|m| {
                            (
                                m.label.clone(),
                                TitleSettings {
                                    harness: settings.harness,
                                    model: Some(m.id.clone()),
                                },
                            )
                        }));
                    }
                } else if let Loadable::Ready(harnesses) = &self.harnesses {
                    choices.extend(
                        harnesses
                            .iter()
                            .filter(|h| {
                                descriptor_enabled(h)
                                    && h.installed
                                    && zeron_harness::supports_titles(h.id)
                                    && h.id != HarnessId::Mock
                            })
                            .map(|h| {
                                (
                                    h.name.clone(),
                                    TitleSettings {
                                        harness: Some(h.id),
                                        model: None,
                                    },
                                )
                            }),
                    );
                }
                row =
                    row.child(
                        div()
                            .id(if is_model {
                                "title-model-options"
                            } else {
                                "title-harness-options"
                            })
                            .max_h(px(240.0))
                            .overflow_y_scroll()
                            .children(choices.into_iter().enumerate().map(
                                |(ix, (label, choice))| {
                                    popover::menu_row(
                                        theme,
                                        &choice == settings,
                                        format!("title-choice-{is_model}-{ix}"),
                                    )
                                    .id(("title-choice", ix))
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.load_titles(Some(choice.clone()), cx)
                                    }))
                                    .child(label)
                                },
                            )),
                    );
            }
            card = card.child(row);
        }
        if let Loadable::Error(error) = &self.title_models {
            card = card.child(widgets::error_strip(theme, error.clone()));
        }
        card.into_any_element()
    }
}
