use super::*;

#[test]
fn target_rejects_mismatched_identity_raw_and_service_layers_but_preserves_unknown() {
    for invalid in [
        "binding",
        "raw",
        "parsed field",
        "parsed tag",
        "service field",
        "service tag",
        "host tag",
        "port",
        "tls",
        "numeric host",
    ] {
        let (store, _, items) = fixture(0, false, false);
        let (_, rows) = store.history_page(0, 64, "18765").unwrap();
        let ExtHistoryRow {
            request: Some(message),
            ..
        } = archived(&rows[0])
        else {
            panic!("missing request")
        };
        let mut metadata: Value = serde_json::from_slice(&items[0].metadata_json).unwrap();
        let request = &mut metadata["record"]["mapping"]["value"]["request"];
        match invalid {
            "binding" => request["binding"]["message_id"] = json!(Uuid::new_v4()),
            "raw" => request["observation"]["value"]["address"] = json!(99),
            "parsed field" => request["parsed_request"]["id"] = json!(3),
            "parsed tag" => {
                request["parsed_request"]["observation"]["value"]["object"]["subtype"] = json!(1)
            }
            _ => {
                let service = &mut request["parsed_request"]["observation"]["value"]["mapping"]["value"]
                    ["service"];
                match invalid {
                    "service field" => service["id"] = json!(1),
                    "service tag" => {
                        service["observation"]["value"]["object"]["subtype"] = json!(1)
                    }
                    _ => {
                        let fields = &mut service["observation"]["value"]["mapping"]["value"];
                        match invalid {
                            "host tag" => {
                                fields["host"]["observation"]["value"]["object"]["subtype"] =
                                    json!(1)
                            }
                            "port" => fields["port"]["observation"]["value"] = json!(0),
                            "tls" => fields["tls"]["observation"]["value"] = json!(1),
                            "numeric host" => {
                                fields["host"]["observation"]["value"]["mapping"]["value"] =
                                    json!("127.1")
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![
                    items[0].id.to_string(),
                    serde_json::to_vec(&metadata).unwrap()
                ],
            )
            .unwrap();
        assert!(
            request_origin(&store.connection, message).is_err(),
            "{invalid}"
        );
    }
    let (store, _, items) = fixture(0, false, false);
    let (_, rows) = store.history_page(0, 64, "18765").unwrap();
    let ExtHistoryRow {
        request: Some(message),
        ..
    } = archived(&rows[0])
    else {
        panic!("missing request")
    };
    let mut metadata: Value = serde_json::from_slice(&items[0].metadata_json).unwrap();
    metadata["record"]["mapping"]["value"]["request"]["parsed_request"] =
        json!({"id": 2, "observation": {"state": "null"}});
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![
                items[0].id.to_string(),
                serde_json::to_vec(&metadata).unwrap()
            ],
        )
        .unwrap();
    assert_eq!(request_origin(&store.connection, message).unwrap(), None);
    assert_eq!(store.history_page(0, 64, "18765").unwrap().0, 0);
    assert_eq!(store.history_page(0, 64, "fixture").unwrap().0, 2);
}

#[test]
fn unmapped_parsed_or_service_objects_keep_the_row_with_unknown_origin() {
    for parsed_layer in [true, false] {
        let (store, _, items) = fixture(0, false, false);
        let (_, rows) = store.history_page(0, 64, "18765").unwrap();
        let ExtHistoryRow {
            request: Some(message),
            ..
        } = archived(&rows[0])
        else {
            panic!("missing request")
        };
        let mut metadata: Value = serde_json::from_slice(&items[0].metadata_json).unwrap();
        let parsed = &mut metadata["record"]["mapping"]["value"]["request"]["parsed_request"]["observation"]
            ["value"];
        let unknown = if parsed_layer {
            parsed
        } else {
            &mut parsed["mapping"]["value"]["service"]["observation"]["value"]
        };
        unknown["object"]["subtype"] = json!(9);
        unknown["mapping"] = json!({"state": "failed", "kind": "Unsupported"});
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![
                    items[0].id.to_string(),
                    serde_json::to_vec(&metadata).unwrap()
                ],
            )
            .unwrap();
        assert_eq!(request_origin(&store.connection, message).unwrap(), None);
        let (total, rows) = store.history_page(0, 64, "").unwrap();
        assert_eq!(total, 4);
        assert!(
            matches!(archived(&rows[3]), ExtHistoryRow { item, request: Some(_), origin: None, .. } if item.id == items[0].id)
        );
        assert_eq!(store.history_page(0, 64, "18765").unwrap().0, 0);
    }
}

#[test]
fn resolved_addresses_that_overflow_sqlite_integers_are_not_rounded_into_aliases() {
    let (store, _, items) = fixture(0, false, false);
    for (index, address) in [(0, 1_u64 << 63), (1, (1_u64 << 63) + 1)] {
        let mut metadata: Value = serde_json::from_slice(&items[index].metadata_json).unwrap();
        metadata["record"]["object"]["address"] = json!(address);
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![
                    items[index].id.to_string(),
                    serde_json::to_vec(&metadata).unwrap()
                ],
            )
            .unwrap();
    }
    let (total, rows) = store.history_page(0, 64, "").unwrap();
    assert_eq!(total, 5);
    for item in &items[..2] {
        assert!(rows.iter().any(|row| row.id()
            == crate::HistoryEntryId::External {
                source_id: item.source_id,
                item_id: item.id,
            }));
    }
}

#[test]
fn repeated_slots_pair_only_across_collections_and_preserve_unmatched_occurrences() {
    let (store, source, items) = fixture(0, false, false);
    let mut query = store
        .connection
        .prepare("SELECT summary FROM ext_messages WHERE item=?1 ORDER BY field_id")
        .unwrap();
    let messages: Vec<ExtMessageSummary> = query
        .query_map([items[0].id.to_string()], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .map(|bytes| serde_json::from_slice(&bytes.unwrap()).unwrap())
        .collect();
    drop(query);
    let mut added = Vec::new();
    for (collection, index) in [("root.f1.f0", 4), ("root.f1.f1", 4), ("root.f1.f1", 5)] {
        let mut item = items[0].clone();
        item.id = Uuid::new_v4();
        item.collection = collection.into();
        item.index = index;
        let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
        let cloned: Vec<_> = messages
            .iter()
            .map(|message| {
                let mut message = message.clone();
                message.id = Uuid::new_v4();
                message.item_id = item.id;
                message.collection = item.collection.clone();
                message.item_index = item.index;
                if message.field_id == 15 {
                    metadata["record"]["mapping"]["value"]["request"]["binding"] = json!({
                        "source_id": source, "item_id": item.id, "message_id": message.id,
                    });
                }
                message
            })
            .collect();
        item.metadata_json = serde_json::to_vec(&metadata).unwrap();
        store.connection.execute(
            "INSERT INTO ext_items(id,source,collection,item_index,object_address,state,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![item.id.to_string(), source.to_string(), item.collection, item.index, item.object_address, item.state, item.metadata_json],
        ).unwrap();
        for message in cloned {
            store
                .connection
                .execute(
                    "INSERT INTO ext_messages(id,item,field_id,summary) VALUES(?1,?2,?3,?4)",
                    params![
                        message.id.to_string(),
                        item.id.to_string(),
                        message.field_id,
                        serde_json::to_vec(&message).unwrap()
                    ],
                )
                .unwrap();
        }
        added.push(item);
    }
    let mut report = read_report(&store.connection, source).unwrap();
    report.indexed_items += 3;
    report.indexed_messages += 6;
    for collection in &mut report.collections {
        for item in added
            .iter()
            .filter(|item| item.collection == collection.path)
        {
            collection.indexed_items += 1;
            collection.message_count += 2;
            collection.logical_items = Some(collection.logical_items.unwrap().max(item.index + 1));
        }
    }
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=?2 WHERE id=?1",
            params![source.to_string(), serde_json::to_vec(&report).unwrap()],
        )
        .unwrap();
    let (total, rows) = store.history_page(0, 64, "").unwrap();
    assert_eq!(total, 6);
    let ids: Vec<_> = rows.iter().map(HistoryRow::id).collect();
    for item in [&items[0], &added[0], &added[2]] {
        assert!(ids.contains(&crate::HistoryEntryId::External {
            source_id: source,
            item_id: item.id
        }));
    }
    for item in [&items[1], &added[1]] {
        assert!(!ids.contains(&crate::HistoryEntryId::External {
            source_id: source,
            item_id: item.id
        }));
    }
}

#[test]
fn original_and_edited_requests_load_their_own_bytes_and_keep_one_source_row_on_reopen() {
    let (mut store, source, items) = fixture(0, true, false);
    let (total, rows) = store.history_page(0, 64, "").unwrap();
    assert_eq!(total, 4);
    assert_eq!(store.history_page(0, 64, "/fixture").unwrap().0, 2);
    assert_eq!(store.history_page(0, 64, "/edited").unwrap().0, 0);
    assert_eq!(store.history_page(0, 64, "PUT").unwrap().0, 0);
    let ExtHistoryRow {
        item,
        request: Some(original),
        edited_request: Some(edited),
        response: Some(response),
        origin,
        ..
    } = rows
        .iter()
        .find(|row| {
            row.id()
                == crate::HistoryEntryId::External {
                    source_id: source,
                    item_id: items[0].id,
                }
        })
        .map(archived)
        .unwrap()
    else {
        panic!("missing request versions")
    };
    assert_eq!(item.message_count, 3);
    assert_eq!(original.field_id, 15);
    assert_eq!(original.request_target.as_deref(), Some("/fixture"));
    assert_eq!(edited.field_id, 17);
    assert_eq!(edited.role, "edited_request");
    assert_eq!(edited.request_target.as_deref(), Some("/edited"));
    assert_ne!(original.id, edited.id);
    assert_eq!(original.item_id, edited.item_id);
    assert_eq!(response.status, Some(201));
    assert_eq!(origin.as_deref(), Some("http://127.0.0.1:18765"));
    let mut workspace = store.workspace().unwrap();
    for (message, raw) in [(original, REQUEST), (edited, EDITED_REQUEST)] {
        assert_eq!(
            store
                .ext_message_fragment(message.id, 0, raw.len())
                .unwrap()
                .bytes,
            raw
        );
        let tab = store
            .ext_repeater_tab(
                source,
                item.id,
                message.id,
                &workspace,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(tab.draft.raw, raw);
        assert_eq!(tab.draft.target, "http://127.0.0.1:18765");
        assert!(tab.archive_response.is_none() && tab.last_flow.is_none());
        workspace.revision += 1;
        workspace.active = Some(tab.id);
        workspace.tabs.push(tab);
        store.save_workspace(&workspace).unwrap();
    }
    assert_eq!(store.page(0, 1, "").unwrap().0, 0);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("versions.mio");
    let copy = directory.path().join("versions-copy.mio");
    store.save(Some(&path)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&path, &copy] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(
            reopened.history_page(0, 64, "").unwrap(),
            (total, rows.clone())
        );
        assert_eq!(
            reopened
                .ext_message_fragment(edited.id, 0, EDITED_REQUEST.len())
                .unwrap()
                .bytes,
            EDITED_REQUEST
        );
        assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
        reopened.close().unwrap();
    }
}

#[test]
fn edited_request_rejects_other_versions_raw_bindings_and_service_substitution() {
    for invalid in ["binding", "raw", "service", "role", "collection", "schema"] {
        let (store, source, items) = fixture(0, true, false);
        let (_, rows) = store.history_page(0, 64, "18765").unwrap();
        let ExtHistoryRow {
            edited_request: Some(edited),
            ..
        } = archived(&rows[0])
        else {
            panic!("missing edited request")
        };
        let mut edited = edited.clone();
        let mut metadata: Value = serde_json::from_slice(&items[0].metadata_json).unwrap();
        let field = &mut metadata["record"]["mapping"]["value"]["edited_request"];
        match invalid {
            "binding" => field["binding"]["message_id"] = json!(Uuid::new_v4()),
            "raw" => field["observation"]["value"]["address"] = json!(0),
            "service" => {
                field["parsed_request"]["observation"]["value"]["mapping"]["value"]["service"]["observation"]
                    ["value"]["mapping"]["value"]["port"]["observation"]["value"] = json!(18767)
            }
            "role" => edited.role = "request".into(),
            "collection" => edited.collection = "root.f2.f1".into(),
            "schema" => {
                let mut report = read_report(&store.connection, source).unwrap();
                report.schema_floor = 228;
                report.schema_revision = 228;
                store
                    .connection
                    .execute(
                        "UPDATE ext_sources SET report=?2 WHERE id=?1",
                        params![source.to_string(), serde_json::to_vec(&report).unwrap()],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![
                    items[0].id.to_string(),
                    serde_json::to_vec(&metadata).unwrap()
                ],
            )
            .unwrap();
        assert!(
            request_origin(&store.connection, &edited).is_err(),
            "{invalid}"
        );
    }
}
