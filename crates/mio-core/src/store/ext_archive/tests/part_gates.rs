use super::*;

#[test]
fn corrupt_chunk_types_sizes_metadata_and_missing_rows_are_not_materialized_as_valid_data() {
    let (mut store, id) = archived(&[3; 256]);
    store
        .finish_source(id, &final_report(256, &[], &[]))
        .unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_source_chunks SET data=zeroblob(?1)",
            [EXT_CHUNK_BYTES as i64 + 1],
        )
        .unwrap();
    assert!(
        store
            .read_source_range(id, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("限额")
    );
    store
        .connection
        .execute("UPDATE ext_source_chunks SET data=?1", ["\0not a blob"])
        .unwrap();
    assert!(
        store
            .read_source_range(id, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("二进制")
    );
    store
        .connection
        .execute("UPDATE ext_source_chunks SET codec=0,data=x'0303'", [])
        .unwrap();
    assert!(
        store
            .read_source_range(id, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("长度")
    );
    store
        .connection
        .execute("UPDATE ext_source_chunks SET codec=9", [])
        .unwrap();
    assert!(
        store
            .read_source_range(id, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("编码")
    );
    store
        .connection
        .execute("UPDATE ext_source_chunks SET raw_bytes=-1", [])
        .unwrap();
    assert!(store.read_source_range(id, 0, 1).is_err());
    store
        .connection
        .execute("DELETE FROM ext_source_chunks", [])
        .unwrap();
    assert!(
        store
            .read_source_range(id, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("缺块")
    );
    store
        .connection
        .execute("UPDATE ext_sources SET written_bytes=255", [])
        .unwrap();
    assert!(
        store
            .read_source_range(id, 0, 0)
            .unwrap_err()
            .to_string()
            .contains("状态损坏")
    );
}

#[test]
fn reports_item_metadata_and_summaries_are_size_gated_and_cannot_disagree_with_indices() {
    let (mut store, id) = archived(&[0; 256]);
    let item = item(id, 0);
    let summary = message(&item, 15);
    let mut invalid = item.clone();
    invalid.metadata_json = vec![b' '; MAX_METADATA_BYTES + 1];
    assert!(store.put_ext_item(&invalid).is_err());
    invalid.metadata_json = b"not JSON".to_vec();
    assert!(store.put_ext_item(&invalid).is_err());
    store
        .put_ext_batch(std::slice::from_ref(&item), std::slice::from_ref(&summary))
        .unwrap();
    let report = final_report(
        256,
        std::slice::from_ref(&item),
        std::slice::from_ref(&summary),
    );
    store.finish_source(id, &report).unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    for column in ["report", "metadata", "summary"] {
        let (table, limit) = match column {
            "report" => ("ext_sources", MAX_METADATA_BYTES),
            "metadata" => ("ext_items", MAX_METADATA_BYTES),
            _ => ("ext_messages", MAX_SUMMARY_BYTES),
        };
        store
            .connection
            .execute(
                &format!("UPDATE {table} SET {column}=zeroblob(?1)"),
                [limit as i64 + 1],
            )
            .unwrap();
        match column {
            "report" => assert!(
                store
                    .ext_report(id)
                    .unwrap_err()
                    .to_string()
                    .contains("限额")
            ),
            "metadata" => assert!(
                store
                    .ext_item(item.id)
                    .unwrap_err()
                    .to_string()
                    .contains("限额")
            ),
            _ => assert!(
                store
                    .ext_message_page(id, None, 0, 1)
                    .unwrap_err()
                    .to_string()
                    .contains("限额")
            ),
        }
        store
            .connection
            .execute(&format!("UPDATE {table} SET {column}=?1"), ["\0TEXT"])
            .unwrap();
        match column {
            "report" => assert!(
                store
                    .ext_report(id)
                    .unwrap_err()
                    .to_string()
                    .contains("二进制")
            ),
            "metadata" => assert!(
                store
                    .ext_item(item.id)
                    .unwrap_err()
                    .to_string()
                    .contains("二进制")
            ),
            _ => assert!(
                store
                    .ext_message_page(id, None, 0, 1)
                    .unwrap_err()
                    .to_string()
                    .contains("二进制")
            ),
        }
        let restored = match column {
            "report" => serde_json::to_vec(&report).unwrap(),
            "metadata" => item.metadata_json.clone(),
            _ => serde_json::to_vec(&summary).unwrap(),
        };
        store
            .connection
            .execute(&format!("UPDATE {table} SET {column}=?1"), [restored])
            .unwrap();
    }
    let mut wrong = summary.clone();
    wrong.source_id = Uuid::new_v4();
    store
        .connection
        .execute(
            "UPDATE ext_messages SET summary=?1",
            [serde_json::to_vec(&wrong).unwrap()],
        )
        .unwrap();
    assert!(
        store
            .ext_message_page(id, None, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("归属")
    );
    assert!(store.ext_message_fragment(summary.id, 0, 1).is_err());
}
