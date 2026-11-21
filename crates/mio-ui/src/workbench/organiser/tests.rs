use super::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, EntityInputHandler, TestAppContext};

use crate::editing::COMPOSITION_PENDING;
use crate::workbench::tests::{Fixture, drain, fixture, record};

fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn binary_item(number: u64, status: OrganiserStatus, path: &str, body: &[u8]) -> OrganiserItem {
    let mut raw = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let mut item = OrganiserItem::from_raw_parts(
        number,
        "Repeater",
        "http://127.0.0.1:9001",
        raw,
        None,
        None,
        HttpVersion::Http11,
    )
    .expect("binary organiser item");
    item.workflow_status = status;
    item
}

fn set_items(f: &Fixture, items: Vec<OrganiserItem>, cx: &mut TestAppContext) {
    let selected = items.first().map(|item| item.id);
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.tool = Tool::Organiser;
                view.workspace.organiser_items = items;
                view.organiser.selected_id = selected;
                view.sync_organiser_table(window, cx);
                view.reload_selected_organiser_detail(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
}

fn save_workspace(f: &mut Fixture, items: usize, cx: &mut TestAppContext) {
    let commands = drain(&mut f.commands);
    let [
        Command::SaveWorkspace {
            project, workspace, ..
        },
    ] = commands.as_slice()
    else {
        panic!(
            "expected one SaveWorkspace, got {} commands",
            commands.len()
        );
    };
    assert_eq!(*project, f.project);
    assert_eq!(workspace.organiser_items.len(), items);
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
}

fn visual_rows(f: &Fixture, cx: &mut TestAppContext) -> Vec<mio_core::Uuid> {
    cx.read(|cx| {
        f.view
            .read(cx)
            .organiser
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|entry| entry.id)
            .collect()
    })
}

