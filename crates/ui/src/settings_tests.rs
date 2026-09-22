//! Extracted module tests.

use super::*;
use super::*;

#[test]
fn composer_send_behavior_is_opt_in_for_old_and_partial_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth": 300, "soundEnabled": false}"#,
    )
    .unwrap();

    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.composer_send_behavior, ComposerSendBehavior::Enter);
    assert!(loaded.open_web_links_in_zeron);
    assert!(loaded.new_thread_composer_background.is_none());
    assert_eq!(
        loaded.new_thread_background_effect,
        NewThreadBackgroundEffect::None
    );
    assert_eq!(loaded.sidebar_width, 300.0);
    assert!(!loaded.sound_enabled);
    for sound in [
        crate::sound::Sound::Done,
        crate::sound::Sound::Request,
        crate::sound::Sound::Attention,
    ] {
        assert!(!loaded.session_sound_enabled(sound));
    }
}

#[test]
fn window_geometry_round_trips_and_legacy_settings_default() {
    let dir = tempfile::tempdir().unwrap();
    let geometry = WindowGeometry {
        display_uuid: None,
        x: -1500.0,
        y: 40.0,
        width: 1200.0,
        height: 800.0,
    };
    let settings = UiSettings {
        window_geometry: Some(geometry),
        ..Default::default()
    };
    settings.save(dir.path()).unwrap();
    assert_eq!(UiSettings::load(dir.path()).window_geometry, Some(geometry));
    assert_eq!(WindowGeometry::from_bounds(geometry.bounds()), geometry);
    let legacy: UiSettings = serde_json::from_str(r#"{"sidebarWidth":300}"#).unwrap();
    assert_eq!(legacy.window_geometry, None);
    assert_eq!(legacy.sidebar_width, 300.0);
}

#[test]
fn window_geometry_restores_display_identity_with_overlapping_local_coordinates() {
    let primary = WindowGeometry {
        display_uuid: Some(uuid::Uuid::from_u128(1)),
        x: 0.0,
        y: 25.0,
        width: 1920.0,
        height: 1080.0,
    };
    let secondary = WindowGeometry {
        display_uuid: Some(uuid::Uuid::from_u128(2)),
        ..primary
    };
    let saved = WindowGeometry {
        x: 100.0,
        y: 80.0,
        width: 1200.0,
        height: 800.0,
        ..secondary
    };
    let encoded = serde_json::to_string(&saved).unwrap();
    let saved: WindowGeometry = serde_json::from_str(&encoded).unwrap();
    assert_eq!(saved.restore(&[primary, secondary], 0), Some((1, saved)));
    assert_eq!(saved.restore(&[secondary, primary], 1), Some((0, saved)));
}

#[test]
fn window_geometry_recenters_when_saved_display_is_disconnected() {
    let primary = WindowGeometry {
        display_uuid: Some(uuid::Uuid::from_u128(1)),
        x: 0.0,
        y: 25.0,
        width: 1440.0,
        height: 900.0,
    };
    let saved = WindowGeometry {
        display_uuid: Some(uuid::Uuid::from_u128(2)),
        x: 500.0,
        y: 300.0,
        width: 1200.0,
        height: 800.0,
    };
    assert_eq!(
        saved.restore(&[primary], 0),
        Some((
            0,
            WindowGeometry {
                display_uuid: primary.display_uuid,
                x: 120.0,
                y: 75.0,
                ..saved
            }
        ))
    );
    assert_eq!(saved.restore(&[], 0), None);
    let oversized = WindowGeometry {
        width: 2400.0,
        height: 1600.0,
        ..saved
    };
    assert_eq!(oversized.restore(&[primary], 0), Some((0, primary)));
}

#[test]
fn window_geometry_without_display_identity_uses_primary() {
    let saved: WindowGeometry =
        serde_json::from_str(r#"{"x":100,"y":80,"width":1200,"height":800}"#).unwrap();
    assert_eq!(saved.display_uuid, None);
    let display = WindowGeometry {
        display_uuid: Some(uuid::Uuid::from_u128(1)),
        x: 0.0,
        y: 25.0,
        width: 1920.0,
        height: 1080.0,
    };
    assert_eq!(
        saved.restore(&[display, display], 1),
        Some((
            1,
            WindowGeometry {
                display_uuid: display.display_uuid,
                ..saved
            }
        ))
    );
}

#[test]
fn window_geometry_rejects_invalid_values_without_resetting_settings() {
    let valid = WindowGeometry {
        display_uuid: None,
        x: 40.0,
        y: 50.0,
        width: 1200.0,
        height: 800.0,
    };
    for geometry in [
        WindowGeometry {
            x: f32::NAN,
            ..valid
        },
        WindowGeometry {
            y: f32::INFINITY,
            ..valid
        },
        WindowGeometry {
            width: 0.0,
            ..valid
        },
        WindowGeometry {
            height: -1.0,
            ..valid
        },
    ] {
        let settings = UiSettings {
            window_geometry: Some(geometry),
            sidebar_width: 300.0,
            ..Default::default()
        }
        .clamped();
        assert_eq!(settings.window_geometry, None);
        assert_eq!(settings.sidebar_width, 300.0);
    }
}

#[test]
fn window_geometry_preserves_position_on_negative_coordinate_display() {
    let display = WindowGeometry {
        display_uuid: None,
        x: -1920.0,
        y: -200.0,
        width: 1920.0,
        height: 1080.0,
    };
    let geometry = WindowGeometry {
        display_uuid: None,
        x: -1800.0,
        y: -100.0,
        width: 1200.0,
        height: 800.0,
    };
    assert_eq!(geometry.fit(display), geometry);
}

#[test]
fn window_geometry_fits_smaller_display_and_keeps_titlebar_visible() {
    let display = WindowGeometry {
        display_uuid: None,
        x: 0.0,
        y: 25.0,
        width: 1280.0,
        height: 720.0,
    };
    let geometry = WindowGeometry {
        display_uuid: None,
        x: 2000.0,
        y: -1000.0,
        width: 2000.0,
        height: 1500.0,
    };
    assert_eq!(geometry.fit(display), display);
    let small = WindowGeometry {
        width: 800.0,
        height: 500.0,
        ..display
    };
    assert_eq!(geometry.fit(small), small);
    let tiny = WindowGeometry {
        width: 100.0,
        height: 100.0,
        ..display
    };
    assert_eq!(tiny.fit(display).width, 900.0);
    assert_eq!(tiny.fit(display).height, 600.0);
}

#[test]
fn unknown_keys_from_a_newer_build_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth":300.0,"someFutureKey":"whatever","anotherOne":{"nested":1}}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.sidebar_width, 300.0);
    assert_eq!(
        loaded.terminal_font_family,
        crate::typography::UiFontFamily::GeistMono
    );
    assert_eq!(
        loaded.terminal_font_size,
        crate::typography::TERMINAL_FONT_SIZE_DEFAULT
    );
    assert_eq!(
        loaded.code_font_family,
        crate::typography::UiFontFamily::GeistMono
    );
    assert_eq!(
        loaded.code_font_size,
        crate::typography::CODE_FONT_SIZE_DEFAULT
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn appshot_shortcut_defaults_round_trip_and_reset() {
    assert_eq!(
        ShortcutId::CaptureAppshot.default_combo_on(true),
        "ctrl-alt-space"
    );
    assert_eq!(
        ShortcutId::CaptureAppshot.default_combo_on(false),
        "mod-alt-space"
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"keymap":{"newSession":"mod-shift-n"}}"#,
    )
    .unwrap();
    let mut settings = UiSettings::load(dir.path());
    assert_eq!(
        settings.keymap.capture_appshot,
        ShortcutId::CaptureAppshot.default_combo()
    );
    assert_eq!(settings.keymap.new_session, "mod-shift-n");
    settings
        .keymap
        .set(ShortcutId::CaptureAppshot, "mod-alt-k".into());
    std::fs::write(
        UiSettings::path(dir.path()),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    let mut restored = UiSettings::load(dir.path());
    assert_eq!(restored.keymap.capture_appshot, "mod-alt-k");
    restored.keymap.reset(ShortcutId::CaptureAppshot);
    assert_eq!(
        restored.keymap.capture_appshot,
        ShortcutId::CaptureAppshot.default_combo()
    );
}

#[test]
fn appshot_settings_are_serialized_only_on_desktop() {
    let value = serde_json::to_value(UiSettings::default()).unwrap();
    for key in [
        "appshotsEnabled",
        "appshotSoundEnabled",
        "appshotDestination",
    ] {
        assert_eq!(
            value.get(key).is_some(),
            crate::appshots::is_desktop(),
            "{key}"
        );
    }
    assert_eq!(
        value["keymap"].get("captureAppshot").is_some(),
        crate::appshots::is_desktop()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn appshot_sound_migrates_mute_and_persists_independently() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(UiSettings::path(dir.path()), r#"{"soundEnabled":false}"#).unwrap();
    let mut loaded = UiSettings::load(dir.path());
    assert!(!loaded.appshot_sound_enabled);
    loaded.appshot_sound_enabled = true;
    std::fs::write(
        UiSettings::path(dir.path()),
        serde_json::to_vec(&loaded).unwrap(),
    )
    .unwrap();
    let restored = UiSettings::load(dir.path());
    assert!(restored.appshot_sound_enabled);
    assert!(!restored.sound_enabled);
}

#[test]
fn background_cleanup_only_removes_files_owned_by_the_setting() {
    let dir = tempfile::tempdir().unwrap();
    let backgrounds = dir.path().join(NEW_THREAD_BACKGROUND_DIR);
    std::fs::create_dir(&backgrounds).unwrap();
    let managed = backgrounds.join("new-thread-background-owned.png");
    let unrelated = dir.path().join("keep.png");
    std::fs::write(&managed, b"managed").unwrap();
    std::fs::write(&unrelated, b"unrelated").unwrap();

    remove_managed_new_thread_background(
        Some(&NewThreadComposerBackground {
            path: unrelated.to_string_lossy().into_owned(),
            name: "keep.png".into(),
        }),
        &backgrounds,
    );
    assert!(unrelated.exists());
    remove_managed_new_thread_background(
        Some(&NewThreadComposerBackground {
            path: managed.to_string_lossy().into_owned(),
            name: "owned.png".into(),
        }),
        &backgrounds,
    );
    assert!(!managed.exists());
}

#[gpui::test]
fn invalid_background_replacement_preserves_previous_image_and_settings(
    cx: &mut gpui::TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([20, 100, 200, 255]))
        .save(&original)
        .unwrap();
    cx.update(|cx| {
            init(UiSettings::default(), dir.path(), cx);
            install_new_thread_composer_background(&original, cx).unwrap();
            let before = current(cx);
            let previous = PathBuf::from(&before.new_thread_composer_background.as_ref().unwrap().path);
            let saved = std::fs::read(UiSettings::path(dir.path())).unwrap();
            let previous_bytes = std::fs::read(&previous).unwrap();
            for (name, bytes) in [
                ("replacement.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#.as_slice()),
                ("corrupt.png", b"not a PNG".as_slice()),
                ("truncated.png", &previous_bytes[..previous_bytes.len() / 2]),
            ] {
                let candidate = dir.path().join(name);
                std::fs::write(&candidate, bytes).unwrap();
                let result = install_new_thread_composer_background(&candidate, cx);
                assert!(result.is_err(), "accepted invalid replacement: {name}");
                assert_eq!(current(cx), before);
                assert_eq!(std::fs::read(UiSettings::path(dir.path())).unwrap(), saved);
                assert_eq!(std::fs::read(&previous).unwrap(), previous_bytes);
                assert_eq!(std::fs::read_dir(dir.path().join(NEW_THREAD_BACKGROUND_DIR)).unwrap().count(), 1);
                assert!(candidate.exists(), "source files must never be deleted");
            }
        });
}

#[gpui::test]
fn valid_background_replacement_persists_renderable_image_before_retiring_previous(
    cx: &mut gpui::TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.png");
    let second = dir.path().join("second.jpg");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([20, 100, 200, 255]))
        .save(&first)
        .unwrap();
    image::RgbImage::from_pixel(12, 10, image::Rgb([200, 100, 20]))
        .save(&second)
        .unwrap();
    cx.update(|cx| {
        let initial = UiSettings {
            new_thread_background_effect: NewThreadBackgroundEffect::Ascii,
            ..Default::default()
        };
        init(initial, dir.path(), cx);
        install_new_thread_composer_background(&first, cx).unwrap();
        let old_path = current(cx).new_thread_composer_background.unwrap().path;
        install_new_thread_composer_background(&second, cx).unwrap();
        let settings = current(cx);
        let replacement = settings.new_thread_composer_background.as_ref().unwrap();
        assert_ne!(replacement.path, old_path);
        let saved_image = std::fs::read(&replacement.path).unwrap();
        assert_eq!(saved_image, std::fs::read(&second).unwrap());
        let decoded = crate::new_thread_background_image::decode(&saved_image).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (12, 10));
        assert_eq!(UiSettings::load(dir.path()), settings);
        assert_eq!(
            settings.new_thread_background_effect,
            NewThreadBackgroundEffect::Ascii
        );
        assert!(!Path::new(&old_path).exists());
        assert!(first.exists() && second.exists());
        assert_eq!(
            std::fs::read_dir(dir.path().join(NEW_THREAD_BACKGROUND_DIR))
                .unwrap()
                .count(),
            1
        );
    });
}

#[gpui::test]
fn invalid_initial_background_import_does_not_create_managed_files_or_settings(
    cx: &mut gpui::TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let candidate = dir.path().join("corrupt.png");
    std::fs::write(&candidate, b"not a PNG").unwrap();
    cx.update(|cx| {
        init(UiSettings::default(), dir.path(), cx);
        assert!(install_new_thread_composer_background(&candidate, cx).is_err());
        assert!(current(cx).new_thread_composer_background.is_none());
        assert!(!dir.path().join(NEW_THREAD_BACKGROUND_DIR).exists());
        assert!(!UiSettings::path(dir.path()).exists());
        assert!(candidate.exists());
    });
}

#[test]
fn obsolete_steering_preference_does_not_reset_other_settings() {
    let loaded: UiSettings = serde_json::from_str(
        r#"{"activeTurnSendBehavior":"steer","sidebarWidth":300,"soundEnabled":false}"#,
    )
    .unwrap();
    assert_eq!(loaded.sidebar_width, 300.0);
    assert!(!loaded.sound_enabled);
    assert!(
        serde_json::to_value(&loaded)
            .unwrap()
            .get("activeTurnSendBehavior")
            .is_none()
    );
}

#[test]
fn round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let settings = UiSettings {
        window_geometry: None,
        sidebar_width: 300.0,
        sidebar_collapsed: true,
        sidebar_layout_version: 1,
        sidebar_grouped: true,
        sidebar_organization: SidebarOrganization::ByDevice,
        sidebar_sort: SidebarSort::Created,
        sidebar_compact: true,
        sidebar_show_project_icon: false,
        sidebar_show_project_label: false,
        sidebar_show_harness: false,
        sidebar_show_branch: false,
        sidebar_show_pull_request: false,
        last_space_id: Some("space-1".into()),
        last_project_action_by_space_id: std::collections::HashMap::from([(
            "space-1".into(),
            "dev".into(),
        )]),
        open_tabs: Some(vec!["b".to_string(), "a".to_string()]),
        space_filter: Some("space-1".into()),
        sidebar_pinned_session_ids_by_profile: HashMap::from([
            (
                "local".to_string(),
                vec!["local-2".to_string(), "local-1".to_string()],
            ),
            (
                "synced:org-1:user-1".to_string(),
                vec!["synced-1".to_string()],
            ),
        ]),
        tab_order: std::collections::HashMap::from([(
            "space-1".to_string(),
            vec!["b".to_string(), "a".to_string()],
        )]),
        space_order: vec!["space-2".to_string(), "space-1".to_string()],
        sound_enabled: false,
        sound_completion_enabled: false,
        sound_input_enabled: true,
        sound_attention_enabled: false,
        notifications_enabled: false,
        notifications_background_only: false,
        right_pane_width: 700.0,
        right_pane_open: true,
        terminal_height: 320.0,
        terminal_open: true,
        keymap: KeymapConfig {
            toggle_sidebar: "mod-shift-s".into(),
            ..KeymapConfig::default()
        },
        escape_stops_active_agent: true,
        composer_send_behavior: ComposerSendBehavior::ModEnter,
        appshots_enabled: false,
        appshot_sound_enabled: true,
        // The destination is only persisted where Appshots exist (macOS and
        // Linux); elsewhere the field is `serde(skip)` and reloads as the
        // default, so the round trip must expect exactly that.
        appshot_destination: if cfg!(any(target_os = "macos", target_os = "linux")) {
            crate::appshots::AppshotDestination::NewSession
        } else {
            crate::appshots::AppshotDestination::Automatic
        },
        appearance: crate::appearance::AppearanceMode::Light,
        git_history_columns: GitHistoryColumns {
            author: false,
            date: true,
            sha: false,
        },
        git_history_column_widths: GitHistoryColumnWidths {
            author: 132.0,
            date: 104.0,
            sha: 82.0,
        },
        git_history_column_order: GitHistoryColumnOrder(vec![
            GitHistoryColumn::Sha,
            GitHistoryColumn::Author,
            GitHistoryColumn::Date,
        ]),
        git_history_author_display: GitHistoryAuthorDisplay::Name,
        ui_font_family: crate::typography::UiFontFamily::Installed("Arial".into()),
        ui_font_size: crate::typography::UiFontSize::ALL[5],
        theme_selection: zeron_theme::ThemeSelection {
            light: "catppuccin-latte".into(),
            dark: "catppuccin-mocha".into(),
        },
        diff_split: true,
        diff_wrap: true,
        code_fences_fit_content: true,
        transcript_width: 960.0,
        open_web_links_in_zeron: false,
        files_autosave_enabled: true,
        files_autosave_delay_ms: 1_500,
        files_word_wrap: true,
        terminal_font_family: crate::typography::UiFontFamily::Installed("Menlo".into()),
        terminal_font_size: 15.0,
        code_font_family: crate::typography::UiFontFamily::Geist,
        code_font_size: 11.0,
        files_show_all: true,
        accent: zeron_theme::AccentSelection::Preset(zeron_theme::AccentPreset::Cyan),
        surface: zeron_theme::SurfacePreference::Frosted,
        new_thread_composer_background: Some(NewThreadComposerBackground {
            path: "/tmp/zeron/new-thread-background.png".into(),
            name: "background.png".into(),
        }),
        new_thread_background_effect: NewThreadBackgroundEffect::Ascii,
        legacy_accent_color: None,
    };
    settings.save(dir.path()).unwrap();
    let json = std::fs::read_to_string(UiSettings::path(dir.path())).unwrap();
    assert!(json.contains(r#""diffWrap": true"#));
    assert_eq!(UiSettings::load(dir.path()), settings);
    assert!(json.contains(r#""codeFencesFitContent": true"#));
    assert!(json.contains(r#""openWebLinksInZeron": false"#));
    assert!(json.contains(r#""newThreadBackgroundEffect": "ascii""#));
    assert!(json.contains(r#""terminalFontFamily": "installed:Menlo""#));
    assert!(json.contains(r#""terminalFontSize": 15.0"#));
    assert!(json.contains(r#""codeFontFamily": "geist""#));
    assert!(json.contains(r#""codeFontSize": 11.0"#));
}

#[test]
fn stale_revision_cannot_be_considered_the_latest_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SettingsStore {
        current: UiSettings::default(),
        data_dir: dir.path().to_path_buf(),
        revision: 0,
        saved_revision: 0,
        code_fences_generation: 0,
        save_task: None,
    };

    store.current.sidebar_width = 300.0;
    store.revision += 1;
    let (stale, stale_revision) = store.snapshot();

    store.current.ui_font_family = crate::typography::UiFontFamily::Installed("Arial".into());
    store.revision += 1;
    stale.save(dir.path()).unwrap();
    assert!(!store.mark_saved(stale_revision));

    let (latest, latest_revision) = store.snapshot();
    latest.save(dir.path()).unwrap();
    assert!(store.mark_saved(latest_revision));
    let reloaded = UiSettings::load(dir.path());
    assert_eq!(reloaded.sidebar_width, 300.0);
    assert_eq!(
        reloaded.ui_font_family,
        crate::typography::UiFontFamily::Installed("Arial".into())
    );
}

#[test]
fn transcript_width_loads_legacy_defaults_and_normalizes_persisted_values() {
    let legacy: UiSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.transcript_width, 736.0);
    for (value, expected) in [
        (100.0, 560.0),
        (2000.0, 1200.0),
        (745.0, 752.0),
        (f32::NAN, 736.0),
    ] {
        let settings = UiSettings {
            transcript_width: value,
            ..Default::default()
        }
        .clamped();
        assert_eq!(settings.transcript_width, expected);
    }
    let dir = tempfile::tempdir().unwrap();
    let settings = UiSettings {
        transcript_width: 1024.0,
        ..Default::default()
    };
    settings.save(dir.path()).unwrap();
    assert_eq!(UiSettings::load(dir.path()).transcript_width, 1024.0);
}

#[test]
fn code_fence_generation_tracks_every_mode_transition_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SettingsStore {
        current: UiSettings::default(),
        data_dir: dir.path().to_path_buf(),
        revision: 0,
        saved_revision: 0,
        code_fences_generation: 0,
        save_task: None,
    };

    assert!(store.update_current(|settings| settings.sidebar_width = 300.0));
    assert_eq!(store.code_fences_generation, 0);

    assert!(store.update_current(|settings| settings.code_fences_fit_content = true));
    assert_eq!(store.code_fences_generation, 1);
    assert!(store.update_current(|settings| settings.code_fences_fit_content = false));
    assert_eq!(store.code_fences_generation, 2);

    assert!(!store.update_current(|settings| settings.code_fences_fit_content = false));
    assert_eq!(store.code_fences_generation, 2);
}

#[test]
fn sidebar_display_defaults_and_preferences_round_trip() {
    let settings: UiSettings = serde_json::from_str("{}").unwrap();
    assert!(!settings.sidebar_compact);
    assert_eq!(
        settings.sidebar_organization,
        SidebarOrganization::ByProject
    );
    assert!(settings.sidebar_show_project_icon);
    assert!(settings.sidebar_show_project_label);
    let customized = UiSettings {
        sidebar_compact: false,
        sidebar_show_project_icon: false,
        sidebar_show_project_label: false,
        sidebar_organization: SidebarOrganization::ByProject,
        ..settings
    };
    let restored: UiSettings =
        serde_json::from_str(&serde_json::to_string(&customized).unwrap()).unwrap();
    assert_eq!(restored.clamped(), customized);
}

#[test]
fn project_organization_survives_loading() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarOrganization":"byProject"}"#,
    )
    .unwrap();

    assert_eq!(
        UiSettings::load(dir.path()).sidebar_organization,
        SidebarOrganization::ByProject
    );
}

/// A settings file written before light mode existed has no `appearance`
/// key; it must load as "follow the OS" rather than failing the whole parse
/// and resetting every other preference to defaults.
#[test]
fn settings_without_appearance_default_to_system() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth": 300, "soundEnabled": false}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.appearance, crate::appearance::AppearanceMode::System);
    assert_eq!(loaded.accent, zeron_theme::AccentSelection::ThemeDefault);
    assert_eq!(loaded.surface, zeron_theme::SurfacePreference::ThemeDefault);
    assert_eq!(loaded.sidebar_width, 300.0);
    assert!(loaded.sidebar_pinned_session_ids_by_profile.is_empty());
    assert!(!loaded.sound_enabled, "other keys still parse");
    assert!(loaded.sound_completion_enabled);
    assert!(loaded.sound_input_enabled);
    assert!(loaded.sound_attention_enabled);
    assert_eq!(
        loaded.files_autosave_delay_ms,
        FILES_AUTOSAVE_DELAY_DEFAULT_MS
    );
    assert!(!loaded.files_autosave_enabled);
    assert!(!loaded.files_word_wrap);
    assert_eq!(
        loaded.code_font_size,
        crate::typography::CODE_FONT_SIZE_DEFAULT
    );
    assert_eq!(
        loaded.terminal_font_size,
        crate::typography::TERMINAL_FONT_SIZE_DEFAULT
    );
    assert!(!loaded.files_show_all);
    assert!(
        loaded.notifications_enabled,
        "pre-banner files default banners on"
    );
    assert!(
        loaded.notifications_background_only,
        "pre-banner files default background-only on"
    );
    assert!(
        !loaded.escape_stops_active_agent,
        "preference files default Escape stopping off"
    );
    assert_eq!(
        loaded.git_history_columns,
        GitHistoryColumns::default(),
        "pre-column files show the complete History table"
    );
    assert_eq!(
        loaded.git_history_column_widths,
        GitHistoryColumnWidths::default(),
        "pre-resize files use the original History column widths"
    );
    assert_eq!(
        loaded.git_history_column_order,
        GitHistoryColumnOrder::default(),
        "pre-reorder files use the original History column order"
    );
    assert_eq!(
        loaded.git_history_author_display,
        GitHistoryAuthorDisplay::Avatar,
        "pre-author-display files default to avatars"
    );
}

#[test]
fn session_sound_preferences_round_trip_and_gate_each_event() {
    let settings = UiSettings {
        sound_enabled: true,
        sound_completion_enabled: false,
        sound_input_enabled: true,
        sound_attention_enabled: false,
        ..Default::default()
    };
    assert!(!settings.session_sound_enabled(crate::sound::Sound::Done));
    assert!(settings.session_sound_enabled(crate::sound::Sound::Request));
    assert!(!settings.session_sound_enabled(crate::sound::Sound::Attention));

    let value = serde_json::to_value(&settings).unwrap();
    let restored: UiSettings = serde_json::from_value(value).unwrap();
    assert_eq!(restored, settings);

    let muted = UiSettings {
        sound_enabled: false,
        ..settings
    };
    assert!(!muted.session_sound_enabled(crate::sound::Sound::Request));
}

#[test]
fn legacy_accent_color_migrates_to_an_explicit_preset() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(UiSettings::path(dir.path()), r#"{"accentColor":"cyan"}"#).unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(
        loaded.accent,
        zeron_theme::AccentSelection::Preset(zeron_theme::AccentPreset::Cyan)
    );
    loaded.save(dir.path()).unwrap();
    let saved = std::fs::read_to_string(UiSettings::path(dir.path())).unwrap();
    assert!(!saved.contains("accentColor"));
}

#[test]
fn settings_without_ui_font_default_to_geist() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth": 300, "soundEnabled": false}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(
        loaded.ui_font_family,
        crate::typography::UiFontFamily::Geist
    );
    assert_eq!(loaded.sidebar_width, 300.0);
    assert!(!loaded.sound_enabled);
    assert!(!loaded.diff_wrap);
    assert_eq!(
        loaded.ui_font_size,
        crate::typography::UiFontSize::default()
    );
}

#[test]
fn unsupported_ui_font_size_snaps_to_the_nearest_choice() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"uiFontSize": 19, "soundEnabled": false}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.ui_font_size.pixels(), 18.0);
    assert!(!loaded.sound_enabled);
}

#[test]
fn unknown_ui_font_falls_back_without_resetting_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth": 300, "uiFontFamily": "futureSans"}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(
        loaded.ui_font_family,
        crate::typography::UiFontFamily::Geist
    );
    assert_eq!(loaded.sidebar_width, 300.0);
}

#[test]
fn missing_and_corrupt_files_yield_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(UiSettings::load(dir.path()), UiSettings::default());
    std::fs::write(UiSettings::path(dir.path()), "{not json").unwrap();
    assert_eq!(UiSettings::load(dir.path()), UiSettings::default());
}

fn signed_in(user_id: &str, org_id: Option<&str>) -> AuthState {
    AuthState::SignedIn {
        user: zeron_proto::UserProfile {
            id: user_id.to_string(),
            email: format!("{user_id}@example.com"),
            name: None,
        },
        org_id: org_id.map(str::to_string),
    }
}

#[test]
fn sidebar_pin_profile_keys_include_the_full_workspace_identity() {
    assert_eq!(
        sidebar_pin_profile_key(Some(WorkspaceScope::Local), None, None).as_deref(),
        Some("local")
    );
    assert_eq!(
        sidebar_pin_profile_key(
            Some(WorkspaceScope::Synced),
            Some(&signed_in("user-1", Some("org-1"))),
            None,
        )
        .as_deref(),
        Some("synced:org-1:user-1")
    );
    assert_eq!(
        sidebar_pin_profile_key(
            Some(WorkspaceScope::Development),
            Some(&signed_in("dev-user@dev-org-2", None)),
            Some("ignored-org"),
        )
        .as_deref(),
        Some("development:dev-org-2:dev-user")
    );
    assert_eq!(
        sidebar_pin_profile_key(
            Some(WorkspaceScope::Development),
            Some(&signed_in("dev-user", None)),
            Some("configured-org"),
        )
        .as_deref(),
        Some("development:configured-org:dev-user")
    );
}

#[test]
fn sidebar_pin_profile_key_waits_for_a_complete_remote_identity() {
    assert_eq!(
        sidebar_pin_profile_key(Some(WorkspaceScope::Synced), None, None),
        None
    );
    assert_eq!(
        sidebar_pin_profile_key(
            Some(WorkspaceScope::Synced),
            Some(&signed_in("user-1", None)),
            None,
        ),
        None
    );
    assert_eq!(
        sidebar_pin_profile_key(Some(WorkspaceScope::Development), None, None),
        None
    );
}

#[test]
fn local_synced_local_switch_restores_each_profiles_pins() {
    let mut settings = UiSettings::default();
    settings
        .sidebar_pins_mut("local".to_string())
        .extend(["local-1".to_string(), "local-2".to_string()]);
    settings
        .sidebar_pins_mut("synced:org-1:user-1".to_string())
        .push("synced-1".to_string());

    assert_eq!(settings.sidebar_pins("local"), ["local-1", "local-2"]);
    assert_eq!(settings.sidebar_pins("synced:org-1:user-1"), ["synced-1"]);
    assert_eq!(settings.sidebar_pins("local"), ["local-1", "local-2"]);
}

#[test]
fn account_switch_restores_each_accounts_pins() {
    let mut settings = UiSettings::default();
    settings
        .sidebar_pins_mut("synced:org-a:user-a".to_string())
        .push("a-1".to_string());
    settings
        .sidebar_pins_mut("synced:org-b:user-b".to_string())
        .push("b-1".to_string());

    assert_eq!(settings.sidebar_pins("synced:org-a:user-a"), ["a-1"]);
    assert_eq!(settings.sidebar_pins("synced:org-b:user-b"), ["b-1"]);
    assert_eq!(settings.sidebar_pins("synced:org-a:user-a"), ["a-1"]);
}

#[test]
fn loaded_values_are_clamped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth": 10000, "rightPaneWidth": 1}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.sidebar_width, SIDEBAR_MAX);
    assert_eq!(loaded.right_pane_width, RIGHT_PANE_MIN);
    assert!(!loaded.code_fences_fit_content);
    assert_eq!(
        UiSettings {
            sidebar_width: 1.0,
            ..Default::default()
        }
        .clamped()
        .sidebar_width,
        SIDEBAR_MIN
    );
    assert_eq!(
        UiSettings {
            files_autosave_delay_ms: 1,
            ..Default::default()
        }
        .clamped()
        .files_autosave_delay_ms,
        FILES_AUTOSAVE_DELAY_MIN_MS
    );
    assert_eq!(
        UiSettings {
            code_font_size: 100.0,
            ..Default::default()
        }
        .clamped()
        .code_font_size,
        crate::typography::FONT_SIZE_MAX
    );
    assert_eq!(
        UiSettings {
            terminal_font_size: 1.0,
            ..Default::default()
        }
        .clamped()
        .terminal_font_size,
        crate::typography::FONT_SIZE_MIN
    );
    assert_eq!(
        UiSettings {
            code_font_size: f32::NAN,
            ..Default::default()
        }
        .clamped()
        .code_font_size,
        crate::typography::CODE_FONT_SIZE_DEFAULT
    );
}

#[test]
fn legacy_files_editor_font_size_becomes_the_code_font_size() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"filesEditorFontSize": 15.0}"#,
    )
    .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.code_font_size, 15.0);
    assert_eq!(
        loaded.terminal_font_size,
        crate::typography::TERMINAL_FONT_SIZE_DEFAULT
    );
}

