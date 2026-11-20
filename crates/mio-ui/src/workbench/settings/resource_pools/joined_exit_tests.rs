use super::*;
use crate::workbench::tests::pump_native;

#[gpui_kit::test]
fn pending_pool_edit_after_runtime_join_error_still_allows_native_exit_confirmation(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings, before) = setup(cx, false);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    click(
        settings,
        SharedString::from(format!("settings-pool-row-{}", before.pools[1].id)),
        cx,
    );
    click(settings, "settings-pool-edit", cx);
    let dialog = dialog(cx);
    name(dialog, "等待回执的编辑", cx);
    click(dialog, "settings-pool-ok", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResourcePools {
            project,
            expected_revision,
            pools,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Edit OK must submit one catalog change")
    };
    assert_eq!(
        (*project, *expected_revision),
        (fixture.project, before.revision)
    );
    assert_eq!(pools[1].name, "等待回执的编辑");
    assert!(!cancel.is_cancelled());
    let cancel = cancel.clone();
    let (finished, joined) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
    cx.update(|cx| {
        let joined = cx.background_spawn(async move { joined.await? });
        Workbench::observe_runtime(&fixture.view, fixture.window, joined, cx);
    });
    finished
        .send(Err(anyhow::anyhow!("pool update runtime join failed")))
        .unwrap();
    pump_native(cx);
    assert!(cancel.is_cancelled());
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(owner.intruder.resource_pools.management.mutation_pending());
        assert_eq!(owner.intruder.resource_pools.catalog.as_ref(), Some(&before));
        assert!(matches!(&owner.runtime_result, Some(Err(error)) if error.contains("pool update runtime join failed")));
    });
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "a joined Runtime cannot send a pool ACK; native Quit must reach the existing joined-exit confirmation"
    );
    cx.simulate_prompt_answer("保留窗口");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    let content = editor(dialog, cx);
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(!owner.quitting);
        assert_eq!(owner.workspace, workspace);
        assert_eq!(owner.intruder.resource_pools.catalog.as_ref(), Some(&before));
        assert!(owner.intruder.resource_pools.management.mutation_pending());
        assert!(matches!(&owner.runtime_result, Some(Err(error)) if error.contains("pool update runtime join failed")));
        assert_eq!(content.read(cx).form.name.read(cx).value().as_ref(), "等待回执的编辑");
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
