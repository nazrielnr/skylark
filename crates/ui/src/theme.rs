//! The app theme — two concrete appearances, one token set.
//!
//! Colors are precomputed from an oklch-derived neutral scale (perceptually even
//! lightness steps; the same scale zeron's Tailwind theme used) into gpui [`Hsla`].
//! **Numbers drive layout, colors are paint**: layout constants live here as plain
//! numbers and never depend on which color is painted.
//!
//! # Light is designed, not inverted
//!
//! Mirroring lightness produces the classic "washed-out inverted" look, for three
//! reasons this module handles explicitly:
//!
//! 1. **Surface order flips meaning.** In dark, the main content panel is the
//!    *darkest* plane and raised surfaces get *lighter*. In light, the content
//!    panel is *white* and the shell/sidebar goes *grey* — chrome recedes by
//!    getting darker, not lighter. Popovers stay white and earn separation from a
//!    border and shadow rather than from lightness.
//! 2. **Elevation reverses.** On dark, a faint *white* wash means "raised". Its
//!    literal translation — a faint *black* wash on white — means "recessed", so
//!    the composer read as a dent instead of a plate. Light lifts with white plus
//!    a border and shadow ([`Theme::input_bg`], the elevation ladder). Fill
//!    *alphas* carry over unchanged ([`INK_FILL_SCALE`]); only hairlines scale, so
//!    a 1px edge survives a bright surround ([`INK_HAIRLINE_SCALE`]).
//! 3. **Accents must move down the scale.** The dark palette's 400-level accents
//!    (indigo/red/amber) are chosen for contrast against near-black; on white they
//!    fall to 2–4:1 and fail WCAG AA. Light mode uses the 600-level siblings at the
//!    same hue, which restores the *contrast ratio* the dark token had.
//!
//! Text tones are chosen so each light token lands within ~0.5 of its dark
//! counterpart's contrast ratio against its own background — the pairing is
//! verified in [`tests::text_contrast_is_paired_across_appearances`], not eyeballed.
//!
//! Installed as a gpui [`Global`] at boot; read with [`Theme::of`].

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use gpui::{App, Global, Hsla, SharedString, hsla};
use serde::{Deserialize, Serialize};
use zeron_syntax::HighlightKind;
use zeron_theme::{
    AccentPreset, AccentSelection, Color as ModelColor, SurfacePreference, SurfaceTreatment,
    ThemeRegistry, ThemeVariant,
};

/// User-selectable accent family. A choice is one color identity, not a
/// miniature multi-hue theme: every interactive accent role stays on the same
/// authored hue in both appearances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccentColor {
    /// The exact upstream Zeron indigo.
    #[default]
    #[serde(alias = "violet", alias = "indigo", alias = "red", alias = "purple")]
    Zeron,
    Orange,
    Amber,
    Green,
    #[serde(alias = "teal")]
    Cyan,
    Blue,
    Pink,
}

impl AccentColor {
    pub const ALL: [Self; 7] = [
        Self::Zeron,
        Self::Orange,
        Self::Amber,
        Self::Green,
        Self::Cyan,
        Self::Blue,
        Self::Pink,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Zeron => "Zeron",
            Self::Orange => "Orange",
            Self::Amber => "Amber",
            Self::Green => "Green",
            Self::Cyan => "Cyan",
            Self::Blue => "Blue",
            Self::Pink => "Pink",
        }
    }

    fn tokens(self, appearance: Appearance) -> AccentTokens {
        // These are deliberately authored pairs. Runtime contrast correction
        // used to gamut-clip OKLCH into sRGB and then mutate HSL lightness,
        // producing different chroma and apparent hues across light/dark.
        let (primary, strong) = match (self, appearance) {
            (Self::Zeron, Appearance::Dark) => {
                (oklch(0.673, 0.182, 276.935), oklch(0.585, 0.233, 277.117))
            }
            (Self::Zeron, Appearance::Light) => {
                (oklch(0.511, 0.262, 276.966), oklch(0.511, 0.262, 276.966))
            }
            (Self::Orange, Appearance::Dark) => (oklch(0.75, 0.18, 55.0), oklch(0.54, 0.19, 55.0)),
            (Self::Orange, Appearance::Light) => (oklch(0.50, 0.19, 55.0), oklch(0.50, 0.19, 55.0)),
            (Self::Amber, Appearance::Dark) => (oklch(0.80, 0.17, 84.0), oklch(0.52, 0.14, 84.0)),
            (Self::Amber, Appearance::Light) => (oklch(0.48, 0.14, 84.0), oklch(0.48, 0.14, 84.0)),
            (Self::Green, Appearance::Dark) => (oklch(0.75, 0.17, 150.0), oklch(0.50, 0.15, 150.0)),
            (Self::Green, Appearance::Light) => {
                (oklch(0.46, 0.14, 150.0), oklch(0.46, 0.14, 150.0))
            }
            (Self::Cyan, Appearance::Dark) => (oklch(0.76, 0.13, 205.0), oklch(0.49, 0.12, 205.0)),
            (Self::Cyan, Appearance::Light) => (oklch(0.45, 0.11, 205.0), oklch(0.45, 0.11, 205.0)),
            (Self::Blue, Appearance::Dark) => (oklch(0.70, 0.17, 255.0), oklch(0.50, 0.20, 255.0)),
            (Self::Blue, Appearance::Light) => (oklch(0.47, 0.21, 255.0), oklch(0.47, 0.21, 255.0)),
            (Self::Pink, Appearance::Dark) => (oklch(0.72, 0.18, 350.0), oklch(0.51, 0.20, 350.0)),
            (Self::Pink, Appearance::Light) => (oklch(0.48, 0.20, 350.0), oklch(0.48, 0.20, 350.0)),
        };
        AccentTokens {
            primary,
            strong,
            wash: match appearance {
                Appearance::Dark => strong.opacity(0.45),
                Appearance::Light => primary.opacity(0.10),
            },
            selection: primary.opacity(if appearance.is_dark() { 0.35 } else { 0.24 }),
            caret: primary,
            code_text: primary,
            code_wash: primary.opacity(match appearance {
                Appearance::Dark => 0.12,
                Appearance::Light => 0.10,
            }),
            activity: primary,
            glyph: GlyphPalette::for_accent(primary, strong, appearance),
        }
    }
}

