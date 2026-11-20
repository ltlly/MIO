use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::*;

pub(in crate::store::ext_archive) fn proxy_source(connection: &Connection) -> Result<Option<Uuid>> {
    let mut query = connection.prepare(
        "SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END FROM ext_sources LIMIT 2",
    )?;
    let mut rows = query.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let id = Uuid::parse_str(&row.get::<_, String>(0)?)?;
    ensure!(
        rows.next()?.is_none(),
        "项目包含多个 外部源归档，未合并历史"
    );
    let report = read_report(connection, id)?;
    Ok((report.complete_archive
        && matches!(
            (report.schema_floor, report.schema_revision),
            (228, 228) | (229, 229)
        ))
    .then_some(id))
}

pub(super) fn expected_http_context(report: &crate::ExtImportReport) -> Result<&'static str> {
    match (report.schema_floor, report.schema_revision) {
        (228, 228) => Ok("http_history_item"),
        (229, 229) => Ok("http_history_229"),
        _ => anyhow::bail!("仅支持已完成的 schema 228/229 HTTP 源归档"),
    }
}

pub(super) fn archive_query() -> String {
    let host = format!("json_extract(metadata,'{FIELDS}.host.observation.value.mapping.value')");
    let port = format!("json_extract(metadata,'{FIELDS}.port.observation.value')");
    let tls = format!("json_extract(metadata,'{FIELDS}.tls.observation.value')");
    format!("{CANONICAL}, summaries AS (
        SELECT c.id,c.collection,c.item_index,
            CASE WHEN typeof(r.summary)='blob' AND length(r.summary)<=1048576 THEN CAST(r.summary AS TEXT) END AS request,
            CASE WHEN typeof(i.metadata)='blob' AND length(i.metadata)<=262144 THEN CAST(i.metadata AS TEXT) END AS metadata
        FROM canonical c JOIN ext_items i ON i.id=c.id
        LEFT JOIN ext_messages r ON r.item=c.id AND r.field_id=15
    ), filtered AS (
        SELECT id,collection,item_index FROM summaries WHERE ?2=''
            OR instr(lower(coalesce(json_extract(request,'$.method'),'')),lower(?2))>0
            OR instr(lower(coalesce(json_extract(request,'$.request_target'),'')),lower(?2))>0
            OR instr(lower(coalesce(CASE WHEN
                json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.state')='value'
                AND json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.value.mapping.state')='value'
                AND json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.state')='value'
                AND json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.value.mapping.state')='value'
                AND json_extract(metadata,'{FIELDS}.host.observation.state')='value'
                AND json_extract(metadata,'{FIELDS}.host.observation.value.mapping.state')='value'
                AND json_extract(metadata,'{FIELDS}.port.observation.state')='value'
                AND json_extract(metadata,'{FIELDS}.tls.observation.state')='value'
                THEN (CASE {tls} WHEN 1 THEN 'https://' ELSE 'http://' END)
                    || (CASE WHEN instr({host},':')>0 THEN '['||{host}||']' ELSE {host} END)
                    || ':' || {port} || coalesce(json_extract(request,'$.request_target'),'') END,'')),lower(?2))>0
    )")
}

pub(in crate::store::ext_archive) fn notes_owner(
    connection: &Connection,
    source: Uuid,
    id: Uuid,
) -> Result<(crate::ExtItem, Value)> {
    validate_http_source(connection, source)?;
    let validate = |id| -> Result<(crate::ExtItem, Value)> {
        let (item, metadata) = read_http_item(connection, source, id)?;
        validate_notes_item(&item, &metadata)?;
        Ok((item, metadata))
    };
    let (item, metadata) = validate(id)?;
    if item.collection == PROXY_COLLECTIONS[0] {
        return Ok((item, metadata));
    }
    let canonical = canonical_item_ids(connection, source, &[id])?
        .remove(&id)
        .context("外部归档 HTTP 备注条目归属不存在")?;
    if canonical == id {
        return Ok((item, metadata));
    }
    let (owner, owner_metadata) = validate(canonical)?;
    ensure!(
        metadata["record"]["object"]["address"] == owner_metadata["record"]["object"]["address"]
            && metadata["record"]["mapping"]["value"]["annotations"]["observation"]
                == owner_metadata["record"]["mapping"]["value"]["annotations"]["observation"],
        "外部归档 HTTP 备注别名不是同一源对象"
    );
    Ok((owner, owner_metadata))
}

