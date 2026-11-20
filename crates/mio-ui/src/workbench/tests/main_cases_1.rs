use super::*;

#[gpui_kit::test]
fn ext_root_tools_start_at_the_content_top_as_42pt_flat_tabs(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("1")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let content = window.find("mio-workspace").bounds();
        let mut previous_right = content.origin.x;
        for (id, label) in [
            ("tool-dashboard", "Dashboard"),
            ("tool-target", "Target"),
            ("tool-proxy", "Proxy"),
            ("tool-intruder", "Intruder"),
            ("tool-repeater", "Repeater"),
            ("tool-collaborator", "Collaborator"),
            ("tool-sequencer", "Sequencer"),
            ("tool-decoder", "Decoder"),
            ("tool-comparer", "Comparer"),
            ("tool-logger", "Logger"),
            ("tool-organiser", "Organizer"),
            ("tool-extensions", "Extensions"),
            ("tool-discover", "Discover"),
        ] {
            let tab = window.find(id);
            let bounds = tab.bounds();
            assert!(tab.visible());
            assert_eq!(
                bounds.origin.y, content.origin.y,
                "root tools belong immediately below the native titlebar, without a project toolbar"
            );
            assert_eq!(
                bounds.size.height,
                px(42.),
                "{id} must use the External tool band height"
            );
            assert!(
                bounds.origin.x >= previous_right,
                "root tools must keep their External order"
            );
            assert_eq!(tab.role(), Some(gpui_kit::accesskit::Role::Tab));
            assert_eq!(tab.label(), Some(label));
            previous_right = bounds.right();
        }
        let tools = Bounds {
            origin: content.origin,
            size: size(previous_right - content.origin.x, px(42.)),
        }
        .scale(window.scale_factor());
        for quad in window.painted_quads() {
            let visible = quad.bounds.intersect(&quad.content_mask.bounds);
            if visible.size.width > gpui_kit::ScaledPixels(0.)
                && visible.size.height > gpui_kit::ScaledPixels(0.)
                && visible.origin.x >= tools.origin.x
                && visible.right() <= tools.right()
                && visible.origin.y >= tools.origin.y
                && visible.bottom() <= tools.bottom()
            {
                assert_eq!(
                    quad.corner_radii,
                    Default::default(),
                    "root tools must not render rounded button pills"
                );
            }
        }
        let actions_panel = window.find("suiteTitleBarActionsPanel").bounds();
        let open_browser = window.find("suiteOpenBrowserButton").bounds();
        let cmd_palette = window.find("suiteCommandPaletteButton").bounds();
        let search = window.find("suiteSearchButton").bounds();
        let settings = window.find("settingsButton").bounds();
        let mio_ai = window.find("suiteMioAiButton").bounds();
        assert_eq!(cmd_palette.origin.x - open_browser.right(), px(2.));
        assert_eq!(search.origin.x - cmd_palette.right(), px(2.));
        assert_eq!(settings.origin.x - search.right(), px(2.));
        assert_eq!(mio_ai.origin.x - settings.right(), px(2.));
        assert!(open_browser.origin.x >= actions_panel.origin.x);
        assert!(mio_ai.right() <= actions_panel.right());
        let scale = window.scale_factor();
        for btn_bounds in [open_browser, cmd_palette, search, settings] {
            let scaled = btn_bounds.scale(scale);
            assert!(
                !window.painted_quads().iter().any(|quad| {
                    quad.bounds.contains(&scaled.center())
                        && quad.background == rgb(0xeeeeee).into()
                        && quad.corner_radii != Default::default()
                }),
                "top action icon buttons must stay borderless/transparent like Ext"
            );
        }
        let mio_ai_scaled = mio_ai.scale(scale);
        assert!(
            window.painted_quads().iter().any(|quad| {
                quad.bounds.contains(&mio_ai_scaled.center())
                    && quad.background == rgb(0xf1f1f1).into()
                    && quad.corner_radii != Default::default()
            }),
            "suiteMioAiButton must paint External neutral light-gray rounded capsule background"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn ext_root_selection_paints_one_3pt_orange_underline_after_real_clicks(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("1")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        for (selected_id, tool) in [
            ("tool-dashboard", Tool::Dashboard),
            ("tool-target", Tool::Target),
            ("tool-intruder", Tool::Intruder),
            ("tool-repeater", Tool::Repeater),
            ("tool-collaborator", Tool::Collaborator),
            ("tool-sequencer", Tool::Sequencer),
            ("tool-decoder", Tool::Decoder),
            ("tool-comparer", Tool::Comparer),
            ("tool-logger", Tool::Logger),
            ("tool-organiser", Tool::Organiser),
            ("tool-extensions", Tool::Extensions),
            ("tool-discover", Tool::Discover),
            ("tool-proxy", Tool::Proxy),
        ] {
            window.click(selected_id, cx);
            assert!(fixture.view.read(cx).tool == tool);
            let selected = window.find(selected_id).bounds();
            let band_top = window.find("mio-workspace").bounds().origin.y;
            let scale = window.scale_factor();
            // Fixed 2026.9.1 PNG: 6px orange above a 2px separator at scale 2.
            let underlines: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background == rgb(0xdb612f).into())
                .map(|quad| {
                    quad.bounds
                        .intersect(&quad.content_mask.bounds)
                        .map(|value| px(value.0 / scale))
                })
                .filter(|bounds| {
                    bounds.origin.y >= band_top && bounds.bottom() <= band_top + px(42.)
                })
                .collect();
            assert_eq!(
                underlines,
                vec![Bounds {
                    origin: point(selected.origin.x, band_top + px(38.)),
                    size: size(selected.size.width, px(3.)),
                }],
                "the selected tool must paint one full-width underline above the 1pt divider"
            );
            for id in [
                "tool-dashboard",
                "tool-target",
                "tool-proxy",
                "tool-intruder",
                "tool-repeater",
                "tool-collaborator",
                "tool-sequencer",
                "tool-decoder",
                "tool-comparer",
                "tool-logger",
                "tool-organiser",
                "tool-extensions",
                "tool-discover",
            ] {
                assert_eq!(window.find(id).selected(), Some(id == selected_id));
            }
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn root_tool_tabs_keep_keyboard_focus_and_enter_space_activation(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("1")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.press("tab", cx);
        assert_eq!(window.find("tool-intruder").focused(), Some(true));
        assert_keyboard_focus_is_visible(window, "tool-intruder");
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Intruder);
        window.press("tab", cx);
        assert_eq!(window.find("tool-repeater").focused(), Some(true));
        assert_keyboard_focus_is_visible(window, "tool-repeater");
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Repeater);
        for (id, expected_tool) in [
            ("tool-collaborator", Tool::Collaborator),
            ("tool-sequencer", Tool::Sequencer),
            ("tool-decoder", Tool::Decoder),
            ("tool-comparer", Tool::Comparer),
            ("tool-logger", Tool::Logger),
            ("tool-organiser", Tool::Organiser),
        ] {
            window.press("tab", cx);
            assert_eq!(window.find(id).focused(), Some(true));
            assert_keyboard_focus_is_visible(window, id);
            window.press("enter", cx);
            assert!(fixture.view.read(cx).tool == expected_tool);
        }
        window.press("tab", cx);
        assert_eq!(window.find("tool-extensions").focused(), Some(true));
        assert_keyboard_focus_is_visible(window, "tool-extensions");
        window.press("space", cx);
        assert!(fixture.view.read(cx).tool == Tool::Extensions);
        window.press("tab", cx);
        assert_eq!(window.find("tool-discover").focused(), Some(true));
        assert_keyboard_focus_is_visible(window, "tool-discover");
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Discover);
    })
    .unwrap();
}

