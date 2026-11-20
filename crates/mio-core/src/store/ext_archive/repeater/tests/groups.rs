use super::*;
use serde_json::{Value, json};

fn field(id: u8, value: Value) -> Value {
    json!({"id": id, "observation": {"state": "value", "value": value}})
}

fn stage() -> (
    ProjectStore,
    Uuid,
    ExtImportReport,
    Vec<ExtItem>,
    Vec<ExtItem>,
) {
    let archive = [vec![0; 80], RAW.to_vec()].concat();
    let (mut store, source) = pending(&archive);
    let mut report = store.ext_report(source).unwrap();
    report.schema_floor = 229;
    report.schema_revision = 229;
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=?1",
            [serde_json::to_vec(&report).unwrap()],
        )
        .unwrap();
    let mut groups = Vec::new();
    for (index, title, color, expanded) in [(0, "Alpha组", 7, true), (1, "Beta组", 12, false)] {
        let mut group = item(source, "root.f2.f4", index);
        group.object_address = Some(200 + index);
        group.metadata_json = serde_json::to_vec(&json!({
            "version": 1, "context": "repeater_group_229",
            "slot": {"state": "value", "value": group.object_address},
            "record": {"object": {"address": 400 + index, "object_type": 0, "subtype": 1,
                "descriptors": [{"id": 3, "relative_offset": 29}, {"id": 4, "relative_offset": 33}]},
                "mapping": {"state": "value", "value": {
                    "name": field(0, json!({"object": {"object_type": 0, "subtype": 0}, "mapping": {"state": "value", "value": title}})),
                    "color": field(1, json!(color)), "expanded": field(2, json!(expanded)),
                }}},
        })).unwrap();
        groups.push(group);
    }
    let mut tabs = Vec::new();
    let mut messages = Vec::new();
    for index in 0..3 {
        let mut tab = item(source, "root.f2.f1", index);
        let mut request = message(&tab, 4, 80, RAW);
        request.role = "current_request".into();
        let group = if index < 2 {
            field(
                32,
                json!({"address": 400 + index, "object_type": 0, "subtype": 1}),
            )
        } else {
            json!({"id": 32, "observation": {"state": "null"}})
        };
        let mut current = field(
            4,
            json!({"kind": "bytes", "address": 72, "payload_bytes": RAW.len(), "element_count": RAW.len(), "total_size": RAW.len() + 8}),
        );
        current["binding"] =
            json!({"source_id": source, "item_id": tab.id, "message_id": request.id});
        current["title"] = field(
            0,
            json!({"object": {"object_type": 0, "subtype": 0}, "mapping": {"state": "value", "value": format!("请求{index}")}}),
        );
        tab.metadata_json = serde_json::to_vec(&json!({
            "version": 1, "context": "repeater_current_229", "slot": {"state": "value", "value": tab.object_address},
            "record": {"object": {"object_type": 0, "subtype": 2}, "mapping": {"state": "value", "value": {
                "current_request": current, "selected_history": field(1, json!(-1)),
                "history": {"id": 2, "observation": {"state": "null"}}, "group": group,
            }}},
        })).unwrap();
        tabs.push(tab);
        messages.push(request);
    }
    store.put_ext_batch(&groups, &[]).unwrap();
    store.put_ext_batch(&tabs, &messages).unwrap();
    report.collections = vec![
        ExtCollectionReport {
            path: "root.f2.f1".into(),
            logical_items: Some(3),
            indexed_items: 3,
            message_count: 3,
            state: "indexed".into(),
        },
        ExtCollectionReport {
            path: "root.f2.f4".into(),
            logical_items: Some(2),
            indexed_items: 2,
            message_count: 0,
            state: "indexed".into(),
        },
    ];
    report.indexed_items = 5;
    report.indexed_messages = 3;
    report.repeater_active_tab = Some(1);
    report.complete_archive = true;
    (store, source, report, tabs, groups)
}

