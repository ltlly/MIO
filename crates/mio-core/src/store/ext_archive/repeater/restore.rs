use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Transaction, params};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::super::{read_item, read_message_summary, read_report};
use crate::{ExtItem, ExtMessageSummary, Uuid, Workspace, http::MAX_BODY};

const TABS: &str = "root.f2.f1";

#[derive(Debug)]
pub(crate) struct ExtRepeaterRestore {
    pub restored_tabs: usize,
    pub restored_groups: usize,
    pub skipped_tabs: usize,
    pub gaps: Vec<String>,
}

enum Selection {
    Ready {
        title: String,
        request: Box<ExtMessageSummary>,
    },
    Skip(&'static str),
}

pub(in crate::store::ext_archive) fn build_workspace(
    transaction: &Transaction<'_>,
    source_id: Uuid,
    cancel: &CancellationToken,
) -> Result<(Workspace, ExtRepeaterRestore)> {
    check_cancel(cancel)?;
    let report = read_report(transaction, source_id)?;
    let schema = (report.schema_floor, report.schema_revision);
    ensure!(
        report.complete_archive && matches!(schema, (228, 228) | (229, 229)),
        "只能恢复已完整归档的 schema 228/229 Repeater 标签"
    );
    let mut workspace = crate::store::metadata::workspace(transaction)?;
    ensure!(
        workspace.tab_count() == 0 && workspace.groups.is_empty(),
        "目标工作区已有标签或分组；未重复恢复 外部归档工作区"
    );
    let mut result = ExtRepeaterRestore {
        restored_tabs: 0,
        restored_groups: 0,
        skipped_tabs: 0,
        gaps: Vec::new(),
    };
    let groups = if schema == (229, 229) {
        let mut groups = super::groups::load(transaction, source_id, &report, cancel)?;
        workspace.groups.append(&mut groups.values);
        result.restored_groups = workspace.groups.len();
        result.gaps.append(&mut groups.gaps);
        if result.restored_groups > 0 {
            workspace.revision = workspace
                .revision
                .checked_add(1)
                .context("恢复分组时工作区 revision 已耗尽")?;
        }
        Some(groups)
    } else {
        None
    };
    let Some(collection) = report.collections.iter().find(|entry| entry.path == TABS) else {
        result
            .gaps
            .push("Repeater 标签未恢复：源标签目录未映射，不能推断标签为空。".into());
        return Ok((workspace, result));
    };
    let Some(logical) = collection.logical_items else {
        result
            .gaps
            .push("Repeater 标签未恢复：源标签目录不可解释，不能推断标签为空。".into());
        return Ok((workspace, result));
    };
    let mut skipped = BTreeMap::<&'static str, usize>::new();
    let mut seen = 0_u64;
    let mut after = -1_i64;
    let mut missing_targets: u64 = 0;
    let mut group_gaps = BTreeMap::new();
    loop {
        check_cancel(cancel)?;
        let page = {
            let mut query = transaction.prepare("SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END,item_index FROM main.ext_items WHERE source=?1 AND collection=?2 AND item_index>?3 ORDER BY item_index LIMIT 64")?;
            query
                .query_map(params![source_id.to_string(), TABS, after], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if page.is_empty() {
            break;
        }
        for (id, index) in page {
            check_cancel(cancel)?;
            ensure!(
                index > after && (index as u64) < logical,
                "源标签索引顺序或范围损坏"
            );
            after = index;
            seen += 1;
            let item = read_item(transaction, Uuid::parse_str(&id)?)?;
            ensure!(
                item.source_id == source_id
                    && item.collection == TABS
                    && item.index == index as u64,
                "源标签归属已改变"
            );
            let (title, request) = match select(transaction, &item, schema)? {
                Selection::Ready { title, request } => (title, request),
                Selection::Skip(reason) => {
                    *skipped.entry(reason).or_default() += 1;
                    continue;
                }
            };
            let length = usize::try_from(request.raw_bytes).context("恢复的请求大小溢出")?;
            ensure!(
                workspace.tab_count() < 512,
                "恢复 Repeater 标签超过 512 个工作标签限额；未截断工作区"
            );
            ensure!(
                length <= MAX_BODY + 256 * 1024,
                "恢复的 Repeater 请求超过可编辑大小限额；未截断工作区"
            );
            ensure!(
                workspace
                    .draft_bytes()?
                    .checked_add(length)
                    .is_some_and(|bytes| bytes <= 64 * 1024 * 1024),
                "恢复 Repeater 工作区超过 64 MiB 草稿限额；未截断工作区"
            );
            let mut tab = super::load_tab(
                transaction,
                source_id,
                request.item_id,
                request.id,
                &workspace,
                cancel,
            )?;
            tab.title = title;
            tab.group = match &groups {
                Some(groups) => groups.membership(&item, &mut group_gaps)?,
                None => None,
            };
            if let Some(source) =
                crate::store::repeater_history::source_for_parent(transaction, source_id, item.id)?
            {
                tab.history = Some(crate::RepeaterHistory {
                    source: Some(source),
                    cursor: (request.role == "request").then_some(crate::RepeaterHistoryCursor {
                        entry: crate::RepeaterHistoryEntryId::Archive(request.item_index),
                        draft_revision: tab.draft.revision,
                    }),
                });
            }
            missing_targets += u64::from(tab.draft.target.is_empty());
            if report.repeater_active_tab == Some(item.index) {
                workspace.active = Some(tab.id);
            }
            workspace.tabs.push(tab);
            result.restored_tabs += 1;
        }
    }
    ensure!(
        seen == collection.indexed_items && seen <= logical,
        "源标签目录与已保存报告不一致"
    );
    if seen < logical {
        skipped.insert("源标签槽位未完整索引", usize::try_from(logical - seen)?);
    }
    result.skipped_tabs = skipped.values().sum();
    result.gaps.extend(
        group_gaps
            .into_iter()
            .map(|(reason, count)| format!("Repeater {reason}：{count}。")),
    );
    result
        .gaps
        .extend(skipped.into_iter().map(|(reason, count)| {
            format!(
                "Repeater 标签未恢复：{reason}（{count} 个源槽位）；原始归档和完整已索引历史保留。"
            )
        }));
    if missing_targets > 0 {
        result.gaps.push(format!("已恢复的 Repeater 标签中，{missing_targets} 个连接目标未映射；未从 Host 或其它历史项推断目标。"));
    }
    if !workspace.tabs.is_empty() {
        if result.restored_groups == 0 {
            workspace.revision = workspace
                .revision
                .checked_add(1)
                .context("恢复工作区 revision 已耗尽")?;
        }
        if workspace.active.is_none() {
            workspace.active = Some(workspace.tabs[0].id);
            result.gaps.push(match report.repeater_active_tab {
                Some(index) => {
                    format!("源 Repeater 活动标签槽位 {index} 未恢复；当前选择首个已恢复标签。")
                }
                None => "源 Repeater 全局活动标签未映射；当前选择首个已恢复标签。".into(),
            });
        }
    } else if let Some(index) = report.repeater_active_tab {
        result.gaps.push(format!(
            "源 Repeater 活动标签槽位 {index} 未恢复；没有可恢复标签，未创建活动标签。"
        ));
    }
    check_cancel(cancel)?;
    Ok((workspace, result))
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    crate::ext_archive::import::check_cancel(cancel)
}

fn select(connection: &Connection, item: &ExtItem, schema: (u16, u16)) -> Result<Selection> {
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    if schema == (228, 228) {
        if !mapped(item, &metadata, "repeater_tab", (0, 2)) {
            return Ok(Selection::Skip("源标签结构未映射"));
        }
        let current = &metadata["record"]["mapping"]["value"]["current_request"];
        return select_bound_request(connection, item, 4, current, (item.index + 1).to_string());
    }
    if !mapped(item, &metadata, "repeater_current_229", (0, 2)) {
        return Ok(Selection::Skip("源标签结构未映射"));
    }
    let fields = &metadata["record"]["mapping"]["value"];
    let current = &fields["current_request"];
    let title = &current["title"];
    let decoded = &title["observation"]["value"];
    let Some(text) = decoded["mapping"]["value"].as_str().filter(|text| {
        !text.is_empty()
            && text.len() <= 4096
            && text.encode_utf16().count() <= crate::ext_archive::MAX_REPEATER_TITLE_UNITS
    }) else {
        return Ok(Selection::Skip("源标签标题未映射"));
    };
    if current["id"] != 4
        || current["binding"]["source_id"] != item.source_id.to_string()
        || current["binding"]["item_id"] != item.id.to_string()
        || title["id"] != 0
        || title["observation"]["state"] != "value"
        || decoded["object"]["object_type"] != 0
        || decoded["object"]["subtype"] != 0
        || decoded["mapping"]["state"] != "value"
    {
        return Ok(Selection::Skip("源标签标题绑定不可解释"));
    }
    let selected = &fields["selected_history"];
    let Some(position) = selected["observation"]["value"]
        .as_i64()
        .filter(|position| i32::try_from(*position).is_ok())
    else {
        return Ok(Selection::Skip("持久选中位置未映射"));
    };
    if selected["id"] != 1 || selected["observation"]["state"] != "value" || position < -1 {
        return Ok(Selection::Skip("持久选中位置无效"));
    }
    let (owner, field_id, field) = if position == -1 {
        (item.clone(), 4, current.clone())
    } else {
        let history = &fields["history"];
        let list = &history["observation"]["value"];
        if history["id"] != 2
            || history["observation"]["state"] != "value"
            || list["object"]["object_type"] != 0
            || list["object"]["subtype"] != 0
            || list["mapping"]["state"] != "value"
            || !list["mapping"]["value"]["logical_len"]
                .as_u64()
                .is_some_and(|length| (position as u64) < length)
        {
            return Ok(Selection::Skip("持久选中历史位置越界或目录未映射"));
        }
        let path = format!("{TABS}[{}].f2", item.index);
        let mut query = connection.prepare("SELECT id FROM main.ext_items WHERE source=?1 AND collection=?2 AND item_index=?3 LIMIT 2")?;
        let mut rows = query.query(params![item.source_id.to_string(), path, position])?;
        let Some(row) = rows.next()? else {
            return Ok(Selection::Skip("持久选中历史槽位未索引"));
        };
        let id: String = row.get(0)?;
        ensure!(rows.next()?.is_none(), "选中历史槽位索引重复");
        let pair = read_item(connection, Uuid::parse_str(&id)?)?;
        ensure!(
            pair.source_id == item.source_id
                && pair.collection == path
                && pair.index == position as u64,
            "选中历史槽位归属不一致"
        );
        let metadata: Value = serde_json::from_slice(&pair.metadata_json)?;
        if !mapped(&pair, &metadata, "repeater_pair_229", (1, 0)) {
            return Ok(Selection::Skip("持久选中历史槽位为 null、失败或未映射"));
        }
        (
            pair,
            2,
            metadata["record"]["mapping"]["value"]["request"].clone(),
        )
    };
    select_bound_request(connection, &owner, field_id, &field, text.to_owned())
}

fn select_bound_request(
    connection: &Connection,
    owner: &ExtItem,
    field_id: u8,
    field: &Value,
    title: String,
) -> Result<Selection> {
    if field["id"] != field_id || field["observation"]["state"] != "value" {
        return Ok(Selection::Skip("持久选中请求缺失、null 或无法解码"));
    }
    let mut query = connection
        .prepare("SELECT id FROM main.ext_messages WHERE item=?1 AND field_id=?2 LIMIT 2")?;
    let mut rows = query.query(params![owner.id.to_string(), field_id])?;
    let Some(row) = rows.next()? else {
        return Ok(Selection::Skip("持久选中请求未索引"));
    };
    let id: String = row.get(0)?;
    ensure!(rows.next()?.is_none(), "选中请求消息索引重复");
    let request = read_message_summary(connection, Uuid::parse_str(&id)?)?;
    let raw = &field["observation"]["value"];
    let binding = &field["binding"];
    if request.source_id != owner.source_id
        || request.item_id != owner.id
        || request.collection != owner.collection
        || request.item_index != owner.index
        || request.field_id != field_id
        || request.role
            != if field_id == 4 {
                "current_request"
            } else {
                "request"
            }
        || binding["source_id"] != owner.source_id.to_string()
        || binding["item_id"] != owner.id.to_string()
        || binding["message_id"] != request.id.to_string()
        || raw["kind"] != "bytes"
        || raw["address"]
            .as_u64()
            .and_then(|address| address.checked_add(8))
            != Some(request.raw_offset)
        || raw["payload_bytes"].as_u64() != Some(request.raw_bytes)
        || raw["element_count"].as_u64() != Some(request.raw_bytes)
        || raw["total_size"].as_u64() != request.raw_bytes.max(1).checked_add(8)
    {
        return Ok(Selection::Skip("持久选中请求与元数据绑定不一致"));
    }
    if request.raw_bytes == 0 || request.declared_version.is_none() {
        return Ok(Selection::Skip("持久选中请求为空或 HTTP 声明版本未识别"));
    }
    Ok(Selection::Ready {
        title,
        request: Box::new(request),
    })
}

fn mapped(item: &ExtItem, metadata: &Value, context: &str, tag: (u8, u8)) -> bool {
    item.state == "mapped"
        && item.object_address.is_some()
        && metadata["version"] == 1
        && metadata["context"] == context
        && metadata["slot"]["state"] == "value"
        && metadata["slot"]["value"].as_u64() == item.object_address
        && metadata["record"]["object"]["object_type"] == tag.0
        && metadata["record"]["object"]["subtype"] == tag.1
        && metadata["record"]["mapping"]["state"] == "value"
}
