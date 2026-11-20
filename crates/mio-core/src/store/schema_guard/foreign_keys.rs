use anyhow::{Result, ensure};
use rusqlite::Transaction;
use tokio_util::sync::CancellationToken;

use super::TABLES;

pub(in crate::store) fn validate_update_dependencies(
    transaction: &Transaction<'_>,
    parent_table: &str,
    updated_columns: &[&str],
    cancel: &CancellationToken,
) -> Result<()> {
    let updated = |column: &str| {
        updated_columns
            .iter()
            .any(|updated| column.eq_ignore_ascii_case(updated))
    };
    let mut primary_key =
        transaction.prepare("SELECT name FROM pragma_table_info(?1,'main') WHERE pk>0")?;
    let mut primary_key = primary_key.query([parent_table])?;
    let mut updates_primary_key = false;
    while let Some(column) = primary_key.next()? {
        updates_primary_key |= updated(&column.get::<_, String>(0)?);
    }
    let mut tables =
        transaction.prepare("SELECT name FROM main.sqlite_schema WHERE type='table'")?;
    let mut tables = tables.query([])?;
    let mut keys = transaction
        .prepare("SELECT \"table\",\"to\",on_update FROM pragma_foreign_key_list(?1,'main')")?;
    while let Some(table) = tables.next()? {
        ensure!(!cancel.is_cancelled(), "项目设置更新已取消");
        let name: String = table.get(0)?;
        let mut keys = keys.query([name])?;
        while let Some(key) = keys.next()? {
            ensure!(!cancel.is_cancelled(), "项目设置更新已取消");
            let target: String = key.get(0)?;
            let column: Option<String> = key.get(1)?;
            let action: String = key.get(2)?;
            let changes_related_data = target.eq_ignore_ascii_case(parent_table)
                && column.as_deref().map_or(updates_primary_key, updated)
                && ["CASCADE", "SET NULL", "SET DEFAULT"]
                    .iter()
                    .any(|mutating| action.eq_ignore_ascii_case(mutating));
            ensure!(
                !changes_related_data,
                "项目 {parent_table} 更新列存在会修改关联数据的外键；未改变设置或扩展数据"
            );
        }
    }
    Ok(())
}