fn assert_keyboard_focus_is_visible(window: &Window, id: &str) {
    let control = window.find(SharedString::from(id.to_owned()));
    assert_eq!(control.focused(), Some(true));
    let bounds = control.bounds().scale(window.scale_factor());
    let margin = gpui_kit::ScaledPixels(8. * window.scale_factor());
    assert!(
        window.painted_quads().iter().any(|quad| {
            let visible = quad.bounds.intersect(&quad.content_mask.bounds);
            quad.border_color.a > 0.
                && quad.border_widths.any(|width| width.0 > 0.)
                && visible.size.width.0 > 0.
                && visible.size.height.0 > 0.
                && quad.bounds.contains(&bounds.center())
                && quad.bounds.size.width <= bounds.size.width + margin
                && quad.bounds.size.height <= bounds.size.height + margin
        }),
        "{id} must paint a visible focus outline after keyboard traversal"
    );
}

#[gpui_kit::test]
fn repeater_plus_and_close_keep_visible_keyboard_focus_and_activation(cx: &mut TestAppContext) {
    let original = tab("1");
    let close_id = format!("close-tab-{}", original.id);
    let mut fixture = fixture(cx, vec![original], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        for _ in 0..6 {
            window.press("tab", cx);
        }
        window.press("tab", cx);
        assert_eq!(window.find("tool-extensions").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("tool-discover").focused(), Some(true));
        window.press("tab", cx);
        assert_keyboard_focus_is_visible(window, &close_id);
        window.press("tab", cx);
        assert_keyboard_focus_is_visible(window, "repeater-new");
        window.press("enter", cx);
        window.press("shift-tab", cx);
        assert_keyboard_focus_is_visible(window, &close_id);
        window.press("space", cx);
        assert!(fixture.view.read(cx).workspace.tabs.is_empty());
    })
    .unwrap();
    assert_eq!(
        drain(&mut fixture.commands)
            .iter()
            .filter(|command| matches!(command, Command::NewRepeater { .. }))
            .count(),
        1
    );
}

