use super::*;

#[test]
fn schema_229_title_binding_and_text_limits_fail_closed() {
    use serde_json::json;

    let (store, source_id, item, message) = titled_229();
    let cancel = CancellationToken::new();
    let workspace = store.workspace().unwrap();
    let report = store.ext_report(source_id).unwrap();
    assert_eq!(
        store
            .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
            .unwrap()
            .title,
        "mio-title-汉🙂"
    );
    let original: serde_json::Value = serde_json::from_slice(&item.metadata_json).unwrap();
    for (pointer, value) in [
        ("/version", json!(2)),
        ("/context", json!("repeater_tab")),
        ("/slot/value", json!(73)),
        ("/record/object/subtype", json!(0)),
        ("/record/mapping/state", json!("failed")),
        ("/record/mapping/value/current_request/id", json!(2)),
        (
            "/record/mapping/value/current_request/observation/state",
            json!("null"),
        ),
        (
            "/record/mapping/value/current_request/binding/source_id",
            json!(Uuid::new_v4()),
        ),
        (
            "/record/mapping/value/current_request/binding/item_id",
            json!(Uuid::new_v4()),
        ),
        (
            "/record/mapping/value/current_request/binding/message_id",
            json!(Uuid::new_v4()),
        ),
        (
            "/record/mapping/value/current_request/observation/value/address",
            json!(u64::MAX),
        ),
        (
            "/record/mapping/value/current_request/observation/value/payload_bytes",
            json!(message.raw_bytes + 1),
        ),
        ("/record/mapping/value/current_request/title/id", json!(33)),
        (
            "/record/mapping/value/current_request/title/observation/state",
            json!("failed"),
        ),
        (
            "/record/mapping/value/current_request/title/observation/value/object/subtype",
            json!(2),
        ),
        (
            "/record/mapping/value/current_request/title/observation/value/mapping/state",
            json!("failed"),
        ),
        (
            "/record/mapping/value/current_request/title/observation/value/mapping/value",
            json!(""),
        ),
        (
            "/record/mapping/value/current_request/title/observation/value/mapping/value",
            json!("汉".repeat(1366)),
        ),
        (
            "/record/mapping/value/current_request/title/observation/value/mapping/value",
            json!("x".repeat(4097)),
        ),
    ] {
        let mut metadata = original.clone();
        *metadata.pointer_mut(pointer).unwrap() = value;
        let bytes = serde_json::to_vec(&metadata).unwrap();
        store
            .connection
            .execute("UPDATE ext_items SET metadata=?1", [&bytes])
            .unwrap();
        let tab = store
            .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
            .unwrap();
        assert_eq!(tab.title, "外部归档请求", "{pointer}");
        assert_eq!(tab.draft.raw, RAW);
        assert!(tab.draft.target.is_empty() && tab.group.is_none() && tab.last_flow.is_none());
        assert_eq!(store.ext_item(item.id).unwrap().metadata_json, bytes);
    }
    assert_eq!(store.ext_report(source_id).unwrap(), report);
    assert!(!report.gaps.is_empty());
    assert_eq!(
        store.read_source_range(source_id, 80, RAW.len()).unwrap(),
        RAW
    );
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.page(0, 1, "").unwrap().0, 0);
    store.close().unwrap();
}

#[test]
fn schema_229_title_is_never_borrowed_by_other_message_contexts() {
    let (store, source_id, item, original) = titled_229();
    let workspace = store.workspace().unwrap();
    let cancel = CancellationToken::new();
    let report = store.ext_report(source_id).unwrap();
    for (schema, collection, role, field_id) in [
        ((228, 228), "root.f2.f1", "current_request", 4),
        ((228, 229), "root.f2.f1", "current_request", 4),
        ((229, 228), "root.f2.f1", "current_request", 4),
        ((229, 229), "root.f1.f0", "request", 15),
        ((229, 229), "root.f2.f1[0].f2", "request", 2),
        ((229, 229), "root.f2.f4", "current_request", 4),
        ((229, 229), "root.f2.f1", "request", 4),
        ((229, 229), "root.f2.f1", "current_request", 2),
    ] {
        let mut message = original.clone();
        message.collection = collection.into();
        message.role = role.into();
        message.field_id = field_id;
        let mut changed = report.clone();
        changed.schema_floor = schema.0;
        changed.schema_revision = schema.1;
        changed.collections[0].path = collection.into();
        store
            .connection
            .execute(
                "UPDATE ext_sources SET report=?1",
                [serde_json::to_vec(&changed).unwrap()],
            )
            .unwrap();
        store
            .connection
            .execute("UPDATE ext_items SET collection=?1", [collection])
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE ext_messages SET field_id=?1,summary=?2",
                params![field_id, serde_json::to_vec(&message).unwrap()],
            )
            .unwrap();
        assert!(
            current_title_229(&store.connection, &message)
                .unwrap()
                .is_none()
        );
        let loaded = store.ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel);
        if schema == (229, 229) && collection == "root.f1.f0" {
            // Only the index labels changed; the metadata still describes a Repeater f4 draft.
            assert!(
                loaded
                    .unwrap_err()
                    .to_string()
                    .contains("原始消息与条目绑定不一致")
            );
        } else {
            let tab = loaded.unwrap();
            assert_eq!(
                tab.title, "外部归档请求",
                "{schema:?}/{collection}/{role}/{field_id}"
            );
            assert_eq!(tab.draft.raw, RAW);
        }
        assert_eq!(
            store.ext_item(item.id).unwrap().metadata_json,
            item.metadata_json
        );
        assert_eq!(store.ext_report(source_id).unwrap(), changed);
    }
    assert_eq!(
        store.read_source_range(source_id, 80, RAW.len()).unwrap(),
        RAW
    );
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.page(0, 1, "").unwrap().0, 0);
    store.close().unwrap();
}