pub(in crate::store::ext_archive) fn validate_notes_item(
    item: &crate::ExtItem,
    metadata: &Value,
) -> Result<()> {
    validate_http_item(item, metadata)?;
    let annotations = &metadata["record"]["mapping"]["value"]["annotations"];
    if !annotations.is_null() {
        ensure!(
            annotations["id"] == 27
                && annotations["binding"]["source_id"] == item.source_id.to_string()
                && annotations["binding"]["item_id"] == item.id.to_string(),
            "外部归档 HTTP 源备注身份绑定不一致"
        );
    }
    Ok(())
}

pub(in crate::store::ext_archive) fn validate_http_source(
    connection: &Connection,
    source: Uuid,
) -> Result<()> {
    ensure!(!source.is_nil(), "外部归档 HTTP 来源 ID 为空");
    let report = read_report(connection, source)?;
    ensure!(
        report.complete_archive,
        "仅支持已完成的 schema 228/229 HTTP 源归档"
    );
    let _ = expected_http_context(&report)?;
    Ok(())
}

pub(in crate::store::ext_archive) fn read_http_item(
    connection: &Connection,
    source: Uuid,
    id: Uuid,
) -> Result<(crate::ExtItem, Value)> {
    ensure!(!id.is_nil(), "外部归档 HTTP 条目 ID 为空");
    let report = read_report(connection, source)?;
    let expected_context = expected_http_context(&report)?;
    let item = read_item(connection, id)?;
    ensure!(item.source_id == source, "外部归档 HTTP 条目不属于所选来源");
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    validate_http_item(&item, &metadata)?;
    ensure!(
        metadata["context"] == expected_context,
        "外部归档 HTTP 条目上下文与源 schema 不一致"
    );
    Ok((item, metadata))
}

pub(in crate::store::ext_archive) fn canonical_item_ids(
    connection: &Connection,
    source: Uuid,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Uuid>> {
    ensure!(
        ids.len() <= 500 && ids.iter().all(|id| !id.is_nil()),
        "外部归档 HTTP 归属查询超限或 ID 为空"
    );
    let mut owners = std::collections::HashMap::with_capacity(ids.len());
    let mut direct = connection.prepare("SELECT id FROM ext_items WHERE source=?1 AND collection='root.f1.f0' AND id IN (SELECT value FROM json_each(?2))")?;
    let mut rows = direct.query(params![source.to_string(), serde_json::to_string(ids)?])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        ensure!(id.len() == 36, "外部归档 HTTP 归属 ID 损坏");
        let id = Uuid::parse_str(&id)?;
        owners.insert(id, id);
    }
    let remaining: Vec<_> = ids
        .iter()
        .copied()
        .filter(|id| !owners.contains_key(id))
        .collect();
    if remaining.is_empty() {
        return Ok(owners);
    }
    // Rank once for the whole source batch; paired directory aliases share an owner,
    // while repeated occurrences inside a directory remain distinct.
    let mut query = connection.prepare(&format!(
        "{CANONICAL} SELECT r.id,coalesce(first.id,r.id) FROM ranked r
        LEFT JOIN ranked first ON r.collection='root.f1.f1'
            AND first.collection='root.f1.f0' AND first.address=r.address
            AND first.alias_rank=r.alias_rank
        WHERE r.id IN (SELECT value FROM json_each(?2))"
    ))?;
    let mut rows = query.query(params![
        source.to_string(),
        serde_json::to_string(&remaining)?
    ])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let owner: String = row.get(1)?;
        ensure!(
            id.len() == 36 && owner.len() == 36,
            "外部归档 HTTP 归属 ID 损坏"
        );
        ensure!(
            owners
                .insert(Uuid::parse_str(&id)?, Uuid::parse_str(&owner)?)
                .is_none(),
            "外部归档 HTTP 条目存在重复归属"
        );
    }
    Ok(owners)
}

pub(in crate::store) fn validate_http_item(
    item: &crate::ExtItem,
    metadata: &Value,
) -> Result<()> {
    let record = &metadata["record"];
    ensure!(
        !item.id.is_nil()
            && !item.source_id.is_nil()
            && PROXY_COLLECTIONS.contains(&item.collection.as_str())
            && item.state == "mapped"
            && item.object_address.is_some()
            && metadata["version"] == 1
            && matches!(
                metadata["context"].as_str(),
                Some("http_history_item" | "http_history_229")
            )
            && metadata["slot"]["state"] == "value"
            && metadata["slot"]["value"].as_u64() == item.object_address
            && record["object"]["object_type"] == 1
            && record["object"]["subtype"] == 0
            && record["object"]["address"]
                .as_u64()
                .is_some_and(|address| address <= i64::MAX as u64)
            && record["mapping"]["state"] == "value",
        "外部归档 HTTP 条目来源或对象绑定不一致"
    );
    Ok(())
}
