use super::*;
use serde_json::{Value, json};

fn field(id: u8, value: Value) -> Value {
    json!({"id": id, "observation": {"state": "value", "value": value}})
}

fn decoded(tag: (u8, u8), value: Value) -> Value {
    json!({
        "object": {"object_type": tag.0, "subtype": tag.1},
        "mapping": {"state": "value", "value": value},
    })
}

fn history_pair(tls: bool) -> (ProjectStore, Uuid, ExtItem, ExtMessageSummary) {
    let archive = [vec![0; 80], RAW.to_vec()].concat();
    let (mut store, source_id) = pending(&archive);
    let mut item = item(source_id, "root.f2.f1[0].f2", u64::from(tls));
    let message = message(&item, 2, 80, RAW);
    let mut request = field(
        2,
        json!({
            "kind": "bytes", "address": 72, "payload_bytes": RAW.len(),
            "element_count": RAW.len(), "total_size": RAW.len() + 8,
        }),
    );
    request["binding"] =
        json!({"source_id": source_id, "item_id": item.id, "message_id": message.id});
    let service = decoded(
        (0, 0),
        json!({
            "host": field(0, decoded((0, 0), json!("127.0.0.1"))),
            "port": field(1, json!(if tls { 9443 } else { 9080 })),
            "tls": field(2, json!(tls)),
        }),
    );
    request["service"] = field(0, service);
    item.metadata_json = serde_json::to_vec(&json!({
        "version": 1,
        "context": "repeater_pair_229",
        "slot": {"state": "value", "value": item.object_address},
        "record": decoded((1, 0), json!({
            "request": request,
            "response": {"id": 3, "observation": {"state": "null"}},
        })),
    }))
    .unwrap();
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&item),
        std::slice::from_ref(&message),
    );
    let mut report = store.ext_report(source_id).unwrap();
    report.schema_floor = 229;
    report.schema_revision = 229;
    report.gaps = vec!["Synthetic history pair; other project fields remain unmapped".into()];
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=?1 WHERE id=?2",
            params![serde_json::to_vec(&report).unwrap(), source_id.to_string()],
        )
        .unwrap();
    (store, source_id, item, message)
}

fn current_with_service(tls: bool) -> (ProjectStore, Uuid, ExtItem, ExtMessageSummary) {
    let (store, source_id, mut item, message) = titled_229();
    let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    metadata["record"]["mapping"]["value"]["current_request"]["service"] = field(
        3,
        decoded(
            (0, 0),
            json!({
                "host": field(0, decoded((0, 0), json!("127.0.0.1"))),
                "port": field(1, json!(if tls { 52483 } else { 65078 })),
                "tls": field(2, json!(tls)),
            }),
        ),
    );
    item.metadata_json = serde_json::to_vec(&metadata).unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?1 WHERE id=?2",
            params![item.metadata_json, item.id.to_string()],
        )
        .unwrap();
    (store, source_id, item, message)
}