#[test]
fn git_history_column_widths_are_clamped_and_nan_heals() {
    let widths = GitHistoryColumnWidths {
        author: 10_000.0,
        date: f32::NAN,
        sha: 1.0,
    }
    .clamped();
    assert_eq!(widths.author, GitHistoryColumnWidths::AUTHOR_MAX);
    assert_eq!(widths.date, GitHistoryColumnWidths::default().date);
    assert_eq!(widths.sha, GitHistoryColumnWidths::SHA_MIN);
}

#[test]
fn git_history_column_order_deduplicates_and_restores_missing_columns() {
    let order = GitHistoryColumnOrder(vec![
        GitHistoryColumn::Sha,
        GitHistoryColumn::Sha,
        GitHistoryColumn::Author,
    ])
    .normalized();
    assert_eq!(
        order,
        GitHistoryColumnOrder(vec![
            GitHistoryColumn::Sha,
            GitHistoryColumn::Author,
            GitHistoryColumn::Date,
        ])
    );
}

#[test]
fn large_right_pane_width_is_preserved() {
    let loaded = UiSettings {
        right_pane_width: 2400.0,
        ..Default::default()
    }
    .clamped();
    assert_eq!(loaded.right_pane_width, 2400.0);
}

#[test]
fn nan_heals_to_default() {
    let healed = UiSettings {
        sidebar_width: f32::NAN,
        ..Default::default()
    }
    .clamped();
    assert_eq!(healed.sidebar_width, SIDEBAR_DEFAULT);
}