#[gpui_kit::test]
fn archive_from_proxy_history_and_repeater_preserves_messages(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![crate::workbench::tests::tab("1")], 64);
    drain(&mut f.commands);

    let mut flow = record("http://127.0.0.1:9001/from-history", "binary body");
    flow.request.url = "http://127.0.0.1:9001/from-history".into();
    flow.summary.status = Some(200);
    flow.response = Some(HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: Vec::new(),
        body: b"resp-ok".to_vec(),
        trailers: Vec::new(),
    });
    let expected_req = request_to_raw(&flow.request);
    f.window
        .update(cx, |_, _, cx| {
            f.view.update(cx, |view, _| {
                view.tool = Tool::Proxy;
                view.proxy_page = ProxyPage::History;
                view.selected_history = Some(HistoryEntryId::Live(flow.summary.id));
                view.detail = Some(Box::new(flow.clone()));
            });
            let _ = cx;
        })
        .unwrap();
    f.window
        .update(cx, |_, window, cx| {
            f.view
                .update(cx, |view, cx| view.archive_to_organiser(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    click(f.window, "tool-organiser", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.organiser_items.len(), 1);
        let item = &view.workspace.organiser_items[0];
        assert_eq!((item.number, item.source_tool.as_str()), (1, "Proxy"));
        assert_eq!(item.url, "http://127.0.0.1:9001/from-history");
        assert_eq!(item.method, "POST");
        assert_eq!(item.status_code, Some(200));
        assert_eq!(item.request_raw, expected_req);
        assert!(
            item.response_raw
                .as_ref()
                .is_some_and(|raw| !raw.is_empty())
        );
    });
    save_workspace(&mut f, 1, cx);

    f.window
        .update(cx, |_, _, cx| {
            f.view.update(cx, |view, _| view.tool = Tool::Repeater);
            let _ = cx;
        })
        .unwrap();
    f.window
        .update(cx, |_, window, cx| {
            f.view
                .update(cx, |view, cx| view.archive_to_organiser(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.organiser_items.len(), 2);
        let item = &view.workspace.organiser_items[1];
        assert_eq!((item.number, item.source_tool.as_str()), (2, "Repeater"));
        assert_eq!(item.target, "http://127.0.0.1:9001");
        assert_eq!(item.url, "http://127.0.0.1:9001/");
        assert_eq!(item.method, "GET");
        assert_eq!(item.status_code, None);
        assert!(item.response_raw.is_none());
        assert!(item.request_raw.starts_with(b"GET / HTTP/1.1\r\n"));
    });
    save_workspace(&mut f, 2, cx);
}

#[gpui_kit::test]
fn filter_buttons_search_and_column_sort_update_rows(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let first = binary_item(1, OrganiserStatus::New, "/first", b"alpha");
    let second = binary_item(2, OrganiserStatus::InProgress, "/second", b"beta");
    let third = binary_item(3, OrganiserStatus::Done, "/third", b"gamma");
    let ids = [first.id, second.id, third.id];
    set_items(&f, vec![first, second, third], cx);
    assert_eq!(visual_rows(&f, cx), ids);

    click(f.window, "organiser-filter-new", cx);
    assert_eq!(visual_rows(&f, cx), vec![ids[0]]);
    click(f.window, "organiser-filter-all", cx);
    assert_eq!(visual_rows(&f, cx), ids);

    f.window
        .update(cx, |_, window, cx| {
            window.click("organiser-search", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("second", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(visual_rows(&f, cx), vec![ids[1]]);
    f.window
        .update(cx, |_, window, cx| {
            window.click("organiser-search", cx);
            window.press(crate::kbd!("a"), cx);
            window.press("backspace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(visual_rows(&f, cx), ids);

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("icon-sort", 0usize)).is_some());
            assert!(window.try_find(("icon-sort", 5usize)).is_none());
        })
        .unwrap();
    click(f.window, ("col-header", 5usize), cx);
    cx.read(|cx| {
        let sort = f.view.read(cx).organiser.sort;
        assert_eq!(
            sort,
            OrganiserSort {
                column: OrganiserSortColumn::Status,
                ascending: false,
            }
        );
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("icon-sort", 5usize)).is_some());
            assert!(window.try_find(("icon-sort", 0usize)).is_none());
        })
        .unwrap();
    click(f.window, ("col-header", 5usize), cx);
    cx.read(|cx| {
        let sort = f.view.read(cx).organiser.sort;
        assert!(sort.ascending);
    });
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn batch_selection_export_tsv_and_prune_completed_work(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let mut first = binary_item(1, OrganiserStatus::New, "/first", b"alpha");
    first.notes = "备注一\t换行".into();
    let second = binary_item(2, OrganiserStatus::InProgress, "/second", b"beta");
    let third = binary_item(3, OrganiserStatus::Done, "/done-item", b"gamma");
    let fourth = binary_item(4, OrganiserStatus::Ignored, "/ignored-item", b"delta");
    set_items(&f, vec![first, second, third, fourth], cx);

    click(f.window, "organiser-export-tsv", cx);
    cx.read(|cx| {
        let tsv = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("TSV copied to clipboard");
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(
            lines[0],
            "#\t时间\t来源\t方法\tURL\t状态码\t长度\t处理状态\t备注摘要"
        );
        assert_eq!(lines.len(), 5);
        assert!(lines[1].contains("http://127.0.0.1:9001/first"));
        assert!(lines[1].contains("新建"));
        assert!(lines[3].contains("已完成"));
        assert!(lines[4].contains("已忽略"));
    });

    click(f.window, "organiser-prune-completed", cx);
    save_workspace(&mut f, 2, cx);
    cx.read(|cx| {
        let nums: Vec<u64> = f
            .view
            .read(cx)
            .workspace
            .organiser_items
            .iter()
            .map(|i| i.number)
            .collect();
        assert_eq!(nums, vec![1, 2]);
    });

    click(f.window, "organiser-select-all", cx);
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).organiser.selected_ids.len(), 2);
    });
    click(f.window, "organiser-card-status-postponed", cx);
    save_workspace(&mut f, 2, cx);
    cx.read(|cx| {
        assert!(
            f.view
                .read(cx)
                .workspace
                .organiser_items
                .iter()
                .all(|item| item.workflow_status == OrganiserStatus::Postponed)
        );
    });

    click(f.window, "organiser-select-all", cx);
    click(f.window, "organiser-delete-item", cx);
    save_workspace(&mut f, 0, cx);
    cx.read(|cx| {
        assert!(f.view.read(cx).workspace.organiser_items.is_empty());
    });
}

#[gpui_kit::test]
fn status_and_notes_updates_persist_through_workspace_save(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let first = binary_item(1, OrganiserStatus::Unknown, "/first", b"alpha");
    let second = binary_item(2, OrganiserStatus::Unknown, "/second", b"beta");
    let first_id = first.id;
    set_items(&f, vec![first, second], cx);
    cx.read(|cx| assert_eq!(f.view.read(cx).organiser.selected_id, Some(first_id)));

    click(f.window, "organiser-card-status-done", cx);
    save_workspace(&mut f, 2, cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).workspace.organiser_items[0].workflow_status,
            OrganiserStatus::Done
        );
    });

    let notes = cx.read(|cx| f.view.read(cx).organiser.notes.clone());
    f.window
        .update(cx, |_, window, cx| {
            window.click(("input", notes.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("中文备注测试", cx);
            window.render_frame(cx);
            window.click("organiser-save-notes", cx);
        })
        .unwrap();
    cx.run_until_parked();
    save_workspace(&mut f, 2, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.organiser_items[0].notes, "中文备注测试");
        assert!(!view.organiser.notes_dirty);
    });
}

