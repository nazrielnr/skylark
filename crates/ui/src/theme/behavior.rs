//! Theme derivation, installation, and surface behavior.

use super::*;

impl Theme {
    // ---- numbers drive layout (px) ----
    /// Frost translucency over the blurred window background (macOS vibrancy
    /// or Windows Acrylic). Linux stays opaque because compositor blur is not
    /// guaranteed; a merely transparent window would expose the raw desktop.
    /// Darkness matched by eye to a reference Electron app's dark glass. That
    /// scrim is 0.76 over `hsl(0 0% 3%)`, but it sits on Electron's
    /// `under-window` vibrancy MATERIAL, which pre-darkens the blur; our bare
    /// backdrop blur has no material layer, so the scrim runs heavier to land
    /// on the same perceived tone (see [`Theme::glass`]).
    pub const GLASS_ALPHA: f32 = if cfg!(any(target_os = "macos", target_os = "windows")) {
        0.80
    } else {
        1.0
    };
    /// Light-mode frost alpha — glass-forward, like dark mode.
    ///
    /// A light tint controls the blur less than a dark one: the desktop's
    /// colour bleeds through more readily, so light frost runs *heavier* than
    /// an equal-looking dark frost to keep the chrome on a known-enough
    /// background for its labels (macOS light sidebars do the same — their
    /// vibrancy material is mostly white). Floating cards compensate further:
    /// see [`Self::glass_overlay`], where light coverage steps up to keep menu
    /// text legible over an unknown backdrop.
    pub const GLASS_ALPHA_LIGHT: f32 = if cfg!(any(target_os = "macos", target_os = "windows")) {
        0.80
    } else {
        1.0
    };
    /// Main-panel header height (skylark `h-11`) — in-card headers (changes pane).
    pub const HEADER_HEIGHT: f32 = 44.0;
    /// The unified window titlebar (traffic lights + cluster + tabs). Content
    /// is vertically centered so the space above and below the 28px controls is equal (6px).
    pub const TITLEBAR_HEIGHT: f32 = 40.0;
    /// Top padding offset for titlebar content flex rows.
    pub const TITLEBAR_TOP_PAD: f32 = 0.0;
    /// Reserved status strip under the content outlet (skylark `h-6`) — the
    /// WorkingIndicator row; reserving it keeps the composer from shifting.
    pub const STATUS_STRIP_HEIGHT: f32 = 24.0;
    /// Height of the gradient that fades the transcript into the panel
    /// background at its bottom edge. The transcript's last row must pad
    /// itself past this band so settled content (message text, the
    /// hover-revealed timestamp) never sits inside the fade when scrolled
    /// to the bottom.
    pub const TRANSCRIPT_FADE_BAND: f32 = 24.0;
    /// Message bubble corner radius.
    pub const BUBBLE_RADIUS: f32 = 16.0;
    /// Panel / card corner radius.
    pub const PANEL_RADIUS: f32 = 10.0;
    /// Small control radius (buttons, chips).
    pub const CONTROL_RADIUS: f32 = 6.0;
    /// Base spacing steps.
    pub const SPACE_XS: f32 = 4.0;
    pub const SPACE_SM: f32 = 8.0;
    pub const SPACE_MD: f32 = 12.0;
    pub const SPACE_LG: f32 = 16.0;
    /// Optical separation for a tightly coupled title/description stack.
    /// This is intentionally outside the base spacing ladder: it corrects
    /// line-box whitespace rather than separating layout regions.
    pub const TEXT_STACK_GAP: f32 = 1.0;