impl From<AccentColor> for AccentPreset {
    fn from(value: AccentColor) -> Self {
        match value {
            AccentColor::Zeron => Self::Zeron,
            AccentColor::Orange => Self::Orange,
            AccentColor::Amber => Self::Amber,
            AccentColor::Green => Self::Green,
            AccentColor::Cyan => Self::Cyan,
            AccentColor::Blue => Self::Blue,
            AccentColor::Pink => Self::Pink,
        }
    }
}

impl From<AccentPreset> for AccentColor {
    fn from(value: AccentPreset) -> Self {
        match value {
            AccentPreset::Zeron => Self::Zeron,
            AccentPreset::Orange => Self::Orange,
            AccentPreset::Amber => Self::Amber,
            AccentPreset::Green => Self::Green,
            AccentPreset::Cyan => Self::Cyan,
            AccentPreset::Blue => Self::Blue,
            AccentPreset::Pink => Self::Pink,
        }
    }
}

/// The three authored rows of the animated 2×3 pixel glyph. Keeping this as a
/// palette entity preserves the mark's light→mid→deep personality while letting
/// every accent preset own it as one coherent family.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphPalette {
    pub light: Hsla,
    pub mid: Hsla,
    pub deep: Hsla,
}

impl GlyphPalette {
    fn for_accent(primary: Hsla, strong: Hsla, appearance: Appearance) -> Self {
        let mut light = primary;
        let mut deep = strong;
        match appearance {
            Appearance::Dark => {
                light.l = (light.l + 0.14).min(0.90);
                light.s *= 0.72;
            }
            Appearance::Light => {
                light.l = (light.l + 0.11).min(0.76);
                light.s *= 0.78;
                deep.l = (deep.l - 0.09).max(0.22);
            }
        }
        Self {
            light,
            mid: primary,
            deep,
        }
    }

    pub fn rows(self) -> [Hsla; 3] {
        [self.light, self.mid, self.deep]
    }
}

#[derive(Debug, Clone, Copy)]
struct AccentTokens {
    primary: Hsla,
    strong: Hsla,
    wash: Hsla,
    selection: Hsla,
    caret: Hsla,
    code_text: Hsla,
    code_wash: Hsla,
    activity: Hsla,
    glyph: GlyphPalette,
}

/// Which appearance the app is painting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Appearance {
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    pub fn is_light(self) -> bool {
        matches!(self, Self::Light)
    }

    /// Map a gpui window appearance onto ours (both vibrant variants are just
    /// the blurred flavour of the same tone).
    pub fn from_window(appearance: gpui::WindowAppearance) -> Self {
        use gpui::WindowAppearance::*;
        match appearance {
            Light | VibrantLight => Self::Light,
            Dark | VibrantDark => Self::Dark,
        }
    }
}

/// Process-wide mirror of the installed theme's appearance.
///
/// The paint helpers ([`ink`], [`hairline`], [`wash`], …) are free functions
/// called from deep inside element builders that have no `cx` in scope, so they
/// read the appearance from here instead of the gpui global. Appearance is
/// genuinely process-wide — one setting for every window — so a single mirror is
/// sound; [`Theme::install`] is the only writer outside tests.
static CURRENT_APPEARANCE: AtomicU8 = AtomicU8::new(0);

/// Bumped every time the appearance actually changes.
///
/// Anything that caches *resolved colors* — most importantly the markdown
/// renderer's cross-frame `TextRun` cache, which bakes an `Hsla` into every run —
/// is only valid for the palette that produced it. Those caches were written when
/// the theme was a compile-time constant, so their validity keys cover content
/// only. Rather than thread the palette through every key, they compare this
/// counter and drop everything when it moves.
static STYLE_GENERATION: AtomicU32 = AtomicU32::new(0);

/// The appearance the context-free paint helpers are painting for.
pub fn current_appearance() -> Appearance {
    match CURRENT_APPEARANCE.load(Ordering::Relaxed) {
        1 => Appearance::Light,
        _ => Appearance::Dark,
    }
}

/// Monotonic id of the current resolved style (palette + UI typography).
pub fn style_generation() -> u32 {
    STYLE_GENERATION.load(Ordering::Relaxed)
}

