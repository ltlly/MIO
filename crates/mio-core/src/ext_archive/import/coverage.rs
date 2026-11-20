use crate::{ExtCategory, ExtCategoryCoverage, ExtCollectionReport, ExtImportReport};

use super::project_config::EMBEDDED_CONFIG_PREFIX;

const HTTP_HISTORY_PATHS: [&str; 2] = ["root.f1.f0", "root.f1.f1"];
const WEBSOCKET_HISTORY_PATHS: [&str; 2] = ["root.f1.f2", "root.f1.f3"];
const REPEATER_TAB_PATH: &str = "root.f2.f1";
const REPEATER_GROUP_PATH: &str = "root.f2.f4";
const MAPPED_HTTP_FIELDS_228: [&str; 2] = ["f15", "f18"];
const MAPPED_HTTP_FIELDS_229: [&str; 8] = ["f2", "f11", "f13", "f15", "f17", "f18", "f20", "f27"];
const MAPPED_WEBSOCKET_FIELDS: [&str; 9] =
    ["f1", "f2", "f3", "f4", "f5", "f6", "f11", "f13", "f27"];
const RESERVED_GAP_HINTS: [&str; 3] = [
    "完整源归档不等于完整语义兼容",
    "不自动执行项目插件或连接原目标",
    "完整源归档保留这些内容",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotClass {
    Present,
    Null,
    Missing,
    Unavailable,
    Failed,
}

fn slot_class(state: &str) -> SlotClass {
    match state {
        "available" | "mapped" => SlotClass::Present,
        "empty" | "null" => SlotClass::Null,
        "missing" => SlotClass::Missing,
        "unmapped" => SlotClass::Present,
        "unsupported" | "failed" => SlotClass::Failed,
        _ => SlotClass::Unavailable,
    }
}

fn new_coverage(category: ExtCategory, collection_paths: Vec<String>) -> ExtCategoryCoverage {
    ExtCategoryCoverage {
        category,
        label: category.label().to_owned(),
        collection_paths,
        total_slots: None,
        non_null_slots: 0,
        recovered_items: 0,
        recovered_messages: 0,
        unmapped_slots: 0,
        observed_types: Vec::new(),
        mapped_fields: Vec::new(),
        unmapped_fields: Vec::new(),
        state: "empty".into(),
        unmapped_reasons: Vec::new(),
    }
}

fn extend_collection(coverage: &mut ExtCategoryCoverage, collection: &ExtCollectionReport) {
    if !coverage
        .collection_paths
        .iter()
        .any(|existing| existing == &collection.path)
    {
        coverage.collection_paths.push(collection.path.clone());
    }
    let class = slot_class(&collection.state);
    if let Some(logical_items) = collection.logical_items {
        let slots = coverage.total_slots.get_or_insert(0);
        *slots = slots.saturating_add(logical_items);
    }
    let logical = collection.logical_items.unwrap_or(0);
    match class {
        SlotClass::Null | SlotClass::Missing => {}
        SlotClass::Present => {
            coverage.non_null_slots = coverage.non_null_slots.saturating_add(logical);
            coverage.recovered_items = coverage
                .recovered_items
                .saturating_add(collection.indexed_items);
            coverage.recovered_messages = coverage
                .recovered_messages
                .saturating_add(collection.message_count);
        }
        SlotClass::Failed => {
            coverage.unmapped_slots = coverage.unmapped_slots.saturating_add(logical.max(1));
        }
        SlotClass::Unavailable => {
            coverage.unmapped_slots = coverage.unmapped_slots.saturating_add(1);
        }
    }
    if collection.state != "empty" && collection.state != "missing" {
        coverage.state = if coverage.state == "empty" {
            collection.state.clone()
        } else if coverage.state != collection.state {
            "mixed".into()
        } else {
            collection.state.clone()
        };
    }
    let reason = match collection.state.as_str() {
        "missing" | "null" | "empty" | "available" | "mapped" => None,
        "unmapped" => Some("对象保留紧凑描述符，业务语义未映射".into()),
        "unsupported" => Some("对象类型或布局不受支持".into()),
        "failed" => Some("槽位或对象读取失败；完整原始字节保留".into()),
        "unavailable" => Some("父对象不可解释，集合未映射".into()),
        _ => Some("集合状态不可解释".into()),
    };
    if let Some(reason) = reason
        && !coverage.unmapped_reasons.contains(&reason)
    {
        coverage.unmapped_reasons.push(reason);
    }
}

fn reason_contains(reason: &str, hints: &[&str]) -> bool {
    hints.iter().any(|hint| reason.contains(hint))
}

fn finish(coverage: &mut ExtCategoryCoverage, mapped_fields: &[&str]) {
    for field in mapped_fields {
        if !coverage
            .mapped_fields
            .iter()
            .any(|existing| existing == field)
        {
            coverage.mapped_fields.push((*field).into());
        }
    }
    if coverage.state == "empty" && coverage.non_null_slots == 0 {
        coverage.unmapped_fields.clear();
    }
}

pub(crate) fn category_coverage(report: &ExtImportReport) -> Vec<ExtCategoryCoverage> {
    let is_228 = (report.schema_floor, report.schema_revision) == (228, 228);
    let is_229 = (report.schema_floor, report.schema_revision) == (229, 229);
    let mut coverage = ExtCategory::ALL.map(|category| {
        let paths = match category {
            ExtCategory::ProxyHttpHistory => HTTP_HISTORY_PATHS
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
            ExtCategory::ProxyWebSocketHistory => WEBSOCKET_HISTORY_PATHS
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
            ExtCategory::RepeaterWorkspaceAndHistory => {
                vec![REPEATER_TAB_PATH.to_owned(), REPEATER_GROUP_PATH.to_owned()]
            }
            ExtCategory::EmbeddedProjectConfig => {
                vec!["root.f4.f0".into(), "root.f4.f2.f1".into()]
            }
            _ => Vec::new(),
        };
        new_coverage(category, paths)
    });
    let [http, websocket, site_map, repeater, embedded, unmapped] = &mut coverage;

    for collection in &report.collections {
        let is_history =
            collection.path.starts_with(REPEATER_TAB_PATH) && collection.path != REPEATER_TAB_PATH;
        if HTTP_HISTORY_PATHS.contains(&collection.path.as_str()) {
            extend_collection(http, collection);
            continue;
        }
        if WEBSOCKET_HISTORY_PATHS.contains(&collection.path.as_str()) {
            extend_collection(websocket, collection);
            continue;
        }
        if collection.path == REPEATER_TAB_PATH
            || collection.path == REPEATER_GROUP_PATH
            || is_history
        {
            extend_collection(repeater, collection);
            continue;
        }
        extend_collection(unmapped, collection);
    }

    if is_228 {
        finish(http, &MAPPED_HTTP_FIELDS_228);
        http.unmapped_fields = vec!["f16".into(), "f17".into(), "f19".into(), "f20".into()];
    } else {
        finish(http, &MAPPED_HTTP_FIELDS_229);
        http.unmapped_fields = vec!["f16".into(), "f19".into()];
    }
    finish(websocket, &MAPPED_WEBSOCKET_FIELDS);
    for gap in &report.gaps {
        if gap.contains("WebSocket") && !websocket.unmapped_reasons.contains(gap) {
            websocket.unmapped_reasons.push(gap.clone());
        }
        if gap.contains("HTTP") && gap.len() <= 160 && !http.unmapped_reasons.contains(gap) {
            http.unmapped_reasons.push(gap.clone());
        }
    }

    site_map.collection_paths = http.collection_paths.clone();
    site_map.total_slots = http.total_slots;
    site_map.non_null_slots = http.non_null_slots;
    site_map.recovered_items = http.recovered_items;
    site_map.recovered_messages = http.recovered_messages;
    site_map.observed_types = http.observed_types.clone();
    site_map.state = http.state.clone();
    site_map.unmapped_fields = http.unmapped_fields.clone();
    site_map.mapped_fields = if is_228 {
        vec!["origin".into(), "url".into()]
    } else {
        vec![
            "origin".into(),
            "url".into(),
            "scope".into(),
            "time".into(),
            "highlight".into(),
            "notes".into(),
        ]
    };
    site_map.unmapped_reasons.push(
        "Site Map 由已验证 HTTP 请求源条目派生；原生 External 二进制站图树节点与被动问题未映射。"
            .into(),
    );

    if report.repeater_active_tab.is_some() {
        repeater.mapped_fields.push("active_tab".into());
    } else if is_229 {
        repeater
            .unmapped_reasons
            .push("root.f2.f0 活动标签未映射或越界".into());
    }
    if !is_229 {
        repeater
            .unmapped_reasons
            .push("schema 228 分组与标题布局未验证".into());
    }
    finish(
        repeater,
        if is_229 {
            &["f0", "f1", "f2", "f3", "f4", "f32", "f33"]
        } else {
            &["f2", "f4"]
        },
    );
    for gap in &report.gaps {
        if (gap.starts_with("Repeater ") || gap.contains("Repeater 标签"))
            && !repeater.unmapped_reasons.contains(gap)
        {
            repeater.unmapped_reasons.push(gap.clone());
        }
    }

    let mut missing_embedded = true;
    for gap in &report.gaps {
        if gap.contains(EMBEDDED_CONFIG_PREFIX) {
            missing_embedded = false;
            if gap.contains("已提取") {
                embedded.recovered_items = embedded.recovered_items.saturating_add(1);
                embedded.non_null_slots = embedded.non_null_slots.saturating_add(1);
                embedded.state = "extracted".into();
            } else if gap.contains("failed:") {
                embedded.unmapped_slots = embedded.unmapped_slots.saturating_add(1);
                embedded.state = "failed".into();
            } else if gap.contains("null") {
                embedded.state = "null".into();
            }
            if !embedded.unmapped_reasons.contains(gap) {
                embedded.unmapped_reasons.push(gap.clone());
            }
        }
        if gap.starts_with("root.f4.f2.f1:") && !embedded.unmapped_reasons.contains(gap) {
            embedded.unmapped_reasons.push(gap.clone());
        }
    }
    if is_229
        && !report
            .gaps
            .iter()
            .any(|gap| gap.starts_with("root.f4.f2.f1:"))
    {
        embedded.recovered_items = embedded.recovered_items.saturating_add(1);
        embedded.non_null_slots = embedded.non_null_slots.saturating_add(1);
        if embedded.state == "empty" {
            embedded.state = "available".into();
        }
    }
    if missing_embedded && is_229 {
        embedded
            .unmapped_reasons
            .push("root.f4.f0 内嵌 JSON 配置槽位未发现；未猜测或改写其它未映射设置。".into());
    }
    finish(
        embedded,
        if is_229 {
            &["root.f4.f0", "root.f4.f2.f1"]
        } else {
            &["root.f4.f0"]
        },
    );

    unmapped.unmapped_slots = unmapped
        .unmapped_slots
        .saturating_add(http.unmapped_slots)
        .saturating_add(websocket.unmapped_slots)
        .saturating_add(repeater.unmapped_slots)
        .saturating_add(embedded.unmapped_slots);
    if unmapped.unmapped_slots > 0 {
        unmapped.state = "partial".into();
    }
    for gap in &report.gaps {
        if reason_contains(gap, &RESERVED_GAP_HINTS)
            || gap.len() > 200
            || unmapped.unmapped_reasons.contains(gap)
        {
            continue;
        }
        unmapped.unmapped_reasons.push(gap.clone());
    }
    finish(unmapped, &[]);
    coverage.into()
}