#[gpui_kit::test]
fn notes_ime_composition_blocks_save_and_selection(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let first = binary_item(1, OrganiserStatus::Unknown, "/first", b"alpha");
    let second = binary_item(2, OrganiserStatus::Unknown, "/second", b"beta");
    let first_id = first.id;
    set_items(&f, vec![first, second], cx);

    let notes = cx.read(|cx| f.view.read(cx).organiser.notes.clone());
    f.window
        .update(cx, |_, window, cx| {
            notes.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
            window.render_frame(cx);
            window.click("organiser-save-notes", cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.error.as_deref(), Some(COMPOSITION_PENDING));
        assert_eq!(view.workspace.organiser_items[0].notes, "");
    });
    assert!(drain(&mut f.commands).is_empty());

    click(f.window, ("organiser-row", 1usize), cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.error.as_deref(), Some(COMPOSITION_PENDING));
        assert_eq!(view.organiser.selected_id, Some(first_id));
    });
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn send_to_repeater_preserves_binary_request_and_target(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let body = [0x00_u8, 0xff, 0xfe, 0x80, b'M', b'I', b'O'];
    let item = binary_item(1, OrganiserStatus::Unknown, "/binary", &body);
    let expected_raw = item.request_raw.clone();
    let expected_target = item.target.clone();
    set_items(&f, vec![item], cx);

    click(f.window, "organiser-to-repeater", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.tool == Tool::Repeater);
        let tab_id = view.workspace.active.expect("new repeater tab active");
        let tab = view
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .expect("new tab stored");
        assert_eq!(tab.draft.target, expected_target);
        assert_eq!(tab.draft.raw, expected_raw);
        assert_eq!(tab.draft.version, HttpVersion::Http11);
    });
    save_workspace(&mut f, 1, cx);
}

#[gpui_kit::test]
fn send_to_comparer_preserves_request_and_response_bytes(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let mut first = binary_item(1, OrganiserStatus::Unknown, "/first", b"alpha");
    first.response_raw = Some(vec![0xde, 0xad, 0xbe, 0xef]);
    let second = binary_item(2, OrganiserStatus::Unknown, "/second", b"beta");
    let expected_req = first.request_raw.clone();
    let expected_resp = first.response_raw.clone().unwrap();
    set_items(&f, vec![first, second], cx);

    click(f.window, "organiser-to-comparer", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.tool == Tool::Comparer);
        let items = &view.extra_tools.comparer_items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].bytes, expected_req);
        assert!(items[0].label.contains("Organiser #1"));
    });

    click(f.window, "tool-organiser", cx);
    click(f.window, "organiser-to-comparer-resp", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.tool == Tool::Comparer);
        let items = &view.extra_tools.comparer_items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].bytes, expected_resp);
        assert_eq!(
            (view.extra_tools.left_index, view.extra_tools.right_index),
            (0, 1)
        );
    });
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn empty_state_matches_ext_layout_and_toggles_learn_more(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    set_items(&f, Vec::new(), cx);

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("organiser-sidebar").is_some());
            assert!(window.try_find("organiser-filter-all").is_some());
            assert!(window.try_find("organiser-filter-new").is_none());
            assert!(window.try_find("organiser-header").is_some());
            assert!(window.try_find("organiser-subtab-contents").is_some());
            assert!(window.try_find("organiser-empty").is_some());
            assert!(window.try_find("organiser-learn-more").is_some());
            assert!(window.try_find("organiser-table-pane").is_none());
            assert!(window.try_find("organiser-toolbar").is_none());
            assert!(window.try_find("inspector-toggle").is_none());
            assert!(window.try_find("organiser-notes-card").is_none());
            assert!(window.try_find("organiser-learn-more-card").is_none());
        })
        .unwrap();

    click(f.window, "organiser-learn-more", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("organiser-learn-more-card").is_some());
        })
        .unwrap();

    click(f.window, "organiser-learn-more-close", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("organiser-learn-more-card").is_none());
        })
        .unwrap();

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.set_organiser_status_filter(Some(OrganiserStatus::New), window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("organiser-empty").is_none());
            assert!(window.try_find("organiser-table-pane").is_some());
            assert!(window.try_find("organiser-toolbar").is_some());
            assert!(window.try_find("organiser-filter-new").is_some());
        })
        .unwrap();

    click(f.window, "organiser-filter-all", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("organiser-empty").is_some());
            assert!(window.try_find("organiser-table-pane").is_none());
            assert!(window.try_find("organiser-filter-new").is_none());
        })
        .unwrap();
}

