//! Shared menu, palette, dialog, and loading primitives.

use super::*;

/// One menu row (skylark `menuItem`): `gap-2.5 rounded-lg px-2 py-1.5
/// text-[13px]`, active = `bg-white/10 text-foreground`, hover wash
/// `white/[0.08]` fading over `transition-colors` (floating-styles.ts) via the
/// per-`fade_key` [`motion::hover_blend`]. The caller adds the id/click
/// listener — `fade_key` must be unique app-wide and stable across frames
/// (the id string is a good choice).
pub fn menu_row(theme: &Theme, active: bool, fade_key: impl Into<SharedString>) -> gpui::Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(MENU_ITEM_RADIUS))
        .text_size(crate::typography::ui_rems(13.0))
        .cursor_pointer();
    if active {
        row.bg(crate::theme::card_selected_bg())
            .text_color(theme.text)
    } else {
        let fade_key = fade_key.into();
        let mut row = row
            .text_color(motion::hover_blend(
                &fade_key,
                theme.text.opacity(0.9),
                theme.text,
            ))
            .bg(motion::hover_blend(
                &fade_key,
                crate::theme::wash(0.0),
                crate::theme::card_selected_bg(),
            ));
        // Imperative form — the caller's `.id(...)` makes the element stateful
        // (hover listeners need element state, `.on_hover` needs `Stateful`).
        row.interactivity()
            .on_hover(motion::hover_listener(fade_key));
        row
    }
}

/// [`menu_row`] with a distinct keyboard-navigation highlight: a selected row
/// carries the full `bg-white/10` wash, the keyboard cursor the lighter
/// `bg-white/[0.08]` (skylark's `data-[highlighted]` styling) — two selected-
/// looking rows never appear at once.
pub fn menu_row_nav(
    theme: &Theme,
    selected: bool,
    highlighted: bool,
    fade_key: impl Into<SharedString>,
) -> gpui::Div {
    let row = menu_row(theme, selected, fade_key);
    if !selected && highlighted {
        row.bg(crate::theme::card_selected_bg())
            .text_color(theme.text)
    } else {
        row
    }
}

/// [`menu_row_nav`] without the hover fade: rows on list surfaces that drive
/// their own repaint through a notify listener (the workspace tree-row
/// treatment — the fork's hover style alone doesn't schedule a draw, and the
/// fade's `window.refresh` repainted the whole window on every row
/// crossing). The caller must add `.id(...)`, the `.on_hover` listener, and
/// the click handler.
pub fn menu_row_nav_snap(theme: &Theme, selected: bool, highlighted: bool) -> gpui::Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(MENU_ITEM_RADIUS))
        .text_size(crate::typography::ui_rems(13.0))
        .cursor_pointer();
    if selected || highlighted {
        row.bg(crate::theme::card_selected_bg())
            .text_color(theme.text)
    } else {
        row.text_color(theme.text.opacity(0.9)).hover(|s| {
            s.bg(crate::theme::card_selected_bg())
                .text_color(theme.text)
        })
    }
}

