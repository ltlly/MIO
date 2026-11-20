use anyhow::{Context, Result, ensure};
use rusqlite::Transaction;

use crate::{Uuid, Workspace};

use super::{MAX_WORKSPACE_BYTES, WORKSPACE_QUERY};

fn check_table(transaction: &Transaction<'_>) -> Result<()> {
    // table_list initializes view/virtual column counts by preparing SELECT * before it
    // filters by name. Reject those target objects from the catalog first; virtual tables
    // and views have no storage root page. This query never expands the metadata object.
    let (kind, root_page): (String, i64) = transaction
        .query_row(
            "SELECT type,rootpage FROM main.sqlite_schema
             WHERE name='metadata' COLLATE NOCASE AND type IN ('table','view')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("项目 metadata 表缺失或损坏")?;
    ensure!(
        kind == "table" && root_page > 0,
        "项目 metadata 必须为 main 中的普通表"
    );
    let kind: String = transaction
        .query_row("PRAGMA main.table_list('metadata')", [], |row| row.get(2))
        .context("项目 metadata 表缺失或损坏")?;
    ensure!(kind == "table", "项目 metadata 必须为 main 中的普通表");

    // Ordinary tables can also contain computed columns. Neither the lookup key nor value
    // may evaluate an expression before the SQL byte/type gate has inspected stored data.
    let mut statement = transaction.prepare("PRAGMA main.table_xinfo('metadata')")?;
    let mut rows = statement.query([])?;
    let mut stored_key = false;
    let mut stored_value = false;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        let hidden: i32 = row.get(6)?;
        if name.eq_ignore_ascii_case("key") {
            stored_key = hidden == 0;
        }
        if name.eq_ignore_ascii_case("value") {
            stored_value = hidden == 0;
        }
    }
    ensure!(
        stored_key && stored_value,
        "项目 metadata 的 key/value 必须为实际存储列"
    );
    Ok(())
}

pub(in crate::store) fn project_id(transaction: &Transaction<'_>) -> Result<Uuid> {
    check_table(transaction)?;
    let mut statement = transaction.prepare(
        "SELECT CASE WHEN typeof(value)='text' THEN
            CASE WHEN length(CAST(value AS BLOB))=36 THEN value END END
         FROM main.metadata WHERE key='id'",
    )?;
    let mut rows = statement.query([])?;
    let row = rows.next()?.context("项目 ID 缺失")?;
    let id: String = row
        .get::<_, Option<String>>(0)?
        .context("项目 ID 必须为 36 字节文本")?;
    ensure!(rows.next()?.is_none(), "项目 ID 记录重复");
    Uuid::parse_str(&id).context("项目 ID 损坏")
}

pub(in crate::store) fn workspace(transaction: &Transaction<'_>) -> Result<Workspace> {
    workspace_with_limit(transaction, MAX_WORKSPACE_BYTES)
}

pub(in crate::store) fn workspace_for_update(transaction: &Transaction<'_>) -> Result<Workspace> {
    read_workspace(transaction, MAX_WORKSPACE_BYTES, true)
}

pub(super) fn workspace_with_limit(
    transaction: &Transaction<'_>,
    limit: usize,
) -> Result<Workspace> {
    read_workspace(transaction, limit, false)
}

fn read_workspace(
    transaction: &Transaction<'_>,
    limit: usize,
    for_update: bool,
) -> Result<Workspace> {
    check_table(transaction)?;
    let mut statement = transaction.prepare(WORKSPACE_QUERY)?;
    let mut rows = statement.query([i64::try_from(limit)?])?;
    let row = rows.next()?.context("项目工作区缺失")?;
    ensure!(
        row.get::<_, String>(0)? == "blob",
        "项目工作区必须为二进制 JSON"
    );
    let length: u64 = row.get(1)?;
    ensure!(length <= limit as u64, "项目工作区超过编码读取限额");
    let bytes = row
        .get::<_, Option<Vec<u8>>>(2)?
        .context("项目工作区缺失")?;
    ensure!(bytes.len() as u64 == length, "项目工作区长度不一致");
    ensure!(rows.next()?.is_none(), "项目工作区记录重复");
    let mut unknown = false;
    let mut decoder = serde_json::Deserializer::from_slice(&bytes);
    let workspace: Workspace = serde_ignored::deserialize(&mut decoder, |_| unknown = true)
        .context("项目工作区 JSON 损坏")?;
    decoder.end().context("项目工作区 JSON 存在尾随内容")?;
    ensure!(
        !for_update || !unknown,
        "工作区包含未识别字段；原始数据保留，未写入修改"
    );
    workspace.validate()?;
    crate::store::ext_archive::validate_workspace_responses(transaction, &workspace)?;
    crate::store::repeater_history::validate_workspace(transaction, &workspace)?;
    Ok(workspace)
}
