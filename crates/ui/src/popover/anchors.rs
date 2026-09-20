//! Anchored popovers, floating menus, and modal layers.

use super::*;

/// Pin a floating layer's origin to the trigger's top-left. The anchored
/// element is absolutely positioned; without explicit insets its *static*
/// position is subject to the trigger's own flex alignment (an `items_center`
/// trigger would vertically center the whole floating layer). A zero-size
/// absolutely-inset wrapper fixes the origin at the corner.
fn pinned_layer(layer: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_0()
        .child(layer)
        .into_any_element()
}

/// Eased exit progress (0..=1) for a [`Popup`] closing instant, computed from
/// the wall clock at render time. Monotonic by construction — unlike the
/// animation element's own clock, it can never replay from 0 mid-exit.
fn exit_progress(since: std::time::Instant) -> f32 {
    let total = motion::MENU_OUT
        .total()
        .mul_f32(motion::speed_scale())
        .as_secs_f32();
    let raw = if total <= 0.0 {
        1.0
    } else {
        (since.elapsed().as_secs_f32() / total).clamp(0.0, 1.0)
    };
    motion::MENU_OUT.progress(raw)
}

/// The frosted card for a popover layer: full blur while open; while exiting
/// the blur radius rides the exit progress down to 0 — the `BackdropBlur`
/// primitive ignores `element_opacity`, so without this the glass slab would
/// hold full strength through the fade and pop off at unmount.
fn frosted_menu(exit: Option<f32>, content: AnyElement) -> AnyElement {
    let blur = crate::frost::MENU_BLUR * (1.0 - exit.unwrap_or(0.0));
    // Outside-dismiss listeners run during capture. Consume that same press
    // during bubble, after dismissal, so content behind the menu cannot act
    // on it too. The following click can reach that content normally.
    let guard = gpui::canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble && !bounds.contains(&event.position) {
                    cx.stop_propagation();
                }
            });
        },
    )
    .absolute()
    .inset_0();
    crate::frost::frosted(
        CARD_RADIUS,
        blur,
        div()
            .relative()
            .child(guard)
            .child(content)
            .into_any_element(),
    )
    .into_any_element()
}

/// Entrance or exit motion for a popover layer. While exiting (the [`Popup`]
/// closing phase, `exit = Some(progress)`) the content plays
/// [`motion::menu_out`] under a fresh animation id (same-id reuse would
/// inherit the entrance's finished clock and snap to the end state) and gets
/// an occluding overlay on top — the dying menu's rows must not take clicks,
/// and the overlay also keeps stray clicks from reaching whatever sits
/// underneath.
fn menu_motion(id: SharedString, exit: Option<f32>, inner: gpui::Div) -> AnyElement {
    if let Some(t) = exit {
        let inner = inner.relative().child(div().absolute().inset_0().occlude());
        motion::menu_out(SharedString::from(format!("{id}-out")), t, inner).into_any_element()
    } else {
        motion::menu_in(id, inner).into_any_element()
    }
}

/// Wrap popover content in a floating anchored layer attached to the trigger:
/// the caller `.child(anchored_menu(...))`s this from the trigger element while
/// open. Plays `menu-in` (0.14s fade + 2px drop); `closing` (the [`Popup`]
/// exit phase) swaps in `menu-out`. Dismissal is the caller's
/// `.on_mouse_down_out` on the content. The layer `.occlude()`s: hitboxes are
/// paint-order only in gpui, so without it clicks on menu rows would ALSO fire
/// whatever clickable sits under the floating layer.
pub fn anchored_menu(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    pinned_layer(
        gpui::deferred(
            gpui::anchored()
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.0))
                .child(menu_motion(
                    id.into(),
                    exit,
                    div().occlude().pt(px(6.0)).child(content),
                )),
        )
        .priority(1)
        .into_any_element(),
    )
}

/// [`anchored_menu`] opening DOWNWARD from the trigger's bottom edge — a
/// dropdown proper (the sidebar's space filter). The default variant pins to
/// the trigger's top-left, which reads fine for context-style menus but
/// covers a button-shaped trigger.
pub fn anchored_menu_below(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    anchored_menu_below_gap(id, content, closing, 6.0)
}

/// [`anchored_menu_below`] right-aligned to the trigger's right edge. This is
/// the dropdown counterpart to [`anchored_menu_above_end`]: trailing sidebar
/// controls can open a full-width card leftward without leaving the sidebar.
pub fn anchored_menu_below_end(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .bottom_0()
        .right_0()
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(
                        id.into(),
                        exit,
                        div().occlude().pt(px(6.0)).child(content),
                    )),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// Open a top-level menu beside the trigger, clamped to the window.
pub fn anchored_menu_right(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .top_0()
        .right(px(-6.0))
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(id.into(), exit, div().occlude().child(content))),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// A nested menu beside its trigger. Callers choose the side that has room;
/// vertical placement still stays within the window's eight-pixel gutter.
pub fn nested_menu(id: impl Into<SharedString>, content: AnyElement, left: bool) -> AnyElement {
    // A nested menu shares the parent's interaction surface. Its outside
    // clicks must reach sibling controls and its trigger; the top-level menu
    // still consumes dismissal clicks before they reach the app underneath.
    let content =
        crate::frost::frosted(CARD_RADIUS, crate::frost::MENU_BLUR, content).into_any_element();
    div()
        .absolute()
        .top_0()
        .size_0()
        .when(left, |el| el.left(px(-(CARD_INSET + 6.0))))
        .when(!left, |el| el.right(px(-(CARD_INSET + 6.0))))
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(if left {
                        Anchor::TopRight
                    } else {
                        Anchor::TopLeft
                    })
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(id.into(), None, div().occlude().child(content))),
            )
            .priority(2),
        )
        .into_any_element()
}