/// Invalidate caches that bake resolved text styles.
pub(crate) fn bump_style_generation() {
    STYLE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

fn model_appearance(appearance: zeron_theme::Appearance) -> Appearance {
    match appearance {
        zeron_theme::Appearance::Dark => Appearance::Dark,
        zeron_theme::Appearance::Light => Appearance::Light,
    }
}

fn model_color(color: ModelColor) -> Hsla {
    let (h, s, l) = rgb_to_hsl(
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
    );
    hsla(h, s, l, color.a as f32 / 255.0)
}

fn harden_model_foreground(
    color: ModelColor,
    backgrounds: &[ModelColor],
    minimum: f32,
    preferred_target: Option<ModelColor>,
) -> ModelColor {
    let minimum_contrast = |candidate: ModelColor| {
        backgrounds
            .iter()
            .map(|background| candidate.contrast(*background))
            .fold(f32::INFINITY, f32::min)
    };
    if minimum_contrast(color) >= minimum {
        return color;
    }
    let mut targets = Vec::with_capacity(3);
    if let Some(target) = preferred_target {
        targets.push(target);
    }
    targets.extend([ModelColor::BLACK, ModelColor::WHITE]);
    let mut best = color;
    let mut best_contrast = minimum_contrast(color);
    for target in targets {
        for step in 1..=100 {
            let candidate = color.mix(target, step as f32 / 100.0);
            let contrast = minimum_contrast(candidate);
            if contrast > best_contrast {
                best = candidate;
                best_contrast = contrast;
            }
            if contrast >= minimum {
                return candidate;
            }
        }
    }
    best
}

/// [`CURRENT_APPEARANCE`] is process-wide, so under the parallel test runner
/// any test that flips it — or asserts on the output of a helper that reads it
/// ([`ink`], [`hairline`], [`wash`], …) — must hold this lock. Crate-visible
/// because such tests exist outside this module too (see `motion::tests`).
/// Tests that flip the appearance restore Dark before releasing the guard.
#[cfg(test)]
pub(crate) fn lock_appearance() -> std::sync::MutexGuard<'static, ()> {
    static APPEARANCE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    APPEARANCE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Point the context-free paint helpers at an appearance. Called by
/// [`Theme::install`]; exposed for tests that build a theme without an `App`.
pub fn set_current_appearance(appearance: Appearance) {
    let encoded = match appearance {
        Appearance::Dark => 0,
        Appearance::Light => 1,
    };
    if CURRENT_APPEARANCE.swap(encoded, Ordering::Relaxed) != encoded {
        bump_style_generation();
    }
}

/// Light-mode alpha multiplier for **fills** (hover/active washes, chip and pill
/// backgrounds).
///
/// This was 0.5 on the theory that dark ink on a bright field reads heavier and
/// should be scaled back. That theory is right for a *large* wash and badly wrong
/// for everything else: this palette leans on very low alphas for its subtle
/// fills — the composer plate is `ink(0.03)`, key caps are `ink(0.05)` — and
/// halving those produced 1.5% black on white, which is nothing. The composer
/// lost its background entirely and selected tabs stopped reading as selected.
///
/// The established light-UI scales (Primer, Radix) land subtle ≈ 3–4%, hover ≈ 8%,
/// selected ≈ 14% black — which is where the dark palette's white alphas already
/// sit. So the honest multiplier is 1: the same number in both appearances, with
/// only the *tone* flipping. Any per-state correction belongs in that state's
/// token, not in a blanket multiplier.
pub const INK_FILL_SCALE: f32 = 1.0;

/// Light-mode alpha multiplier for **hairlines** (borders, dividers, rings).
/// Opposite of fills: a 1px edge has to hold its own against a bright surround,
/// and the dark palette's white hairlines are deliberately faint. Scaling up
/// keeps separators legible instead of dissolving into the panel.
pub const INK_HAIRLINE_SCALE: f32 = 1.35;

/// Paint-only syntax colors. The hues follow the Git history graph's lane
/// palette (indigo, pink, emerald, amber, red, neutral), while light-mode
/// variants are darkened enough to remain readable as text on white.
#[derive(Debug, Clone)]
pub struct SyntaxPalette {
    pub comment: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub string_special: Hsla,
    pub escape: Hsla,
    pub number: Hsla,
    pub boolean: Hsla,
    pub type_name: Hsla,
    pub type_builtin: Hsla,
    pub constructor: Hsla,
    pub function: Hsla,
    pub function_builtin: Hsla,
    pub macro_name: Hsla,
    pub property: Hsla,
    pub constant: Hsla,
    pub variable: Hsla,
    pub variable_special: Hsla,
    pub parameter: Hsla,
    pub operator: Hsla,
    pub punctuation: Hsla,
    pub tag: Hsla,
    pub attribute: Hsla,
    pub label: Hsla,
    pub markup_heading: Hsla,
    pub markup_raw: Hsla,
    pub markup_link: Hsla,
    pub markup_reference: Hsla,
    pub markup_emphasis: Hsla,
    pub markup_strong: Hsla,
    pub invalid: Hsla,
}

impl SyntaxPalette {
    pub fn color(&self, kind: HighlightKind) -> Hsla {
        match kind {
            HighlightKind::Comment => self.comment,
            HighlightKind::Keyword => self.keyword,
            HighlightKind::String => self.string,
            HighlightKind::StringSpecial => self.string_special,
            HighlightKind::Escape => self.escape,
            HighlightKind::Number => self.number,
            HighlightKind::Boolean => self.boolean,
            HighlightKind::Type => self.type_name,
            HighlightKind::TypeBuiltin => self.type_builtin,
            HighlightKind::Constructor => self.constructor,
            HighlightKind::Function => self.function,
            HighlightKind::FunctionBuiltin => self.function_builtin,
            HighlightKind::Macro => self.macro_name,
            HighlightKind::Property => self.property,
            HighlightKind::Constant => self.constant,
            HighlightKind::Variable => self.variable,
            HighlightKind::VariableSpecial => self.variable_special,
            HighlightKind::Parameter => self.parameter,
            HighlightKind::Operator => self.operator,
            HighlightKind::Punctuation | HighlightKind::Embedded => self.punctuation,
            HighlightKind::Tag => self.tag,
            HighlightKind::Attribute => self.attribute,
            HighlightKind::Label => self.label,
            HighlightKind::MarkupHeading => self.markup_heading,
            HighlightKind::MarkupRaw => self.markup_raw,
            HighlightKind::MarkupLink => self.markup_link,
            HighlightKind::MarkupReference => self.markup_reference,
            HighlightKind::MarkupEmphasis => self.markup_emphasis,
            HighlightKind::MarkupStrong => self.markup_strong,
            HighlightKind::Invalid => self.invalid,
        }
    }

    fn from_variant(variant: &ThemeVariant, fallback: Self) -> Self {
        let color = |key: &str, fallback: Hsla| {
            variant
                .syntax
                .get(key)
                .copied()
                .map(model_color)
                .unwrap_or(fallback)
        };
        Self {
            comment: color("comment", fallback.comment),
            keyword: color("keyword", fallback.keyword),
            string: color("string", fallback.string),
            string_special: color("stringSpecial", fallback.string_special),
            escape: color("escape", fallback.escape),
            number: color("number", fallback.number),
            boolean: color("boolean", fallback.boolean),
            type_name: color("type", fallback.type_name),
            type_builtin: color("typeBuiltin", fallback.type_builtin),
            constructor: color("constructor", fallback.constructor),
            function: color("function", fallback.function),
            function_builtin: color("functionBuiltin", fallback.function_builtin),
            macro_name: color("macro", fallback.macro_name),
            property: color("property", fallback.property),
            constant: color("constant", fallback.constant),
            variable: color("variable", fallback.variable),
            variable_special: color("variableSpecial", fallback.variable_special),
            parameter: color("parameter", fallback.parameter),
            operator: color("operator", fallback.operator),
            punctuation: color("punctuation", fallback.punctuation),
            tag: color("tag", fallback.tag),
            attribute: color("attribute", fallback.attribute),
            label: color("label", fallback.label),
            markup_heading: color("markupHeading", fallback.markup_heading),
            markup_raw: color("markupRaw", fallback.markup_raw),
            markup_link: color("markupLink", fallback.markup_link),
            markup_reference: color("markupReference", fallback.markup_reference),
            markup_emphasis: color("markupEmphasis", fallback.markup_emphasis),
            markup_strong: color("markupStrong", fallback.markup_strong),
            invalid: color("invalid", fallback.invalid),
        }
    }

    fn dark(text: Hsla, comment: Hsla, danger: Hsla) -> Self {
        // Same sources and 72% saturation treatment as history::graph_color.
        let indigo = git_graph_tone(oklch(0.673, 0.182, 276.935));
        let pink = git_graph_tone(oklch(0.718, 0.202, 349.761));
        let emerald = git_graph_tone(oklch(0.765, 0.177, 163.223));
        let amber = git_graph_tone(oklch(0.828, 0.189, 84.429));
        let red = git_graph_tone(danger);
        Self {
            comment,
            keyword: indigo,
            string: emerald,
            string_special: pink,
            escape: pink,
            number: amber,
            boolean: amber,
            type_name: amber,
            type_builtin: emerald,
            constructor: amber,
            function: indigo,
            function_builtin: pink,
            macro_name: pink,
            property: amber,
            constant: emerald,
            variable: text,
            variable_special: pink,
            parameter: text,
            operator: text,
            punctuation: text,
            tag: pink,
            attribute: amber,
            label: amber,
            markup_heading: indigo,
            markup_raw: emerald,
            markup_link: pink,
            markup_reference: amber,
            markup_emphasis: pink,
            markup_strong: indigo,
            invalid: red,
        }
    }

    fn light(text: Hsla, comment: Hsla, danger: Hsla) -> Self {
        // Match the light graph's hue families at text-safe lightness.
        let indigo = git_graph_tone(oklch(0.47, 0.20, 276.966));
        let pink = git_graph_tone(oklch(0.47, 0.17, 0.584));
        let emerald = git_graph_tone(oklch(0.46, 0.11, 163.225));
        let amber = git_graph_tone(oklch(0.47, 0.12, 48.998));
        let red = git_graph_tone(danger);
        Self {
            comment,
            keyword: indigo,
            string: emerald,
            string_special: pink,
            escape: pink,
            number: amber,
            boolean: amber,
            type_name: amber,
            type_builtin: emerald,
            constructor: amber,
            function: indigo,
            function_builtin: pink,
            macro_name: pink,
            property: amber,
            constant: emerald,
            variable: text,
            variable_special: pink,
            parameter: text,
            operator: text,
            punctuation: text,
            tag: pink,
            attribute: amber,
            label: amber,
            markup_heading: indigo,
            markup_raw: emerald,
            markup_link: pink,
            markup_reference: amber,
            markup_emphasis: pink,
            markup_strong: indigo,
            invalid: red,
        }
    }

    fn for_appearance(appearance: Appearance, text: Hsla, comment: Hsla, danger: Hsla) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(text, comment, danger),
            Appearance::Light => Self::light(text, comment, danger),
        }
    }
}

