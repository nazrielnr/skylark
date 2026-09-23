//! Markdown renderer tests.

use super::*;
use super::*;
use crate::markdown::parser::{InlineStyle, parse_full};
use gpui::TestAppContext;

struct CodeSelectionHarness;

impl Render for CodeSelectionHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let opts = RenderOptions::settled("code-selection-test".into());
        let plain = |text: &str| {
            vec![InlineRun {
                text: text.into(),
                style: InlineStyle::default(),
            }]
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(selection_frame_reset())
            .child(text_element(
                &plain("before"),
                MD_TEXT_SIZE,
                MD_LINE_HEIGHT,
                false,
                0,
                0,
                &opts,
                &theme,
            ))
            .child(render_code_block_source(
                None,
                "selectable\n\nsecond",
                1,
                1,
                &opts,
                &theme,
                None,
            ))
            .child(text_element(
                &plain("after"),
                MD_TEXT_SIZE,
                MD_LINE_HEIGHT,
                false,
                2,
                2,
                &opts,
                &theme,
            ))
    }
}

#[gpui::test]
fn code_block_lines_participate_in_text_selection(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| CodeSelectionHarness);
    cx.simulate_resize(size(px(640.0), px(240.0)));
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });

    let before_key = "code-selection-test:0";
    let first_key = "code-selection-test-code1-line0";
    let blank_key = "code-selection-test-code1-line1";
    let second_key = "code-selection-test-code1-line2";
    let after_key = "code-selection-test:2";
    let before_bounds = selection_test_bounds(before_key);
    let first_bounds = selection_test_bounds(first_key);
    selection_test_bounds(blank_key);
    let second_bounds = selection_test_bounds(second_key);
    let after_bounds = selection_test_bounds(after_key);

    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: first_bounds.origin + point(px(5.0), px(9.0)),
        click_count: 2,
        ..Default::default()
    });
    assert_eq!(
        super::super::selection::selected_text().as_deref(),
        Some("selectable")
    );
    super::super::selection::end_active_drag();
    super::super::selection::clear_if_owner(first_key);

    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: first_bounds.origin + point(px(1.0), px(9.0)),
        click_count: 1,
        ..Default::default()
    });
    cx.simulate_event(gpui::MouseMoveEvent {
        position: point(second_bounds.right(), second_bounds.top() + px(9.0)),
        pressed_button: Some(gpui::MouseButton::Left),
        ..Default::default()
    });
    assert_eq!(
        super::super::selection::selected_text().as_deref(),
        Some("selectable\n\nsecond")
    );
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: point(second_bounds.right(), second_bounds.top() + px(9.0)),
        ..Default::default()
    });
    super::super::selection::clear_if_owner(first_key);

    super::super::selection::begin(before_key, 0);
    assert!(update_drag_at(point(
        after_bounds.right(),
        after_bounds.top() + px(9.0)
    )));
    assert_eq!(
        super::super::selection::selected_text().as_deref(),
        Some("before\nselectable\n\nsecond\nafter")
    );
    super::super::selection::end_active_drag();
    super::super::selection::clear_if_owner(before_key);
    assert!(before_bounds.top() < first_bounds.top());
    assert!(second_bounds.bottom() < after_bounds.bottom());
}

/// Markdown code blocks are the surface the shared setting's default was
/// taken from, so they scale 1:1 and need no ratio of their own.
#[test]
fn the_default_code_font_size_reproduces_the_historical_code_block_size() {
    assert_eq!(CODE_TEXT_SIZE, crate::typography::CODE_FONT_SIZE_DEFAULT);
    let theme = crate::theme::Theme::dark();
    assert_eq!(theme.code_font_size, CODE_TEXT_SIZE);
    assert_eq!(
        theme.code_font_size * CODE_LINE_HEIGHT_RATIO,
        CODE_LINE_HEIGHT
    );
}