#[test]
fn defaults_match_zeron() {
    let d = UiSettings::default();
    assert_eq!(d.sidebar_width, 256.0);
    assert_eq!(d.right_pane_width, 520.0);
    assert_eq!(d.terminal_height, 280.0);
    assert!(!d.sidebar_collapsed && !d.right_pane_open && !d.terminal_open);
    assert!(!d.escape_stops_active_agent);
}

#[test]
fn legacy_panel_defaults_migrate_and_custom_shortcuts_survive() {
    let dir = tempfile::tempdir().unwrap();
    for (sidebar, right, expected_sidebar, expected_right) in [
        ("mod-s", "mod-b", "mod-b", "mod-r"),
        ("mod-shift-x", "mod-alt-b", "mod-shift-x", "mod-alt-b"),
    ] {
        std::fs::write(
            UiSettings::path(dir.path()),
            serde_json::json!({
                "keymap": {"toggleSidebar": sidebar, "toggleChanges": right}
            })
            .to_string(),
        )
        .unwrap();
        let settings = UiSettings::load(dir.path());
        assert_eq!(settings.keymap.toggle_sidebar, expected_sidebar);
        assert_eq!(settings.keymap.toggle_changes, expected_right);
        assert_eq!(settings.keymap.save_file, "mod-s");
    }
    // Once the new map has been saved, old chords can be assigned deliberately.
    let mut settings = UiSettings::default();
    settings.keymap.save_file = "mod-shift-s".into();
    settings.keymap.toggle_sidebar = "mod-s".into();
    settings.save(dir.path()).unwrap();
    assert_eq!(UiSettings::load(dir.path()).keymap, settings.keymap);
}

