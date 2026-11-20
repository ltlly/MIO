use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

use super::*;
use crate::workbench::intruder::tests::{attack, default_pool_catalog, intruder_fixture};
use crate::workbench::tests::drain;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreateEdit {
    Unchanged,
    Name,
    Composition,
}

fn delete_legacy_create(edit: CreateEdit, cx: &mut TestAppContext) {
    let mut catalog = default_pool_catalog();
    let default = catalog.default_pool_id.unwrap();
    let mut pool = catalog.pools[0].clone();
    pool.id = Uuid::new_v4();
    pool.name = "已存在的 Create 池".into();
    pool.max_concurrent_requests = Some(2);
    catalog.pools.push(pool.clone());
    let mut tab = attack();
    tab.spec.resource_pool = Some(IntruderPoolSelection::Create(pool.clone()));
    let id = tab.id;
    let mut fixture = intruder_fixture(cx, vec![tab], 64);
    let path = PathBuf::from("/tmp/mio-deleted-create-regression.mio");
    fixture
        .view
        .update(cx, |view, _| view.project_path = Some(path.clone()));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-sidebar-resource-pool", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderResourcePools { query_revision, .. }] = commands.as_slice() else {
        panic!("Resource pool opens by reading the confirmed directory")
    };
    fixture
        .events
        .try_send(Event::IntruderResourcePools {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(catalog.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    let mut committed = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let before = committed.clone();
    committed.revision += 1;
    committed.intruder.tabs[0].spec.revision += 1;
    committed.intruder.tabs[0].spec.resource_pool = Some(IntruderPoolSelection::Existing(default));
    let name = cx.read(|cx| {
        fixture.view.read(cx).intruder.editors[&id]
            .pool_form
            .name
            .clone()
    });
    if edit != CreateEdit::Unchanged {
        cx.update_window(fixture.window, |_, window, cx| {
            window.click(("input", name.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            if edit == CreateEdit::Name {
                window.input("用户刚修改的池", cx);
            } else {
                name.update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
                });
            }
        })
        .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut fixture.commands);
        if edit == CreateEdit::Name {
            let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
                panic!("the new name is a real user edit, awaiting its own save acknowledgement")
            };
            let Some(IntruderPoolSelection::Create(saved)) =
                &workspace.intruder.tabs[0].spec.resource_pool
            else {
                panic!("new Create draft")
            };
            assert_eq!(saved.name, "用户刚修改的池");
        } else {
            assert!(commands.is_empty(), "preedit is never persisted");
        }
    }
    catalog.revision += 1;
    catalog.pools.retain(|current| current.id != pool.id);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.apply_deleted_pool_state(
                fixture.project,
                pool.id,
                &catalog,
                (&before, &committed),
                window,
                cx,
            )
            .unwrap();
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "applying a committed deletion cannot replay or save local input"
    );
    if edit != CreateEdit::Unchanged {
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            let form_id = view.intruder.editors[&id].pool_form.id;
            assert!(
                view.intruder.editors[&id].pool_form.creating,
                "new local Create input remains editable"
            );
            assert_ne!(
                form_id, pool.id,
                "new input cannot recreate the deleted identity"
            );
            assert_eq!(window.find("intruder-pool-create").checked(), Some(true));
            if edit == CreateEdit::Name {
                assert_eq!(name.read(cx).value().as_ref(), "用户刚修改的池");
                let snapshot = fixture
                    .view
                    .update(cx, |view, cx| view.snapshot_intruder_spec(id, window, cx))
                    .unwrap();
                let Some(IntruderPoolSelection::Create(current)) = snapshot.resource_pool else {
                    panic!("new Create snapshot")
                };
                assert_eq!(current.id, form_id);
                assert_eq!(current.name, "用户刚修改的池");
                assert_eq!(current.max_concurrent_requests, Some(2));
            } else {
                assert!(name.update(cx, |input, cx| {
                    input.marked_text_range(window, cx).is_some()
                }));
                assert!(
                    fixture
                        .view
                        .update(cx, |view, cx| view.snapshot_intruder_spec(id, window, cx))
                        .is_err()
                );
            }
        })
        .unwrap();
        return;
    }
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("s"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: None,
            workspace: None,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "Save after deletion must retain the already-confirmed Existing default, never revive legacy Create through its old editor"
        )
    };
    assert_eq!(*project, fixture.project);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, committed);
        assert!(!view.intruder.editors[&id].pool_form.creating);
        assert_eq!(view.intruder.editors[&id].pool_form.existing, Some(default));
    });
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-start", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("Start first saves its current Existing snapshot")
    };
    assert_eq!(
        workspace.intruder.tabs[0].spec.resource_pool,
        Some(IntruderPoolSelection::Existing(default))
    );
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::StartIntruder { spec, .. }] = commands.as_slice() else {
        panic!("one explicit Start using the default pool")
    };
    assert_eq!(
        spec.resource_pool,
        Some(IntruderPoolSelection::Existing(default))
    );
}

#[gpui_kit::test]
fn deleting_legacy_create_pool_keeps_save_and_start_bound_to_default(cx: &mut TestAppContext) {
    delete_legacy_create(CreateEdit::Unchanged, cx);
}

#[gpui_kit::test]
fn deleting_legacy_create_pool_preserves_newer_name_on_fresh_identity(cx: &mut TestAppContext) {
    delete_legacy_create(CreateEdit::Name, cx);
}

#[gpui_kit::test]
fn deleting_legacy_create_pool_preserves_name_ime_without_harvesting(cx: &mut TestAppContext) {
    delete_legacy_create(CreateEdit::Composition, cx);
}