/// Section heading inside a floating menu (Linear / modern macOS title-case style).
pub fn menu_heading(theme: &Theme, label: &str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .px(px(8.0))
        .pb(px(4.0))
        .pt(px(6.0))
        .font_family(theme.font_sans.clone())
        .text_size(crate::typography::ui_rems(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_muted.opacity(0.7))
        .child(SharedString::from(label.to_string()))
}

/// Uppercase + hair-space tracking (see [`menu_heading`]).
pub fn tracked_upper(label: &str) -> String {
    let upper = label.to_uppercase();
    let mut out = String::with_capacity(upper.len() * 2);
    let mut first = true;
    for ch in upper.chars() {
        if !first {
            out.push('\u{200A}'); // hair space ≈ 0.1em tracking
        }
        out.push(ch);
        first = false;
    }
    out
}

/// Hairline divider between menu sections (skylark `MenuSeparator`:
/// `mx-1 my-1 h-px bg-white/[0.07]`).
pub fn menu_separator() -> gpui::Div {
    // Full-bleed: negative margins cancel the card's inset so the hairline
    // runs border to border (user request).
    div()
        .h(px(1.0))
        .mx(px(-CARD_INSET))
        .my(px(MENU_GAP))
        .bg(hairline(0.07))
}

/// The recessed band tone for a palette/picker header or footer strip — a
/// translucent black so the glass still reads through (the add-space palette
/// converged on this; measured subtler tones vanish against the dim scrim).
/// Free function (like [`ink`]/[`hairline`]/[`wash`]), mirroring
/// [`Theme::band`], for the several callers with no `Theme`/`cx` in scope
/// (some outside this crate's `ui` module tree — threading a `&Theme` param
/// would ripple past this task's file scope).
pub fn band() -> gpui::Hsla {
    crate::theme::band()
}

/// Shared shell for command-palette-style flows. The recessed header/footer
/// bands are supplied by callers, while this owns the glass tint, outline,
/// radius, clipping, and shadow that make Cmd+K and its sibling flows read as
/// one component family.
pub fn palette_card(theme: &Theme, width: Pixels, corner_radius: f32) -> gpui::Div {
    div()
        .w(width)
        .rounded(px(corner_radius))
        .border_1()
        .border_color(hairline(0.10))
        .bg(surface_bg(theme))
        .shadow_lg()
        .overflow_hidden()
        .flex()
        .flex_col()
        .text_color(theme.text)
}

/// A compact search glyph with the same 16px slot as palette action icons.
pub fn palette_search_icon(theme: &Theme) -> gpui::Div {
    div()
        .size(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            crate::icons::icon(crate::icons::PALETTE_SEARCH)
                .size(px(16.0))
                .text_color(theme.text_muted),
        )
}

/// One footer key-cap (22px, rounded-5, `white/[0.05]`) holding arbitrary
/// children — the base of [`key_hint`]/[`key_hint_pair`] and the search-bar
/// chips ("⌘K", "esc").
pub fn key_cap(_theme: &Theme) -> gpui::Div {
    div()
        .h(px(22.0))
        .px(px(5.0))
        .rounded(px(5.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .bg(ink(0.05))
}

/// The tiny verb after a key-cap.
fn key_hint_label(theme: &Theme, label: &'static str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .text_size(crate::typography::ui_rems(10.5))
        .text_color(theme.text_muted)
        .child(SharedString::from(label))
}

/// A footer legend: one icon key-cap + tiny verb (the add-space palette's
/// footer voice, shared by the pickers).
pub fn key_hint(theme: &Theme, icon_path: &'static str, label: &'static str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(
            key_cap(theme).child(
                crate::icons::icon(icon_path)
                    .size(px(12.5))
                    .text_color(theme.text_muted),
            ),
        )
        .child(key_hint_label(theme, label))
}

/// A footer legend whose cap holds a WORD ("tab", "esc") instead of a glyph
/// — for keys with no icon in the set.
pub fn key_hint_text(theme: &Theme, cap: &'static str, label: &'static str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(
            key_cap(theme)
                .text_size(px(11.0))
                .font_family(theme.font_mono.clone())
                .text_color(theme.text_muted)
                .child(SharedString::from(cap)),
        )
        .child(key_hint_label(theme, label))
}

/// A footer legend whose cap holds TWO glyphs split by a hairline
/// ("[ ↑ | ↓ ] Navigate") sharing one verb.
pub fn key_hint_pair(
    theme: &Theme,
    first: &'static str,
    second: &'static str,
    label: &'static str,
) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(
            key_cap(theme)
                .child(
                    crate::icons::icon(first)
                        .size(px(12.5))
                        .text_color(theme.text_muted),
                )
                .child(div().w(px(1.0)).h(px(11.0)).bg(hairline(0.10)))
                .child(
                    crate::icons::icon(second)
                        .size(px(12.5))
                        .text_color(theme.text_muted),
                ),
        )
        .child(key_hint_label(theme, label))
}

/// A muted kbd hint chip inside menu rows (`⌘↵`-style accelerators).
pub fn kbd_hint(theme: &Theme, label: &str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex_none()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(5.0))
        .bg(ink(0.05))
        .text_size(crate::typography::ui_rems(10.0))
        .font_family(theme.font_mono.clone())
        .text_color(theme.text_muted)
        .child(SharedString::from(label.to_string()))
}

/// The search/text input frame at the top of a picker popover (skylark
/// `searchInput`: `w-full rounded-lg bg-white/[0.04] px-2.5 py-1.5
/// text-[13px]` + `mb-1`, borderless — full width inside the card's own
/// p-1, only a 4px bottom margin).
pub fn search_input_frame(_theme: &Theme, input: AnyElement) -> gpui::Div {
    div()
        .mb(px(4.0))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(ink(0.04))
        .text_size(crate::typography::ui_rems(13.0))
        .child(input)
}

/// A bordered trailing menu section (skylark picker action groups /
/// branch-picker worktree block: `mt-1 flex flex-col gap-0.5 border-t
/// border-white/[0.06] pt-1` — the hairline runs edge-to-edge of the card's
/// p-1 inset, unlike [`menu_separator`]'s mx-1).
pub fn menu_section() -> gpui::Div {
    div()
        .mt(px(4.0))
        .pt(px(4.0))
        .border_t_1()
        .border_color(hairline(0.06))
        .flex()
        .flex_col()
        .gap(px(2.0))
}

// ---------------------------------------------------------------------------
// Dialog primitives (skylark dialog.tsx / sidebar dialogs.tsx)
// ---------------------------------------------------------------------------

/// Centered dialog with the shared popover surface. A filled drop shadow
/// would show through the translucent card, so only opaque cards use it.
pub fn dialog_card(theme: &Theme) -> gpui::Div {
    div()
        .w(px(360.0))
        .p(px(20.0))
        .rounded(px(16.0))
        .bg(surface_bg(theme))
        .border_1()
        .border_color(hairline(0.10))
        .shadow_lg()
        .flex()
        .flex_col()
        .text_color(theme.text)
}

/// Dialog title: `text-[15px] font-semibold tracking-tight`.
pub fn dialog_title(theme: &Theme, title: &str) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(15.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(SharedString::from(title.to_string()))
}

/// Dialog body copy: `text-[13px] leading-relaxed text-muted-foreground`.
pub fn dialog_body(theme: &Theme, copy: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(13.0))
        .line_height(px(19.0))
        .text_color(theme.text_muted)
        .child(copy.into())
}

