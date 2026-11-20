use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::super::read_item;
use crate::{ExtImportReport, ExtItem, TabGroup, Uuid};

const GROUPS: &str = "root.f2.f4";

pub(super) struct Groups {
    pub values: Vec<TabGroup>,
    pub gaps: Vec<String>,
    source: Uuid,
    complete_catalog: bool,
    by_address: BTreeMap<u64, Option<Uuid>>,
}

pub(super) fn load(
    connection: &Connection,
    source: Uuid,
    report: &ExtImportReport,
    cancel: &CancellationToken,
) -> Result<Groups> {
    crate::ext_archive::import::check_cancel(cancel)?;
    ensure!(
        report.complete_archive && (report.schema_floor, report.schema_revision) == (229, 229),
        "仅恢复完整 schema 229 来源的 Repeater 分组"
    );
    let mut result = Groups {
        values: Vec::new(),
        gaps: Vec::new(),
        source,
        complete_catalog: false,
        by_address: BTreeMap::new(),
    };
    let Some(collection) = report.collections.iter().find(|entry| entry.path == GROUPS) else {
        result
            .gaps
            .push("Repeater 分组目录未映射，未推断源项目没有分组。".into());
        return Ok(result);
    };
    let Some(logical) = collection.logical_items else {
        result
            .gaps
            .push("Repeater 分组目录不可解释，未推断源项目没有分组。".into());
        return Ok(result);
    };
    ensure!(
        logical <= 512,
        "Repeater 分组超过 512 个恢复限额；未截断分组"
    );
    let mut seen = 0_u64;
    let mut after = -1_i64;
    let mut skipped = BTreeMap::<&str, usize>::new();
    let mut colors = BTreeMap::<u8, usize>::new();
    loop {
        crate::ext_archive::import::check_cancel(cancel)?;
        let page = {
            let mut query = connection.prepare("SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END,item_index FROM main.ext_items WHERE source=?1 AND collection=?2 AND item_index>?3 ORDER BY item_index LIMIT 64")?;
            query
                .query_map(params![source.to_string(), GROUPS, after], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if page.is_empty() {
            break;
        }
        for (id, index) in page {
            crate::ext_archive::import::check_cancel(cancel)?;
            ensure!(
                index > after && (index as u64) < logical,
                "Repeater 分组源槽位越界或顺序损坏"
            );
            after = index;
            seen += 1;
            let item = read_item(connection, Uuid::parse_str(&id)?)?;
            ensure!(
                item.source_id == source && item.collection == GROUPS && item.index == index as u64,
                "Repeater 分组来源或槽位不一致"
            );
            if item.state != "mapped" {
                *skipped.entry("分组槽位为 null、失败或未映射").or_default() += 1;
                continue;
            }
            let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
            let record = &metadata["record"];
            ensure!(
                item.object_address.is_some()
                    && metadata["version"] == 1
                    && metadata["context"] == "repeater_group_229"
                    && metadata["slot"]["state"] == "value"
                    && metadata["slot"]["value"].as_u64() == item.object_address
                    && record["object"]["object_type"] == 0
                    && record["object"]["subtype"] == 1
                    && record["mapping"]["state"] == "value",
                "Repeater 分组对象绑定不一致"
            );
            let address = record["object"]["address"]
                .as_u64()
                .context("Repeater 分组缺少已解析对象地址")?;
            ensure!(
                result.by_address.insert(address, None).is_none(),
                "Repeater 分组目录重复引用同一对象，未猜测分组身份"
            );
            let fields = &record["mapping"]["value"];
            let name = &fields["name"];
            let decoded = &name["observation"]["value"];
            let expanded = &fields["expanded"];
            if name["observation"]["state"] != "value"
                || decoded["mapping"]["state"] != "value"
                || expanded["observation"]["state"] != "value"
            {
                *skipped.entry("分组名称或展开状态未映射").or_default() += 1;
                continue;
            }
            ensure!(
                name["id"] == 0
                    && decoded["object"]["object_type"] == 0
                    && decoded["object"]["subtype"] == 0
                    && expanded["id"] == 2,
                "Repeater 分组名称或展开字段布局不一致"
            );
            let title = decoded["mapping"]["value"]
                .as_str()
                .filter(|name| {
                    !name.is_empty() && name.len() <= 4096 && name.encode_utf16().count() <= 4096
                })
                .context("Repeater 分组名称无效或超限")?;
            let expanded = expanded["observation"]["value"]
                .as_bool()
                .context("Repeater 分组展开状态不是布尔值")?;
            let color = &fields["color"];
            let color = if color["observation"]["state"] == "value" {
                ensure!(color["id"] == 1, "Repeater 分组颜色字段不一致");
                let raw = color["observation"]["value"]
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .context("Repeater 分组颜色不是原始单字节值")?;
                if raw <= 14 {
                    Some(raw)
                } else {
                    *colors.entry(raw).or_default() += 1;
                    None
                }
            } else {
                *skipped
                    .entry("分组颜色未映射，已保留名称与成员")
                    .or_default() += 1;
                None
            };
            let id = Uuid::new_v4();
            result.by_address.insert(address, Some(id));
            result.values.push(TabGroup {
                id,
                title: title.into(),
                collapsed: !expanded,
                color,
            });
        }
    }
    ensure!(
        seen == collection.indexed_items && seen <= logical,
        "Repeater 分组目录与报告条目数不一致"
    );
    result.complete_catalog = seen == logical;
    if !result.complete_catalog {
        result.gaps.push(format!(
            "Repeater 分组目录有 {} 个源槽位未索引，未推断缺失成员关系。",
            logical - seen
        ));
    }
    result.gaps.extend(
        skipped
            .into_iter()
            .map(|(reason, count)| format!("Repeater {reason}：{count}；原始归档保留。")),
    );
    result.gaps.extend(colors.into_iter().map(|(color, count)| format!("Repeater 颜色 ID {color} 尚未映射：{count} 个分组；名称与成员已恢复，原颜色值保留在归档元数据。")));
    result.gaps.push(format!(
        "已恢复 {} 个 Repeater 分组的名称、可解释颜色与展开状态。",
        result.values.len()
    ));
    Ok(result)
}

impl Groups {
    pub(super) fn membership(
        &self,
        item: &ExtItem,
        gaps: &mut BTreeMap<&'static str, usize>,
    ) -> Result<Option<Uuid>> {
        ensure!(
            item.source_id == self.source && item.collection == "root.f2.f1",
            "Repeater 标签与分组不属于同一来源"
        );
        let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
        ensure!(
            metadata["version"] == 1 && metadata["context"] == "repeater_current_229",
            "Repeater 标签分组上下文未确认"
        );
        let group = &metadata["record"]["mapping"]["value"]["group"];
        if group.is_null()
            || group["observation"]["state"] != "value" && group["observation"]["state"] != "null"
        {
            *gaps
                .entry("标签分组字段未映射，未推断为未分组")
                .or_default() += 1;
            return Ok(None);
        }
        ensure!(group["id"] == 32, "Repeater 标签分组字段不是 f32");
        if group["observation"]["state"] == "null" {
            return Ok(None);
        }
        let target = &group["observation"]["value"];
        ensure!(
            target["object_type"] == 0 && target["subtype"] == 1,
            "Repeater 标签分组引用对象类型不一致"
        );
        let address = target["address"]
            .as_u64()
            .context("Repeater 标签分组缺少已解析对象地址")?;
        if let Some(group) = self.by_address.get(&address) {
            if group.is_none() {
                *gaps.entry("标签所引用分组的名称或状态未恢复").or_default() += 1;
            }
            return Ok(*group);
        }
        ensure!(
            !self.complete_catalog,
            "Repeater 标签分组引用不属于当前来源分组目录"
        );
        *gaps
            .entry("分组目录不完整，标签分组引用未恢复")
            .or_default() += 1;
        Ok(None)
    }
}