#[test]
fn schema_229_current_service_target_keeps_title_raw_and_no_history_response_across_reopens() {
    for (tls, expected) in [
        (false, "http://127.0.0.1:65078"),
        (true, "https://127.0.0.1:52483"),
    ] {
        let (mut store, source_id, item, message) = current_with_service(tls);
        let mut workspace = store.workspace().unwrap();
        let tab = store
            .ext_repeater_tab(
                source_id,
                item.id,
                message.id,
                &workspace,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(
            tab.draft.target, expected,
            "current f4 must use its own f3 service, not request Host"
        );
        assert_eq!(tab.title, "mio-title-汉🙂");
        assert_eq!(tab.draft.raw, RAW);
        assert_eq!(tab.draft.version, HttpVersion::Http11);
        assert!(tab.archive_response.is_none() && tab.last_flow.is_none());
        workspace.revision += 1;
        workspace.active = Some(tab.id);
        workspace.tabs.push(tab);
        store.save_workspace(&workspace).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("current-target.mio");
        let copy = directory.path().join("current-target-copy.mio");
        store.save(Some(&first)).unwrap();
        store.save(None).unwrap();
        store.save(Some(&copy)).unwrap();
        store.close().unwrap();
        for path in [&first, &copy] {
            let reopened = ProjectStore::open(path).unwrap();
            assert_eq!(reopened.workspace().unwrap(), workspace);
            assert_eq!(
                reopened.ext_item(item.id).unwrap().metadata_json,
                item.metadata_json
            );
            assert_eq!(
                reopened
                    .read_source_range(source_id, 80, RAW.len())
                    .unwrap(),
                RAW
            );
            assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
            reopened.close().unwrap();
        }
    }
}

#[test]
fn schema_229_current_service_rejects_cross_context_and_request_binding_mismatches() {
    let (store, source_id, item, message) = current_with_service(false);
    let original: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    let workspace = store.workspace().unwrap();
    let request = "/record/mapping/value/current_request";
    let service = format!("{request}/service");
    let variants = [
        ("/context".into(), json!("repeater_pair_229")),
        ("/record/object/object_type".into(), json!(1)),
        ("/record/object/subtype".into(), json!(0)),
        ("/slot/value".into(), json!(73)),
        (format!("{request}/id"), json!(2)),
        (
            format!("{request}/binding/source_id"),
            json!(Uuid::new_v4()),
        ),
        (format!("{request}/binding/item_id"), json!(Uuid::new_v4())),
        (
            format!("{request}/binding/message_id"),
            json!(Uuid::new_v4()),
        ),
        (format!("{request}/observation/value/address"), json!(73)),
        (
            format!("{request}/observation/value/payload_bytes"),
            json!(RAW.len() + 1),
        ),
        (
            format!("{request}/observation/value/element_count"),
            json!(RAW.len() - 1),
        ),
        (
            format!("{request}/observation/value/total_size"),
            json!(RAW.len() + 9),
        ),
        (format!("{service}/id"), json!(0)),
        (
            format!("{service}/observation/value/object/subtype"),
            json!(1),
        ),
        (
            format!("{service}/observation/value/mapping/value/port/id"),
            json!(2),
        ),
        (
            format!("{service}/observation/value/mapping/value/port/observation/value"),
            json!(0),
        ),
        (
            format!("{service}/observation/value/mapping/value/tls/observation/value"),
            json!(1),
        ),
    ];
    let mut accepted = Vec::new();
    for (pointer, value) in variants {
        let mut metadata = original.clone();
        *metadata.pointer_mut(&pointer).unwrap() = value;
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?1 WHERE id=?2",
                params![serde_json::to_vec(&metadata).unwrap(), item.id.to_string()],
            )
            .unwrap();
        if store
            .ext_repeater_tab(
                source_id,
                item.id,
                message.id,
                &workspace,
                &CancellationToken::new(),
            )
            .is_ok()
        {
            accepted.push(pointer);
        }
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(
            store.read_source_range(source_id, 80, RAW.len()).unwrap(),
            RAW
        );
    }
    store.close().unwrap();
    assert!(
        accepted.is_empty(),
        "current-service mismatch cannot fall back to empty target: {accepted:?}"
    );

    let (store, source_id, item, message) = history_pair(false);
    let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    metadata["record"]["mapping"]["value"]["request"]["service"]["id"] = json!(3);
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?1 WHERE id=?2",
            params![serde_json::to_vec(&metadata).unwrap(), item.id.to_string()],
        )
        .unwrap();
    let rejected = store
        .ext_repeater_tab(
            source_id,
            item.id,
            message.id,
            &store.workspace().unwrap(),
            &CancellationToken::new(),
        )
        .is_err();
    store.close().unwrap();
    assert!(
        rejected,
        "history pair must not accept the current-tab service field id"
    );
}

#[test]
fn schema_229_unknown_current_service_keeps_empty_target_original_title_and_request() {
    let (store, source_id, item, message) = current_with_service(false);
    let original: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    let workspace = store.workspace().unwrap();
    for service in [
        None,
        Some(json!({"id": 3, "observation": {"state": "null"}})),
        Some(
            json!({"id": 3, "observation": {"state": "unavailable", "parent": "unverified service"}}),
        ),
    ] {
        let mut metadata = original.clone();
        let current = metadata["record"]["mapping"]["value"]["current_request"]
            .as_object_mut()
            .unwrap();
        match service {
            None => {
                current.remove("service");
            }
            Some(service) => {
                current.insert("service".into(), service);
            }
        }
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?1 WHERE id=?2",
                params![serde_json::to_vec(&metadata).unwrap(), item.id.to_string()],
            )
            .unwrap();
        let tab = store
            .ext_repeater_tab(
                source_id,
                item.id,
                message.id,
                &workspace,
                &CancellationToken::new(),
            )
            .unwrap();
        assert!(tab.draft.target.is_empty());
        assert_eq!(tab.title, "mio-title-汉🙂");
        assert_eq!(tab.draft.raw, RAW);
        assert!(tab.archive_response.is_none() && tab.last_flow.is_none());
        assert_eq!(store.workspace().unwrap(), workspace);
    }
    store.close().unwrap();
}