    /// The selected theme's shell tint painted over the blurred window
    /// background (macOS glass). Keeping the hue theme-owned matters when a
    /// user forces frost onto a palette authored for an opaque workbench: a
    /// fixed Skylark grey would erase that palette's identity.
    pub fn glass(&self) -> Hsla {
        if self.surface_treatment == SurfaceTreatment::Opaque {
            return self.surface;
        }
        #[cfg(target_os = "windows")]
        {
            // Windows 11 DWM handles dark/light backdrop tinting natively via
            // DWMWA_USE_IMMERSIVE_DARK_MODE and DWMWA_SYSTEMBACKDROP_TYPE (Mica Alt).
            // A lighter root tint lets Mica Alt's wallpaper depth show through clearly,
            // while light mode needs heavier coverage to maintain crisp contrast.
            let alpha = match self.appearance {
                Appearance::Dark => 0.20,
                Appearance::Light => 0.70,
            };
            return self.surface.opacity(alpha);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let base = match self.appearance {
                Appearance::Dark => Self::GLASS_ALPHA,
                Appearance::Light => Self::GLASS_ALPHA_LIGHT,
            };
            self.surface
                .opacity(self.contrast_checked_glass_alpha(base))
        }
    }

    /// Raise a requested tint's coverage until primary and muted shell text
    /// remain legible against the adverse desktop luminance for this
    /// appearance. A theme with unusually delicate contrast may therefore get
    /// denser glass, never silently broken text.
    fn contrast_checked_glass_alpha(&self, base: f32) -> f32 {
        self.contrast_checked_tint_alpha(self.surface, base, self.adverse_backdrop())
    }

    /// Increase tint coverage only as far as needed for Skylark's shared text
    /// roles. This is used for both window glass and in-app frosted surfaces,
    /// whose blurred content can otherwise invalidate an imported palette's
    /// original solid-background assumptions.
    fn contrast_checked_tint_alpha(&self, tint: Hsla, base: f32, backdrop: Hsla) -> f32 {
        for step in 0..=20 {
            let alpha = base + (1.0 - base) * step as f32 / 20.0;
            let composite = flatten(tint.opacity(alpha), backdrop);
            if painted_contrast(self.text, composite) >= 4.5
                && painted_contrast(self.text_muted, composite) >= 3.0
            {
                return alpha;
            }
        }
        1.0
    }

    fn adverse_backdrop(&self) -> Hsla {
        match self.appearance {
            Appearance::Dark => grey(0xff),
            Appearance::Light => grey(0),
        }
    }

    /// Whether this appearance paints translucent chrome over the blurred
    /// desktop. Window-glass recipes such as translucent chrome and per-glyph
    /// edge fades must gate on this, not on
    /// [`Self::GLASS_ALPHA`]: that constant is platform-wide, while the frost
    /// alpha (and with it whether glass is on at all) is per-appearance.
    pub fn is_glass(&self) -> bool {
        self.glass().a < 1.0
    }

    /// Whether FLOATING surfaces (popovers, the composer pill) paint their
    /// backdrop blur and translucent tints. Unlike [`Self::is_glass`] this is
    /// scene-level: the blur runs on in-app content inside the window, not on
    /// the desktop behind it, so it needs no compositor vibrancy — macOS
    /// rasterizes it in Metal, Linux in wgpu, and Windows in Direct3D.
    /// Windows window chrome uses native Acrylic independently of these
    /// scene-level blurs.
    pub fn is_frost(&self) -> bool {
        self.surface_treatment == SurfaceTreatment::Frosted
            && cfg!(any(
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            ))
    }

    /// Theme-owned hover wash for chrome that sits on glass (sidebar rows,
    /// tabs, titlebar buttons). The importer maps this role from the source
    /// theme, so forcing frost does not reintroduce Skylark's neutral hover.
    pub fn glass_hover(&self) -> Hsla {
        self.element_hover
    }

    /// Muted popup text is the theme foreground composited onto the glass.
    /// Fixed opaque grays turn muddy over colorful or bright backgrounds.
    pub fn for_popup(&self) -> Self {
        let mut popup = self.clone();
        if self.is_frost() {
            popup.text_muted = self.text.opacity(0.64);
            popup.text_faint = self.text.opacity(0.48);
        }
        popup
    }