/// Dialog text-field frame: `rounded-lg border border-white/[0.08]
/// bg-white/[0.04] px-3 py-2 text-[14px]`.
pub fn dialog_field(input: AnyElement) -> gpui::Div {
    div()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(hairline(0.08))
        .bg(ink(0.04))
        .text_size(crate::typography::ui_rems(14.0))
        .child(input)
}

/// Ghost button (`btnGhost`): quiet text, hover wash fading over
/// `transition-colors` (skylark dialogs.tsx). Caller adds id + click; `fade_key`
/// as in [`menu_row`].
pub fn btn_ghost(theme: &Theme, label: &str, fade_key: impl Into<SharedString>) -> gpui::Div {
    let fade_key = fade_key.into();
    let mut btn = div()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(motion::hover_blend(&fade_key, theme.text_muted, theme.text))
        .bg(motion::hover_blend(
            &fade_key,
            crate::theme::wash(0.0),
            ink(0.06),
        ))
        .cursor_pointer()
        .child(SharedString::from(label.to_string()));
    btn.interactivity()
        .on_hover(motion::hover_listener(fade_key));
    btn
}

/// Primary button (`btnPrimary`): white fill, near-black text.
pub fn btn_primary(theme: &Theme, label: &str) -> gpui::Div {
    div()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(theme.text)
        .text_size(crate::typography::ui_rems(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.on_solid)
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(SharedString::from(label.to_string()))
}

/// Destructive button (`btnDestructive`): the muted red fill.
pub fn btn_danger(theme: &Theme, label: &str) -> gpui::Div {
    div()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(theme.danger_strong)
        .text_size(crate::typography::ui_rems(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(gpui::white())
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(SharedString::from(label.to_string()))
}

/// Pulsing skeleton rows shown while a list loads (skylark:
/// `h-7 animate-pulse rounded-md bg-white/[0.04]`).
pub fn skeleton_rows(
    _id: &'static str,
    _theme: &Theme,
    count: usize,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let wash = ink(0.04);
    let delta = motion::pulse_delta(&SKYLARK_PULSE, view, cx);
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .py(px(4.0))
        .children((0..count).map(move |i| {
            let phase = motion::staggered_phase(delta, i, 0.08);
            div()
                .h(px(28.0))
                .rounded(px(Theme::CONTROL_RADIUS))
                .bg(wash)
                .opacity(0.35 + 0.4 * motion::pulse_wave(phase))
        }))
        .into_any_element()
}

/// One pulsing ghost label — the trigger chip's label slot while the
/// selected model still resolves (a chip collapsing to its bare icon read
/// as broken; user report).
pub fn skeleton_bar(width: f32, view: gpui::EntityId, cx: &mut gpui::App) -> AnyElement {
    let delta = motion::pulse_delta(&SKYLARK_PULSE, view, cx);
    div()
        .w(px(width))
        .h(px(11.0))
        .rounded(px(5.5))
        .bg(ink(0.08))
        .opacity(0.35 + 0.4 * motion::pulse_wave(motion::staggered_phase(delta, 0, 0.0)))
        .into_any_element()
}

/// [`skeleton_rows`] shaped like a MENU loading: shorter bars of varied
/// widths reading as ghost labels rather than full-width slabs (the model
/// picker's loading state — reference design's skeleton). Widths cycle a
/// small deterministic ladder so the stagger reads organic without
/// randomness (randomness would repaint differently every open).
pub fn skeleton_menu_rows(
    _id: &'static str,
    _theme: &Theme,
    count: usize,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    const WIDTHS: [f32; 4] = [0.42, 0.58, 0.48, 0.66];
    let wash = ink(0.05);
    let delta = motion::pulse_delta(&SKYLARK_PULSE, view, cx);
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .py(px(6.0))
        .px(px(4.0))
        .children((0..count).map(move |i| {
            let phase = motion::staggered_phase(delta, i, 0.08);
            div()
                .h(px(14.0))
                .w(gpui::relative(WIDTHS[i % WIDTHS.len()]))
                .rounded(px(7.0))
                .bg(wash)
                .opacity(0.35 + 0.4 * motion::pulse_wave(phase))
        }))
        .into_any_element()
}

/// Inline error row + Retry affordance (the caller attaches the listener to the
/// returned id).
pub fn error_row(theme: &Theme, message: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(Theme::SPACE_SM))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.danger)
        .child(gpui::SharedString::from(message.to_string()))
}