#[test]
fn schema_229_history_pair_service_target_survives_save_as_and_both_reopens() {
    for (tls, expected) in [
        (false, "http://127.0.0.1:9080"),
        (true, "https://127.0.0.1:9443"),
    ] {
        let (mut store, source_id, item, message) = history_pair(tls);
        let cancel = CancellationToken::new();
        let mut workspace = store.workspace().unwrap();
        let report = store.ext_report(source_id).unwrap();
        let tab = store
            .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
            .unwrap();
        assert_eq!(
            tab.draft.target, expected,
            "service target must not be inferred from the request Host"
        );
        assert_eq!(tab.draft.raw, RAW);
        assert!(
            tab.draft
                .raw
                .windows(b"Host: unresolved.invalid:443".len())
                .any(|window| window == b"Host: unresolved.invalid:443")
        );
        assert_eq!(tab.draft.version, HttpVersion::Http11);
        assert_eq!(tab.group, None);
        assert_eq!(tab.last_flow, None);
        assert_eq!(store.workspace().unwrap(), workspace);
        workspace.revision += 1;
        workspace.active = Some(tab.id);
        workspace.tabs.push(tab);
        store.save_workspace(&workspace).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("history-target.mio");
        let copy = directory.path().join("history-target-copy.mio");
        store.save(Some(&first)).unwrap();
        store.save(None).unwrap();
        store.save(Some(&copy)).unwrap();
        store.close().unwrap();
        for path in [&first, &copy] {
            let reopened = ProjectStore::open(path).unwrap();
            assert_eq!(reopened.workspace().unwrap(), workspace);
            assert_eq!(reopened.ext_report(source_id).unwrap(), report);
            assert_eq!(
                reopened.ext_item(item.id).unwrap().metadata_json,
                item.metadata_json
            );
            assert_eq!(
                reopened
                    .ext_message_fragment(message.id, 0, RAW.len())
                    .unwrap()
                    .bytes,
                RAW
            );
            assert_eq!(
                reopened.page(0, 1, "").unwrap().0,
                0,
                "loading or saving must not send archived requests"
            );
            reopened.close().unwrap();
        }
    }
}

#[test]
fn schema_229_history_target_rejects_misbound_service_or_request_without_empty_fallback() {
    let (store, source_id, item, message) = history_pair(false);
    let workspace = store.workspace().unwrap();
    let original: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    let request = "/record/mapping/value/request";
    let service = "/record/mapping/value/request/service";
    let variants = [
        (
            format!("{request}/binding/source_id"),
            json!(Uuid::new_v4()),
        ),
        (format!("{request}/binding/item_id"), json!(Uuid::new_v4())),
        (
            format!("{request}/binding/message_id"),
            json!(Uuid::new_v4()),
        ),
        (format!("{request}/observation/value/address"), json!(73)),
        (
            format!("{request}/observation/value/payload_bytes"),
            json!(RAW.len() + 1),
        ),
        (
            format!("{request}/observation/value/element_count"),
            json!(RAW.len() - 1),
        ),
        (
            format!("{request}/observation/value/total_size"),
            json!(RAW.len() + 9),
        ),
        (format!("{request}/id"), json!(3)),
        ("/slot/value".into(), json!(73)),
        ("/record/object/object_type".into(), json!(0)),
        (format!("{service}/id"), json!(1)),
        (
            format!("{service}/observation/value/object/subtype"),
            json!(1),
        ),
        (
            format!(
                "{service}/observation/value/mapping/value/host/observation/value/object/subtype"
            ),
            json!(1),
        ),
        (
            format!(
                "{service}/observation/value/mapping/value/host/observation/value/mapping/value"
            ),
            json!("host.invalid/path"),
        ),
        (
            format!("{service}/observation/value/mapping/value/port/observation/value"),
            json!(0),
        ),
        (
            format!("{service}/observation/value/mapping/value/port/observation/value"),
            json!(65536),
        ),
        (
            format!("{service}/observation/value/mapping/value/tls/observation/value"),
            json!(1),
        ),
    ];
    let mut accepted = Vec::new();
    for (pointer, value) in variants {
        let mut metadata = original.clone();
        *metadata
            .pointer_mut(&pointer)
            .expect("fixture pointer exists") = value;
        let encoded = serde_json::to_vec(&metadata).unwrap();
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?1 WHERE id=?2",
                params![encoded, item.id.to_string()],
            )
            .unwrap();
        if store
            .ext_repeater_tab(
                source_id,
                item.id,
                message.id,
                &workspace,
                &CancellationToken::new(),
            )
            .is_ok()
        {
            accepted.push(pointer);
        }
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(
            store.read_source_range(source_id, 80, RAW.len()).unwrap(),
            RAW
        );
    }
    store.close().unwrap();
    assert!(
        accepted.is_empty(),
        "misbound or invalid target metadata must return an error, never silently load an empty target: {accepted:?}"
    );
}