    /// The theme-owned tint floating cards paint over their backdrop blur (see
    /// [`crate::frost::frosted`]). Light coverage stays heavier because dark
    /// text is more vulnerable to unpredictable content behind a popover.
    pub fn glass_overlay(&self) -> Hsla {
        let base = match self.appearance {
            Appearance::Dark => self.surface_overlay.opacity(0.50),
            Appearance::Light => self.surface_overlay.opacity(0.85),
        };
        if !self.is_frost() {
            return self.surface_overlay;
        }
        self.surface_overlay
            .opacity(self.contrast_checked_tint_alpha(
                self.surface_overlay,
                base.a,
                self.adverse_backdrop(),
            ))
    }

    /// Move toward the right pane tone while keeping the backdrop visible.
    /// Solve the overlay in RGB: target = tint * alpha + canvas * (1 - alpha).
    pub fn composer_sidebar_tint(&self) -> Hsla {
        let target = if self.is_glass() {
            flatten(self.bg.opacity(0.4), flatten(self.glass(), self.bg))
        } else {
            self.bg
        };
        // The transcript has no fill of its own: its canvas is the shell glass.
        let canvas = flatten(self.glass(), self.bg);
        let canvas = hsl_to_rgb(canvas.h, canvas.s, canvas.l);
        let target = hsl_to_rgb(target.h, target.s, target.l);
        let mut alpha: f32 = 0.60;
        for (base, desired) in canvas.into_iter().zip(target) {
            let needed = if desired > base {
                (desired - base) / (1.0 - base).max(f32::EPSILON)
            } else {
                (base - desired) / base.max(f32::EPSILON)
            };
            alpha = alpha.max(needed);
        }
        let rgb = std::array::from_fn::<_, 3, _>(|i| {
            ((target[i] - canvas[i] * (1.0 - alpha)) / alpha).clamp(0.0, 1.0)
        });
        let (h, s, l) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
        // Use the compensated hue, but leave 85% of the blurred backdrop visible.
        hsla(h, s, l, 0.15)
    }

    /// Shared fill for the composer, queue tray, and input panels. Without
    /// frost, composite the theme's input tint onto the page to preserve its
    /// color while hiding the transcript and overlapping surfaces underneath.
    /// Frosted surfaces retain their translucent, contrast-checked tint.
    pub fn input_glass_bg(&self) -> Hsla {
        if !self.is_frost() {
            return flatten(self.input_bg, self.bg);
        }
        let base = if matches!(self.appearance, Appearance::Light) {
            0.30
        } else {
            self.input_bg.a
        };
        let window = flatten(self.glass(), self.adverse_backdrop());
        self.input_bg
            .opacity(self.contrast_checked_tint_alpha(self.input_bg, base, window))
    }

    /// Section-card fill (settings cards and similar in-panel cards). The
    /// opaque `surface` tone read as a harsh solid slab floating on the
    /// frosted blur (user report), so glass thins it to a translucent tint;
    /// opaque platforms keep the true card tone.
    pub fn card_glass_bg(&self) -> Hsla {
        if !self.is_frost() {
            return self.surface;
        }
        let window = flatten(self.glass(), self.adverse_backdrop());
        self.surface
            .opacity(self.contrast_checked_tint_alpha(self.surface, 0.40, window))
    }

    /// The standard modal backdrop — see [`scrim`].
    pub fn scrim(&self) -> Hsla {
        scrim_for(self.appearance, SCRIM_ALPHA_DARK)
    }

