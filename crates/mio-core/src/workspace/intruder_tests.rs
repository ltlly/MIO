use super::*;
use crate::{IntruderSpec, IntruderTab, intruder::AttackType};

fn tab() -> IntruderTab {
    IntruderTab {
        id: Uuid::new_v4(),
        title: "未发送".into(),
        group: None,
        spec: IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft {
                revision: 1,
                target: String::new(),
                raw: vec![255],
                version: HttpVersion::Http11,
            },
            attack_type: AttackType::Sniper,
            positions: vec![],
            payload_sets: vec![],
            max_concurrent_requests: 0,
            request_timeout_ms: 0,
            update_host_header: false,
            save_to_project: true,
        },
        last_run: None,
    }
}

#[test]
fn intruder_identity_group_and_active_membership_are_tool_scoped() {
    let mut workspace = Workspace::default();
    let repeater = workspace.open_request(None).unwrap();
    let intruder = tab();
    workspace.intruder.active = Some(intruder.id);
    workspace.intruder.tabs.push(intruder.clone());
    workspace.validate().unwrap();

    workspace.intruder.tabs[0].id = repeater;
    workspace.intruder.active = Some(repeater);
    assert!(
        workspace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("ID 重复")
    );
    workspace.intruder.tabs[0] = intruder.clone();
    assert!(
        workspace.validate().is_err(),
        "a Repeater tab cannot be active in Intruder"
    );
    workspace.intruder.active = Some(intruder.id);
    workspace
        .intruder
        .closed
        .push(workspace.intruder.tabs.remove(0));
    assert!(
        workspace.validate().is_err(),
        "closed Intruder tabs cannot be active"
    );
    workspace.intruder.active = None;
    workspace.validate().unwrap();

    let group = TabGroup {
        id: Uuid::new_v4(),
        title: "组".into(),
        collapsed: true,
        color: None,
    };
    workspace.groups.push(group.clone());
    workspace.intruder.closed[0].group = Some(group.id);
    assert!(
        workspace.validate().is_err(),
        "Intruder cannot use a Repeater group"
    );
    workspace.intruder.groups.push(group);
    assert!(
        workspace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("ID 重复")
    );
    workspace.groups.clear();
    workspace.intruder.tab_sidebar_width = 640;
    workspace.validate().unwrap();
    workspace.intruder.tab_sidebar_width = 641;
    assert!(workspace.validate().is_err());
}

#[test]
fn global_payload_count_includes_empty_values_and_closed_intruder_tabs() {
    let mut workspace = Workspace::default();
    let mut open = tab();
    open.spec.payload_sets = vec![vec![vec![]; MAX_PAYLOADS]];
    workspace.intruder.tabs.push(open);
    let mut closed = tab();
    closed.spec.payload_sets = vec![vec![vec![0, 255]]];
    workspace.intruder.closed.push(closed);
    assert_eq!(workspace.draft_bytes().unwrap(), 4);
    assert!(
        workspace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("工作区 payload")
    );
    workspace.intruder.tabs[0].spec.payload_sets[0].pop();
    workspace.validate().unwrap();
}

#[test]
fn tab_and_raw_payload_budgets_cover_both_tools_and_closed_tabs() {
    let mut workspace = Workspace::default();
    workspace
        .tabs
        .push(WorkTab::from_request(None, "Repeater".into()));
    for _ in 0..511 {
        workspace.intruder.closed.push(tab());
    }
    workspace.validate().unwrap();
    workspace.intruder.tabs.push(tab());
    assert_eq!(workspace.tab_count(), 513);
    assert!(
        workspace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("512")
    );

    workspace = Workspace::default();
    for _ in 0..4 {
        let mut repeater = WorkTab::from_request(None, "Repeater".into());
        repeater.draft.raw = vec![0; MAX_BODY];
        workspace.closed.push(repeater);
    }
    workspace.validate().unwrap();
    let mut intruder = tab();
    intruder.spec.draft.raw.clear();
    intruder.spec.payload_sets = vec![vec![vec![255]]];
    workspace.intruder.closed.push(intruder);
    assert!(
        workspace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("64 MiB")
    );
}