#[test]
fn legacy_migration_reserves_custom_chords_and_survives_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    for (custom, expected_save) in [
        ("mod-s", "mod-shift-s"),
        ("mod-b", "mod-s"),
        ("mod-r", "mod-s"),
    ] {
        std::fs::write(UiSettings::path(dir.path()), serde_json::json!({
                "keymap": {"toggleSidebar": "mod-shift-b", "toggleChanges": "mod-b", "newSession": custom}
            }).to_string()).unwrap();
        let loaded = UiSettings::load(dir.path());
        assert_eq!(loaded.keymap.new_session, custom);
        assert_eq!(loaded.keymap.toggle_sidebar, "mod-shift-b");
        assert_eq!(loaded.keymap.save_file, expected_save);
        assert!(conflicted_shortcuts(&loaded.keymap).is_empty());
        loaded.save(dir.path()).unwrap();
        assert_eq!(UiSettings::load(dir.path()).keymap, loaded.keymap);
    }
    // If every candidate is already custom-bound, leave the new action
    // unbound rather than steal a working shortcut from another action.
    std::fs::write(UiSettings::path(dir.path()), serde_json::json!({
            "keymap": {"toggleSidebar": "mod-shift-s", "newSession": "mod-s", "toggleChanges": "mod-r"}
        }).to_string()).unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.keymap.save_file, "");
    assert_eq!(loaded.keymap.new_session, "mod-s");
    assert!(conflicted_shortcuts(&loaded.keymap).is_empty());
}