/// Git history intentionally softens lane saturation so the graph remains
/// colorful without competing with content. Syntax uses the same treatment.
fn git_graph_tone(mut color: Hsla) -> Hsla {
    color.s *= 0.72;
    color
}

/// The app theme. Two concrete instances — [`Theme::dark`] and [`Theme::light`].
#[derive(Debug, Clone)]
pub struct Theme {
    /// Which appearance these tokens were built for.
    pub appearance: Appearance,
    /// Stable id of the resolved theme variant.
    pub variant_id: SharedString,
    /// Stable id of the family that owns [`Self::variant_id`].
    pub family_id: SharedString,
    /// Whether the base theme or a user preset owns interactive identity.
    pub accent_selection: AccentSelection,
    /// The persisted policy that resolved [`Self::surface_treatment`].
    pub surface_preference: SurfacePreference,
    /// The effective treatment after applying [`Self::surface_preference`] to
    /// the selected variant's recommendation.
    pub surface_treatment: SurfaceTreatment,
    /// The selected interactive accent used to build this theme.
    pub accent_color: AccentColor,

    // ---- paint: neutral surfaces ----
    /// Main content panel. Dark: the deepest plane (#060606). Light: pure white —
    /// long-form content reads best on an unbroken white field.
    pub bg: Hsla,
    /// Shell / sidebar surface. Dark: one step *up* from `bg`. Light: one step
    /// *down* (grey) — chrome recedes from the content plane in both, which is
    /// the direction a naive invert gets backwards.
    pub surface: Hsla,
    /// Raised surface: opaque pills and chips that sit proud of the panel.
    /// Dark: lighter than `surface`. Light: white, separated by `border` +
    /// shadow rather than by lightness.
    pub surface_raised: Hsla,

