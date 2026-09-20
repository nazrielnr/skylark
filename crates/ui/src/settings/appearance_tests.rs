//! Appearance settings tests.

use super::*;
use super::*;

#[test]
fn every_mode_gets_a_card() {
    assert_eq!(AppearanceMode::ALL.len(), 3);
    for mode in AppearanceMode::ALL {
        assert!(!mode.label().is_empty());
    }
}

#[test]
fn registry_offers_both_appearances_and_keeps_single_dark_families_valid() {
    let registry = ThemeRegistry::builtin();
    assert_eq!(
        registry
            .variants_for(zeron_theme::Appearance::Light)
            .count(),
        10
    );
    assert_eq!(
        registry.variants_for(zeron_theme::Appearance::Dark).count(),
        20
    );
}

#[test]
fn accent_helper_explains_default_and_override_scope() {
    assert!(accent_helper(AccentSelection::ThemeDefault).contains("intended"));
    let copy = accent_helper(AccentSelection::Preset(AccentPreset::Pink));
    assert!(copy.starts_with("Pink ·"));
    assert!(copy.contains("glyphs"));
}

#[test]
fn surface_helper_explains_theme_default_and_global_overrides() {
    let default = surface_helper(SurfacePreference::ThemeDefault, SurfaceTreatment::Opaque);
    assert!(default.contains("opaque default"));
    assert!(
        surface_helper(SurfacePreference::Frosted, SurfaceTreatment::Opaque)
            .contains("where supported")
    );
    assert!(
        surface_helper(SurfacePreference::Opaque, SurfaceTreatment::Frosted)
            .contains("every theme")
    );
}

#[test]
fn font_options_appear_once_in_stable_order() {
    let catalog = FontAvailability::all();
    let labels: Vec<_> = catalog.choices().iter().map(UiFontFamily::label).collect();
    assert_eq!(labels.len(), 5);
    assert_eq!(
        labels,
        ["Geist", "Geist Mono", "System UI", "Arial", "Menlo"]
    );
    let unique = labels.into_iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), 5);
}

#[test]
fn only_the_terminal_catalog_is_narrowed_to_fixed_width() {
    let all = FontAvailability::all();
    let terminal: Vec<_> = FontKind::Terminal
        .choices_for(&all)
        .iter()
        .map(UiFontFamily::label)
        .collect();
    assert_eq!(terminal, ["Geist Mono", "Menlo"]);

    for kind in [FontKind::Ui, FontKind::Code] {
        assert_eq!(kind.choices_for(&all), all.choices());
        assert!(kind.is_available_for(&all, &UiFontFamily::System));
        assert!(kind.is_available_for(&all, &UiFontFamily::Installed("Arial".into())));
    }
    for proportional in [
        UiFontFamily::System,
        UiFontFamily::Geist,
        UiFontFamily::Installed("Arial".into()),
    ] {
        assert!(!FontKind::Terminal.is_available_for(&all, &proportional));
    }
    assert!(FontKind::Terminal.is_available_for(&all, &UiFontFamily::GeistMono));
    // A query that only matches proportional families leaves the terminal
    // highlight on the bundled mono face rather than on System UI.
    assert_eq!(
        first_available(&[], FontKind::Terminal, &all),
        UiFontFamily::GeistMono
    );
}

#[test]
fn font_keyboard_navigation_stops_at_edges_and_skips_unavailable() {
    let all = FontAvailability::all();
    let choices = all.choices().to_vec();
    assert_eq!(
        step_font(&UiFontFamily::Geist, -1, &choices, FontKind::Ui, &all),
        UiFontFamily::Geist
    );
    assert_eq!(
        step_font(
            &UiFontFamily::Installed("Menlo".into()),
            1,
            &choices,
            FontKind::Ui,
            &all
        ),
        UiFontFamily::Installed("Menlo".into())
    );
    let without_arial = all.without(&UiFontFamily::Installed("Arial".into()));
    assert_eq!(
        step_font(
            &UiFontFamily::System,
            1,
            &choices,
            FontKind::Ui,
            &without_arial
        ),
        UiFontFamily::Installed("Menlo".into())
    );
}