#[test]
fn new_project_shortcut_migrates_and_persists() {
    let mut keymap: KeymapConfig = serde_json::from_str(r#"{"newSession":"mod-alt-n"}"#).unwrap();
    assert_eq!(keymap.get(ShortcutId::NewProject), "mod-shift-n");
    assert_eq!(keymap.get(ShortcutId::NewSession), "mod-alt-n");
    keymap.set(ShortcutId::NewProject, "mod-alt-p".into());
    let mut restored: KeymapConfig =
        serde_json::from_str(&serde_json::to_string(&keymap).unwrap()).unwrap();
    assert_eq!(restored.get(ShortcutId::NewProject), "mod-alt-p");
    restored.reset(ShortcutId::NewProject);
    assert_eq!(restored.get(ShortcutId::NewProject), "mod-shift-n");
}

#[test]
fn keymap_defaults_and_reset() {
    let mut keymap = KeymapConfig::default();
    assert_eq!(keymap.get(ShortcutId::SaveFile), "mod-s");
    keymap.set(ShortcutId::SaveFile, "mod-shift-s".into());
    keymap.reset(ShortcutId::SaveFile);
    assert_eq!(keymap.get(ShortcutId::SaveFile), "mod-s");
    assert_eq!(keymap.get(ShortcutId::ToggleSidebar), "mod-b");
    assert_eq!(keymap.get(ShortcutId::ToggleChanges), "mod-r");
    assert_eq!(keymap.get(ShortcutId::ToggleTerminal), "mod-j");
    let ctrl = if cfg!(target_os = "macos") {
        "ctrl"
    } else {
        "mod"
    };
    assert_eq!(keymap.get(ShortcutId::NextSession), format!("{ctrl}-tab"));
    assert_eq!(
        keymap.get(ShortcutId::PrevSession),
        format!("{ctrl}-shift-tab")
    );
    assert_eq!(keymap.get(ShortcutId::NewSession), "mod-n");
    assert_eq!(keymap.get(ShortcutId::ArchiveSession), "mod-shift-a");
    keymap.set(ShortcutId::ToggleSidebar, "mod-shift-x".into());
    assert_eq!(keymap.get(ShortcutId::ToggleSidebar), "mod-shift-x");
    keymap.reset(ShortcutId::ToggleSidebar);
    assert_eq!(keymap.get(ShortcutId::ToggleSidebar), "mod-b");
    keymap.set(ShortcutId::ArchiveSession, "mod-shift-y".into());
    assert_eq!(keymap.get(ShortcutId::ArchiveSession), "mod-shift-y");
    keymap.reset(ShortcutId::ArchiveSession);
    assert_eq!(keymap.get(ShortcutId::ArchiveSession), "mod-shift-a");
}

#[test]
fn every_shortcut_default_is_unique_and_bindable() {
    // A new shortcut must not ship in conflict with an existing one, and
    // its default must parse on this platform.
    assert!(conflicted_shortcuts(&KeymapConfig::default()).is_empty());
    for id in ShortcutId::ALL {
        assert!(
            gpui::Keystroke::parse(&platform_combo(id.default_combo())).is_ok(),
            "{:?} default combo does not parse",
            id
        );
    }
}

#[test]
fn combo_recording() {
    // How this platform spells a recorded ctrl (see `combo_from_keystroke_on`).
    let ctrl_combo = |suffix: &str| {
        if cfg!(target_os = "macos") {
            format!("ctrl-{suffix}")
        } else {
            format!("mod-{suffix}")
        }
    };
    assert_eq!(
        combo_from_keystroke(true, false, false, false, "s"),
        Some(ctrl_combo("s"))
    );
    assert_eq!(
        combo_from_keystroke(false, false, false, true, "s"),
        Some("mod-s".into())
    );
    assert_eq!(
        combo_from_keystroke(true, false, true, false, "tab"),
        Some(ctrl_combo("shift-tab"))
    );
    assert_eq!(
        combo_from_keystroke(true, true, true, false, "K"),
        Some(ctrl_combo("alt-shift-k"))
    );
    // Plain keys record without modifiers (Esc is filtered by the caller).
    assert_eq!(
        combo_from_keystroke(false, false, false, false, "f5"),
        Some("f5".into())
    );
    // Bare modifier presses record nothing.
    assert_eq!(
        combo_from_keystroke(true, false, false, false, "ctrl"),
        None
    );
    assert_eq!(
        combo_from_keystroke(false, false, true, false, "shift"),
        None
    );
    assert_eq!(combo_from_keystroke(false, false, false, false, ""), None);
}

#[test]
fn every_default_is_spelled_the_way_the_recorder_spells_it() {
    // The invariant `default_combo_on` documents. Checked for BOTH
    // platforms because the hazard only exists off macOS, so a single-OS
    // CI run would never see it.
    for mac in [true, false] {
        for id in ShortcutId::ALL {
            let combo = id.default_combo_on(mac);
            // Via the platform spelling, where modifier names are
            // unambiguous, so the decode can't inherit the bug it checks.
            let bound = platform_combo_on(mac, combo);
            let mut parts: Vec<&str> = bound.split('-').collect();
            let key = parts.pop().expect("a combo always ends in a key");
            let recorded = combo_from_keystroke_on(
                mac,
                parts.contains(&"ctrl"),
                parts.contains(&"alt"),
                parts.contains(&"shift"),
                parts.contains(&"cmd"),
                key,
            );
            assert_eq!(
                recorded.as_deref(),
                Some(combo),
                "{} default {combo:?} is unreachable from the recorder (mac={mac})",
                id.label()
            );
        }
    }
}

#[test]
fn defaults_are_distinct_physical_keys() {
    // Distinct STRINGS is not enough — two defaults could still resolve to
    // the same keystroke through `platform_combo`.
    let mut seen = std::collections::HashSet::new();
    for id in ShortcutId::ALL {
        let bound = platform_combo(id.default_combo());
        assert!(seen.insert(bound.clone()), "{bound:?} bound twice");
    }
}

#[test]
fn conflict_detection() {
    let mut keymap = KeymapConfig::default();
    assert!(conflicted_shortcuts(&keymap).is_empty());
    keymap.set(ShortcutId::ToggleChanges, "mod-b".into());
    let conflicts = conflicted_shortcuts(&keymap);
    assert!(conflicts.contains(&ShortcutId::ToggleSidebar));
    assert!(conflicts.contains(&ShortcutId::ToggleChanges));
    assert!(!conflicts.contains(&ShortcutId::ToggleTerminal));
    keymap.reset(ShortcutId::ToggleChanges);
    assert!(conflicted_shortcuts(&keymap).is_empty());
}

#[test]
fn combo_translation() {
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    assert_eq!(platform_combo("mod-s"), format!("{primary}-s"));
    assert_eq!(platform_combo("alt-f4"), "alt-f4");
    let display_primary = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    assert_eq!(
        display_combo("mod-shift-s"),
        format!("{display_primary}+Shift+S")
    );
    assert_eq!(display_combo("f5"), "F5");
    assert_eq!(display_combo_on(true, "mod-alt-up"), "Cmd+Opt+Up");
    assert_eq!(display_combo_on(false, "mod-alt-up"), "Ctrl+Alt+Up");
    // Literal ctrl passes through untouched — the macOS spelling of
    // session cycling.
    assert_eq!(platform_combo("ctrl-shift-tab"), "ctrl-shift-tab");
    assert_eq!(display_combo("ctrl-shift-tab"), "Ctrl+Shift+Tab");
}

#[test]
fn keymap_survives_old_settings_files() {
    // Files written before the keymap existed load with defaults.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(UiSettings::path(dir.path()), r#"{"sidebarWidth": 300}"#).unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.keymap, KeymapConfig::default());
    assert!(!loaded.sidebar_grouped);
    assert!(!loaded.escape_stops_active_agent);
}

#[test]
fn escape_stopping_is_opt_in_for_old_and_partial_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"sidebarWidth": 300, "soundEnabled": false}"#,
    )
    .unwrap();

    let loaded = UiSettings::load(dir.path());
    assert!(!loaded.escape_stops_active_agent);
    assert_eq!(loaded.sidebar_width, 300.0);
    assert!(!loaded.sound_enabled);
}