#[test]
fn code_block_indices_include_nested_quotes_and_lists() {
    let quoted = parse_full("> ```rust\n> let x = 1;\n> ```\n");
    assert_eq!(code_block_indices(&quoted.blocks[0].block, 0), vec![0]);

    let listed = parse_full("- Result:\n\n  ```json\n  {\"ok\":true}\n  ```\n");
    assert_eq!(code_block_indices(&listed.blocks[0].block, 0), vec![1]);

    let top_level = parse_full("paragraph\n\n```text\nvalue\n```\n");
    assert_eq!(code_block_indices(&top_level.blocks[1].block, 1), vec![1]);
}

#[test]
fn sole_workspace_link_gets_a_file_path() {
    let runs = vec![InlineRun {
        text: "slides.ts".into(),
        style: InlineStyle {
            link: Some("src/slides.ts#L12".into()),
            ..Default::default()
        },
    }];
    assert_eq!(
        sole_workspace_file_link(&runs, "/work/comet"),
        Some("src/slides.ts".into())
    );
}

#[test]
fn direct_file_decoration_excludes_mixed_and_external_links() {
    let linked = InlineRun {
        text: "slides.ts".into(),
        style: InlineStyle {
            link: Some("src/slides.ts".into()),
            ..Default::default()
        },
    };
    let mixed = vec![
        InlineRun {
            text: "See ".into(),
            style: InlineStyle::default(),
        },
        linked,
    ];
    assert_eq!(sole_workspace_file_link(&mixed, "/work/comet"), None);

    let external = vec![InlineRun {
        text: "website".into(),
        style: InlineStyle {
            link: Some("https://example.com/slides.ts".into()),
            ..Default::default()
        },
    }];
    assert_eq!(sole_workspace_file_link(&external, "/work/comet"), None);
}

#[test]
fn standalone_recognized_filename_gets_a_decorative_identity() {
    let plain = vec![InlineRun {
        text: "AudienceView.tsx".into(),
        style: InlineStyle::default(),
    }];
    assert_eq!(
        sole_file_reference(&plain, "/work/comet"),
        Some("AudienceView.tsx".into())
    );

    let linked_unknown = vec![InlineRun {
        text: "artifact.unknown".into(),
        style: InlineStyle {
            link: Some("build/artifact.unknown".into()),
            ..Default::default()
        },
    }];
    assert_eq!(
        sole_file_reference(&linked_unknown, "/work/comet"),
        Some("build/artifact.unknown".into())
    );
}

#[test]
fn plain_file_decoration_rejects_prose_code_and_unknown_dotted_tokens() {
    let plain = |text: &str| {
        vec![InlineRun {
            text: text.into(),
            style: InlineStyle::default(),
        }]
    };
    assert_eq!(
        sole_file_reference(&plain("version 1.2"), "/work/comet"),
        None
    );
    assert_eq!(
        sole_file_reference(&plain("example.invalid"), "/work/comet"),
        None
    );

    let code = vec![InlineRun {
        text: "slides.ts".into(),
        style: InlineStyle {
            code: true,
            ..Default::default()
        },
    }];
    assert_eq!(sole_file_reference(&code, "/work/comet"), None);
}

#[test]
fn hard_break_file_list_resolves_every_recognized_line() {
    let runs = vec![InlineRun {
        text: "slides.ts\ncourseDecks.ts\nAudienceView.tsx\nglobals.css".into(),
        style: InlineStyle::default(),
    }];
    assert_eq!(
        plain_file_reference_lines(&runs, "/work/comet"),
        Some(vec![
            "slides.ts".into(),
            "courseDecks.ts".into(),
            "AudienceView.tsx".into(),
            "globals.css".into(),
        ])
    );

    let mixed = vec![InlineRun {
        text: "slides.ts\nnot a file".into(),
        style: InlineStyle::default(),
    }];
    assert_eq!(plain_file_reference_lines(&mixed, "/work/comet"), None);
}