    // ---- paint: elevation ladder ----
    //
    // Dark mode distinguishes floating planes by lightness, and the steps are
    // *small* (#0e → #10 → #16 → #1e). They are not interchangeable: collapsing
    // them onto one token visibly lifts popovers off their intended plane.
    //
    // Light mode cannot use the same trick, because the content plane is already
    // white and there is nothing lighter to climb to. All three land on white and
    // let `border` + shadow carry the separation instead — the standard light-UI
    // answer, and the reason this is a ladder of tokens rather than an arithmetic
    // offset applied to one.
    /// Inline card resting on the main panel (auth gate, empty-state cards).
    pub surface_card: Hsla,
    /// Modal dialog, floating over a [`Theme::scrim`].
    pub surface_dialog: Hsla,
    /// Popover, menu and command-palette surface — the highest plane.
    pub surface_overlay: Hsla,
    /// Hover wash for interactive rows/buttons.
    pub element_hover: Hsla,
    /// Active/selected wash.
    pub element_active: Hsla,
    /// Hairline border.
    pub border: Hsla,
    /// Stronger border for focused/raised edges.
    pub border_strong: Hsla,

    // ---- paint: text ----
    /// Primary text. ~17.5:1 on its own background in both appearances.
    pub text: Hsla,
    /// Muted text: timestamps, secondary labels. ~7.5–8:1.
    pub text_muted: Hsla,
    /// Faint text: placeholders, disabled. ~4.5:1 — AA for body copy.
    pub text_faint: Hsla,
    /// One notch below `text_muted` — the diff file-path tone. It exists as its
    /// own token rather than being folded into `text_muted` because the dark
    /// value was sampled (#989898) and folding it would shift that label, which
    /// is a palette change dressed up as a refactor.
    pub text_dim: Hsla,

    // ---- paint: high-contrast solid (primary buttons) ----
    /// The maximum-contrast solid fill: near-white on dark, near-black on light.
    /// This is the primary button plate.
    pub solid: Hsla,
    /// Label/icon color on top of [`Self::solid`] — its inverse.
    pub on_solid: Hsla,

    // ---- paint: accents ----
    /// Primary tone in the selected accent family.
    pub accent: Hsla,
    /// Stronger accent for fills that carry [`Self::on_accent`] text.
    pub accent_strong: Hsla,
    /// Low-emphasis wash of the same accent for tinted identity surfaces.
    pub accent_wash: Hsla,
    /// Label color on top of [`Self::accent_strong`].
    pub on_accent: Hsla,
    /// Danger — red (errors, stop button).
    pub danger: Hsla,
    /// Softer danger for secondary/inline error copy.
    pub danger_muted: Hsla,
    /// Warning — amber (offline notices, awaiting-input).
    pub warning: Hsla,
    /// Softer warning for secondary copy.
    pub warning_muted: Hsla,
    /// Success / online — emerald.
    pub success: Hsla,
    /// Working / streaming indicator in the selected accent family.
    pub busy: Hsla,
    /// Three-tone animated pixel glyph palette in the selected accent family.
    pub glyph: GlyphPalette,
    /// Softer success for text on a success-tinted chip.
    pub success_muted: Hsla,