#[test]
fn groups_restore_source_order_membership_color_and_expansion_then_keep_mio_edits_on_reopen() {
    let (mut store, source, report, _, groups) = stage();
    let restored = store
        .finish_source_with_repeater_workspace(source, &report, &CancellationToken::new())
        .unwrap();
    let mut workspace = store.workspace().unwrap();
    assert_eq!(workspace.groups.len(), 2);
    assert_eq!(
        workspace
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect::<Vec<_>>(),
        ["Alpha组", "Beta组"]
    );
    assert_eq!(
        workspace
            .groups
            .iter()
            .map(|group| (group.color, group.collapsed))
            .collect::<Vec<_>>(),
        [(Some(7), false), (Some(12), true)]
    );
    assert!(
        workspace
            .groups
            .iter()
            .all(|group| groups.iter().all(|source| source.id != group.id))
    );
    assert_ne!(workspace.groups[0].id, workspace.groups[1].id);
    assert_eq!(
        workspace
            .tabs
            .iter()
            .map(|tab| tab.group)
            .collect::<Vec<_>>(),
        [
            Some(workspace.groups[0].id),
            Some(workspace.groups[1].id),
            None
        ]
    );
    assert_eq!(workspace.active, Some(workspace.tabs[1].id));
    assert!(workspace.tabs.iter().all(|tab| tab.draft.raw == RAW));
    assert!(
        restored
            .gaps
            .iter()
            .any(|gap| gap.contains("已恢复 2 个 Repeater 分组"))
    );
    let raw = store
        .read_source_range(source, 0, report.source_bytes as usize)
        .unwrap();
    workspace.revision += 1;
    workspace.groups[0].title = "Renamed组".into();
    workspace.groups[0].color = Some(12);
    workspace.groups[0].collapsed = true;
    workspace.groups[1].collapsed = false;
    workspace.tabs[0].group = None;
    workspace.tabs[2].group = Some(workspace.groups[1].id);
    store.save_workspace(&workspace).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("groups.mio");
    let copy = directory.path().join("groups-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(reopened.ext_report(source).unwrap(), restored);
        assert_eq!(
            reopened
                .read_source_range(source, 0, report.source_bytes as usize)
                .unwrap(),
            raw
        );
        assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
        reopened.close().unwrap();
    }
}

#[test]
fn wrong_group_reference_tag_source_or_duplicate_identity_cannot_publish_workspace() {
    for invalid in ["reference", "tag", "field", "duplicate"] {
        let (mut store, source, report, tabs, groups) = stage();
        let item = if invalid == "duplicate" {
            &groups[1]
        } else {
            &tabs[0]
        };
        let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
        match invalid {
            "reference" => {
                metadata["record"]["mapping"]["value"]["group"]["observation"]["value"]["address"] =
                    json!(999)
            }
            "tag" => {
                metadata["record"]["mapping"]["value"]["group"]["observation"]["value"]["subtype"] =
                    json!(0)
            }
            "field" => metadata["record"]["mapping"]["value"]["group"]["id"] = json!(31),
            "duplicate" => metadata["record"]["object"]["address"] = json!(400),
            _ => unreachable!(),
        }
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![item.id.to_string(), serde_json::to_vec(&metadata).unwrap()],
            )
            .unwrap();
        let before = store.workspace().unwrap();
        assert!(
            store
                .finish_source_with_repeater_workspace(source, &report, &CancellationToken::new())
                .is_err(),
            "{invalid}"
        );
        assert_eq!(store.workspace().unwrap(), before);
        assert!(!store.ext_report(source).unwrap().complete_archive);
    }
    let (mut store, source, report, tabs, _) = stage();
    store.finish_source(source, &report).unwrap();
    let groups = super::super::groups::load(
        &store.connection,
        source,
        &report,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut foreign = tabs[0].clone();
    foreign.source_id = Uuid::new_v4();
    assert!(groups.membership(&foreign, &mut BTreeMap::new()).is_err());
}