#[test]
fn jump_slots_get_set_and_reset() {
    let mut keymap = KeymapConfig::default();
    assert_eq!(keymap.get(ShortcutId::JumpSession(0)), "mod-1");
    assert_eq!(keymap.get(ShortcutId::JumpSession(8)), "mod-9");
    // Past the last slot there is no shortcut, not a panic.
    assert_eq!(keymap.get(ShortcutId::JumpSession(9)), "");
    assert_eq!(ShortcutId::JumpSession(9).jump_slot(), None);
    assert_eq!(ShortcutId::JumpSession(0).jump_slot(), Some(0));
    assert_eq!(ShortcutId::ArchiveSession.jump_slot(), None);

    keymap.set(ShortcutId::JumpSession(2), "mod-alt-3".into());
    assert_eq!(keymap.get(ShortcutId::JumpSession(2)), "mod-alt-3");
    keymap.reset(ShortcutId::JumpSession(2));
    assert_eq!(keymap.get(ShortcutId::JumpSession(2)), "mod-3");
    // A write past the last slot is dropped, and grows nothing.
    keymap.set(ShortcutId::JumpSession(9), "mod-0".into());
    assert_eq!(keymap.jump_session.len(), JUMP_SLOTS);
}

#[test]
fn short_or_long_jump_lists_heal_to_the_slot_count() {
    // Short: surviving entries keep their slot, the rest take defaults.
    let mut keymap = KeymapConfig {
        jump_session: vec!["mod-alt-1".into()],
        ..KeymapConfig::default()
    };
    keymap.heal_jump_slots();
    assert_eq!(keymap.jump_session.len(), JUMP_SLOTS);
    assert_eq!(keymap.get(ShortcutId::JumpSession(0)), "mod-alt-1");
    assert_eq!(keymap.get(ShortcutId::JumpSession(1)), "mod-2");

    // Long: the tail is dropped.
    let mut keymap = KeymapConfig {
        jump_session: (0..20).map(|i| format!("mod-{i}")).collect(),
        ..KeymapConfig::default()
    };
    keymap.heal_jump_slots();
    assert_eq!(keymap.jump_session.len(), JUMP_SLOTS);

    // Absent: the whole list comes back.
    let mut keymap = KeymapConfig {
        jump_session: Vec::new(),
        ..KeymapConfig::default()
    };
    keymap.heal_jump_slots();
    assert_eq!(keymap.jump_session, KeymapConfig::default().jump_session);
}