    /// How the platform should composite the window behind our paint.
    ///
    /// Only dark macOS wants the blurred desktop — light chrome is opaque by
    /// design ([`Self::GLASS_ALPHA_LIGHT`]), so it keeps opaque compositing
    /// (subpixel-friendly, no vibrancy cost for a blur nothing shows). This is
    /// a method rather than a constant because it has to be *re-applied* after
    /// every theme swap: gpui's macOS backend tears the `NSVisualEffectView`
    /// out of the hierarchy whenever the value is anything but `Blurred`, and
    /// the re-apply in `appearance::apply` is what restores vibrancy when the
    /// user switches back to dark. See zed's `crates/zed/src/main.rs`, which
    /// runs the same loop on every settings change.
    ///
    /// Linux composites with alpha instead: the shell draws CSD chrome, and
    /// rounded window corners (when floating) need the corner cutouts to be
    /// genuinely transparent. The frost itself is opaque off macOS
    /// ([`Self::GLASS_ALPHA`]), so nothing else shows through — only the
    /// corners.
    pub fn window_background_appearance(&self) -> gpui::WindowBackgroundAppearance {
        if cfg!(target_os = "linux") {
            gpui::WindowBackgroundAppearance::Transparent
        } else if self.is_glass() {
            #[cfg(target_os = "windows")]
            {
                gpui::WindowBackgroundAppearance::MicaAltBackdrop
            }
            #[cfg(not(target_os = "windows"))]
            {
                gpui::WindowBackgroundAppearance::Blurred
            }
        } else {
            gpui::WindowBackgroundAppearance::Opaque
        }
    }

    /// Build the dark theme. The surface tones are sampled straight from the
    /// reference screenshots of the original app (docs/reference): main panel
    /// `#060606`, shell/sidebar `#0d0d0d`.
    pub fn dark() -> Self {
        Self::dark_with_accent(AccentColor::default())
    }

    pub fn dark_with_accent(accent_color: AccentColor) -> Self {
        let accent = accent_color.tokens(Appearance::Dark);
        Self {
            appearance: Appearance::Dark,
            variant_id: "skylark-dark".into(),
            family_id: "skylark".into(),
            accent_selection: AccentSelection::Preset(accent_color.into()),
            surface_preference: SurfacePreference::ThemeDefault,
            surface_treatment: SurfaceTreatment::Frosted,
            accent_color,
            bg: grey(6),       // main panel — sampled #060606
            surface: grey(13), // shell / sidebar — sampled #0d0d0d
            surface_raised: neutral(0.235),
            surface_card: grey(0x0e),
            surface_dialog: grey(0x10),
            surface_overlay: grey(0x16),
            element_hover: hsla(0.0, 0.0, 0.92, 0.11),
            element_active: hsla(0.0, 0.0, 0.92, 0.16),
            border: hsla(0.0, 0.0, 1.0, 0.08),
            border_strong: hsla(0.0, 0.0, 1.0, 0.14),
            text: neutral(0.922),       // ~neutral-200
            text_muted: neutral(0.708), // ~neutral-400
            text_faint: neutral(0.556), // ~neutral-500
            text_dim: grey(0x98),
            solid: neutral(0.922), // near-white plate
            on_solid: grey(0x0e),  // near-black label
            accent: accent.primary,
            accent_strong: accent.strong,
            accent_wash: accent.wash,
            on_accent: neutral(0.985),
            danger: oklch(0.704, 0.191, 22.216),       // red-400
            danger_muted: oklch(0.808, 0.114, 19.571), // red-300
            warning: oklch(0.828, 0.189, 84.429),      // amber-400
            warning_muted: oklch(0.924, 0.12, 95.746), // amber-200
            success: oklch(0.765, 0.177, 163.223),     // emerald-400
            busy: accent.activity,
            glyph: accent.glyph,
            success_muted: oklch(0.845, 0.143, 164.978), // emerald-300
            surface_raised_hover: neutral(0.29),
            band: band_for(Appearance::Dark),
            input_bg: hsla(0.0, 0.0, 1.0, 0.03),
            selection: accent.selection,
            cursor: hsla(0.0, 0.0, 1.0, 0.35),
            caret: accent.caret,
            danger_strong: oklch(0.58, 0.16, 25.0),
            code_text: accent.code_text,
            code_wash: accent.code_wash,
            syntax: SyntaxPalette::for_appearance(
                Appearance::Dark,
                neutral(0.922),
                neutral(0.60),
                oklch(0.704, 0.191, 22.216),
            ),
            diff_add: oklch(0.765, 0.177, 163.223), // emerald-400
            diff_del: oklch(0.704, 0.191, 22.216),  // red-400
            diff_hunk_bg: hsla(0.6, 0.35, 0.6, 0.05),
            terminal: TerminalColors::skylark(Appearance::Dark),
            font_sans: "Geist".into(),
            font_sans_fixed: "Geist".into(),
            font_mono: "Geist Mono".into(),
            font_terminal: "Geist Mono".into(),
            code_font_size: crate::typography::CODE_FONT_SIZE_DEFAULT,
            terminal_font_size: crate::typography::TERMINAL_FONT_SIZE_DEFAULT,
            font_sans_fallback: system_sans().into(),
            font_mono_fallback: system_mono().into(),
        }
    }