/// Model GPUI's upstream affinity at a soft-wrap boundary: byte 5 is
/// reported at the end of row 0, while byte 6 is after the first glyph on
/// row 1.
fn wrapped_position(ix: usize) -> Option<gpui::Point<gpui::Pixels>> {
    (ix <= 9).then(|| {
        if ix <= 5 {
            point(px(ix as f32 * 10.0), px(0.0))
        } else {
            point(px((ix - 5) as f32 * 10.0), px(22.0))
        }
    })
}

fn wrapped_range_rects(range: Range<usize>) -> Vec<Bounds<gpui::Pixels>> {
    range_rects_with_positions(
        Bounds::new(point(px(0.0), px(0.0)), size(px(50.0), px(44.0))),
        px(22.0),
        &range,
        0.0,
        0.0,
        wrapped_position,
    )
}

#[test]
fn range_starting_at_soft_wrap_includes_first_glyph() {
    let rects = wrapped_range_rects(5..9);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].origin, point(px(0.0), px(22.0)));
    assert_eq!(rects[0].size, size(px(40.0), px(22.0)));
}

#[test]
fn range_crossing_soft_wrap_includes_first_continuation_glyph() {
    let rects = wrapped_range_rects(2..9);
    assert_eq!(rects.len(), 2);
    assert_eq!(rects[0].origin, point(px(20.0), px(0.0)));
    assert_eq!(rects[0].size, size(px(30.0), px(22.0)));
    assert_eq!(rects[1].origin, point(px(0.0), px(22.0)));
    assert_eq!(rects[1].size, size(px(40.0), px(22.0)));
}

#[test]
fn code_line_runs_cover_exactly() {
    let theme = Theme::dark();
    let mono = font(theme.font_mono.clone());
    let line = r#"let x = "hi"; // done"#;
    let document = skylark_syntax::highlight(skylark_syntax::HighlightRequest {
        source: line,
        path: None,
        fence_tag: Some("rust"),
    })
    .unwrap();
    let runs = runs_for_syntax_line(line, &document.lines[0], &mono, &theme);
    let total: usize = runs.iter().map(|r| r.len).sum();
    assert_eq!(total, line.len());
    assert!(
        runs.iter().all(|r| r.font == mono),
        "highlight must not change fonts"
    );
    // At least one non-plain color made it through.
    assert!(runs.iter().any(|r| r.color != theme.text));
}

#[test]
fn tree_sitter_runs_are_rich_and_paint_only() {
    let theme = Theme::dark();
    let mono = font(theme.font_mono.clone());
    let line = "let widget = build!(42);";
    let document = skylark_syntax::highlight(skylark_syntax::HighlightRequest {
        source: line,
        path: None,
        fence_tag: Some("rust"),
    })
    .unwrap();
    let runs = runs_for_syntax_line(line, &document.lines[0], &mono, &theme);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), line.len());
    assert!(runs.iter().all(|run| run.font == mono));
    let colors = runs.iter().map(|run| run.color).collect::<Vec<_>>();
    assert!(colors.contains(&theme.syntax.keyword));
    assert!(colors.contains(&theme.syntax.macro_name));
    assert!(colors.contains(&theme.syntax.number));
}