    // ---- paint: components ----
    /// Hover tone for an *opaque* raised pill. Hover must brighten the plate in
    /// dark mode, never swap it for a translucent wash (that made pills go
    /// see-through — user-reported); in light mode it darkens instead, same idea.
    pub surface_raised_hover: Hsla,
    /// Recessed band behind a palette/picker header or footer strip. Translucent
    /// so the glass still reads through.
    pub band: Hsla,
    /// The composer pill and other input plates.
    ///
    /// Its own token because "lifted" inverts between appearances. On dark, a
    /// faint *white* wash over near-black reads as raised. The literal light
    /// translation — a faint *black* wash on white — reads as **recessed**, a dent
    /// rather than a plate, which is why the prompt looked like bare text on a
    /// smudge. Light mode lifts the way light UIs actually do: pure white, with
    /// the border and shadow carrying the elevation.
    pub input_bg: Hsla,
    /// Text-selection highlight in the composer and inputs.
    pub selection: Hsla,
    /// Terminal block cursor.
    pub cursor: Hsla,
    /// Composer text caret in the selected accent family.
    pub caret: Hsla,
    /// Destructive-action button fill (danger plate, carries [`Self::on_accent`]).
    pub danger_strong: Hsla,

    // ---- paint: code & diff ----
    /// Inline-code text in the selected accent family.
    pub code_text: Hsla,
    /// Inline-code wash behind [`Self::code_text`].
    pub code_wash: Hsla,
    /// Shared paint-only syntax palette.
    pub syntax: SyntaxPalette,
    /// Diff: added lines.
    pub diff_add: Hsla,
    /// Diff: deleted lines.
    pub diff_del: Hsla,
    /// Diff: hunk-header wash (bluish grey).
    pub diff_hunk_bg: Hsla,

    /// Theme-owned terminal background, selection, and ANSI16 palette.
    pub terminal: TerminalColors,

    // ---- fonts ----
    /// UI font family (bundling of Geist lands with asset work; until then the
    /// text system falls back to the system sans when the family is missing).
    pub font_sans: SharedString,
    /// Fixed Geist chrome for code-adjacent surfaces and recovery controls.
    pub font_sans_fixed: SharedString,
    /// User-selected family for code, diffs, and file editors.
    pub font_mono: SharedString,
    /// User-selected family for terminal output.
    pub font_terminal: SharedString,
    /// Absolute pixel sizes for those two surfaces. They live on the theme
    /// because every render site that needs them already holds a `Theme`.
    pub code_font_size: f32,
    pub terminal_font_size: f32,
    /// Explicit system fallbacks, for callers that want to skip the lookup.
    pub font_sans_fallback: SharedString,
    pub font_mono_fallback: SharedString,
}

#[derive(Debug, Clone)]
pub struct TerminalColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub selection: Hsla,
    pub ansi: [Hsla; 16],
}

impl TerminalColors {
    fn from_variant(variant: &ThemeVariant) -> Self {
        Self {
            background: model_color(variant.terminal.background),
            foreground: model_color(variant.terminal.foreground),
            selection: model_color(variant.terminal.selection),
            ansi: variant.terminal.ansi.map(model_color),
        }
    }

    fn zeron(appearance: Appearance) -> Self {
        let id = match appearance {
            Appearance::Dark => "zeron-dark",
            Appearance::Light => "zeron-light",
        };
        let registry = ThemeRegistry::active();
        Self::from_variant(registry.variant(id).expect("Zeron terminal palette exists"))
    }
}

#[path = "theme/behavior.rs"]
mod behavior;

fn scrollbar_thumb_colors(theme: &Theme) -> (Hsla, Hsla, Hsla) {
    (
        theme.text.opacity(0.30),
        theme.text.opacity(0.42),
        theme.text.opacity(0.55),
    )
}

fn sync_gpui_base_scrollbar(theme: &Theme, cx: &mut App) {
    let (normal, hover, active) = scrollbar_thumb_colors(theme);
    gpui_base::Theme::global_mut(cx).scrollbar.styles = gpui_base::ScrollbarStyles::default()
        .thumb(|style| style.bg(normal))
        .thumb_hover(|style| style.bg(hover))
        .thumb_active(|style| style.bg(active));
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Global for Theme {}

fn system_sans() -> &'static str {
    if cfg!(target_os = "macos") {
        "Helvetica"
    } else if cfg!(target_os = "windows") {
        "Segoe UI"
    } else {
        "DejaVu Sans"
    }
}

fn system_mono() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

/// A neutral (chroma 0) oklch tone as Hsla. Chroma 0 means r == g == b exactly,
/// so this goes straight to an achromatic Hsla (skipping the hue math avoids
/// float-noise saturation).
pub fn neutral(lightness: f32) -> Hsla {
    let [v, _, _] = oklch_to_srgb(lightness, 0.0, 0.0);
    hsla(0.0, 0.0, v, 1.0)
}

/// Translucent **fill** ink for interactive states and chip plates: soft-white on
/// dark, soft-black on light at [`INK_FILL_SCALE`] of the alpha.
///
/// Alphas are quoted in *dark-mode terms* at every call site — the dark theme is
/// the tuned one — and the light value is derived. Callers keep one number and
/// both appearances stay in the relationship the dark tuning established.
///
/// Fills must never rest on transparent BLACK in dark mode: fully opaque washes
/// killed the glass and flashed dark mid-fade (user reports), so hover fades rest
/// on `ink(0.0)`, which stays tonally correct at zero alpha.
pub fn ink(alpha: f32) -> Hsla {
    ink_for(current_appearance(), alpha)
}