#[gpui_kit::test]
fn returning_from_ext_report_keeps_first_tool_keyboard_focus_visible(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.press("tab", cx);
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Intruder);
        window.press("tab", cx);
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Repeater);
    })
    .unwrap();
    project_menu(&fixture, "查看 外部导入记录", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryExtReport { query_revision, .. }] = commands.as_slice() else {
        panic!("the native menu queries the empty project's report")
    };
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(None),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("ext-dismiss", cx);
        assert!(!fixture.view.read(cx).ext_import.visible);
        window.press("tab", cx);
        assert_eq!(window.find("tool-dashboard").focused(), Some(true));
        let bounds = window
            .find("tool-dashboard")
            .bounds()
            .scale(window.scale_factor());
        let margin = gpui_kit::ScaledPixels(8. * window.scale_factor());
        // Top/left are clipped by the workspace; the next tool/body cover right/bottom.
        let visible_focus = window.painted_quads().iter().any(|quad| {
            let visible = quad
                .bounds
                .intersect(&quad.content_mask.bounds)
                .intersect(&bounds);
            quad.border_color.a > 0.
                && quad.bounds.contains(&bounds.center())
                && quad.bounds.size.width <= bounds.size.width + margin
                && quad.bounds.size.height <= bounds.size.height + margin
                && visible.size.width.0 > 0.
                && visible.size.height.0 > 0.
                && (visible.origin.x < quad.bounds.origin.x + quad.border_widths.left
                    || visible.right() > quad.bounds.right() - quad.border_widths.right
                    || visible.origin.y < quad.bounds.origin.y + quad.border_widths.top
                    || visible.bottom() > quad.bounds.bottom() - quad.border_widths.bottom)
        });
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Dashboard);
        window.press("tab", cx);
        assert_keyboard_focus_is_visible(window, "tool-target");
        window.press("tab", cx);
        assert_keyboard_focus_is_visible(window, "tool-proxy");
        window.press("enter", cx);
        assert!(fixture.view.read(cx).tool == Tool::Proxy);
        assert!(window.is_action_available(&SaveProject, cx));
        assert!(
            visible_focus,
            "the first tool must paint its focus outline inside its unobscured bounds"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn ext_project_and_repeater_controls_do_not_add_duplicate_toolbars(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("1")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        for id in [
            "new-project",
            "open-project",
            "save-project",
            "ext-import",
            "ext-report",
            "repeater-restore",
            "tabs-orientation",
        ] {
            assert!(
                window.try_find(id).is_none(),
                "{id} must use its native menu, shortcut, or tab context menu instead of an extra toolbar"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn ext_repeater_47pt_tab_strip_follows_root_tools_and_ends_with_a_working_plus(
    cx: &mut TestAppContext,
) {
    let first = tab("1");
    let second = tab("2");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first, second], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        let tool = window.find("tool-repeater").bounds();
        let strip = window.find("work-tabs").bounds();
        let last = window
            .find(SharedString::from(format!("work-tab-{second_id}")))
            .bounds();
        let plus = window.find("repeater-new");
        let plus_bounds = plus.bounds();
        assert_eq!(
            strip.origin.y,
            tool.bottom(),
            "the work tabs must directly follow the root tool band"
        );
        assert_eq!(strip.size.height, px(47.));
        assert_eq!(plus.label(), Some("新建请求标签"));
        assert!(plus.visible());
        assert!(plus_bounds.origin.x >= last.right());
        assert!(plus_bounds.origin.y >= strip.origin.y && plus_bounds.bottom() <= strip.bottom());
        assert_eq!(plus_bounds.center().y, strip.center().y);
        window.click("repeater-new", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let commands = acknowledge_history_query(&fixture, commands, cx);
    assert!(matches!(
        commands.as_slice(),
        [Command::NewRepeater { project }] if *project == fixture.project
    ));
}

#[gpui_kit::test]
fn top_command_palette_and_global_search_buttons_filter_and_jump_to_hits(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("1")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("suiteCommandPaletteButton").bounds().size, size(px(27.), px(18.)));
        assert_eq!(window.find("suiteSearchButton").bounds().size, size(px(28.), px(18.)));
        window.click("suiteCommandPaletteButton", cx);
        window.render_frame(cx);
        assert!(window.find("global-command-palette-overlay").visible());
        assert!(fixture.view.read(cx).command_palette.read(cx).matched_count() >= 60);
        window.input("Organiser", cx);
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).command_palette.read(cx).matched_count(), 2);
        let first_selected = fixture.view.read(cx).command_palette.read(cx).selected_index();
        window.press("down", cx);
        window.render_frame(cx);
        let second_selected = fixture.view.read(cx).command_palette.read(cx).selected_index();
        assert_ne!(first_selected, second_selected);
        window.press("up", cx);
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).command_palette.read(cx).selected_index(), first_selected);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).tool, Tool::Organiser);
        assert!(!fixture.view.read(cx).command_palette_open);

        window.click("suiteCommandPaletteButton", cx);
        window.render_frame(cx);
        assert!(window.find("global-command-palette-overlay").visible());
        assert_eq!(fixture.view.read(cx).command_palette.read(cx).query(cx).as_ref(), "");
        window.input("Decoder", cx);
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).command_palette.read(cx).query(cx).as_ref(), "Decoder");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(fixture.view.read(cx).command_palette_open);
        assert_eq!(fixture.view.read(cx).command_palette.read(cx).query(cx).as_ref(), "");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(!fixture.view.read(cx).command_palette_open);

        window.click("suiteSearchButton", cx);
        window.render_frame(cx);
        assert!(window.find("global-search-overlay").visible());
        window.input("secret-token", cx);
        window.click("global-search-run", cx);
    })
    .unwrap();
    let (first_query, first_cancel) = drain(&mut fixture.commands).into_iter().find_map(|cmd| match cmd {
        Command::QueryGlobalSearch { query, cancel } => Some((query, cancel)),
        _ => None,
    }).expect("global search query sent");
    assert_eq!(first_query.term, "secret-token");
    assert!(!first_query.regex);
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(fixture.view.read(cx).global_search.loading);
        window.click("global-search-regex", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(first_cancel.is_cancelled(), "new search while loading must cancel previous token");
    let query = drain(&mut fixture.commands).into_iter().find_map(|cmd| match cmd {
        Command::QueryGlobalSearch { query, .. } => Some(query),
        _ => None,
    }).expect("toggling regex while loading must immediately issue updated query");
    assert!(query.regex);
    assert_eq!(query.query_revision, first_query.query_revision + 1);
    let flow = record("https://api.example.com/secret", "secret-token");
    let flow_id = flow.summary.id;
    fixture.events.try_send(Event::GlobalSearchPage {
        query,
        result: Ok(mio_core::GlobalSearchResult {
            total: 1,
            hits: vec![mio_core::GlobalSearchHit {
                target: mio_core::GlobalSearchTarget::LoggerFlow(flow_id),
                tool_label: "Logger · HTTP".into(),
                location_label: flow_id.to_string(),
                method_or_direction: "GET".into(),
                url: "https://api.example.com/secret".into(),
                status: Some(200),
                matched_in: "响应".into(),
                snippet: "secret-token".into(),
                notes_preview: String::new(),
                history_row: Some(HistoryRow::Live(flow.summary)),
                websocket_summary: None,
                site_map_entry: None,
            }],
        }),
    }).unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("global-search-result-0", cx);
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).tool, Tool::Logger);
        assert_eq!(fixture.view.read(cx).extra_tools.logger_selected, Some(flow_id));
        assert!(!fixture.view.read(cx).global_search.open);

        window.click("suiteSearchButton", cx);
        window.render_frame(cx);
        assert!(window.find("global-search-overlay").visible());
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(!fixture.view.read(cx).global_search.open);
    }).unwrap();
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("suiteOpenBrowserButton", cx);
    })
    .unwrap();
    let cmds = drain(&mut fixture.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::OpenConfiguredBrowser { project, url: None, .. }] if *project == fixture.project
    ));
    let proxy_addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();
    fixture
        .events
        .try_send(Event::BrowserLaunched {
            project: fixture.project,
            address: proxy_addr,
            executable: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .status
                .contains("已启动配置浏览器（代理 127.0.0.1:8080）")
        );
    });
}