#[test]
fn affected_language_roles_flow_through_markdown_paint_only() {
    let cases: &[(&str, &str, &[HighlightKind])] = &[
        (
            "typescript",
            "export function derive(name: string) { return call(name); }",
            &[
                HighlightKind::Keyword,
                HighlightKind::Function,
                HighlightKind::Parameter,
                HighlightKind::TypeBuiltin,
            ],
        ),
        (
            "tsx",
            "function card(props: Props): JSX.Element { return <main id={props.id} />; }",
            &[
                HighlightKind::Tag,
                HighlightKind::Attribute,
                HighlightKind::Type,
            ],
        ),
        (
            "kotlin",
            "fun greet(name: String) = println(name)",
            &[
                HighlightKind::Keyword,
                HighlightKind::Function,
                HighlightKind::Parameter,
                HighlightKind::TypeBuiltin,
            ],
        ),
        (
            "dockerfile",
            "RUN echo \"hello\"",
            &[
                HighlightKind::Keyword,
                HighlightKind::Function,
                HighlightKind::String,
            ],
        ),
    ];
    for &(fence_tag, line, required) in cases {
        let document = skylark_syntax::highlight(skylark_syntax::HighlightRequest {
            source: line,
            path: None,
            fence_tag: Some(fence_tag),
        })
        .unwrap();
        let kinds = document.lines[0]
            .iter()
            .map(|span| span.kind)
            .collect::<Vec<_>>();
        for &kind in required {
            assert!(kinds.contains(&kind), "missing {kind:?} for {fence_tag}");
        }
        for theme in [Theme::dark(), Theme::light()] {
            let mono = font(theme.font_mono.clone());
            let runs = runs_for_syntax_line(line, &document.lines[0], &mono, &theme);
            assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), line.len());
            assert!(runs.iter().all(|run| run.font == mono));
            let colors = runs.iter().map(|run| run.color).collect::<Vec<_>>();
            for &kind in required {
                assert!(
                    colors.contains(&token_color(kind, &theme)),
                    "missing {kind:?} color for {fence_tag}"
                );
            }
        }
    }
}

#[test]
fn code_line_runs_with_no_tokens_are_one_plain_run() {
    let theme = Theme::dark();
    let mono = font(theme.font_mono.clone());
    let runs = runs_for_syntax_line("plain text", &[], &mono, &theme);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].len, 10);
}

#[test]
fn flatten_collects_and_merges_inline_code_ranges() {
    let theme = Theme::dark();
    let code = |text: &str| InlineRun {
        text: text.into(),
        style: InlineStyle {
            code: true,
            ..Default::default()
        },
    };
    let plain = |text: &str| InlineRun {
        text: text.into(),
        style: InlineStyle::default(),
    };
    let flat = flatten_runs(
        &[
            plain("use "),
            code("foo"),
            code("()"),
            plain(" and "),
            code("bar"),
        ],
        &theme,
        false,
    );
    // Adjacent code runs merge into ONE wash box; separated ones don't.
    assert_eq!(flat.code_ranges, vec![4..9, 14..17]);
    // Code text is the violet tint; the square run background is gone
    // (the rounded wash is painted by the canvas underlay instead).
    assert_eq!(flat.runs[1].color, inline_code_text(&theme));
    assert_eq!(flat.runs[1].background_color, None);
    assert_eq!(flat.runs[0].color, theme.text);
}

#[test]
fn code_palette_is_colored_and_shared() {
    // Round 9: transcript code blocks paint the soft hues (rose keyword,
    // green string, amber number); comments stay faint neutral.
    let theme = Theme::dark();
    assert_ne!(token_color(HighlightKind::Keyword, &theme), theme.text);
    assert_ne!(
        token_color(HighlightKind::String, &theme),
        token_color(HighlightKind::Keyword, &theme)
    );
    assert_ne!(token_color(HighlightKind::Comment, &theme), theme.text);
}

#[test]
fn flatten_runs_maps_links_and_styles() {
    let theme = Theme::dark();
    let runs = vec![
        InlineRun {
            text: "go ".into(),
            style: InlineStyle::default(),
        },
        InlineRun {
            text: "here".into(),
            style: InlineStyle {
                link: Some("https://x.dev".into()),
                ..Default::default()
            },
        },
        InlineRun {
            text: " now".into(),
            style: InlineStyle {
                bold: true,
                ..Default::default()
            },
        },
    ];
    let flat = flatten_runs(&runs, &theme, false);
    assert_eq!(flat.text, "go here now");
    assert_eq!(flat.links, vec![(3..7, "https://x.dev".to_string())]);
    let total: usize = flat.runs.iter().map(|r| r.len).sum();
    assert_eq!(total, flat.text.len());
    // Links stay monochrome (foreground + underline), never accent-tinted.
    assert_eq!(flat.runs[1].color, theme.text);
    assert!(flat.runs[1].underline.is_some());
    assert_eq!(flat.runs[2].font.weight, FontWeight::SEMIBOLD);
}