fn ink_for(appearance: Appearance, alpha: f32) -> Hsla {
    match appearance {
        // Soft-white, not pure white: alphas are high enough to stay visible at
        // the brightest backdrop the 0.90 glass scrim can produce.
        Appearance::Dark => hsla(0.0, 0.0, 1.0, alpha),
        Appearance::Light => hsla(0.0, 0.0, 0.0, alpha * INK_FILL_SCALE),
    }
}

/// Translucent **hairline** ink for borders, dividers and rings: white on dark,
/// black on light at [`INK_HAIRLINE_SCALE`] of the alpha.
///
/// Separate from [`ink`] because edges and fills scale in opposite directions
/// when the field brightens — a 1px line needs *more* ink on white, a plate needs
/// less.
pub fn hairline(alpha: f32) -> Hsla {
    hairline_for(current_appearance(), alpha)
}

fn hairline_for(appearance: Appearance, alpha: f32) -> Hsla {
    match appearance {
        Appearance::Dark => hsla(0.0, 0.0, 1.0, alpha),
        Appearance::Light => hsla(0.0, 0.0, 0.0, (alpha * INK_HAIRLINE_SCALE).min(0.5)),
    }
}

/// Interactive-state wash: a softened [`ink`] that stops short of pure black or
/// white so hover plates read as tinted glass rather than paint.
pub fn wash(alpha: f32) -> Hsla {
    wash_for(current_appearance(), alpha)
}

fn wash_for(appearance: Appearance, alpha: f32) -> Hsla {
    match appearance {
        Appearance::Dark => hsla(0.0, 0.0, 0.92, alpha),
        Appearance::Light => hsla(0.0, 0.0, 0.10, alpha * INK_FILL_SCALE),
    }
}

/// Alpha of the standard modal backdrop in dark mode. Call sites that need a
/// heavier or lighter scrim pass their own dark-mode alpha to [`scrim`].
pub const SCRIM_ALPHA_DARK: f32 = 0.60;

/// Modal backdrop at `alpha_dark` (quoted, as everywhere, in dark-mode terms).
///
/// Black in both appearances — a scrim's job is to darken what is behind it, and
/// a "light scrim" of white would wash the modal out rather than seat it. What
/// changes is strength: on a bright field a dark-mode-weight scrim reads as a
/// blackout, so light mode scales to roughly half.
pub fn scrim(alpha_dark: f32) -> Hsla {
    scrim_for(current_appearance(), alpha_dark)
}

fn scrim_for(appearance: Appearance, alpha_dark: f32) -> Hsla {
    match appearance {
        Appearance::Dark => hsla(0.0, 0.0, 0.0, alpha_dark),
        Appearance::Light => hsla(0.0, 0.0, 0.0, 0.32 * (alpha_dark / SCRIM_ALPHA_DARK)),
    }
}

/// Recessed band behind a palette/picker header or footer strip.
///
/// A free function as well as a [`Theme`] field because the picker chrome that
/// paints it is built from context-free helpers; both resolve to the same value.
pub fn band() -> Hsla {
    band_for(current_appearance())
}

fn band_for(appearance: Appearance) -> Hsla {
    match appearance {
        Appearance::Dark => hsla(0.0, 0.0, 0.0, 0.16),
        // A recessed strip on white needs far less ink than on near-black; the
        // dark 16% would read as a bruise.
        Appearance::Light => hsla(0.0, 0.0, 0.0, 0.045),
    }
}

/// Selected-state glass treatment (tabs, session rows, space rows): a
/// TRANSLUCENT wash the vibrancy reads through — heavier flat washes blocked
/// the glass (user request). Dark: the 11% [`wash`]. Light: the tone-flipped
/// wash at 6% — 11% black read too dark over the bright frost (user report;
/// light also previously ran a near-opaque white chip, rejected the same
/// way). Same fill as [`Theme::glass_hover`] — the ring in
/// [`glass_selected_shadows`] is what distinguishes selection. Selection
/// *inside floating cards* is different — see [`card_selected_bg`].
pub fn glass_selected_bg() -> Hsla {
    match current_appearance() {
        Appearance::Dark => wash(0.11),
        Appearance::Light => wash(0.14),
    }
}

/// The user message bubble's plate: the same translucent wash family as
/// [`glass_selected_bg`], one step softer — at the selection weight the
/// bubble read too strong for settled content (user report), and an opaque
/// plate before that read as a solid slab over glass.
pub fn user_bubble_bg() -> Hsla {
    match current_appearance() {
        Appearance::Dark => wash(0.08),
        Appearance::Light => wash(0.04),
    }
}

/// Selected/keyboard-active treatment for rows and chips INSIDE a floating
/// card (menu rows, the picker rail, segmented chips). The card is already the
/// bright plane in light mode, so a white lift can't read there — selection is
/// the tone-flipped grey wash.
pub fn card_selected_bg() -> Hsla {
    match current_appearance() {
        Appearance::Dark => wash(0.11),
        Appearance::Light => wash(0.12),
    }
}

