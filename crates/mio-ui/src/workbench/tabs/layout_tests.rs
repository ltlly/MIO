use super::*;
use crate::workbench::tests::{
    Fixture, drain, fixture, record,
    settings::{acknowledge, open_settings, saved},
    tab,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use mio_core::{FlowState, Header, HttpResponse, HttpVersion, RepeaterLayoutMode};

fn click(fixture: &Fixture, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn settle(fixture: &mut Fixture, cx: &mut TestAppContext) -> Vec<Workspace> {
    let mut persisted = Vec::new();
    for _ in 0..8 {
        let commands = drain(&mut fixture.commands);
        if commands.is_empty() {
            return persisted;
        }
        for command in commands {
            if let Command::QueryRepeaterHistory {
                project,
                query_revision,
                tab_id,
                start,
                ..
            } = command
            {
                fixture
                    .events
                    .try_send(Event::RepeaterHistoryPage {
                        project,
                        query_revision,
                        tab_id,
                        result: Ok(mio_core::RepeaterHistoryPage {
                            total: 0,
                            start,
                            current_position: None,
                            rows: Vec::new(),
                            next_offset: None,
                        }),
                    })
                    .unwrap();
                continue;
            }
            let Command::SaveWorkspace { workspace, .. } = command else {
                panic!("unexpected non-SaveWorkspace command during layout settle");
            };
            workspace.validate().unwrap();
            fixture
                .events
                .try_send(Event::WorkspaceChanged {
                    project: fixture.project,
                    workspace: workspace.clone(),
                })
                .unwrap();
            persisted.push(workspace);
        }
        cx.run_until_parked();
    }
    panic!("repeater layout saves did not settle within bounded acknowledgements")
}

#[gpui_kit::test]
fn repeater_layout_modes_preserve_combined_draft_send_latest_and_reopen_with_vertical_tabs(
    cx: &mut TestAppContext,
) {
    let initial = tab("三布局测试");
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    click(&f, "tool-repeater", cx);
    settle(&mut f, cx);

    // 1. Default SideBySide layout renders both request and response headers side by side.
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("repeater-layout-side").checked(), Some(true));
            assert_eq!(window.find("repeater-layout-stack").checked(), Some(false));
            assert_eq!(
                window.find("repeater-layout-combined").checked(),
                Some(false)
            );
            let scale = window.scale_factor();
            for (btn_id, expected_count) in [
                ("repeater-layout-side", 2usize),
                ("repeater-layout-stack", 2usize),
                ("repeater-layout-combined", 1usize),
            ] {
                let btn_bounds = window.find(btn_id).bounds();
                let icon_quads = window
                    .painted_quads()
                    .into_iter()
                    .map(|quad| {
                        quad.bounds
                            .intersect(&quad.content_mask.bounds)
                            .map(|v| px(v.0 / scale))
                    })
                    .filter(|b| {
                        b.origin.x > btn_bounds.origin.x
                            && b.right() < btn_bounds.right()
                            && b.origin.y > btn_bounds.origin.y
                            && b.bottom() < btn_bounds.bottom()
                            && b.size.width >= px(5.)
                            && b.size.height >= px(3.5)
                    })
                    .count();
                assert_eq!(
                    icon_quads, expected_count,
                    "{btn_id} must paint {expected_count} solid layout block(s)"
                );
            }
            let req_bounds = window.find("repeater-request-raw").bounds();
            let resp_bounds = window.find("repeater-response-raw").bounds();
            assert!(
                req_bounds.origin.x < resp_bounds.origin.x,
                "SideBySide must place request left of response: {req_bounds:?} vs {resp_bounds:?}"
            );
        })
        .unwrap();

    // 2. Switch to TopBottom (stack) layout and verify vertical stacking.
    click(&f, "repeater-layout-stack", cx);
    let saved_stack = settle(&mut f, cx);
    assert_eq!(
        saved_stack.last().unwrap().repeater_settings.layout,
        RepeaterLayoutMode::TopBottom
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("repeater-stack-container").visible());
            assert_eq!(window.find("repeater-layout-stack").checked(), Some(true));
            let req_bounds = window.find("repeater-request-raw").bounds();
            let resp_bounds = window.find("repeater-response-raw").bounds();
            assert!(
                req_bounds.origin.y < resp_bounds.origin.y,
                "TopBottom must stack request above response: {req_bounds:?} vs {resp_bounds:?}"
            );
        })
        .unwrap();

    // 3. Switch to Combined layout and edit request, switch to Response and back, then send from Response view.
    click(&f, "repeater-layout-combined", cx);
    let saved_combined = settle(&mut f, cx);
    assert_eq!(
        saved_combined.last().unwrap().repeater_settings.layout,
        RepeaterLayoutMode::Combined
    );

    let req_entity = cx.read(|cx| f.view.read(cx).tab_editors[&tab_id].request.raw.clone());
    let edited_raw = "POST /combined-h2 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Length: 11\r\n\r\nhello=world";
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("repeater-combined-container").visible());
            assert_eq!(window.find("repeater-combined-req").checked(), Some(true));
            assert_eq!(window.find("repeater-combined-resp").checked(), Some(false));
            let req_tab_bounds = window.find("repeater-combined-req").bounds();
            let resp_tab_bounds = window.find("repeater-combined-resp").bounds();
            let layout_side_bounds = window.find("repeater-layout-side").bounds();
            let pretty_bounds = window.find("repeater-request-pretty").bounds();
            assert_eq!(req_tab_bounds.size.width, px(90.));
            assert_eq!(req_tab_bounds.size.height, px(39.));
            assert_eq!(resp_tab_bounds.size.width, px(102.));
            assert_eq!(resp_tab_bounds.size.height, px(39.));
            assert_eq!(resp_tab_bounds.origin.x, req_tab_bounds.right());
            assert_eq!(resp_tab_bounds.origin.y, req_tab_bounds.origin.y);
            assert!(layout_side_bounds.origin.x > resp_tab_bounds.right());
            assert!(layout_side_bounds.origin.y >= req_tab_bounds.origin.y);
            assert!(layout_side_bounds.bottom() <= req_tab_bounds.bottom());
            assert_eq!(pretty_bounds.origin.y, req_tab_bounds.bottom());
            window.click(("input", req_entity.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(edited_raw, cx);
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();

    // Switch to Response view in Combined mode; draft and editor state must stay intact.
    click(&f, "repeater-combined-resp", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("repeater-combined-req").checked(), Some(false));
            assert_eq!(window.find("repeater-combined-resp").checked(), Some(true));
            assert!(window.try_find("repeater-response-raw").is_some());
            assert!(window.try_find("repeater-request-raw").is_none());
            let req_tab_bounds = window.find("repeater-combined-req").bounds();
            let resp_tab_bounds = window.find("repeater-combined-resp").bounds();
            let render_bounds = window.find("repeater-response-render").bounds();
            assert_eq!(req_tab_bounds.size.width, px(90.));
            assert_eq!(resp_tab_bounds.size.width, px(102.));
            assert_eq!(render_bounds.origin.y, resp_tab_bounds.bottom());
        })
        .unwrap();
    cx.read(|cx| {
        let editors = &f.view.read(cx).tab_editors[&tab_id];
        assert_eq!(editors.request.raw.read(cx).value().as_ref(), edited_raw);
        assert!(editors.dirty);
    });

    // Switch back to Request view to verify draft text is untouched, then switch to Response and click Send.
    click(&f, "repeater-combined-req", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("repeater-combined-req").checked(), Some(true));
            assert!(window.try_find("repeater-request-raw").is_some());
        })
        .unwrap();
    cx.read(|cx| {
        let editors = &f.view.read(cx).tab_editors[&tab_id];
        assert_eq!(editors.request.raw.read(cx).value().as_ref(), edited_raw);
    });

    // Sending while viewing Response sub-tab still harvests the latest Request draft!
    click(&f, "repeater-combined-resp", cx);
    click(&f, "repeater-send", cx);
    let ws_before_send = saved(&mut f.commands);
    assert_eq!(ws_before_send.tabs[0].draft.raw, edited_raw.as_bytes());
    assert_eq!(ws_before_send.tabs[0].draft.revision, 2);
    acknowledge(&f, ws_before_send, cx);

    let sent_commands = drain(&mut f.commands);
    let (run_id, sent_draft) = sent_commands
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send {
                tab_id: id,
                run_id,
                draft,
                ..
            } if id == tab_id => Some((run_id, draft)),
            _ => None,
        })
        .expect("clicking Send in Combined mode must emit Command::Send with latest draft");
    assert_eq!(sent_draft.raw, edited_raw.as_bytes());
    assert_eq!(sent_draft.revision, 2);

    // Switch back to Request while in-flight, then deliver SendFinished and verify Response is populated.
    click(&f, "repeater-combined-req", cx);
    let mut finished_record = record("http://127.0.0.1:9001/combined-h2", "hello=world");
    finished_record.summary.id = run_id;
    finished_record.summary.status = Some(200);
    finished_record.summary.state = FlowState::Complete;
    finished_record.response = Some(HttpResponse {
        status: 200,
        version: HttpVersion::Http2,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Length".into(),
            value: b"7".to_vec(),
        }],
        body: b"resp-ok".to_vec(),
        trailers: Vec::new(),
    });
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id,
            draft_revision: sent_draft.revision,
            record: Box::new(finished_record),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    let after_finish_ws = settle(&mut f, cx);
    let reopened_snapshot = after_finish_ws.last().unwrap().clone();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("repeater-combined-resp").checked(),
                Some(true),
                "SendFinished automatically reveals the Response sub-tab in Combined mode"
            );
        })
        .unwrap();

    // 4. Combine with Orientation::Vertical and verify both vertical tab rail and Combined pane coexist.
    f.window
        .update(cx, |_, window, cx| {
            f.view
                .update(cx, |view, cx| view.toggle_orientation(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let vertical_ws = settle(&mut f, cx);
    let mut final_ws = vertical_ws.last().cloned().unwrap_or(reopened_snapshot);
    assert_eq!(final_ws.orientation, Orientation::Vertical);
    assert_eq!(
        final_ws.repeater_settings.layout,
        RepeaterLayoutMode::Combined
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let tab_bounds = window.find(format!("work-tab-{tab_id}")).bounds();
            let combined_bounds = window.find("repeater-combined-container").bounds();
            assert!(
                tab_bounds.origin.x < combined_bounds.origin.x,
                "Vertical tab bar must sit to the left of the Combined message pane"
            );
        })
        .unwrap();

    // 5. Simulate project reopen and verify RepeaterLayoutMode::Combined is restored.
    final_ws.revision += 1;
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace: final_ws,
        })
        .unwrap();
    cx.run_until_parked();
    let _ = drain(&mut f.commands);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("repeater-layout-combined").checked(),
                Some(true)
            );
            assert!(window.find("repeater-combined-container").visible());
        })
        .unwrap();
}

