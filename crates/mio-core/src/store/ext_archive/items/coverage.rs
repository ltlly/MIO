use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use rusqlite::params;
use serde_json::Value;

use super::{MAX_COLLECTION_BYTES, MAX_METADATA_BYTES, ProjectStore, read_blob, source_state};
use crate::store::ext_archive::read_report;
use crate::{ExtCategory, ExtCoverageReport, ExtObjectTypeCount, Uuid};

const HTTP_ALLOWED_228: [u8; 2] = [15, 18];
const HTTP_ALLOWED_229: [u8; 8] = [2, 11, 13, 15, 17, 18, 20, 27];
const WEBSOCKET_ALLOWED: [u8; 9] = [1, 2, 3, 4, 5, 6, 11, 13, 27];
const REPEATER_TAB_ALLOWED_228: [u8; 2] = [2, 4];
const REPEATER_TAB_ALLOWED_229: [u8; 6] = [0, 1, 2, 3, 4, 32];
const REPEATER_GROUP_ALLOWED_229: [u8; 3] = [0, 1, 2];
const REPEATER_PAIR_ALLOWED_228: [u8; 2] = [2, 3];
const REPEATER_PAIR_ALLOWED_229: [u8; 3] = [0, 2, 3];

#[derive(Default)]
struct BucketStats {
    non_null_slots: u64,
    mapped_items: u64,
    unmapped_slots: u64,
    types: BTreeMap<(u8, u8), u64>,
    mapped_fields: BTreeSet<u8>,
    unmapped_fields: BTreeSet<u8>,
    unknown_subtypes: BTreeMap<(String, u8, u8), u64>,
}

impl ProjectStore {
    pub fn ext_coverage_report(&self, source_id: Uuid) -> Result<ExtCoverageReport> {
        if let Some(direct) = self.read_direct_ext_source()?
            && direct.id == source_id
        {
            let transaction = self.connection.unchecked_transaction()?;
            let workspace = crate::store::metadata::workspace(&transaction)?;
            transaction.commit()?;
            let mut coverage = direct.report.coverage_report();
            let scope_rules = (workspace.scope.include.len()
                + workspace.scope.exclude.len()
                + workspace.scope.advanced_include.len()
                + workspace.scope.advanced_exclude.len()) as u64;
            if let Some(http_cov) = coverage
                .categories
                .iter()
                .find(|c| c.category == ExtCategory::ProxyHttpHistory)
                .cloned()
                && let Some(site_map) = coverage
                    .categories
                    .iter_mut()
                    .find(|c| c.category == ExtCategory::TargetSiteMapAndScope)
            {
                site_map.non_null_slots = http_cov.non_null_slots;
                site_map.recovered_items = http_cov.recovered_items + scope_rules;
                site_map.unmapped_slots = http_cov.unmapped_slots;
                site_map.observed_types = http_cov.observed_types;
                if scope_rules > 0 {
                    site_map.state = "mapped".into();
                } else {
                    site_map.state = http_cov.state;
                }
            }
            return Ok(coverage);
        }
        let transaction = self.connection.unchecked_transaction()?;
        ensure!(
            source_state(&transaction, source_id)?.complete,
            "External 源文件导入尚未完成"
        );
        let report = read_report(&transaction, source_id)?;
        let workspace = crate::store::metadata::workspace(&transaction)?;
        let is_229 = (report.schema_floor, report.schema_revision) == (229, 229);
        let mut coverage = report.coverage_report();
        let mut buckets: BTreeMap<ExtCategory, BucketStats> = BTreeMap::new();

        {
            let mut query = transaction.prepare(
                "SELECT CASE WHEN typeof(collection)='text' AND length(CAST(collection AS BLOB)) BETWEEN 1 AND ?2 THEN collection END,
                CASE WHEN typeof(state)='text' AND length(CAST(state AS BLOB)) BETWEEN 1 AND ?2 THEN state END,
                typeof(metadata),
                CASE WHEN typeof(metadata)='blob' THEN length(metadata) END,
                CASE WHEN typeof(metadata)='blob' THEN CASE WHEN length(metadata)<=?3 THEN metadata END END
                FROM ext_items WHERE source=?1",
            )?;
            let mut rows = query.query(params![
                source_id.to_string(),
                MAX_COLLECTION_BYTES as i64,
                MAX_METADATA_BYTES as i64,
            ])?;
            while let Some(row) = rows.next()? {
                let collection: String = row.get(0)?;
                let state: String = row.get(1)?;
                let metadata_bytes = read_blob(row, 2, MAX_METADATA_BYTES, "外部归档条目元数据")?;
                let metadata: Value = serde_json::from_slice(&metadata_bytes)?;
                let (target, allowed) = classify_collection(&collection, is_229);
                let stats = buckets.entry(target).or_default();
                accumulate_item(stats, &collection, &state, &metadata, allowed);
            }
        }
        transaction.commit()?;

        let mut total_unmapped_slots = 0u64;
        let mut global_unmapped_fields = BTreeSet::new();
        let mut global_unknown_types: BTreeMap<(u8, u8), u64> = BTreeMap::new();

        for entry in &mut coverage.categories {
            if let Some(stats) = buckets.get(&entry.category) {
                entry.non_null_slots = stats.non_null_slots;
                entry.recovered_items = stats.mapped_items;
                entry.unmapped_slots = stats.unmapped_slots;
                entry.observed_types = stats
                    .types
                    .iter()
                    .map(|(&(object_type, subtype), &count)| ExtObjectTypeCount {
                        object_type,
                        subtype,
                        count,
                    })
                    .collect();
                if !stats.mapped_fields.is_empty() {
                    entry.mapped_fields = stats
                        .mapped_fields
                        .iter()
                        .map(|id| format!("f{id}"))
                        .collect();
                }
                for &id in &stats.unmapped_fields {
                    let name = format!("f{id}");
                    if !entry.unmapped_fields.contains(&name) {
                        entry.unmapped_fields.push(name.clone());
                    }
                    global_unmapped_fields.insert(name);
                }
                for ((path, obj_type, subtype), count) in &stats.unknown_subtypes {
                    *global_unknown_types
                        .entry((*obj_type, *subtype))
                        .or_default() += *count;
                    let detail = format!(
                        "{path} 含 {count} 个未映射对象（type={obj_type}, subtype={subtype}）"
                    );
                    if !entry.unmapped_reasons.contains(&detail) {
                        entry.unmapped_reasons.push(detail);
                    }
                }
                if entry.total_slots == Some(0) && stats.non_null_slots == 0 {
                    entry.state = "empty".into();
                } else if stats.mapped_items > 0 && stats.unmapped_slots == 0 {
                    entry.state = "mapped".into();
                } else if stats.mapped_items > 0 {
                    entry.state = "partial".into();
                } else if stats.unmapped_slots > 0 {
                    entry.state = "unmapped".into();
                }
                total_unmapped_slots = total_unmapped_slots.saturating_add(stats.unmapped_slots);
            }
        }

        let scope_rules = (workspace.scope.include.len()
            + workspace.scope.exclude.len()
            + workspace.scope.advanced_include.len()
            + workspace.scope.advanced_exclude.len()) as u64;
        if let Some(http_cov) = coverage
            .categories
            .iter()
            .find(|c| c.category == ExtCategory::ProxyHttpHistory)
            .cloned()
            && let Some(site_map) = coverage
                .categories
                .iter_mut()
                .find(|c| c.category == ExtCategory::TargetSiteMapAndScope)
        {
            site_map.non_null_slots = http_cov.non_null_slots;
            site_map.recovered_items = http_cov.recovered_items + scope_rules;
            site_map.unmapped_slots = http_cov.unmapped_slots;
            site_map.observed_types = http_cov.observed_types;
            if scope_rules > 0 {
                site_map.state = "mapped".into();
            } else {
                site_map.state = http_cov.state;
            }
        }

        if let Some(unmapped) = coverage
            .categories
            .iter_mut()
            .find(|c| c.category == ExtCategory::UnmappedSlots)
        {
            unmapped.unmapped_slots = unmapped.unmapped_slots.max(total_unmapped_slots);
            for field in global_unmapped_fields {
                if !unmapped.unmapped_fields.contains(&field) {
                    unmapped.unmapped_fields.push(field);
                }
            }
            unmapped.observed_types = global_unknown_types
                .into_iter()
                .map(|((object_type, subtype), count)| ExtObjectTypeCount {
                    object_type,
                    subtype,
                    count,
                })
                .collect();
            if unmapped.unmapped_slots > 0 {
                unmapped.state = "unmapped".into();
            }
        }

        Ok(coverage)
    }
}

