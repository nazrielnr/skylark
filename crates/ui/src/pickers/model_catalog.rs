use gpui::SharedString;
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{HarnessId, Model, ReasoningLevel};

/// Sentinel for "no keyboard-highlighted row" (`active`): matches no index,
/// and `usize::MAX as isize == -1` — `menu_step` treats it like `None`, so
/// the first Down lands on row 0.
pub(crate) const NO_ACTIVE_ROW: usize = usize::MAX;

/// Which pane the harness/model picker's icon rail is showing (t3code
/// ModelPickerContent `selectedInstanceId | "favorites"`). `Harness` means
/// "the effective harness's list" — the rail has no browse-without-commit
/// state; clicking a brand icon picks that harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ModelRail {
    Favorites,
    #[default]
    Harness,
}

/// Cache key for the flattened model-row list: any input that changes the
/// list's CONTENT (not its highlight/selection, which render per-row).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ModelRowsKey {
    pub(crate) query: String,
    pub(crate) rail: ModelRail,
    pub(crate) effective: Option<HarnessId>,
    pub(crate) locked: bool,
    pub(crate) catalog_rev: u64,
}

/// One row of the model list: the model plus the harness it belongs to —
/// search results and the favorites view mix harnesses, and every row's
/// subline names its harness (t3code ModelListRow `showProvider`).
#[derive(Debug, Clone)]
pub(crate) struct ModelRowData {
    pub(crate) harness: HarnessId,
    pub(crate) harness_name: SharedString,
    pub(crate) model: Model,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelSetting {
    Reasoning,
    Option(String),
}

#[derive(Clone)]
pub(crate) struct SettingChoice {
    pub(crate) label: String,
    pub(crate) reasoning: Option<ReasoningLevel>,
    pub(crate) value: String,
    pub(crate) selected: bool,
    pub(crate) default: bool,
}

pub(crate) struct SettingGroup {
    pub(crate) id: ModelSetting,
    pub(crate) label: String,
    pub(crate) choices: Vec<SettingChoice>,
}

/// The harness's default model: the first catalog row (both curated catalogs
/// lead with the flagship — zeron's `pickDefaultModel` Opus preference maps to
/// the same row here).
pub fn default_model(models: &[Model]) -> Option<&Model> {
    models.first()
}

/// A model's default reasoning: X-High when the ladder offers it (zeron
/// `DEFAULT_REASONING = "xhigh"`), else High, else the ladder's first entry.
/// `None` only for ladder-less models (e.g. Haiku's thinking toggle instead).
pub fn default_reasoning(ladder: &[ReasoningLevel]) -> Option<ReasoningLevel> {
    // The recommended default is High (user-corrected — not X-High globally);
    // fall to Medium then the ladder's first entry for shorter ladders.
    if ladder.contains(&ReasoningLevel::High) {
        return Some(ReasoningLevel::High);
    }
    if ladder.contains(&ReasoningLevel::Medium) {
        return Some(ReasoningLevel::Medium);
    }
    ladder.first().copied()
}

/// Clamp a picked/remembered level to what the model actually offers: keep it
/// when the ladder lists it, else fall to the model's default (never a stale
/// or foreign level — zeron use-run-config.ts's derived-model discipline).
pub fn clamp_reasoning(
    level: Option<ReasoningLevel>,
    ladder: &[ReasoningLevel],
) -> Option<ReasoningLevel> {
    match level {
        Some(level) if ladder.contains(&level) => Some(level),
        _ => default_reasoning(ladder),
    }
}

pub fn reasoning_label(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Minimal => "Minimal",
        ReasoningLevel::Low => "Low",
        ReasoningLevel::Medium => "Medium",
        ReasoningLevel::High => "High",
        ReasoningLevel::XHigh => "X-High",
        ReasoningLevel::Max => "Max",
        ReasoningLevel::Ultra => "Ultra",
        ReasoningLevel::Ultracode => "Ultracode",
        ReasoningLevel::Ultrathink => "Ultrathink",
    }
}