    /// Build the light theme.
    ///
    /// Neutrals are the same oklch scale read from the other end, but the *roles*
    /// are reassigned rather than mirrored (see the module docs): content plane
    /// white, chrome grey, raised surfaces white-plus-shadow. Text tones are
    /// picked to reproduce the dark theme's contrast ratios, and accents drop
    /// from the 400 to the 600 step at identical hue so they clear WCAG AA on
    /// white instead of glowing.
    pub fn light() -> Self {
        Self::light_with_accent(AccentColor::default())
    }

    pub fn light_with_accent(accent_color: AccentColor) -> Self {
        let accent = accent_color.tokens(Appearance::Light);
        Self {
            appearance: Appearance::Light,
            variant_id: "skylark-light".into(),
            family_id: "skylark".into(),
            accent_selection: AccentSelection::Preset(accent_color.into()),
            surface_preference: SurfacePreference::ThemeDefault,
            surface_treatment: SurfaceTreatment::Frosted,
            accent_color,
            bg: grey(0xff), // main panel — clean white
            // Deeper than ~neutral-100 looks on paper: the content card is pure
            // white and sits *inside* this surface, so too small a step leaves the
            // whole window one flat sheet with a hairline drawn on it.
            surface: neutral(0.968),
            // A real grey, NOT white. This is the opaque-plate tone — user
            // message bubbles, the jump-to-bottom pill — and those sit directly
            // on the white content plane with no border or shadow to save them.
            // White here made the user's own messages vanish into the page.
            // Popovers do not use this; they have their own ladder below.
            surface_raised: neutral(0.940),
            surface_card: grey(0xff),
            surface_dialog: grey(0xff),
            surface_overlay: grey(0xff),
            element_hover: hsla(0.0, 0.0, 0.10, 0.06),
            element_active: hsla(0.0, 0.0, 0.10, 0.10),
            border: hsla(0.0, 0.0, 0.0, 0.10),
            border_strong: hsla(0.0, 0.0, 0.0, 0.17),
            // ~neutral-850. Pure neutral-900 measures 17.9:1 on white — *more*
            // contrast than dark mode's 16.1:1, which reads as harsh rather than
            // crisp. Backing off to 0.25 lands at ~16:1: the same perceived
            // weight as the dark theme, not the maximum available.
            text: neutral(0.25),
            text_muted: neutral(0.439), // ~neutral-600 → ~7.7:1
            // A touch darker than dark mode's neutral-500 counterpart: the light
            // sidebar is a real grey, and faint text has to clear its floor there
            // too, not just on the white content plane.
            text_faint: neutral(0.535),
            text_dim: neutral(0.50),
            solid: neutral(0.205),    // near-black plate, deeper than body text
            on_solid: neutral(0.985), // near-white label
            accent: accent.primary,
            accent_strong: accent.strong,
            accent_wash: accent.wash,
            on_accent: neutral(0.985),
            danger: oklch(0.577, 0.245, 27.325),        // red-600
            danger_muted: oklch(0.505, 0.213, 27.518),  // red-700
            warning: oklch(0.555, 0.163, 48.998),       // amber-700 — carries 12px text
            warning_muted: oklch(0.473, 0.137, 46.201), // amber-800
            success: oklch(0.596, 0.145, 163.225),      // emerald-600
            busy: accent.activity,
            glyph: accent.glyph,
            success_muted: oklch(0.508, 0.118, 165.612), // emerald-700
            // Opaque pills darken on hover here rather than brighten — same
            // "brighten the plate, don't wash it out" rule, read the other way.
            surface_raised_hover: neutral(0.900),
            // A recessed strip on white needs far less ink than on near-black;
            // the dark 16% would read as a bruise.
            band: band_for(Appearance::Light),
            input_bg: grey(0xff),
            selection: accent.selection,
            cursor: hsla(0.0, 0.0, 0.0, 0.55),
            caret: accent.caret,
            danger_strong: oklch(0.51, 0.20, 25.0),
            code_text: accent.code_text,
            code_wash: accent.code_wash,
            syntax: SyntaxPalette::for_appearance(
                Appearance::Light,
                neutral(0.25),
                neutral(0.48),
                oklch(0.505, 0.213, 27.518),
            ),
            diff_add: oklch(0.596, 0.145, 163.225), // emerald-600
            diff_del: oklch(0.577, 0.245, 27.325),  // red-600
            diff_hunk_bg: hsla(0.6, 0.35, 0.35, 0.07),
            terminal: TerminalColors::skylark(Appearance::Light),
            font_sans: "Geist".into(),
            font_sans_fixed: "Geist".into(),
            font_mono: "Geist Mono".into(),
            font_terminal: "Geist Mono".into(),
            code_font_size: crate::typography::CODE_FONT_SIZE_DEFAULT,
            terminal_font_size: crate::typography::TERMINAL_FONT_SIZE_DEFAULT,
            font_sans_fallback: system_sans().into(),
            font_mono_fallback: system_mono().into(),
        }
    }