#[test]
fn unknown_color_preserves_groups_and_missing_legacy_membership_is_reported() {
    let (mut store, source, report, _, groups) = stage();
    let mut metadata: Value = serde_json::from_slice(&groups[0].metadata_json).unwrap();
    metadata["record"]["mapping"]["value"]["color"]["observation"]["value"] = json!(255);
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![
                groups[0].id.to_string(),
                serde_json::to_vec(&metadata).unwrap()
            ],
        )
        .unwrap();
    let restored = store
        .finish_source_with_repeater_workspace(source, &report, &CancellationToken::new())
        .unwrap();
    let workspace = store.workspace().unwrap();
    assert_eq!(workspace.groups[0].title, "Alpha组");
    assert_eq!(workspace.groups[0].color, None);
    assert_eq!(workspace.tabs[0].group, Some(workspace.groups[0].id));
    assert!(restored.gaps.iter().any(|gap| gap.contains("颜色 ID 255")));
    assert_eq!(
        store.ext_item(groups[0].id).unwrap().metadata_json,
        serde_json::to_vec(&metadata).unwrap()
    );

    let (mut store, source, report, _, groups) = stage();
    let mut metadata: Value = serde_json::from_slice(&groups[0].metadata_json).unwrap();
    metadata["record"]["mapping"]["value"]["name"]["observation"]["state"] = json!("failed");
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![
                groups[0].id.to_string(),
                serde_json::to_vec(&metadata).unwrap()
            ],
        )
        .unwrap();
    let restored = store
        .finish_source_with_repeater_workspace(source, &report, &CancellationToken::new())
        .unwrap();
    let workspace = store.workspace().unwrap();
    assert_eq!(workspace.groups.len(), 1);
    assert_eq!(workspace.tabs.len(), 3);
    assert_eq!(workspace.tabs[0].group, None);
    assert_eq!(workspace.tabs[0].draft.raw, RAW);
    assert!(
        restored
            .gaps
            .iter()
            .any(|gap| gap.contains("分组名称或展开状态未映射"))
    );

    let (mut store, source, mut report, tabs, _) = stage();
    store
        .connection
        .execute("DELETE FROM ext_items WHERE collection='root.f2.f4'", [])
        .unwrap();
    report
        .collections
        .retain(|collection| collection.path != "root.f2.f4");
    report.indexed_items -= 2;
    for tab in tabs {
        let mut metadata: Value = serde_json::from_slice(&tab.metadata_json).unwrap();
        metadata["record"]["mapping"]["value"]
            .as_object_mut()
            .unwrap()
            .remove("group");
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![tab.id.to_string(), serde_json::to_vec(&metadata).unwrap()],
            )
            .unwrap();
    }
    let restored = store
        .finish_source_with_repeater_workspace(source, &report, &CancellationToken::new())
        .unwrap();
    let workspace = store.workspace().unwrap();
    assert_eq!(workspace.tabs.len(), 3);
    assert!(workspace.groups.is_empty());
    assert!(
        workspace
            .tabs
            .iter()
            .all(|tab| tab.group.is_none() && tab.draft.raw == RAW)
    );
    assert!(
        restored
            .gaps
            .iter()
            .any(|gap| gap.contains("分组目录未映射"))
    );
    assert!(
        restored
            .gaps
            .iter()
            .any(|gap| gap.contains("标签分组字段未映射") && gap.contains('3'))
    );
}

#[test]
fn groups_are_persisted_when_no_tabs_can_be_restored_and_cancel_does_not_publish() {
    let (mut store, source, mut report, _, _) = stage();
    store
        .connection
        .execute("DELETE FROM ext_items WHERE collection='root.f2.f1'", [])
        .unwrap();
    let tabs = report
        .collections
        .iter_mut()
        .find(|collection| collection.path == "root.f2.f1")
        .unwrap();
    tabs.logical_items = Some(0);
    tabs.indexed_items = 0;
    tabs.message_count = 0;
    report.indexed_items = 2;
    report.indexed_messages = 0;
    report.repeater_active_tab = None;
    let before = store.workspace().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        store
            .finish_source_with_repeater_workspace(source, &report, &cancel)
            .is_err()
    );
    assert_eq!(store.workspace().unwrap(), before);
    store
        .finish_source_with_repeater_workspace(source, &report, &CancellationToken::new())
        .unwrap();
    let workspace = store.workspace().unwrap();
    assert!(workspace.tabs.is_empty());
    assert_eq!(workspace.groups.len(), 2);
    assert_eq!(workspace.revision, before.revision + 1);
}