/// [`anchored_menu_below`] with a caller-chosen trigger→card gap — the
/// changes-header dropdowns hang off a tight titlebar band and need more
/// breathing room than the default 6px (user report; t3code sits near 10).
pub fn anchored_menu_below_gap(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
    gap: f32,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(
                        id.into(),
                        exit,
                        div().occlude().pt(px(gap)).child(content),
                    )),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// [`anchored_menu`] opening UPWARD from the trigger (composer pickers, the
/// user menu — anything anchored near the window bottom; Radix flips these
/// automatically, gpui's `anchored` needs the side picked).
pub fn anchored_menu_above(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    pinned_layer(
        gpui::deferred(
            gpui::anchored()
                .anchor(Anchor::BottomLeft)
                .snap_to_window_with_margin(px(8.0))
                .child(menu_motion(
                    id.into(),
                    exit,
                    div().occlude().pb(px(6.0)).child(content),
                )),
        )
        .priority(1)
        .into_any_element(),
    )
}

/// Open an upward menu at a point inside a relative trigger. Useful for text
/// completions, whose natural anchor is the token/caret rather than the input
/// element's outer edge.
pub fn anchored_menu_above_at(
    id: impl Into<SharedString>,
    position: Point<Pixels>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    div()
        .absolute()
        .left(position.x)
        .top(position.y)
        .size_0()
        .child(anchored_menu_above(id, content, closing))
        .into_any_element()
}

pub fn full_width_menu_above(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .bottom_full()
        .left_0()
        .right_0()
        .child(
            gpui::deferred(menu_motion(
                id.into(),
                exit,
                div().occlude().pb(px(6.0)).child(content),
            ))
            .priority(1),
        )
        .into_any_element()
}

/// [`anchored_menu_above`] right-aligned to the trigger's right edge (t3code
/// ComboboxPopup `align="end"` — right-side triggers like the composer's ref
/// picker open leftward instead of running off the window).
pub fn anchored_menu_above_end(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .top_0()
        .right_0()
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::BottomRight)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(
                        id.into(),
                        exit,
                        div().occlude().pb(px(6.0)).child(content),
                    )),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// A floating menu at an explicit window position (context menus). Occludes
/// like [`anchored_menu`] so row clicks never reach elements underneath.
pub fn menu_at(
    id: impl Into<SharedString>,
    position: Point<Pixels>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    gpui::deferred(
        gpui::anchored()
            .position(position)
            .anchor(Anchor::TopLeft)
            .snap_to_window_with_margin(px(8.0))
            .child(menu_motion(id.into(), exit, div().occlude().child(content))),
    )
    .priority(1)
    .into_any_element()
}

/// Modal/overlay scrim at the *current* appearance, quoted in dark-mode terms
/// like [`ink`]/[`hairline`] — for callers (`modal`, the attachment lightbox)
/// that paint from a `deferred`/`anchored` layer with no `Theme`/`cx` in
/// scope. Mirrors [`Theme::scrim`], which is pinned at `X = 0.6` dark /
/// `0.32` light; other dark-mode alphas scale the light side by the same
/// ratio so the *dark* result is always exactly `alpha_dark` (never routed
/// through [`Hsla::opacity`], whose `0..=1` clamp would clip a
/// larger-than-0.6 alpha before it could scale the light side).
pub(crate) fn scrim_alpha(alpha_dark: f32) -> gpui::Hsla {
    crate::theme::scrim(alpha_dark)
}

/// Full-window modal: dim scrim + centered card with the `dialog-in` entrance.
/// The scrim swallows clicks; the caller wires its own dismiss/confirm.
/// `viewport` is the window size (an `anchored` layer sizes to its children,
/// so the scrim needs explicit dimensions). The frost radius matches
/// [`dialog_card`]'s 16px rounding.
pub fn modal(
    id: impl Into<ElementId>,
    viewport: gpui::Size<Pixels>,
    card: AnyElement,
) -> AnyElement {
    modal_with(id, viewport, card, 16.0, 0.35)
}

/// [`modal`] with custom rounding for glass palettes. Both use a light
/// scrim so the blurred backdrop retains its hue instead of becoming gray.
/// `corner_radius` must match the card's rounding.
pub fn modal_glass(
    id: impl Into<ElementId>,
    viewport: gpui::Size<Pixels>,
    card: AnyElement,
    corner_radius: f32,
) -> AnyElement {
    modal_with(id, viewport, card, corner_radius, 0.35)
}

fn modal_with(
    id: impl Into<ElementId>,
    viewport: gpui::Size<Pixels>,
    card: AnyElement,
    corner_radius: f32,
    scrim: f32,
) -> AnyElement {
    let card =
        crate::frost::frosted(corner_radius, crate::frost::MENU_BLUR, card).into_any_element();
    gpui::deferred(
        gpui::anchored()
            .position(gpui::point(px(0.0), px(0.0)))
            .child(
                div()
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .bg(scrim_alpha(scrim))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::dialog_in(id, div().child(card))),
            ),
    )
    .priority(2)
    .into_any_element()
}