#[test]
fn filtering_narrows_to_matches_and_navigation_stays_inside_them() {
    let all = FontAvailability::all();
    let catalog = all.choices().to_vec();

    assert_eq!(filter_families("", &catalog), catalog);
    assert_eq!(filter_families("   ", &catalog), catalog);
    assert!(filter_families("helvetica", &catalog).is_empty());
    // Case-insensitive, and the bundled entries still lead when they match.
    assert_eq!(
        filter_families("GEIST", &catalog),
        vec![UiFontFamily::Geist, UiFontFamily::GeistMono]
    );

    // "Menlo" prefix-matches, so it outranks the "System UI" substring hit.
    let matches = filter_families("m", &catalog);
    assert_eq!(
        matches,
        vec![
            UiFontFamily::Installed("Menlo".into()),
            UiFontFamily::GeistMono,
            UiFontFamily::System,
        ]
    );
    // Stepping never escapes the filtered list.
    assert_eq!(
        step_font(&UiFontFamily::System, 1, &matches, FontKind::Ui, &all),
        UiFontFamily::System
    );
    assert_eq!(
        first_available(&matches, FontKind::Ui, &all),
        UiFontFamily::Installed("Menlo".into())
    );
    assert_eq!(
        last_available(&matches, FontKind::Ui, &all),
        UiFontFamily::System
    );
}

#[test]
fn each_font_kind_gets_distinct_labels_and_element_ids() {
    let slugs: Vec<_> = FontKind::ALL.iter().map(|kind| kind.slug()).collect();
    let labels: Vec<_> = FontKind::ALL.iter().map(|kind| kind.label()).collect();
    assert_eq!(
        slugs.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    assert_eq!(
        labels
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
}

#[test]
fn pixel_sizes_render_whole_and_fractional_values() {
    assert_eq!(format_px(13.0), "13 px");
    assert_eq!(format_px(typography::CODE_FONT_SIZE_DEFAULT), "12.5 px");
    assert_eq!(
        typography::clamp_font_size(typography::FONT_SIZE_MAX + 1.0),
        typography::FONT_SIZE_MAX
    );
    assert_eq!(
        typography::clamp_font_size(typography::FONT_SIZE_MIN - 1.0),
        typography::FONT_SIZE_MIN
    );
}

#[test]
fn font_size_options_are_ordered_and_include_the_default() {
    let values = UiFontSize::ALL.map(UiFontSize::pixels);
    assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(UiFontSize::ALL.contains(&UiFontSize::default()));
}

#[test]
fn mono_size_ladder_keeps_both_defaults_exactly_reachable() {
    assert!(MONO_FONT_SIZES.windows(2).all(|pair| pair[0] < pair[1]));
    for default in [
        typography::TERMINAL_FONT_SIZE_DEFAULT,
        typography::CODE_FONT_SIZE_DEFAULT,
    ] {
        assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(default)], default);
    }
    // Off-ladder values (older settings, hand-edited files) snap, never drop.
    assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(0.0)], MONO_FONT_SIZES[0]);
    assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(99.0)], 20.0);
    assert_eq!(MONO_FONT_SIZES[nearest_mono_ix(12.4)], 12.5);
}

#[test]
fn every_size_dropdown_labels_its_whole_ladder_in_pixels() {
    for kind in FontKind::ALL {
        let labels = kind.size_labels();
        assert_eq!(labels.len(), kind.size_count());
        assert!(labels.iter().all(|label| label.ends_with(" px")));
    }
    assert!(FontKind::Terminal.size_labels().contains(&"13 px".into()));
    assert!(FontKind::Code.size_labels().contains(&"12.5 px".into()));
}

