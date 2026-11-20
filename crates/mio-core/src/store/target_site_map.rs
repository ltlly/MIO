//! Bounded Target site map aggregation over live, pending, and archived HTTP traffic.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use rusqlite::{Connection, OptionalExtension, params};
use tokio_util::sync::CancellationToken;

mod archive;
mod query;
#[cfg(test)]
mod tests;
mod tree;

pub const MAX_SITE_MAP_NODES: usize = 4096;
pub const MAX_SITE_MAP_DEPTH: u8 = 16;
pub const MAX_SITE_MAP_PAGE_LIMIT: usize = 200;

const MAX_TREE_BUILD_NODES: usize = 65_536;
const MAX_SITE_MAP_SUMMARY_BYTES: usize = 1024 * 1024;
const MAX_FILTER_TEXT_BYTES: usize = 4096;
const MAX_PENDING_ENTRIES: usize = 512;
const MAX_NOTES_BYTES: usize = 64 * 1024;
const NOTES_PREVIEW_CHARACTERS: usize = 256;
const EXT_PROXY_COLLECTIONS: [&str; 2] = ["root.f1.f0", "root.f1.f1"];
const MAX_ENTRY_BYTES: usize = 128 * 1024 * 1024;
const MAX_TOTAL_ENTRY_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteMapEntryInput {
    History(crate::HistoryTransferEntry),
    Draft {
        owner_id: crate::Uuid,
        source: crate::TrafficSource,
        draft: crate::Draft,
        response_raw: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteMapEntryReceipt {
    pub entry_id: crate::Uuid,
    pub revision: u64,
    pub node_key: String,
    pub row: crate::HistoryRow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteMapEntryDetail {
    pub row_id: crate::HistoryEntryId,
    pub record: crate::FlowRecord,
    pub original: crate::OriginalMessages,
    pub request_raw: Vec<u8>,
    pub response_raw: Option<Vec<u8>>,
    pub original_request_raw: Option<Vec<u8>>,
    pub original_response_raw: Option<Vec<u8>>,
}

impl SiteMapEntryDetail {
    pub(crate) fn from_draft(owner_id: crate::Uuid, source: crate::TrafficSource, draft: &crate::Draft, response_raw: Option<Vec<u8>>) -> Result<Self> {
        ensure!(!owner_id.is_nil() && draft.revision > 0, "站点地图草稿身份无效");
        let request = crate::parse_repeater_request(&draft.raw, &draft.target, draft.version, false)?;
        let response = response_raw.as_deref().map(|raw| crate::parse_response(raw, draft.version)).transpose()?;
        let summary = crate::FlowSummary {
            id: owner_id, revision: draft.revision, source, method: request.method.clone(), url: request.url.clone(),
            status: response.as_ref().map(|response| response.status), state: crate::FlowState::Pending,
            request_bytes: request.body.len() as u64, response_bytes: response.as_ref().map_or(0, |response| response.body.len() as u64),
            started_unix_ms: 0, elapsed_ms: 0,
        };
        Ok(Self { row_id: crate::HistoryEntryId::Live(owner_id), record: crate::FlowRecord { summary, request, response },
            original: crate::OriginalMessages::default(), request_raw: draft.raw.clone(), response_raw,
            original_request_raw: None, original_response_raw: None })
    }
}

pub(in crate::store) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch("CREATE TABLE site_map_entries (
        id TEXT PRIMARY KEY NOT NULL,
        owner TEXT NOT NULL UNIQUE,
        revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
        row BLOB NOT NULL CHECK(typeof(row)='blob' AND length(row)<=1048576),
        detail BLOB NOT NULL CHECK(typeof(detail)='blob' AND length(detail)<=134217728)
    ) WITHOUT ROWID;")?;
    Ok(())
}

fn validate_detail(detail: &SiteMapEntryDetail) -> Result<()> {
    let record = &detail.record;
    ensure!(!record.summary.id.is_nil() && record.summary.revision > 0, "站点地图消息身份无效");
    let row_uuid = match detail.row_id { crate::HistoryEntryId::Live(id) => id, crate::HistoryEntryId::External { source_id, item_id } => {
        ensure!(!source_id.is_nil(), "站点地图归档来源无效"); item_id
    }};
    ensure!(row_uuid == record.summary.id, "站点地图详情来源身份不一致");
    ensure!(tree::parse_site_map_url(&record.request.url).is_some(), "站点地图请求需要有效 HTTP URL");
    ensure!(crate::http::token(record.request.method.as_bytes())
        && record.summary.method == record.request.method && record.summary.url == record.request.url
        && record.summary.status == record.response.as_ref().map(|response| response.status)
        && record.summary.request_bytes == record.request.body.len() as u64
        && record.summary.response_bytes == record.response.as_ref().map_or(0, |response| response.body.len() as u64),
        "站点地图摘要与完整消息不一致");
    ensure!(record.request.body.len() <= crate::http::MAX_BODY
        && record.response.as_ref().is_none_or(|response| response.body.len() <= crate::http::MAX_BODY)
        && !detail.request_raw.is_empty() && detail.request_raw.len() <= crate::http::MAX_BODY + 256 * 1024
        && detail.response_raw.as_ref().is_none_or(|raw| raw.len() <= crate::http::MAX_BODY + 256 * 1024),
        "站点地图消息超过完整读取限额");
    Ok(())
}

impl crate::store::ProjectStore {
    pub(crate) fn add_site_map_entry(
        &mut self,
        workspace_revision: u64,
        owner: &str,
        mut row: crate::HistoryRow,
        mut detail: SiteMapEntryDetail,
        draft: bool,
        cancel: &CancellationToken,
    ) -> Result<SiteMapEntryReceipt> {
        ensure!(!cancel.is_cancelled(), "站点地图添加已取消");
        ensure!(!owner.is_empty() && owner.len() <= 4096, "站点地图来源身份无效");
        validate_detail(&detail)?;
        let transaction = self.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::store::ensure_schema_validated_in_tx(&self.cached_workspace, &transaction, crate::store::SCHEMA)?;
        ensure!(crate::store::metadata::project_id(&transaction)? == self.id
            && crate::store::save::workspace_arc_in_tx(&self.cached_workspace, &transaction)?.revision == workspace_revision,
            "站点地图项目或工作区 revision 已变化");
        let before_tc = crate::store::query_total_changes(&transaction)?;
        let previous: Option<(String, u64, Vec<u8>)> = transaction.query_row(
            "SELECT id,revision,CASE WHEN typeof(detail)='blob' AND length(detail)<=?2 THEN detail END FROM main.site_map_entries WHERE owner=?1",
            params![owner, MAX_ENTRY_BYTES], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        let entry_id = previous.as_ref().map(|(id, _, _)| crate::Uuid::parse_str(id)).transpose()?.unwrap_or_else(crate::Uuid::new_v4);
        if draft {
            detail.record.summary.id = entry_id;
            detail.row_id = crate::HistoryEntryId::Live(entry_id);
            if let crate::HistoryRow::Live(summary) = &mut row { summary.id = entry_id; }
        }
        ensure!(row.id() == detail.row_id, "站点地图摘要与详情身份不一致");
        let encoded = serde_json::to_vec(&detail)?;
        ensure!(encoded.len() <= MAX_ENTRY_BYTES, "站点地图完整消息超过 128 MiB");
        let unchanged = previous.as_ref().is_some_and(|(_, _, bytes)| bytes == &encoded);
        let revision = match &previous {
            Some((_, revision, _)) if unchanged => *revision,
            Some((_, revision, _)) => revision.checked_add(1).filter(|value| *value <= i64::MAX as u64).context("站点地图 revision 已耗尽")?,
            None => 1,
        };
        let row_encoded = serde_json::to_vec(&row)?;
        ensure!(row_encoded.len() <= MAX_SITE_MAP_SUMMARY_BYTES, "站点地图摘要超过 1 MiB");
        if !unchanged {
            let (count, bytes): (usize, usize) = transaction.query_row(
                "SELECT count(*),coalesce(sum(length(detail)),0) FROM main.site_map_entries WHERE owner!=?1", [owner],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            ensure!(count < MAX_SITE_MAP_NODES && bytes.checked_add(encoded.len()).is_some_and(|bytes| bytes <= MAX_TOTAL_ENTRY_BYTES),
                "站点地图显式条目超过 4096 项或 512 MiB");
            crate::store::validate_site_map_update_deps(&self.cached_workspace, &transaction, cancel)?;
            transaction.execute("INSERT INTO main.site_map_entries(id,owner,revision,row,detail) VALUES(?1,?2,?3,?4,?5)
                ON CONFLICT(owner) DO UPDATE SET revision=excluded.revision,row=excluded.row,detail=excluded.detail",
                params![entry_id.to_string(), owner, revision, row_encoded, encoded])?;
        }
        ensure!(!cancel.is_cancelled(), "站点地图添加已取消");
        transaction.commit()?;
        if !unchanged {
            crate::store::advance_workspace_cache_changes(&self.connection, before_tc)?;
        }
        let node_key = tree::parse_site_map_url(&detail.record.request.url).context("站点地图 URL 无效")?.full_key;
        Ok(SiteMapEntryReceipt { entry_id, revision, node_key, row })
    }

    pub fn read_site_map_entry(&self, entry_id: crate::Uuid, revision: u64) -> Result<SiteMapEntryDetail> {
        ensure!(!entry_id.is_nil() && revision > 0, "站点地图读取身份无效");
        let bytes: Vec<u8> = self.connection.query_row(
            "SELECT CASE WHEN typeof(detail)='blob' AND length(detail)<=?3 THEN detail END
             FROM main.site_map_entries WHERE id=?1 AND revision=?2",
            params![entry_id.to_string(), revision, MAX_ENTRY_BYTES], |row| row.get(0),
        ).optional()?.context("站点地图条目不存在或 revision 已变化")?;
        let detail = serde_json::from_slice(&bytes).context("站点地图完整消息 JSON 损坏")?;
        validate_detail(&detail)?;
        Ok(detail)
    }
}

pub type HttpHistoryRow = crate::HistoryRow;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteMapFilter {
    pub in_scope_only: bool,
    pub search_term: String,
    pub status_classes: [bool; 4],
    pub hide_not_found: bool,
}

impl Default for SiteMapFilter {
    fn default() -> Self {
        Self {
            in_scope_only: false,
            search_term: String::new(),
            status_classes: [true; 4],
            hide_not_found: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SiteMapNodeKind {
    Origin,
    Folder,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteMapNode {
    pub key: String,
    pub parent_key: Option<String>,
    pub label: String,
    pub url_prefix: String,
    pub kind: SiteMapNodeKind,
    pub depth: u8,
    pub in_scope: bool,
    pub direct_count: usize,
    pub subtree_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteMapQuery {
    pub selected_node_key: Option<String>,
    pub filter: SiteMapFilter,
    pub offset: usize,
    pub limit: usize,
}

impl Default for SiteMapQuery {
    fn default() -> Self {
        Self {
            selected_node_key: None,
            filter: SiteMapFilter::default(),
            offset: 0,
            limit: MAX_SITE_MAP_PAGE_LIMIT,
        }
    }
}

impl SiteMapQuery {
    fn selected_raw(&self) -> Option<&str> {
        self.selected_node_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_SITE_MAP_PAGE_LIMIT).contains(&self.limit),
            "Site Map 分页大小必须为 1–200"
        );
        ensure!(
            self.filter.search_term.len() <= MAX_FILTER_TEXT_BYTES,
            "Site Map 搜索词超过 4096 字节"
        );
        if let Some(key) = self.selected_raw() {
            ensure!(
                key.len() <= MAX_FILTER_TEXT_BYTES,
                "Site Map 节点键超过 4096 字节"
            );
        }
        i64::try_from(self.offset).context("Site Map 分页偏移溢出")?;
        self.offset
            .checked_add(self.limit)
            .context("Site Map 分页范围溢出")?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteMapItem {
    pub row: HttpHistoryRow,
    pub referer_url: Option<String>,
}

impl SiteMapItem {
    pub fn id(&self) -> crate::HistoryEntryId {
        self.row.id()
    }

    pub fn resolve_relative_or_absolute_url(base_url: &str, raw_candidate: &str) -> Option<String> {
        let trimmed = raw_candidate.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_FILTER_TEXT_BYTES {
            return None;
        }
        if let Ok(parsed) = url::Url::parse(trimmed)
            && matches!(parsed.scheme(), "http" | "https")
            && parsed.host_str().is_some()
        {
            return Some(parsed.to_string());
        }
        if let Ok(base) = url::Url::parse(base_url)
            && let Ok(joined) = base.join(trimmed)
            && matches!(joined.scheme(), "http" | "https")
            && joined.host_str().is_some()
        {
            return Some(joined.to_string());
        }
        None
    }

    pub fn extract_referer_from_headers(
        request_url: &str,
        headers: &[crate::Header],
    ) -> Option<String> {
        headers
            .iter()
            .find(|h| {
                h.name.eq_ignore_ascii_case("referer") || h.name.eq_ignore_ascii_case("referrer")
            })
            .and_then(|h| std::str::from_utf8(&h.value).ok())
            .and_then(|val| Self::resolve_relative_or_absolute_url(request_url, val))
    }

    pub fn extract_redirect_location_from_headers(
        request_url: &str,
        status: Option<u16>,
        headers: &[crate::Header],
    ) -> Option<String> {
        if let Some(code) = status
            && !(300..=399).contains(&code)
        {
            return None;
        }
        headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("location"))
            .and_then(|h| std::str::from_utf8(&h.value).ok())
            .and_then(|val| Self::resolve_relative_or_absolute_url(request_url, val))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteMapSnapshot {
    pub nodes: Vec<SiteMapNode>,
    pub truncated_nodes: bool,
    pub items: Vec<HttpHistoryRow>,
    pub site_map_items: Vec<SiteMapItem>,
    pub crawl_edges: Vec<(Option<String>, String)>,
    pub total_items: usize,
    pub offset: usize,
    pub explicit_entries: Vec<(crate::HistoryEntryId, crate::Uuid, u64)>,
}

impl SiteMapSnapshot {
    pub fn referer_for(&self, id: crate::HistoryEntryId) -> Option<&str> {
        self.site_map_items
            .iter()
            .find(|item| item.id() == id)
            .and_then(|item| item.referer_url.as_deref())
    }

    pub fn referer_for_url(&self, url: &str) -> Option<&str> {
        self.crawl_edges
            .iter()
            .find(|(_, target)| target == url)
            .and_then(|(referer, _)| referer.as_deref())
    }
}
