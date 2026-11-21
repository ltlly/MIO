use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use mio_plugin_api::protocol::{MAX_STORAGE_KEY_BYTES, MAX_STORAGE_VALUE_BYTES};
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Deserializer as _, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{FlowId, TrafficSource, Uuid};

mod ops;

const PREFIX: &str = "plugin_storage:";
pub(in crate::store) const LIVE_HTTP_ANNOTATIONS_KEY: &str = "live_http_annotations";
const MAX_PLUGIN_STORAGE_BYTES: usize = 2 * 1024 * 1024;
pub(in crate::store) const MAX_LIVE_ANNOTATIONS_BYTES: usize = 8 * 1024 * 1024;
const MAX_LIVE_NOTE_BYTES: usize = 64 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_NAMESPACES: usize = 64;

pub(super) use ops::{audit_pinned_flows, read_audit_state};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct LiveHttpAnnotation {
    #[serde(default)]
    pub notes_revision: u64,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub highlight_revision: u64,
    #[serde(default)]
    pub highlight: u8,
}

impl LiveHttpAnnotation {
    pub(in crate::store) fn is_default(&self) -> bool {
        self.notes_revision == 0
            && self.notes.is_empty()
            && self.highlight_revision == 0
            && self.highlight == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginKvMap {
    entries: BTreeMap<String, Vec<u8>>,
}

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    ensure!(
        !plugin_id.is_empty() && plugin_id.len() <= 128,
        "插件存储插件 ID 为空或超过 128 字节"
    );
    ensure!(
        plugin_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
        "插件存储插件 ID 含未授权字符"
    );
    Ok(())
}

fn validate_key(key: &str) -> Result<()> {
    ensure!(
        !key.is_empty() && key.len() <= MAX_STORAGE_KEY_BYTES,
        "插件存储 key 为空或超过限额"
    );
    Ok(())
}

fn validate_value(value: &[u8]) -> Result<()> {
    ensure!(
        value.len() <= MAX_STORAGE_VALUE_BYTES,
        "插件存储 value 超过限额"
    );
    Ok(())
}

fn namespace(plugin_id: &str) -> Result<String> {
    validate_plugin_id(plugin_id)?;
    Ok(format!("{PREFIX}{plugin_id}"))
}

fn decode_map(plugin_id: &str, bytes: &[u8]) -> Result<PluginKvMap> {
    validate_plugin_id(plugin_id)?;
    ensure!(
        bytes.len() <= MAX_PLUGIN_STORAGE_BYTES,
        "插件存储超过编码限额"
    );
    let map: PluginKvMap =
        serde_json::from_slice(bytes).context("插件存储 JSON 损坏或包含未知字段")?;
    ensure!(map.entries.len() <= MAX_ENTRIES, "插件存储键数量超过限额");
    for (key, value) in &map.entries {
        validate_key(key)?;
        validate_value(value)?;
    }
    Ok(map)
}

fn read_map(transaction: &Transaction<'_>, plugin_id: &str) -> Result<Option<PluginKvMap>> {
    let key = namespace(plugin_id)?;
    let mut statement = transaction.prepare(
        "SELECT key,typeof(value),
                CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
         FROM main.metadata WHERE key=?1 COLLATE BINARY",
    )?;
    let mut rows = statement.query(params![
        key.as_str(),
        i64::try_from(MAX_PLUGIN_STORAGE_BYTES)?
    ])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    ensure!(row.get::<_, String>(0)? == key, "插件存储键编码无效");
    ensure!(
        row.get::<_, String>(1)? == "blob",
        "插件存储必须为二进制 JSON"
    );
    let bytes = row
        .get_ref(2)?
        .as_blob()
        .context("插件存储类型或长度无效")?;
    let map = decode_map(plugin_id, bytes)?;
    ensure!(rows.next()?.is_none(), "插件存储记录重复");
    Ok(Some(map))
}

fn namespace_count(transaction: &Transaction<'_>) -> Result<usize> {
    let count: i64 = transaction.query_row(
        "SELECT count(*) FROM main.metadata WHERE key GLOB 'plugin_storage:*'",
        [],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count)?)
}

struct LiveAnnotationsVisitor;

impl<'de> serde::de::Visitor<'de> for LiveAnnotationsVisitor {
    type Value = BTreeMap<Uuid, LiveHttpAnnotation>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("实时 HTTP 注解 JSON 对象")
    }

    fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut map = BTreeMap::new();
        while let Some((key, value)) = access.next_entry::<Uuid, LiveHttpAnnotation>()? {
            if map.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("实时 HTTP 注解包含重复键"));
            }
        }
        Ok(map)
    }
}