    /// Build the theme for an appearance.
    pub fn for_appearance(appearance: Appearance) -> Self {
        Self::for_preferences(appearance, AccentColor::default())
    }

    pub fn for_preferences(appearance: Appearance, accent: AccentColor) -> Self {
        match appearance {
            Appearance::Dark => Self::dark_with_accent(accent),
            Appearance::Light => Self::light_with_accent(accent),
        }
    }

    fn with_font_sans(mut self, family: SharedString) -> Self {
        self.font_sans = family;
        self
    }

    fn with_font_mono(mut self, family: SharedString) -> Self {
        self.font_mono = family;
        self
    }

    fn with_font_terminal(mut self, family: SharedString) -> Self {
        self.font_terminal = family;
        self
    }

    fn with_code_font_size(mut self, size: f32) -> Self {
        self.code_font_size = size;
        self
    }

    fn with_terminal_font_size(mut self, size: f32) -> Self {
        self.terminal_font_size = size;
        self
    }

    /// Resolve one complete registered variant and then apply the narrowly
    /// scoped accent policy. Components consume only the resulting semantic
    /// tokens; VS Code keys never escape the importer.
    pub fn for_selection(
        appearance: Appearance,
        variant_id: &str,
        accent_selection: AccentSelection,
        surface_preference: SurfacePreference,
    ) -> Self {
        let registry = ThemeRegistry::active();
        let fallback_id = match appearance {
            Appearance::Dark => "skylark-dark",
            Appearance::Light => "skylark-light",
        };
        let variant = registry
            .variant(variant_id)
            .filter(|variant| model_appearance(variant.appearance) == appearance)
            .or_else(|| registry.variant(fallback_id))
            .expect("the built-in registry contains both Skylark appearances");
        Self::from_variant(variant, accent_selection, surface_preference)
    }