#[test]
fn table_columns_floor_and_padding() {
    // A short column keeps its content width (floored at MIN_COLUMN_CONTENT
    // + padding); a wide one may wrap but no narrower than minColumnWidth.
    let geo = table_columns(&[10.0, 200.0]);
    assert_eq!(geo.naturals, vec![72.0, 224.0]); // 48+24, 200+24
    assert_eq!(geo.minimums, vec![72.0, 96.0]);
    assert_eq!(geo.min_table_width, 168.0);
}

#[test]
fn table_columns_are_content_proportional_not_equal() {
    let geo = table_columns(&[300.0, 60.0, 60.0]);
    // Flex grow factors are the naturals — a prose column gets a larger
    // share than short ones (not equal thirds).
    assert!(geo.naturals[0] > 3.0 * geo.naturals[1] * 0.9);
    assert_eq!(geo.naturals[1], geo.naturals[2]);
}

#[test]
fn table_header_flattens_at_weight_700() {
    let theme = Theme::dark();
    let runs = vec![InlineRun {
        text: "Header".into(),
        style: InlineStyle::default(),
    }];
    let flat = flatten_runs_weighted(&runs, &theme, TABLE_HEADER_WEIGHT);
    assert_eq!(flat.runs[0].font.weight, FontWeight::BOLD);
    // Strong runs inside a 700 header stay 700 (never drop to semibold).
    let bold_runs = vec![InlineRun {
        text: "Strong".into(),
        style: InlineStyle {
            bold: true,
            ..Default::default()
        },
    }];
    let flat = flatten_runs_weighted(&bold_runs, &theme, TABLE_HEADER_WEIGHT);
    assert_eq!(flat.runs[0].font.weight, FontWeight::BOLD);
}

#[test]
fn adjacent_same_link_runs_merge_into_one_range() {
    let theme = Theme::dark();
    let style = InlineStyle {
        link: Some("https://x.dev".into()),
        ..Default::default()
    };
    let runs = vec![
        InlineRun {
            text: "bold".into(),
            style: InlineStyle {
                bold: true,
                ..style.clone()
            },
        },
        InlineRun {
            text: " tail".into(),
            style,
        },
    ];
    let flat = flatten_runs(&runs, &theme, false);
    assert_eq!(flat.links, vec![(0..9, "https://x.dev".to_string())]);
}

#[test]
fn viewport_cache_releases_offscreen_paint_data() {
    let mut cache = RenderCache::default();
    for row in ["visible", "offscreen"] {
        cache.flats.insert(
            (row.into(), 0, 0),
            Rc::new(FlatText {
                original: None,
                text: "text".into(),
                runs: Vec::new(),
                links: Vec::new(),
                code_ranges: Vec::new(),
            }),
        );
        cache.code.insert(
            (row.into(), 0, 0),
            Rc::new(CachedCode {
                code_len: 0,
                hl_key: (0, 0),
                lines: Vec::new(),
            }),
        );
    }
    cache.retain_rows(&std::collections::HashSet::from(["visible".into()]));
    assert_eq!(cache.flats.len(), 1);
    assert_eq!(cache.code.len(), 1);
    assert!(cache.flats.keys().all(|(row, _, _)| row == "visible"));
    cache.retain_rows(&std::collections::HashSet::new());
    assert!(cache.flats.is_empty());
    assert!(cache.code.is_empty());
}

#[test]
fn style_generation_change_invalidates_cached_runs() {
    let mut cache = RenderCache {
        generation: 10,
        ..Default::default()
    };
    cache.flats.insert(
        ("row".into(), 0, 0),
        Rc::new(FlatText {
            original: None,
            text: "cached".into(),
            runs: Vec::new(),
            links: Vec::new(),
            code_ranges: Vec::new(),
        }),
    );
    cache.sync_generation(10);
    assert_eq!(cache.flats.len(), 1, "same style is idempotent");
    cache.sync_generation(11);
    assert!(
        cache.flats.is_empty(),
        "font or color changes invalidate runs"
    );
    assert!(cache.code.is_empty());
}