#[gpui::test]
fn conversation_width_drag_coalesces_and_persists_the_last_value(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::dark());
        crate::settings::init(Default::default(), dir.path(), cx);
    });
    let window = cx.add_window(|_, cx| AppearancePage::new(cx));
    window
        .update(cx, |page, window, cx| {
            page.width_bounds.set(Some(gpui::Bounds::new(
                gpui::point(px(100.0), px(0.0)),
                gpui::size(px(240.0), px(28.0)),
            )));
            page.drag_width(px(0.0), window, cx);
            assert_eq!(page.pending_width, Some(560.0));
            page.drag_width(px(1000.0), window, cx);
            assert_eq!(page.pending_width, Some(1200.0));
            page.drag_width(px(220.0), window, cx);
            assert_eq!(page.pending_width, Some(880.0));
            assert_eq!(
                crate::settings::transcript_width(cx),
                736.0,
                "pointer events must coalesce before publishing"
            );
            page.apply_pending_width(cx);
            assert_eq!(crate::settings::transcript_width(cx), 880.0);
            crate::settings::flush(cx);
            assert_eq!(
                crate::settings::UiSettings::load(dir.path()).transcript_width,
                880.0
            );
        })
        .unwrap();
}

#[gpui::test]
fn size_dropdowns_commit_from_the_ladder_and_enter_leaves_menus_closed(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::dark());
        typography::init(
            UiFontFamily::Geist,
            UiFontSize::default(),
            UiFontFamily::GeistMono,
            typography::TERMINAL_FONT_SIZE_DEFAULT,
            UiFontFamily::GeistMono,
            typography::CODE_FONT_SIZE_DEFAULT,
            FontAvailability::all(),
            cx,
        );
    });
    let window = cx.add_window(|_, cx| AppearancePage::new(cx));
    window
        .update(cx, |page, window, cx| {
            for kind in [FontKind::Terminal, FontKind::Code] {
                page.toggle_size_menu(kind, cx);
                assert!(page.size_menu(kind).is_open());
                assert_eq!(page.selected_size_ix(kind), kind.size_ix(cx));
                let target = kind.size_ix(cx) + 1;
                page.set_selected_size_ix(kind, target);
                page.commit_size(kind, window, cx);
                assert_eq!(kind.pixel_size(cx), MONO_FONT_SIZES[target]);
                assert!(!page.size_menu(kind).is_open());
            }

            // Opening a family menu closes a size menu left open elsewhere.
            page.toggle_size_menu(FontKind::Ui, cx);
            assert!(page.size_menu(FontKind::Ui).is_open());
            page.toggle_font_menu(FontKind::Code, window, cx);
            assert!(!page.size_menu(FontKind::Ui).is_open());
            assert!(page.font_menu(FontKind::Code).is_open());

            // Enter commits and reports the key consumed — without that the
            // trigger's `is_open` guard reads the just-closed menu and
            // reopens it on the very same event.
            let enter = KeyDownEvent {
                keystroke: gpui::Keystroke::parse("enter").expect("valid keystroke"),
                is_held: false,
                prefer_character_input: false,
            };
            assert!(page.on_font_key_down(FontKind::Code, &enter, window, cx));
            assert!(!page.font_menu(FontKind::Code).is_open());
        })
        .expect("window is open");
}

/// A settings file written before the terminal picker was constrained can
/// still name a proportional family; startup must not hand it to the grid.
#[gpui::test]
fn persisted_proportional_terminal_family_resolves_to_geist_mono(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::dark());
        typography::init(
            UiFontFamily::Geist,
            UiFontSize::default(),
            UiFontFamily::Installed("Arial".into()),
            typography::TERMINAL_FONT_SIZE_DEFAULT,
            UiFontFamily::Installed("Arial".into()),
            typography::CODE_FONT_SIZE_DEFAULT,
            FontAvailability::all(),
            cx,
        );
        assert_eq!(
            typography::terminal_effective(cx),
            UiFontFamily::GeistMono,
            "proportional terminal family must fall back"
        );
        assert_eq!(
            typography::code_effective(cx),
            UiFontFamily::Installed("Arial".into()),
            "code and diffs keep proportional picks"
        );
        // The setter path rejects the same family too.
        assert!(!typography::set_terminal_family(UiFontFamily::System, cx));
        assert_eq!(typography::terminal_effective(cx), UiFontFamily::GeistMono);
    });
}