#[test]
fn a_malformed_jump_list_heals_without_losing_other_settings() {
    // Healing happens on load, so an odd jumpSession must not cost the
    // user their sidebar width or their other combos.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            UiSettings::path(dir.path()),
            r#"{"sidebarWidth": 300, "keymap": {"toggleSidebar": "mod-shift-x", "jumpSession": ["mod-alt-1", "mod-alt-2"]}}"#,
        )
        .unwrap();
    let loaded = UiSettings::load(dir.path());
    assert_eq!(loaded.sidebar_width, 300.0);
    assert_eq!(loaded.keymap.get(ShortcutId::ToggleSidebar), "mod-shift-x");
    assert_eq!(loaded.keymap.get(ShortcutId::JumpSession(0)), "mod-alt-1");
    assert_eq!(loaded.keymap.get(ShortcutId::JumpSession(8)), "mod-9");
    assert_eq!(loaded.keymap.jump_session.len(), JUMP_SLOTS);
}

#[test]
fn legacy_modifier_enter_shortcut_heals_without_losing_other_customizations() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"keymap": {"newSession": "mod-enter", "toggleSidebar": "mod-shift-x"}}"#,
    )
    .unwrap();

    let loaded = UiSettings::load(dir.path());
    assert_eq!(
        loaded.keymap.get(ShortcutId::NewSession),
        ShortcutId::NewSession.default_combo()
    );
    assert_eq!(loaded.keymap.get(ShortcutId::ToggleSidebar), "mod-shift-x");
}