/// The selected chip's bright outline, as an INSET shadow: gpui paints inset
/// shadows ON TOP of the background, edges only — a border with zero layout
/// cost. Drop shadows are filled rects painted BEHIND the element, and behind
/// a 5% fill they showed straight through as an opaque dark plate with a
/// greyed ring (user report) — nothing may paint behind a glass chip.
///
/// Light pins the ring at a flat 7% black rather than the scaled hairline:
/// heavier rings (the [`INK_HAIRLINE_SCALE`]d value, then 12%) outlined every
/// selected chip in a dark box (user reports) — the ring should define the
/// chip the way dark's 9% white ring does, not frame it.
///
/// There is deliberately NO drop-shadow seat under the light chip. Three
/// recipes were tried (a tight 10% layer, a 6% contact + 5% ambient pair, a
/// lone 4% whisper) and every one failed on sight: layers sum into a grey rim
/// exactly where the chip meets the frost, gpui's small-radius blur reads
/// coarse on a bright field, and the tab strip is a scroll container that
/// clips its children vertically — any shadow escaping the chip gets cut off
/// mid-fade. The near-opaque fill plus the ring carry selection, exactly as
/// dark's wash plus ring does; the two appearances share one recipe now.
pub fn glass_selected_shadows() -> Vec<gpui::BoxShadow> {
    card_selected_shadows()
}

/// Selection outline for rows and chips INSIDE a floating card (menu rows,
/// the picker rail, segmented chips): the inset ring alone, in both
/// appearances. Card rows fill with a translucent wash
/// ([`card_selected_bg`]), and a drop shadow — a filled rect painted BEHIND
/// the element — shows straight through a translucent fill as a grey plate
/// (the same lesson [`glass_selected_shadows`] records for dark glass). The
/// card already carries the elevation shadow; selection inside it only needs
/// the edge.
pub fn card_selected_shadows() -> Vec<gpui::BoxShadow> {
    let color = match current_appearance() {
        Appearance::Dark => hairline(0.09),
        Appearance::Light => hsla(0.0, 0.0, 0.0, 0.07),
    };
    vec![gpui::BoxShadow {
        color,
        offset: gpui::point(gpui::px(0.0), gpui::px(0.0)),
        blur_radius: gpui::px(0.0),
        spread_radius: gpui::px(1.0),
        inset: true,
    }]
}

/// An exact achromatic tone from an 8-bit channel value (`grey(13)` ≡ `#0d0d0d`)
/// — for surfaces matched against reference-screenshot samples.
pub fn grey(value: u8) -> Hsla {
    hsla(0.0, 0.0, value as f32 / 255.0, 1.0)
}

/// Convert an oklch color (CSS notation: L 0..1, C, H in degrees) to gpui Hsla.
pub fn oklch(l: f32, c: f32, h_deg: f32) -> Hsla {
    let [r, g, b] = oklch_to_srgb(l, c, h_deg);
    let (h, s, l) = rgb_to_hsl(r, g, b);
    hsla(h, s, l, 1.0)
}

/// oklch → sRGB (each 0..1, clamped/gamut-clipped per channel).
/// Reference: Björn Ottosson's OKLab definition (the same matrices CSS Color 4 uses).
pub(crate) fn oklch_to_srgb(l: f32, c: f32, h_deg: f32) -> [f32; 3] {
    let h = h_deg.to_radians();
    let a = c * h.cos();
    let b = c * h.sin();

    // OKLab → LMS (cube roots undone)
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    // LMS → linear sRGB
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_93 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;

    [gamma_encode(r), gamma_encode(g), gamma_encode(b)]
}

fn gamma_encode(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB (0..1 components) → HSL, all components 0..1 (gpui's Hsla convention).
pub(crate) fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    (h, s, l)
}

/// HSL (gpui convention, all 0..1) → sRGB components 0..1.
pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= f32::EPSILON {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0)]
}

/// WCAG 2.1 relative luminance of an opaque color.
pub fn relative_luminance(color: Hsla) -> f32 {
    let lin = |c: f32| {
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = hsl_to_rgb(color.h, color.s, color.l);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG 2.1 contrast ratio between two opaque colors (1.0 … 21.0).
///
/// Used by the palette tests to prove each light token reproduces the contrast
/// its dark counterpart had, rather than merely looking plausible.
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn painted_contrast(foreground: Hsla, background: Hsla) -> f32 {
    contrast_ratio(flatten(foreground, background), background)
}

/// Composite `fg` (which may be translucent) over an opaque `bg`, returning the
/// opaque result — the color the eye actually receives.
pub fn flatten(fg: Hsla, bg: Hsla) -> Hsla {
    let a = fg.a.clamp(0.0, 1.0);
    let [fr, fg_, fb] = hsl_to_rgb(fg.h, fg.s, fg.l);
    let [br, bg_, bb] = hsl_to_rgb(bg.h, bg.s, bg.l);
    let (h, s, l) = rgb_to_hsl(
        fr * a + br * (1.0 - a),
        fg_ * a + bg_ * (1.0 - a),
        fb * a + bb * (1.0 - a),
    );
    hsla(h, s, l, 1.0)
}

/// Linear per-component mix of two colors (paint helper for the gradient spinner).
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    // Mix through hue naively — both spinner endpoints sit close enough on the
    // wheel that shortest-arc handling isn't needed for our palette.
    hsla(
        lerp(a.h, b.h),
        lerp(a.s, b.s),
        lerp(a.l, b.l),
        lerp(a.a, b.a),
    )
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