/// The TraitsPicker trigger summary: the effective reasoning level plus every
/// model option's effective choice — the explicit pick when one is saved and
/// still offered, else the option's default — joined with " · " ("High · 1M ·
/// Fast", Cursor's "Agent · Balance"). Defaults are spelled out rather than
/// hidden so the run's configuration reads without opening the popover; `None`
/// only when the model has nothing to describe (no ladder, no options).
pub fn traits_summary(
    model: Option<&Model>,
    reasoning: Option<ReasoningLevel>,
    selections: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(level) = reasoning {
        parts.push(reasoning_label(level).to_string());
    }
    if let Some(model) = model {
        for option in &model.options {
            let choice_id = selections
                .get(&option.id)
                .and_then(|v| v.as_str())
                .filter(|id| option.choices.iter().any(|c| c.id == *id))
                .unwrap_or(&option.default_choice);
            if let Some(choice) = option.choices.iter().find(|c| c.id == choice_id) {
                parts.push(choice.label.clone());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// Keep only the picks `model` still offers. Remembered picks outlive the
/// model they were made on, and harnesses apply some options blindly (Claude
/// appends `[1m]` to any model id when `contextWindow` is "1m").
pub fn offered_options(
    model: &Model,
    mut selections: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    selections.retain(|id, choice| {
        model.options.iter().any(|option| {
            option.id == *id
                && choice
                    .as_str()
                    .is_some_and(|choice| option.choices.iter().any(|c| c.id == choice))
        })
    });
    selections
}

/// Whether any trait departs from its default — the trigger brightens only
/// then, so a customized run still stands out now that the summary always
/// names the effective choices.
pub fn traits_customized(
    model: Option<&Model>,
    reasoning: Option<ReasoningLevel>,
    ladder: &[ReasoningLevel],
    selections: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if reasoning != default_reasoning(ladder) {
        return true;
    }
    model.is_some_and(|model| {
        model.options.iter().any(|option| {
            selections
                .get(&option.id)
                .and_then(|v| v.as_str())
                .is_some_and(|id| {
                    id != option.default_choice && option.choices.iter().any(|c| c.id == id)
                })
        })
    })
}

/// Display-side model-list hygiene, mirroring the engine's discovery-side
/// fold (`models_from_session`) for catalogs served by OLDER engines (the
/// space's device may run any version): the `default` alias row drops when a
/// real row exists, an orphan `<model>[1m]` variant presents as its base id
/// with the Context Window trait pinned to 1M, and Claude rows adopt the
/// curated catalog's labels so the version number always shows ("Opus 5",
/// not the wire's terse "Opus" alias — user request). Idempotent over
/// already-clean lists. The send path recomposes the advertised id from the
/// base + trait (`pick_model_value`), so a folded pick still runs.
pub(crate) fn normalize_model_rows(harness: HarnessId, models: Vec<Model>) -> Vec<Model> {
    fn strip_1m(id: &str) -> Option<&str> {
        id.strip_suffix("[1m]").or_else(|| id.strip_suffix("-1m"))
    }
    fn norm(id: &str) -> String {
        id.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    }
    let catalog = match harness {
        HarnessId::ClaudeCode => zeron_harness::claude::catalog::static_models(),
        _ => Vec::new(),
    };
    // Curated label for an id: exact normalized match, else — for bare
    // alphabetic aliases like `opus` — the first (flagship-ordered) family
    // row. Versioned foreign ids never fuzzy-match.
    let curated_label = |id: &str| -> Option<String> {
        let id_norm = norm(id);
        if let Some(row) = catalog.iter().find(|m| norm(&m.id) == id_norm) {
            return Some(row.label.clone());
        }
        (!id_norm.is_empty() && id_norm.chars().all(|c| c.is_ascii_alphabetic()))
            .then(|| catalog.iter().find(|m| norm(&m.id).contains(&id_norm)))
            .flatten()
            .map(|m| m.label.clone())
    };
    let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
    let has_real = ids.iter().any(|id| !id.eq_ignore_ascii_case("default"));
    models
        .into_iter()
        .filter_map(|mut model| {
            if has_real && model.id.eq_ignore_ascii_case("default") {
                return None;
            }
            if let Some(base) = strip_1m(&model.id.clone()) {
                if ids.iter().any(|other| other == base) {
                    // The bare base is listed too — the engine already gave
                    // it the Context Window trait; the variant row is noise.
                    return None;
                }
                model.id = base.to_string();
                // "Opus (1M context)" → "Opus".
                if let Some(at) = model.label.rfind(" (")
                    && model.label.ends_with(')')
                {
                    model.label.truncate(at);
                    while model.label.ends_with(' ') {
                        model.label.pop();
                    }
                }
                if !model.options.iter().any(|o| o.id == "contextWindow") {
                    model.options.push(zeron_proto::ModelOption {
                        id: "contextWindow".into(),
                        label: "Context Window".into(),
                        choices: vec![
                            zeron_proto::ModelOptionChoice {
                                id: "200k".into(),
                                label: "200K".into(),
                            },
                            zeron_proto::ModelOptionChoice {
                                id: "1m".into(),
                                label: "1M".into(),
                            },
                        ],
                        default_choice: "1m".into(),
                    });
                }
            }
            if let Some(label) = curated_label(&model.id) {
                model.label = label;
            }
            Some(model)
        })
        .collect()
}

pub(crate) fn harness_brand_icon(harness: HarnessId) -> (&'static str, Option<gpui::Hsla>) {
    match harness {
        HarnessId::ClaudeCode | HarnessId::Mock => (
            crate::icons::CLAUDE_MARK,
            Some(crate::icons::claude_brand()),
        ),
        HarnessId::Codex => (crate::icons::OPENAI_MARK, None),
        HarnessId::Cursor => (crate::icons::CURSOR_MARK, None),
        // Cognition's mark (the Devin product icon), monochrome.
        HarnessId::Devin => (crate::icons::DEVIN_MARK, None),
        // Monochrome mark, tinted by the surface like OpenAI's.
        HarnessId::Grok => (crate::icons::GROK_MARK, None),
        // Nous Research's mark (the Hermes product icon), monochrome.
        HarnessId::Hermes => (crate::icons::HERMES_MARK, None),
        HarnessId::Pi => (crate::icons::PI_MARK, None),
        // The pixel-"o" from opencode's wordmark (their favicon), monochrome.
        HarnessId::Opencode => (crate::icons::OPENCODE_MARK, None),
        HarnessId::Antigravity => (crate::icons::ANTIGRAVITY_MARK, None),
    }
}

/// `ZERON_HARNESS=mock` (the e2e/dev rig) opts the mock harness into the UI;
/// production launches never set it, so the mock never surfaces there.
pub(crate) fn mock_harness_enabled() -> bool {
    std::env::var("ZERON_HARNESS")
        .ok()
        .as_deref()
        .map(str::trim)
        == Some("mock")
}

/// Production pickers AND chip resolution hide the mock harness — the
/// registry always lists it, but it must never surface in real UI (neither in
/// the picker rail nor as the eager default the chips resolve against).
/// `ZERON_HARNESS=mock` shows it; otherwise it only remains when it's
/// literally all there is (a dev build with no real harness registered).
pub fn visible_harnesses(list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
    visible_harnesses_impl(list, mock_harness_enabled())
}

pub(crate) fn visible_harnesses_impl(list: &[HarnessDescriptor], allow_mock: bool) -> Vec<HarnessDescriptor> {
    if allow_mock {
        return list.to_vec();
    }
    let real: Vec<HarnessDescriptor> = list
        .iter()
        .filter(|d| d.id != HarnessId::Mock)
        .cloned()
        .collect();
    if real.is_empty() { list.to_vec() } else { real }
}

/// What the composer actually offers: [`visible_harnesses`] narrowed to the
/// catalog device's enabled set AND installed CLIs (Settings → Agents is
/// per-device state, so a space on another device follows THAT device's
/// toggles; a default-enabled agent whose CLI is missing would only
/// manufacture NotInstalled errors at send). The dev-rig mock opt-in
/// survives the filter. There is NO fallback: a catalog where nothing is
/// both enabled and installed offers nothing, and the composer surfaces the
/// no-agents empty state + blocks new sends — resurrecting descriptors that
/// can only fail with NotInstalled is the #128 bug.
pub fn offered_harnesses(list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
    offered_harnesses_impl(list, mock_harness_enabled())
}

pub(crate) fn offered_harnesses_impl(list: &[HarnessDescriptor], allow_mock: bool) -> Vec<HarnessDescriptor> {
    visible_harnesses_impl(list, allow_mock)
        .into_iter()
        .filter(|d| {
            d.installed
                && (zeron_engine::registry::descriptor_enabled(d)
                    || (allow_mock && d.id == HarnessId::Mock))
        })
        .collect()
}