#[test]
fn jump_hints_need_an_exact_modifier_match() {
    let keymap = KeymapConfig::default();
    // Mod alone matches mod-1..9.
    assert!(jump_hints_visible(&keymap, true, false, false));
    // Extra modifiers are a different chord (Cmd+Shift+4 screenshots).
    assert!(!jump_hints_visible(&keymap, true, false, true));
    assert!(!jump_hints_visible(&keymap, true, true, false));
    // Nothing held, nothing shown.
    assert!(!jump_hints_visible(&keymap, false, false, false));
    // Alt alone is not a jump modifier by default.
    assert!(!jump_hints_visible(&keymap, false, true, false));

    // Rebinding moves the trigger with it.
    let mut rebound = KeymapConfig::default();
    for slot in 0..JUMP_SLOTS {
        rebound.set(ShortcutId::JumpSession(slot), format!("mod-alt-{slot}"));
    }
    assert!(!jump_hints_visible(&rebound, true, false, false));
    assert!(jump_hints_visible(&rebound, true, true, false));

    // A jump combo with no modifiers must not pin the overlay open.
    let mut bare = KeymapConfig::default();
    bare.set(ShortcutId::JumpSession(0), "f5".into());
    assert!(!jump_hints_visible(&bare, false, false, false));
}

#[test]
fn modifier_send_hint_needs_only_the_primary_modifier() {
    assert!(modifier_send_hint_visible(true, false, false));
    assert!(!modifier_send_hint_visible(false, false, false));
    assert!(!modifier_send_hint_visible(true, true, false));
    assert!(!modifier_send_hint_visible(true, false, true));
}

#[test]
fn badge_combos_use_mac_glyphs_and_linux_text() {
    // macOS: glyphs in canonical ⌃⌥⇧⌘ order, no separators — the model
    // picker's ⌘N chip form.
    assert_eq!(badge_combo_on(true, "mod-2"), "⌘2");
    assert_eq!(badge_combo_on(true, "mod-shift-a"), "⇧⌘A");
    assert_eq!(badge_combo_on(true, "mod-alt-3"), "⌥⌘3");
    // A literal ctrl segment (macOS recorder spelling) is ⌃.
    assert_eq!(badge_combo_on(true, "ctrl-tab"), "⌃Tab");
    // Elsewhere the textual form stands.
    assert_eq!(badge_combo_on(false, "mod-2"), "Ctrl+2");
    assert_eq!(badge_combo_on(false, "mod-shift-a"), "Ctrl+Shift+A");
}

#[test]
fn combo_modifiers_reads_the_stored_form() {
    assert_eq!(combo_modifiers("mod-1"), (true, false, false));
    assert_eq!(combo_modifiers("mod-alt-shift-k"), (true, true, true));
    assert_eq!(combo_modifiers("f5"), (false, false, false));
    assert_eq!(combo_modifiers("shift-tab"), (false, false, true));
}

#[test]
fn a_keymap_missing_newer_shortcuts_keeps_its_customizations() {
    // Upgrade path: a file from a build that predates session cycling and
    // archiving carries the user's rebinds and defaults only the new rows.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        UiSettings::path(dir.path()),
        r#"{"keymap": {"toggleSidebar": "mod-shift-x"}}"#,
    )
    .unwrap();
    let keymap = UiSettings::load(dir.path()).keymap;
    assert_eq!(keymap.get(ShortcutId::ToggleSidebar), "mod-shift-x");
    assert_eq!(keymap.get(ShortcutId::ToggleTerminal), "mod-j");
    assert_eq!(
        keymap.get(ShortcutId::NextSession),
        ShortcutId::NextSession.default_combo()
    );
    assert_eq!(keymap.get(ShortcutId::ArchiveSession), "mod-shift-a");
}

#[test]
fn terminal_height_clamps_on_load() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(UiSettings::path(dir.path()), r#"{"terminalHeight": 5}"#).unwrap();
    assert_eq!(
        UiSettings::load(dir.path()).terminal_height,
        TERMINAL_MIN_HEIGHT
    );
    std::fs::write(UiSettings::path(dir.path()), r#"{"terminalHeight": 99999}"#).unwrap();
    assert_eq!(
        UiSettings::load(dir.path()).terminal_height,
        TERMINAL_ABS_MAX_HEIGHT
    );
}