#[gpui_kit::test]
fn repeater_default_urlpath_naming_duplicate_tab_group_and_vertical_sidebar(
    cx: &mut TestAppContext,
) {
    let group = TabGroup {
        id: Uuid::new_v4(),
        title: "API 组".into(),
        collapsed: false,
        color: Some(5),
    };
    let mut seed = tab("1");
    seed.group = Some(group.id);
    seed.draft.target = "https://example.com:8443".into();
    seed.draft.raw = b"GET /api/v1/users?id=1 HTTP/1.1\r\nHost: example.com:8443\r\n\r\n".to_vec();
    seed.draft.version = HttpVersion::Http2;
    let seed_id = seed.id;
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace: Workspace {
                revision: 1,
                groups: vec![group.clone()],
                tabs: vec![seed],
                active: Some(seed_id),
                ..Default::default()
            },
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);

    click(&f, "tool-repeater", cx);
    settle(&mut f, cx);

    f.window
        .update(cx, |_, window, cx| {
            f.view
                .update(cx, |view, cx| view.copy_draft_to_repeater(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let saved_copy = settle(&mut f, cx);
    let ws_after_copy = saved_copy.last().unwrap();
    assert_eq!(ws_after_copy.tabs.len(), 2);
    assert_eq!(ws_after_copy.tabs[1].title, "2: /api/v1/users");
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.next_repeater_tab_title(None, Some(b"GET / HTTP/1.1\r\n\r\n")),
            "3: /"
        );
        let long_title = view.next_repeater_tab_title(
            None,
            Some(b"POST /api/v1/organizations/memberships/details?page=2 HTTP/1.1\r\n\r\n"),
        );
        assert_eq!(long_title, "3: /api/v1/organizations/membe…");
    });
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.select_tab(seed_id, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    settle(&mut f, cx);
    let req_entity = cx.read(|cx| f.view.read(cx).tab_editors[&seed_id].request.raw.clone());
    let updated_raw = "POST /api/v1/orders?limit=10 HTTP/2\r\nHost: example.com:8443\r\n\r\nx=1";
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", req_entity.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(updated_raw, cx);
            f.view.update(cx, |view, cx| {
                view.duplicate_repeater_tab(seed_id, window, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    let saved_dup_tab = settle(&mut f, cx);
    let ws_dup_tab = saved_dup_tab.last().unwrap();
    assert_eq!(ws_dup_tab.tabs.len(), 3);
    assert_eq!(ws_dup_tab.tabs[1].title, "3: /api/v1/orders");
    assert_eq!(ws_dup_tab.tabs[1].group, Some(group.id));
    assert_eq!(ws_dup_tab.tabs[1].draft.target, "https://example.com:8443");
    assert_eq!(ws_dup_tab.tabs[1].draft.raw, updated_raw.as_bytes());
    assert_eq!(ws_dup_tab.tabs[1].draft.version, HttpVersion::Http2);
    assert_eq!(ws_dup_tab.active, Some(ws_dup_tab.tabs[1].id));
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.duplicate_repeater_group(group.id, window, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    let saved_dup_group = settle(&mut f, cx);
    let ws_dup_group = saved_dup_group.last().unwrap();
    assert_eq!(ws_dup_group.groups.len(), 2);
    let cloned_group = &ws_dup_group.groups[1];
    assert_eq!(cloned_group.title, "API 组 副本");
    assert_eq!(cloned_group.color, group.color);
    let cloned_members: Vec<&WorkTab> = ws_dup_group
        .tabs
        .iter()
        .filter(|t| t.group == Some(cloned_group.id))
        .collect();
    assert_eq!(cloned_members.len(), 2);
    assert_eq!(cloned_members[0].title, "4: /api/v1/orders");
    assert_eq!(cloned_members[1].title, "5: /api/v1/orders");
    assert_eq!(cloned_members[0].draft.target, "https://example.com:8443");
    click(&f, "repeater-toggle-vertical-tabs", cx);
    let saved_vert = settle(&mut f, cx);
    assert_eq!(
        saved_vert.last().unwrap().orientation,
        Orientation::Vertical
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let sidebar = window.find("repeater-vertical-sidebar").bounds();
            assert_eq!(sidebar.size.width, px(220.));
            assert!(window.find("repeater-vertical-header").visible());
            assert!(window.find("repeater-vertical-new-tab").visible());
            assert!(window.find("repeater-vertical-new-group").visible());
            let grp_header = window.find(format!("group-{}", group.id));
            assert_eq!(grp_header.expanded(), Some(true));
            assert!(window.find(format!("work-tab-{seed_id}")).visible());
        })
        .unwrap();
}

#[gpui_kit::test]
fn repeater_settings_page_persists_h2_strip_h1_close_max_hops_and_layout(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("设置同步")], 64);
    drain(&mut f.commands);
    let settings = open_settings(&f, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find("settings-repeater-strip-h2-conn-headers")
                .checked(),
            Some(true)
        );
        assert_eq!(
            window.find("settings-repeater-redirect-never").selected(),
            Some(true)
        );
        window.click("settings-repeater-strip-h2-conn-headers", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws1 = saved(&mut f.commands);
    assert!(!ws1.repeater_settings.strip_connection_headers_over_http2);
    acknowledge(&f, ws1, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-repeater-redirect-in-scope", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws2 = saved(&mut f.commands);
    assert_eq!(
        ws2.repeater_settings.follow_redirects,
        RepeaterRedirectMode::InScopeOnly
    );
    acknowledge(&f, ws2, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-repeater-redirect-in-scope").selected(),
            Some(true)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn repeater_opens_inspector_by_default_and_respects_user_close(cx: &mut TestAppContext) {
    struct ResetAutoOpen;
    impl Drop for ResetAutoOpen {
        fn drop(&mut self) {
            render_pane::set_auto_open_repeater_inspector_in_test(false);
        }
    }
    let _reset = ResetAutoOpen;
    render_pane::set_auto_open_repeater_inspector_in_test(true);

    let initial = tab("Inspector默认展开");
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    click(&f, "tool-repeater", cx);
    settle(&mut f, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.find("inspector-sidebar").visible(),
                "Repeater must open Inspector by default when user has not dismissed it"
            );
            window.click("inspector-toggle", cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("inspector-sidebar").is_none(),
                "Repeater must keep Inspector closed after user explicitly dismisses it"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn repeater_tab_editors_materialize_lazily_and_evict_clean_idle_tabs_by_lru(
    cx: &mut TestAppContext,
) {
    let tabs: Vec<WorkTab> = (0..30).map(|i| tab(&format!("Tab-{i}"))).collect();
    let ids: Vec<TabId> = tabs.iter().map(|t| t.id).collect();
    let mut f = fixture(cx, tabs, 64);
    drain(&mut f.commands);

    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.tab_editors.len(),
            1,
            "initial workspace sync must materialize only the active tab editor"
        );
        assert!(view.tab_editors.contains_key(&ids[0]));
    });

    // Visit tabs 1..=18 via select_tab; resident count must stay bounded at <= 12
    // and oldest clean idle tabs must be evicted.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                for &id in &ids[1..=18] {
                    view.select_tab(id, window, cx);
                }
            });
        })
        .unwrap();

    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(
            view.tab_editors.len() <= 12,
            "resident tab editors should be bounded at <= 12, got {}",
            view.tab_editors.len()
        );
        assert!(view.tab_editors.contains_key(&ids[18]));
        assert!(
            !view.tab_editors.contains_key(&ids[0]),
            "oldest clean idle tab editor should have been evicted"
        );
    });

    // Now mark tab 10 dirty (without flushing via select_tab) and materialize tabs 20..=29
    // via ensure_tab_editor; dirty tab 10 and active tab 18 must never be evicted.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.tab_editors.get_mut(&ids[10]).unwrap().dirty = true;
                for &id in &ids[20..=29] {
                    assert!(view.ensure_tab_editor(id, window, cx));
                }
            });
        })
        .unwrap();

    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.tab_editors.len() <= 12);
        assert!(view.tab_editors.contains_key(&ids[18]));
        assert!(
            view.tab_editors.contains_key(&ids[10]),
            "dirty tab editor must not be evicted"
        );
    });
}