#[gpui_kit::test]
fn sidebar_custom_collection_create_filter_assign_and_delete(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let first = binary_item(1, OrganiserStatus::New, "/auth", b"alpha");
    let second = binary_item(2, OrganiserStatus::Done, "/other", b"beta");
    let first_id = first.id;
    let second_id = second.id;
    set_items(&f, vec![first, second], cx);

    click(f.window, "organiser-new-collection", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("organiser-new-collection-input", cx);
            window.input("认证接口", cx);
            window.render_frame(cx);
            window.click("organiser-new-collection-confirm", cx);
        })
        .unwrap();
    cx.run_until_parked();

    let collection_id = cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.organiser.custom_collections.len(), 1);
        let col = &view.organiser.custom_collections[0];
        assert_eq!(col.name, "认证接口");
        assert_eq!(view.organiser.active_collection, Some(col.id));
        col.id
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("organiser-empty").is_some());
        })
        .unwrap();

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.assign_organiser_items_to_collection(collection_id, vec![first_id], window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(visual_rows(&f, cx), vec![first_id]);

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("organiser-collection-search", cx);
            window.input("认证", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!(
                        "organiser-collection-{collection_id}"
                    )))
                    .is_some()
            );
            assert!(window.try_find("organiser-filter-all").is_none());
            window.click("organiser-collection-search", cx);
            window.press(crate::kbd!("a"), cx);
            window.press("backspace", cx);
        })
        .unwrap();
    cx.run_until_parked();

    click(
        f.window,
        SharedString::from(format!("organiser-collection-delete-{collection_id}")),
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.organiser.custom_collections.is_empty());
        assert!(view.workspace.organiser_collections.is_empty());
        assert_eq!(view.organiser.active_collection, None);
    });
    assert_eq!(visual_rows(&f, cx), vec![first_id, second_id]);
}

#[gpui_kit::test]
fn organiser_collections_persist_rename_remove_and_restore_on_project_open(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let first = binary_item(1, OrganiserStatus::New, "/login", b"alpha");
    let second = binary_item(2, OrganiserStatus::InProgress, "/profile", b"beta");
    let first_id = first.id;
    let second_id = second.id;
    set_items(&f, vec![first, second], cx);

    click(f.window, "organiser-new-collection", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("organiser-new-collection-input", cx);
            window.input("登录链", cx);
            window.render_frame(cx);
            window.click("organiser-new-collection-confirm", cx);
        })
        .unwrap();
    cx.run_until_parked();
    save_workspace(&mut f, 2, cx);

    let collection_id = cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.organiser_collections.len(), 1);
        assert_eq!(view.workspace.organiser_collections[0].name, "登录链");
        view.workspace.organiser_collections[0].id
    });

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.assign_organiser_items_to_collection(
                    collection_id,
                    vec![first_id, second_id],
                    window,
                    cx,
                );
            });
        })
        .unwrap();
    cx.run_until_parked();
    save_workspace(&mut f, 2, cx);

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert!(view.rename_organiser_collection(
                    collection_id,
                    "认证核心".into(),
                    window,
                    cx,
                ));
            });
        })
        .unwrap();
    cx.run_until_parked();
    save_workspace(&mut f, 2, cx);

    let persisted = cx.read(|cx| (*f.view.read(cx).workspace).clone());
    assert_eq!(persisted.organiser_collections.len(), 1);
    assert_eq!(persisted.organiser_collections[0].name, "认证核心");
    assert_eq!(
        persisted.organiser_collections[0].item_ids,
        vec![first_id, second_id]
    );

    // Simulate reloading the saved project from disk -> collections must restore with zero loss.
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace: persisted,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.organiser.custom_collections.len(), 1);
        assert_eq!(view.organiser.custom_collections[0].id, collection_id);
        assert_eq!(view.organiser.custom_collections[0].name, "认证核心");
        assert!(view.organiser.custom_collections[0].item_ids.contains(&first_id));
        assert!(view.organiser.custom_collections[0].item_ids.contains(&second_id));
    });

    // Remove one item from collection and verify persistence.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.remove_organiser_items_from_collection(
                    collection_id,
                    vec![second_id],
                    window,
                    cx,
                );
            });
        })
        .unwrap();
    cx.run_until_parked();
    save_workspace(&mut f, 2, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.organiser_collections[0].item_ids, vec![first_id]);
        assert_eq!(view.organiser.custom_collections[0].item_ids.len(), 1);
    });
}