    pub(crate) fn from_variant(
        variant: &ThemeVariant,
        accent_selection: AccentSelection,
        surface_preference: SurfacePreference,
    ) -> Self {
        let appearance = model_appearance(variant.appearance);
        let accent_color = match accent_selection {
            AccentSelection::ThemeDefault => AccentColor::Skylark,
            AccentSelection::Preset(preset) => preset.into(),
        };
        let mut theme = Self::for_preferences(appearance, accent_color);
        let colors = &variant.colors;
        let accent = variant.accent_for(accent_selection);
        let text_backgrounds = [
            colors.background,
            colors.shell,
            colors.raised,
            colors.card,
            colors.dialog,
            colors.overlay,
            colors.input,
        ];
        let is_curated_builtin = ThemeRegistry::builtin()
            .variant(&variant.id)
            .is_some_and(|builtin| builtin == variant);
        let safe_text = if is_curated_builtin {
            colors.text
        } else {
            harden_model_foreground(colors.text, &text_backgrounds, 4.5, None)
        };
        let safe_text_muted = if is_curated_builtin {
            colors.text_muted
        } else {
            harden_model_foreground(colors.text_muted, &text_backgrounds, 4.5, Some(safe_text))
        };
        theme.variant_id = variant.id.clone().into();
        theme.family_id = variant.family_id.clone().into();
        theme.accent_selection = accent_selection;
        theme.surface_preference = surface_preference;
        theme.surface_treatment = surface_preference.resolve(variant.recommended_surface_treatment);
        theme.bg = model_color(colors.background);
        theme.surface = model_color(colors.shell);
        theme.surface_raised = model_color(colors.raised);
        theme.surface_card = model_color(colors.card);
        theme.surface_dialog = model_color(colors.dialog);
        theme.surface_overlay = model_color(colors.overlay);
        theme.element_hover = model_color(colors.hover);
        theme.element_active = model_color(colors.active);
        theme.border = model_color(colors.border);
        theme.border_strong = model_color(colors.border_strong);
        theme.text = model_color(safe_text);
        theme.text_muted = model_color(safe_text_muted);
        theme.text_faint = model_color(colors.text_faint);
        theme.text_dim = model_color(safe_text_muted);
        theme.solid = model_color(colors.solid);
        theme.on_solid = model_color(if is_curated_builtin {
            colors.on_solid
        } else {
            harden_model_foreground(colors.on_solid, &[colors.solid], 4.5, Some(safe_text))
        });
        theme.accent = model_color(accent.primary);
        theme.accent_strong = model_color(accent.strong);
        theme.accent_wash = model_color(accent.wash);
        theme.on_accent = model_color(accent.on);
        theme.danger = model_color(colors.danger);
        theme.danger_muted = model_color(colors.danger_muted);
        theme.warning = model_color(colors.warning);
        theme.warning_muted = model_color(colors.warning_muted);
        theme.success = model_color(colors.success);
        theme.success_muted = model_color(colors.success_muted);
        theme.busy = model_color(accent.activity);
        theme.glyph = GlyphPalette {
            light: model_color(accent.glyph[0]),
            mid: model_color(accent.glyph[1]),
            deep: model_color(accent.glyph[2]),
        };
        theme.surface_raised_hover = model_color(colors.raised);
        theme.band = model_color(colors.hover);
        theme.input_bg = model_color(colors.input);
        theme.selection = model_color(accent.selection);
        theme.cursor = model_color(colors.cursor);
        theme.caret = model_color(accent.caret);
        theme.danger_strong = model_color(colors.danger);
        theme.code_text = model_color(accent.primary);
        theme.code_wash = model_color(accent.wash);
        theme.syntax = SyntaxPalette::from_variant(variant, theme.syntax);
        theme.diff_add = model_color(colors.diff_add);
        theme.diff_del = model_color(colors.diff_delete);
        theme.diff_hunk_bg = model_color(colors.diff_hunk);
        theme.terminal = TerminalColors::from_variant(variant);
        if !is_curated_builtin {
            theme.terminal.foreground = model_color(harden_model_foreground(
                variant.terminal.foreground,
                &[variant.terminal.background],
                4.5,
                Some(safe_text),
            ));
        }
        theme
    }