pub(in crate::store) fn read_live_http_annotations(
    connection: &Connection,
) -> Result<BTreeMap<Uuid, LiveHttpAnnotation>> {
    let mut query = connection.prepare(
        "SELECT key, typeof(value),
                CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
         FROM main.metadata WHERE key=?1 COLLATE BINARY",
    )?;
    let mut rows = query.query(params![
        LIVE_HTTP_ANNOTATIONS_KEY,
        i64::try_from(MAX_LIVE_ANNOTATIONS_BYTES)?
    ])?;
    let Some(row) = rows.next()? else {
        return Ok(BTreeMap::new());
    };
    ensure!(
        row.get::<_, String>(0)? == LIVE_HTTP_ANNOTATIONS_KEY,
        "实时 HTTP 注解目录键编码无效"
    );
    ensure!(
        row.get::<_, String>(1)? == "blob",
        "实时 HTTP 注解必须为二进制 JSON"
    );
    let bytes = row
        .get_ref(2)?
        .as_blob()
        .context("实时 HTTP 注解类型无效或超过 8 MiB 限额")?;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let map = deserializer
        .deserialize_map(LiveAnnotationsVisitor)
        .and_then(|map| {
            deserializer.end()?;
            Ok(map)
        })
        .context("实时 HTTP 注解结构或字段无效")?;
    ensure!(rows.next()?.is_none(), "实时 HTTP 注解记录重复");
    for (id, entry) in &map {
        ensure!(!id.is_nil(), "实时 HTTP 注解包含空 Flow ID");
        ensure!(
            entry.notes_revision <= i64::MAX as u64
                && entry.highlight_revision <= i64::MAX as u64
                && entry.notes.len() <= MAX_LIVE_NOTE_BYTES
                && entry.highlight <= 9
                && (entry.notes_revision > 0 || entry.notes.is_empty())
                && (entry.highlight_revision > 0 || entry.highlight == 0),
            "实时 HTTP 注解 revision、文本长度或颜色无效"
        );
    }
    Ok(map)
}

pub(in crate::store) fn write_live_http_annotations(
    transaction: &Transaction<'_>,
    map: &mut BTreeMap<Uuid, LiveHttpAnnotation>,
    cancel: &CancellationToken,
) -> Result<()> {
    map.retain(|_, entry| !entry.is_default());
    for (id, entry) in &*map {
        ensure!(!id.is_nil(), "实时 HTTP 注解包含空 Flow ID");
        ensure!(
            entry.notes_revision <= i64::MAX as u64
                && entry.highlight_revision <= i64::MAX as u64
                && entry.notes.len() <= MAX_LIVE_NOTE_BYTES
                && entry.highlight <= 9,
            "实时 HTTP 注解 revision、文本长度或颜色无效"
        );
    }
    let encoded = serde_json::to_vec(map)?;
    ensure!(
        encoded.len() <= MAX_LIVE_ANNOTATIONS_BYTES,
        "实时 HTTP 注解超过 8 MiB 限额"
    );
    ensure!(!cancel.is_cancelled(), "实时 HTTP 注解写入已取消");
    if map.is_empty() {
        transaction.execute(
            "DELETE FROM main.metadata WHERE key=?1 COLLATE BINARY",
            [LIVE_HTTP_ANNOTATIONS_KEY],
        )?;
    } else {
        transaction.execute(
            "INSERT INTO main.metadata(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![LIVE_HTTP_ANNOTATIONS_KEY, encoded.as_slice()],
        )?;
    }
    Ok(())
}

pub(in crate::store) fn remove_live_http_annotations(
    transaction: &Transaction<'_>,
    flow_ids: &[FlowId],
) -> Result<()> {
    if flow_ids.is_empty() {
        return Ok(());
    }
    let mut map = read_live_http_annotations(transaction)?;
    let mut changed = false;
    for id in flow_ids {
        changed |= map.remove(id).is_some();
    }
    if changed {
        write_live_http_annotations(transaction, &mut map, &CancellationToken::new())?;
    }
    Ok(())
}

pub(in crate::store) fn validate_live_proxy_flow(
    connection: &Connection,
    flow_id: FlowId,
) -> Result<()> {
    ensure!(!flow_id.is_nil(), "实时 HTTP 记录 ID 为空");
    let mut stmt = connection.prepare(
        "SELECT CASE WHEN typeof(summary)='blob' AND length(summary)<=?2 THEN summary END
         FROM main.flows WHERE id=?1",
    )?;
    let mut rows = stmt.query(params![
        flow_id.to_string(),
        crate::store::ext_archive::MAX_SUMMARY_BYTES as i64
    ])?;
    let Some(row) = rows.next()? else {
        let site_map: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='site_map_entries')
             AND EXISTS(SELECT 1 FROM main.site_map_entries WHERE id=?1)",
            [flow_id.to_string()], |row| row.get(0),
        )?;
        ensure!(site_map, "HTTP 历史记录 {flow_id} 不存在");
        return Ok(());
    };
    let bytes: Vec<u8> = row
        .get::<_, Option<Vec<u8>>>(0)?
        .context("HTTP 历史记录摘要损坏或超过限额")?;
    let summary: crate::FlowSummary =
        serde_json::from_slice(&bytes).context("HTTP 历史记录摘要 JSON 损坏")?;
    ensure!(
        summary.id == flow_id && summary.source == TrafficSource::Proxy,
        "HTTP 历史记录 {flow_id} 不是 Proxy 实时记录"
    );
    ensure!(rows.next()?.is_none(), "HTTP 历史记录重复");
    Ok(())
}

#[cfg(test)]
mod tests;