/// Destructive cleanup requires a complete ownership map, unlike ordinary project loading.
pub(in crate::store) fn validate_intruder_discard(transaction: &Transaction<'_>) -> Result<()> {
    let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let foreign_keys: bool =
        transaction.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    ensure!(
        schema == crate::store::SCHEMA && foreign_keys,
        "不能确认 Intruder 丢弃的项目版本或外键保护"
    );
    let temporary: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE type IN ('table','view','trigger'))",
        [],
        |row| row.get(0),
    )?;
    ensure!(!temporary, "连接包含临时数据对象；不能安全丢弃 Intruder");
    let mut statement = transaction.prepare(
        "SELECT type,CASE WHEN length(CAST(name AS BLOB))<=128 THEN name END,rootpage
         FROM main.sqlite_schema WHERE type IN ('table','view','trigger') LIMIT ?1",
    )?;
    let mut rows = statement.query([TABLES.len() + 4])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let table: String = row.get(1)?;
        let root: i64 = row.get(2)?;
        ensure!(
            kind == "table" && root > 0,
            "项目包含非普通表；不能安全丢弃 Intruder"
        );
        if matches!(
            table.as_str(),
            "sqlite_sequence" | "sqlite_stat1" | "sqlite_stat4"
        ) {
            continue;
        }
        let (_, _, required) = TABLES
            .iter()
            .find(|(_, name, _)| *name == table)
            .ok_or_else(|| anyhow::anyhow!("项目包含未知数据表；不能确认 Intruder 数据归属"))?;
        count += 1;
        let mut columns = transaction.prepare(&format!("PRAGMA main.table_xinfo(\"{table}\")"))?;
        let mut columns = columns.query([])?;
        let mut column_count = 0;
        while let Some(column) = columns.next()? {
            let name: String = column.get(1)?;
            let hidden: i32 = column.get(6)?;
            let primary_key: usize = column.get(5)?;
            let not_null: bool = column.get(3)?;
            if matches!(
                table.as_str(),
                "intruder_payloads" | "messages" | "repeater_history" | "site_map_entries" | "websocket_originals"
            ) || (matches!(table.as_str(), "metadata" | "intruder_results") && primary_key > 0)
            {
                ensure!(not_null, "项目删除键允许 NULL；不能安全丢弃 Intruder");
            }
            let expected_key = match (table.as_str(), name.as_str()) {
                ("metadata", "key")
                | ("flows" | "intruder_runs" | "site_map_entries", "id")
                | ("messages", "flow")
                | ("websocket_originals", "message")
                | ("repeater_history", "tab")
                | ("intruder_payloads" | "intruder_results", "run") => 1,
                ("messages", "side")
                | ("intruder_results", "ordinal")
                | ("repeater_history", "archive_boundary")
                | ("intruder_payloads", "set_index") => 2,
                ("intruder_payloads" | "repeater_history", "ordinal") => 3,
                _ => 0,
            };
            if matches!(
                table.as_str(),
                "metadata"
                    | "flows"
                    | "messages"
                    | "websocket_originals"
                    | "intruder_runs"
                    | "intruder_payloads"
                    | "intruder_results"
                    | "repeater_history"
                    | "site_map_entries"
            ) {
                ensure!(
                    primary_key == expected_key,
                    "项目主键布局未知；不能有界丢弃 Intruder"
                );
            }
            ensure!(
                hidden == 0
                    && (required.contains(&name.as_str())
                        || (table == "intruder_runs"
                            && matches!(
                                name.as_str(),
                                "paused"
                                    | "title"
                                    | "title_revision"
                                    | "baseline_ordinal"
                                    | "pool_id"
                                    | "pool_revision"
                            ))
                        || (schema >= 25 && table == "flows" && name == "listener_port")
                        || (schema >= 27
                            && table == "flows"
                            && crate::store::FLOW_DENORM_COLUMN_NAMES.contains(&name.as_str()))
                        || (schema >= 25
                            && table == "intruder_results"
                            && matches!(
                                name.as_str(),
                                "notes" | "notes_revision" | "highlight" | "highlight_revision"
                            ))),
                "项目包含未知或计算列；不能确认 Intruder 数据归属"
            );
            column_count += 1;
        }
        ensure!(
            column_count
                == required.len()
                    + 6 * usize::from(table == "intruder_runs")
                    + usize::from(schema >= 25 && table == "flows")
                    + crate::store::FLOW_DENORM_COLUMN_NAMES.len()
                        * usize::from(schema >= 27 && table == "flows")
                    + 4 * usize::from(schema >= 25 && table == "intruder_results"),
            "项目缺少数据列；不能安全丢弃 Intruder"
        );
        if matches!(
            table.as_str(),
            "metadata"
                | "flows"
                | "messages"
                | "intruder_runs"
                | "intruder_payloads"
                | "intruder_results"
                | "repeater_history"
                | "site_map_entries"
        ) {
            let mut indexes =
                transaction.prepare(&format!("PRAGMA main.index_list(\"{table}\")"))?;
            let mut indexes = indexes.query([])?;
            let mut primary_indexes = 0;
            while let Some(index) = indexes.next()? {
                if index.get::<_, String>(3)? != "pk" {
                    continue;
                }
                primary_indexes += 1;
                let name = index.get::<_, String>(1)?.replace('"', "\"\"");
                let mut columns =
                    transaction.prepare(&format!("PRAGMA main.index_xinfo(\"{name}\")"))?;
                let mut columns = columns.query([])?;
                while let Some(column) = columns.next()? {
                    if column.get::<_, bool>(5)? {
                        ensure!(
                            column.get::<_, Option<String>>(4)?.as_deref() == Some("BINARY"),
                            "项目主键比较规则未知；不能安全丢弃 Intruder"
                        );
                    }
                }
            }
            ensure!(
                primary_indexes == 1,
                "项目主键索引未知；不能安全丢弃 Intruder"
            );
        }
        let mut keys =
            transaction.prepare(&format!("PRAGMA main.foreign_key_list(\"{table}\")"))?;
        let mut keys = keys.query([])?;
        let mut key_count = 0;
        while let Some(key) = keys.next()? {
            key_count += 1;
            let sequence: i64 = key.get(1)?;
            let target: String = key.get(2)?;
            let from: String = key.get(3)?;
            let to: Option<String> = key.get(4)?;
            if table == "repeater_history" {
                ensure!(
                    key.get::<_, String>(5)? == "NO ACTION"
                        && key.get::<_, String>(6)? == "RESTRICT",
                    "Repeater 历史外键保护已改变；不能安全丢弃 Intruder"
                );
            }
            ensure!(
                key_count <= 2
                    && sequence == 0
                    && matches!(
                        (
                            table.as_str(),
                            from.as_str(),
                            target.as_str(),
                            to.as_deref()
                        ),
                        (
                            "messages"
                                | "websocket_messages"
                                | "websocket_connections"
                                | "repeater_history",
                            "flow",
                            "flows",
                            Some("id")
                        ) | ("intruder_results", "flow", "flows", Some("id"))
                            | (
                                "intruder_results" | "intruder_payloads",
                                "run",
                                "intruder_runs",
                                Some("id")
                            )
                            | (
                                "websocket_payloads" | "websocket_notes" | "websocket_originals",
                                "message",
                                "websocket_messages",
                                Some("id")
                            )
                            | (
                                "ext_source_chunks" | "ext_items",
                                "source",
                                "ext_sources",
                                Some("id")
                            )
                            | (
                                "ext_messages" | "ext_http_notes" | "ext_http_highlights",
                                "item",
                                "ext_items",
                                Some("id")
                            )
                    ),
                "项目包含未知外键；不能确认 Intruder 数据归属"
            );
        }
        let expected_keys = match table.as_str() {
            "metadata" | "flows" | "intruder_runs" | "ext_sources" | "site_map_entries" => 0,
            "intruder_results" => 2,
            _ => 1,
        };
        ensure!(
            key_count == expected_keys,
            "项目外键保护不完整；不能安全丢弃 Intruder"
        );
    }
    ensure!(
        count == TABLES.len(),
        "项目数据表不完整；不能安全丢弃 Intruder"
    );
    let unknown_metadata: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.metadata
         WHERE typeof(key)!='text' OR (
           key NOT IN ('id','workspace','intruder_resource_pools','audit_state','live_http_annotations','ext_direct_report','next_proxy_capture_number')
           AND key NOT GLOB 'intruder_discarded:*'
           AND key NOT GLOB 'plugin_storage:*'
           AND key NOT GLOB 'flow_provenance:*'
           AND key NOT GLOB 'http_history_meta:*'
         ))",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        !unknown_metadata,
        "项目包含未知元数据；不能确认 Intruder 数据归属"
    );
    crate::store::intruder::validate_retirements(transaction)?;
    crate::store::resource_pools::read(transaction)?;
    crate::store::plugin_storage::read_audit_state(transaction)?;
    crate::store::plugin_storage::read_live_http_annotations(transaction)?;
    crate::store::plugin_storage::validate_all(transaction)?;
    crate::store::provenance::validate_all(transaction)?;
    crate::store::messages::validate_all_http_history_metadata(transaction)?;
    Ok(())
}