    /// Install the theme for `appearance` as the gpui global and point the
    /// context-free paint helpers at it. The **only** way the appearance should
    /// change — setting the global directly leaves [`current_appearance`] stale.
    pub fn install(appearance: Appearance, cx: &mut App) {
        Self::install_preferences(appearance, AccentColor::default(), cx);
    }

    pub fn install_preferences(appearance: Appearance, accent: AccentColor, cx: &mut App) {
        let accent_changed = cx
            .try_global::<Theme>()
            .is_some_and(|theme| theme.accent_color != accent);
        set_current_appearance(appearance);
        let next = Self::for_preferences(appearance, accent)
            .with_font_sans(crate::typography::effective_family_name(cx))
            .with_font_mono(crate::typography::code_effective_family_name(cx))
            .with_font_terminal(crate::typography::terminal_effective_family_name(cx))
            .with_code_font_size(crate::typography::code_font_size(cx))
            .with_terminal_font_size(crate::typography::terminal_font_size(cx));
        sync_gpui_base_scrollbar(&next, cx);
        cx.set_global(next);
        // An accent-only swap leaves CURRENT_APPEARANCE unchanged, but cached
        // resolved colors still need to be discarded for the next frame.
        if accent_changed {
            bump_style_generation();
        }
    }

    pub fn install_selection(
        appearance: Appearance,
        variant_id: &str,
        accent_selection: AccentSelection,
        surface_preference: SurfacePreference,
        cx: &mut App,
    ) {
        Self::install_selection_inner(
            appearance,
            variant_id,
            accent_selection,
            surface_preference,
            false,
            cx,
        );
    }

    /// Re-resolve a selection after its registry entry changed in place.
    /// Linked themes keep stable ids across reloads, so identity comparisons
    /// cannot detect that their resolved colors moved. Forcing the generation
    /// invalidates paint caches that contain already-resolved colors.
    pub fn reinstall_selection(
        appearance: Appearance,
        variant_id: &str,
        accent_selection: AccentSelection,
        surface_preference: SurfacePreference,
        cx: &mut App,
    ) {
        Self::install_selection_inner(
            appearance,
            variant_id,
            accent_selection,
            surface_preference,
            true,
            cx,
        );
    }

    fn install_selection_inner(
        appearance: Appearance,
        variant_id: &str,
        accent_selection: AccentSelection,
        surface_preference: SurfacePreference,
        force_generation: bool,
        cx: &mut App,
    ) {
        let next =
            Self::for_selection(appearance, variant_id, accent_selection, surface_preference)
                .with_font_sans(crate::typography::effective_family_name(cx))
                .with_font_mono(crate::typography::code_effective_family_name(cx))
                .with_font_terminal(crate::typography::terminal_effective_family_name(cx))
                .with_code_font_size(crate::typography::code_font_size(cx))
                .with_terminal_font_size(crate::typography::terminal_font_size(cx));
        let changed = cx.try_global::<Theme>().is_some_and(|theme| {
            theme.variant_id != next.variant_id
                || theme.accent_selection != next.accent_selection
                || theme.surface_preference != next.surface_preference
                || theme.appearance != next.appearance
        });
        set_current_appearance(appearance);
        sync_gpui_base_scrollbar(&next, cx);
        cx.set_global(next);
        if changed || force_generation {
            bump_style_generation();
        }
    }

    /// Read the theme global.
    pub fn of(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }

    /// Overlay ink at `alpha` — see [`ink`].
    pub fn ink(&self, alpha: f32) -> Hsla {
        ink_for(self.appearance, alpha)
    }

    /// Hairline ink at `alpha` — see [`hairline`].
    pub fn hairline(&self, alpha: f32) -> Hsla {
        hairline_for(self.appearance, alpha)
    }

    /// State wash at `alpha` — see [`wash`].
    pub fn wash(&self, alpha: f32) -> Hsla {
        wash_for(self.appearance, alpha)
    }
}
