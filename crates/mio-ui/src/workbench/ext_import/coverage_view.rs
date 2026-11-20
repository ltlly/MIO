use mio_core::{ExtCollectionReport, ExtImportReport};

use super::*;

fn is_proxy_or_repeater_collection(path: &str) -> bool {
    matches!(path, "root.f1.f0" | "root.f1.f1" | "root.f2.f1" | "root.f2.f4")
        || path.starts_with("root.f2.f1[")
}

fn sum_collection_count(
    collections: &[ExtCollectionReport],
    predicate: impl Fn(&str) -> bool,
) -> Option<u64> {
    let mut matched = false;
    let mut sum = 0_u64;
    for col in collections.iter().filter(|col| predicate(&col.path)) {
        matched = true;
        let count = if col.indexed_items > 0 {
            col.indexed_items
        } else {
            col.logical_items.unwrap_or(0)
        };
        sum = sum.saturating_add(count);
    }
    matched.then_some(sum)
}

fn proxy_history_count(collections: &[ExtCollectionReport]) -> Option<u64> {
    let primary = collections.iter().find(|col| col.path == "root.f1.f0");
    let secondary = collections.iter().find(|col| col.path == "root.f1.f1");
    match (primary, secondary) {
        (Some(col), _) | (None, Some(col)) => {
            if col.indexed_items > 0 {
                Some(col.indexed_items)
            } else {
                col.logical_items
            }
        }
        (None, None) => None,
    }
}

pub(super) fn render_report(report: &ExtImportReport, id: &'static str) -> AnyElement {
    let proxy_label = proxy_history_count(&report.collections)
        .map_or_else(|| "未知".to_owned(), |count| format!("{count} 条"));
    let repeater_tabs_label =
        sum_collection_count(&report.collections, |path| path == "root.f2.f1")
            .map_or_else(|| "未知".to_owned(), |count| format!("{count} 个"));
    let repeater_groups_label =
        sum_collection_count(&report.collections, |path| path == "root.f2.f4")
            .map_or_else(|| "0 个".to_owned(), |count| format!("{count} 个"));
    let summary_line = format!(
        "Proxy 历史抓包：{proxy_label} · Repeater 标签：{repeater_tabs_label} · Repeater 分组：{repeater_groups_label}"
    );
    let mut view = div()
        .id(id)
        .test_support()
        .flex()
        .flex_col()
        .gap(px(5.))
        .p(px(10.))
        .border_1()
        .border_color(theme::border())
        .rounded(px(4.))
        .bg(theme::surface())
        .child(format!(
            "存储 schema {}/{} · 源文件 {} 字节",
            report.schema_floor, report.schema_revision, report.source_bytes
        ))
        .child(
            div()
                .id("ext-proxy-repeater-summary")
                .test_support()
                .aria_label(summary_line.clone())
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(summary_line),
        )
        .child(format!(
            "导入条目 {} 个 · 消息 {} 条",
            report.indexed_items, report.indexed_messages
        ));
    for (index, collection) in report
        .collections
        .iter()
        .filter(|collection| is_proxy_or_repeater_collection(&collection.path))
        .enumerate()
    {
        let label = format!(
            "{} · {} · 逻辑位置 {} · 已索引 {} · 消息 {}",
            collection.path,
            collection.state,
            collection
                .logical_items
                .map_or_else(|| "未知".into(), |count| count.to_string()),
            collection.indexed_items,
            collection.message_count,
        );
        view = view.child(
            div()
                .id(("ext-report-row", index))
                .test_support()
                .aria_label(label.clone())
                .child(label),
        );
    }
    view.into_any_element()
}