fn classify_collection(path: &str, is_229: bool) -> (ExtCategory, &'static [u8]) {
    match path {
        "root.f1.f0" | "root.f1.f1" => (
            ExtCategory::ProxyHttpHistory,
            if is_229 {
                &HTTP_ALLOWED_229
            } else {
                &HTTP_ALLOWED_228
            },
        ),
        "root.f1.f2" | "root.f1.f3" => (ExtCategory::ProxyWebSocketHistory, &WEBSOCKET_ALLOWED),
        "root.f2.f1" => (
            ExtCategory::RepeaterWorkspaceAndHistory,
            if is_229 {
                &REPEATER_TAB_ALLOWED_229
            } else {
                &REPEATER_TAB_ALLOWED_228
            },
        ),
        "root.f2.f4" => (
            ExtCategory::RepeaterWorkspaceAndHistory,
            if is_229 {
                &REPEATER_GROUP_ALLOWED_229
            } else {
                &[]
            },
        ),
        other if other.starts_with("root.f2.f1[") && other.ends_with("].f2") => (
            ExtCategory::RepeaterWorkspaceAndHistory,
            if is_229 {
                &REPEATER_PAIR_ALLOWED_229
            } else {
                &REPEATER_PAIR_ALLOWED_228
            },
        ),
        _ => (ExtCategory::UnmappedSlots, &[]),
    }
}

fn accumulate_item(
    stats: &mut BucketStats,
    collection: &str,
    state: &str,
    metadata: &Value,
    allowed_fields: &[u8],
) {
    if matches!(state, "null" | "missing") {
        return;
    }
    stats.non_null_slots = stats.non_null_slots.saturating_add(1);
    if state == "mapped" {
        stats.mapped_items = stats.mapped_items.saturating_add(1);
    } else {
        stats.unmapped_slots = stats.unmapped_slots.saturating_add(1);
    }
    let object = &metadata["record"]["object"];
    if let (Some(obj_type), Some(subtype)) = (
        object["object_type"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok()),
        object["subtype"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok()),
    ) {
        *stats.types.entry((obj_type, subtype)).or_default() += 1;
        if state != "mapped" {
            *stats
                .unknown_subtypes
                .entry((collection.to_owned(), obj_type, subtype))
                .or_default() += 1;
        }
    }
    if let Some(descriptors) = object["descriptors"].as_array() {
        for descriptor in descriptors {
            let Some(field_id) = descriptor["id"].as_u64().and_then(|v| u8::try_from(v).ok())
            else {
                continue;
            };
            if state == "mapped" && allowed_fields.contains(&field_id) {
                stats.mapped_fields.insert(field_id);
            } else {
                stats.unmapped_fields.insert(field_id);
            }
        }
    }
}
