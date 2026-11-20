use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator};
use gpui_kit::component::{
    Icon, Selectable, Sizable,
    button::{ButtonRounded, ButtonVariants},
    input::{Editor, Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarHandle, ScrollbarMode},
};
use gpui_kit::{
    AnyElement, Context, DragMoveEvent, Entity, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, StatefulInteractiveElement, Styled, TestSupportExt, UniformListScrollHandle,
    WeakEntity, Window, div, px, rgb,
};
use mio_core::{
    AdvancedScopeRule, ExtResponseRef, FlowRecord, HistoryEntryId, HistoryRow,
    HistoryTransferEntry, HttpHistoryRowMetadata, OriginalMessages, ScopeProtocol, ScopeRule,
    SiteMapEntryDetail, SiteMapEntryInput, SiteMapEntryReceipt, SiteMapFilter, SiteMapNode,
    SiteMapNodeKind, SiteMapQuery, SiteMapSnapshot, TargetScope, Uuid,
    audit_engine::AuditFinding,
    export_target_scope_json as core_export_target_scope_json, http_history_path_and_query,
    parse_target_scope_json as core_parse_target_scope_json,
    target_issues::{IssueDefinition, IssueSeverity, filter_issue_definitions, issue_definitions},
};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{SendToScanner, SendToSequencer};

type MioIssueDefinition = IssueDefinition;
type MioIssueSeverity = IssueSeverity;

pub(in crate::workbench) const SITEMAP_COL_HOST: f32 = 140.;
pub(in crate::workbench) const SITEMAP_COL_METHOD: f32 = 58.;
pub(in crate::workbench) const SITEMAP_COL_URL_MIN: f32 = 235.;
pub(in crate::workbench) const SITEMAP_COL_PARAMS: f32 = 60.;
pub(in crate::workbench) const SITEMAP_COL_STATUS: f32 = 88.;
pub(in crate::workbench) const SITEMAP_COL_LENGTH: f32 = 62.;
pub(in crate::workbench) const SITEMAP_COL_MIME: f32 = 80.;
pub(in crate::workbench) const SITEMAP_COL_TITLE: f32 = 136.;
pub(in crate::workbench) const SITEMAP_COL_NOTES: f32 = 185.;
pub(in crate::workbench) const SITEMAP_COL_COUNT: usize = 9;
pub(in crate::workbench) const SITEMAP_DEFAULT_COL_WIDTHS: [f32; SITEMAP_COL_COUNT] = [
    SITEMAP_COL_HOST,
    SITEMAP_COL_METHOD,
    SITEMAP_COL_URL_MIN,
    SITEMAP_COL_PARAMS,
    SITEMAP_COL_STATUS,
    SITEMAP_COL_LENGTH,
    SITEMAP_COL_MIME,
    SITEMAP_COL_TITLE,
    SITEMAP_COL_NOTES,
];
pub(in crate::workbench) const SITEMAP_COL_NAMES: [&str; SITEMAP_COL_COUNT] = [
    "Host",
    "Method",
    "URL",
    "Params",
    "Status code",
    "Length",
    "MIME type",
    "Title",
    "Notes",
];

#[derive(Clone, Copy)]
pub(in crate::workbench) struct ResizeTargetCol {
    pub(in crate::workbench) entity_id: gpui_kit::EntityId,
    pub(in crate::workbench) col_ix: usize,
}

impl gpui_kit::Render for ResizeTargetCol {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum SiteMapSubView {
    #[default]
    Contents,
    Issues,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum SiteMapViewMode {
    #[default]
    UrlView,
    CrawlPathsView,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum SiteMapLayoutMode {
    #[default]
    TreeLeftStackedRight,
    ThreeColumn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum SiteMapSortColumn {
    Host,
    Method,
    Url,
    Params,
    Status,
    Length,
    Mime,
    Title,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct SiteMapSort {
    pub(in crate::workbench) column: SiteMapSortColumn,
    pub(in crate::workbench) ascending: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum IssueSortColumn {
    Name,
    Severity,
    #[default]
    TypeIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum BranchItemAction {
    OpenScanLauncher,
    ActiveScan,
    PassiveScan,
    DiscoverContent,
    SimulateManualTesting,
    SendToRepeater,
    SendToIntruder,
}

pub(in crate::workbench) fn table_header_bg() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xf1f1f1 }).into()
}

pub(in crate::workbench) fn table_header_fg() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0xd4d4d4 } else { 0x141414 }).into()
}

pub(in crate::workbench) fn table_stripe_bg() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xfafafa }).into()
}

pub(in crate::workbench) fn selected_row_border() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x5b9bd5 } else { 0x4a89c7 }).into()
}

pub(in crate::workbench) fn mio_vertical_scrollbar_track<H: ScrollbarHandle + Clone>(
    id: impl Into<gpui_kit::ElementId>,
    handle: &H,
) -> AnyElement {
    let dark = theme::is_dark();
    let track_bg: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xf7f7f7 }).into();
    let thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5c626b } else { 0xc8c8c8 }).into();
    let thumb_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x6e7580 } else { 0xb0b0b0 }).into();
    div()
        .id(id)
        .relative()
        .w(px(12.))
        .h_full()
        .flex_shrink_0()
        .border_l_1()
        .border_color(rgb(if dark { 0x3c3f41 } else { 0xe0e0e0 }))
        .bg(track_bg)
        .child(
            Scrollbar::vertical(handle)
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .styles(move |styles| {
                    styles
                        .track(|s| s.width(px(11.)).bg(track_bg))
                        .track_hover(|s| s.width(px(11.)).bg(track_bg))
                        .track_active(|s| s.width(px(11.)).bg(track_bg))
                        .thumb(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(34.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(34.))
                                .bg(thumb_hover_bg)
                        })
                        .thumb_active(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(34.))
                                .bg(thumb_hover_bg)
                        })
                }),
        )
        .into_any_element()
}

pub(in crate::workbench) fn mio_horizontal_scrollbar_track<H: ScrollbarHandle + Clone>(
    id: impl Into<gpui_kit::ElementId>,
    handle: &H,
) -> AnyElement {
    let dark = theme::is_dark();
    let track_bg: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xf7f7f7 }).into();
    let thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5c626b } else { 0xc8c8c8 }).into();
    let thumb_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x6e7580 } else { 0xb0b0b0 }).into();
    div()
        .id(id)
        .relative()
        .h(px(12.))
        .flex_1()
        .min_w_0()
        .border_t_1()
        .border_color(rgb(if dark { 0x3c3f41 } else { 0xe0e0e0 }))
        .bg(track_bg)
        .child(
            Scrollbar::horizontal(handle)
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .styles(move |styles| {
                    styles
                        .track(|s| s.width(px(11.)).bg(track_bg))
                        .track_hover(|s| s.width(px(11.)).bg(track_bg))
                        .track_active(|s| s.width(px(11.)).bg(track_bg))
                        .thumb(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(36.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(36.))
                                .bg(thumb_hover_bg)
                        })
                        .thumb_active(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(36.))
                                .bg(thumb_hover_bg)
                        })
                }),
        )
        .into_any_element()
}

pub(in crate::workbench) fn mio_scope_table_triangle() -> AnyElement {
    let fg: gpui_kit::Hsla = if theme::is_dark() {
        theme::foreground()
    } else {
        rgb(0x111111).into()
    };
    div()
        .h(theme::scaled_px(170.))
        .pl(px(2.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(
            Icon::default()
                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 8 10\" fill=\"currentColor\"><polygon points=\"1,1 7,5 1,9\"/></svg>")
                .w(px(7.))
                .h(px(10.))
                .text_color(fg),
        )
        .into_any_element()
}

pub(in crate::workbench) fn site_map_row_passes_ui_filters(
    row: &HistoryRow,
    loaded_response: Option<&[u8]>,
    meta: Option<&HttpHistoryRowMetadata>,
    hide_not_found: bool,
    status_classes: [bool; 4],
    hide_css_image_binary: bool,
) -> bool {
    let status = row_status_code(row);
    if hide_not_found && (status.is_none() || status == Some(404)) {
        return false;
    }
    if let Some(code) = status {
        let allowed = match code {
            200..=299 => status_classes[0],
            300..=399 => status_classes[1],
            400..=499 => status_classes[2],
            500..=599 => status_classes[3],
            _ => true,
        };
        if !allowed {
            return false;
        }
    }
    if hide_css_image_binary {
        let known_mime = meta
            .map(|m| m.mime_type.trim())
            .filter(|m| !m.is_empty())
            .map(crate::history::format_history_mime_display)
            .or_else(|| {
                loaded_response
                    .filter(|b| !b.is_empty())
                    .map(|_| target_row_summary_with_metadata(row, loaded_response, meta).mime)
            })
            .or_else(|| mime::mime_from_url_extension(&row_url(row)).map(str::to_owned));
        if let Some(mime) = known_mime {
            let m = mime.trim();
            if m.eq_ignore_ascii_case("CSS")
                || m.eq_ignore_ascii_case("Image")
                || m.eq_ignore_ascii_case("Flash")
                || m.eq_ignore_ascii_case("Other binary")
                || m.eq_ignore_ascii_case("Binary")
                || m.eq_ignore_ascii_case("Data")
            {
                return false;
            }
        }
    }
    true
}

pub(in crate::workbench) fn mio_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Checkbox {
    let label = label.into();
    let has_label = !label.is_empty();
    let mut cb = Checkbox::new(id)
        .checked(checked)
        .disabled(!enabled)
        .tab_stop(false)
        .flex()
        .items_center()
        .gap(px(7.))
        .px(px(2.))
        .py(px(2.))
        .rounded(px(2.))
        .focus_visible(|style| style.bg(theme::selected()))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(if checked {
                    theme::accent()
                } else if theme::is_dark() {
                    rgb(0x6b717a).into()
                } else {
                    rgb(0xb5b5b5).into()
                })
                .bg(if checked {
                    theme::accent()
                } else {
                    theme::surface()
                })
                .rounded(px(3.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(px(11.))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        );
    if has_label {
        cb = cb.accessibility_label(label.clone()).child(label);
    }
    cb
}

pub(in crate::workbench) fn mio_neutral_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    width: Option<f32>,
) -> Button {
    let dark = theme::is_dark();
    let label = label.into();
    let mut btn = Button::new(id)
        .ghost()
        .small()
        .rounded(ButtonRounded::Size(px(5.)))
        .h(theme::scaled_px(26.).max(px(24.)))
        .px(theme::scaled_px(10.))
        .border_1()
        .border_color(rgb(if dark { 0x4e535b } else { 0xd0d0d0 }))
        .bg(rgb(if dark { 0x35383d } else { 0xececec }))
        .text_color(if dark {
            theme::foreground()
        } else {
            rgb(0x1f1f1f).into()
        })
        .accessibility_label(label.clone())
        .child(
            div()
                .text_size(theme::ui_font_size())
                .font_weight(gpui_kit::FontWeight::BOLD)
                .child(label),
        );
    if let Some(w) = width {
        btn = btn.w(theme::scaled_px(w)).px_0();
    }
    btn
}

pub(in crate::workbench) fn sort_issue_definitions(
    mut list: Vec<&'static MioIssueDefinition>,
    column: IssueSortColumn,
    ascending: bool,
) -> Vec<&'static MioIssueDefinition> {
    list.sort_by(|left, right| {
        let primary = match column {
            IssueSortColumn::Name => left
                .name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase()),
            IssueSortColumn::Severity => left
                .typical_severity
                .rank()
                .cmp(&right.typical_severity.rank()),
            IssueSortColumn::TypeIndex => left.id.cmp(right.id),
        };
        let ordered = if ascending { primary } else { primary.reverse() };
        ordered.then_with(|| left.id.cmp(right.id))
    });
    list
}

pub(in crate::workbench) fn default_first_issue_definition() -> Option<&'static MioIssueDefinition> {
    sort_issue_definitions(
        issue_definitions().iter().collect(),
        IssueSortColumn::TypeIndex,
        true,
    )
    .into_iter()
    .next()
}

pub(in crate::workbench) fn canonical_site_map_url_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (scheme_raw, after_scheme) = trimmed.split_once("://")?;
    let scheme = scheme_raw.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let without_frag = after_scheme.split('#').next().unwrap_or(after_scheme);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let (authority, path_raw) = match without_query.split_once('/') {
        Some((a, p)) => (a, p),
        None => (without_query, ""),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let default_port = if scheme == "https" { 443u16 } else { 80u16 };
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (ipv6, tail) = rest.split_once(']')?;
        let port = if let Some(p) = tail.strip_prefix(':') {
            p.parse::<u16>().ok()?
        } else if tail.is_empty() {
            default_port
        } else {
            return None;
        };
        (format!("[{}]", ipv6.to_ascii_lowercase()), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        (h.to_ascii_lowercase(), p.parse::<u16>().ok()?)
    } else {
        (authority.to_ascii_lowercase(), default_port)
    };
    if host.is_empty() {
        return None;
    }
    let segments: Vec<&str> = path_raw.split('/').filter(|seg| !seg.is_empty()).collect();
    if segments.is_empty() {
        Some(format!("{scheme}://{host}:{port}"))
    } else {
        Some(format!("{scheme}://{host}:{port}/{}", segments.join("/")))
    }
}

pub(in crate::workbench) fn url_matches_node_prefix(url: &str, prefix_or_key: &str) -> bool {
    let prefix_trimmed = prefix_or_key.trim().trim_end_matches('/');
    if prefix_trimmed.is_empty() {
        return false;
    }
    if let (Some(url_key), Some(prefix_key)) = (
        canonical_site_map_url_key(url),
        canonical_site_map_url_key(prefix_trimmed),
    ) {
        return url_key == prefix_key
            || (url_key.len() > prefix_key.len()
                && url_key.starts_with(&prefix_key)
                && url_key.as_bytes()[prefix_key.len()] == b'/');
    }
    let url_trimmed = url.trim();
    url_trimmed == prefix_trimmed
        || url_trimmed
            .strip_prefix(prefix_trimmed)
            .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('?'))
}

pub(in crate::workbench) fn is_url_hidden(url: &str, hidden_prefixes: &[String]) -> bool {
    hidden_prefixes
        .iter()
        .any(|hidden| url_matches_node_prefix(url, hidden))
}

pub(in crate::workbench) fn row_url(row: &HistoryRow) -> String {
    match row {
        HistoryRow::Live(summary) => summary.url.clone(),
        HistoryRow::ExtProxy(archive) => {
            let req = archive.edited_request.as_ref().or(archive.request.as_ref());
            req.and_then(|request| request.request_target.clone())
                .map(|target| {
                    if target.starts_with('/') {
                        format!("{}{target}", archive.origin.as_deref().unwrap_or_default())
                    } else {
                        target
                    }
                })
                .unwrap_or_default()
        }
    }
}

pub(in crate::workbench) fn row_status_code(row: &HistoryRow) -> Option<u16> {
    match row {
        HistoryRow::Live(summary) => summary.status,
        HistoryRow::ExtProxy(archive) => archive
            .edited_response
            .as_ref()
            .or(archive.response.as_ref())
            .and_then(|resp| resp.status),
    }
}

pub(in crate::workbench) fn row_response_length(row: &HistoryRow) -> u64 {
    match row {
        HistoryRow::Live(summary) => summary.response_bytes,
        HistoryRow::ExtProxy(archive) => archive
            .edited_response
            .as_ref()
            .or(archive.response.as_ref())
            .map_or(0, |resp| resp.raw_bytes),
    }
}

#[allow(dead_code)]
pub(in crate::workbench) fn sort_site_map_indices(
    items: &[HistoryRow],
    selected_id: Option<HistoryEntryId>,
    loaded_response: Option<&[u8]>,
    hidden_prefixes: &[String],
    sort: Option<SiteMapSort>,
) -> Vec<usize> {
    sort_site_map_indices_with_metadata(items, selected_id, loaded_response, None, hidden_prefixes, sort)
}

fn cmp_ascii_lowercase(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(b.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn cmp_ascii_uppercase(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|byte| byte.to_ascii_uppercase())
        .cmp(b.bytes().map(|byte| byte.to_ascii_uppercase()))
}

fn compare_precomputed_target_entries(
    left: &(usize, mime::TargetRowSummary, u64, u16),
    right: &(usize, mime::TargetRowSummary, u64, u16),
    sort: SiteMapSort,
) -> std::cmp::Ordering {
    let primary = match sort.column {
        SiteMapSortColumn::Host => cmp_ascii_lowercase(&left.1.host, &right.1.host),
        SiteMapSortColumn::Method => cmp_ascii_uppercase(&left.1.method, &right.1.method),
        SiteMapSortColumn::Url => left.1.url.cmp(&right.1.url),
        SiteMapSortColumn::Params => left.1.params.cmp(&right.1.params),
        SiteMapSortColumn::Status => left.3.cmp(&right.3),
        SiteMapSortColumn::Length => left.2.cmp(&right.2),
        SiteMapSortColumn::Mime => left.1.mime.cmp(&right.1.mime),
        SiteMapSortColumn::Title => cmp_ascii_lowercase(&left.1.title, &right.1.title),
    };
    let ordered = if sort.ascending {
        primary
    } else {
        primary.reverse()
    };
    ordered
        .then_with(|| left.1.url.cmp(&right.1.url))
        .then_with(|| left.0.cmp(&right.0))
}

#[allow(clippy::too_many_arguments)]
pub(in crate::workbench) fn build_sorted_target_rows_with_metadata(
    items: &[HistoryRow],
    selected_id: Option<HistoryEntryId>,
    loaded_response: Option<&[u8]>,
    row_metadata: Option<&HashMap<HistoryEntryId, HttpHistoryRowMetadata>>,
    hidden_prefixes: &[String],
    hidden_item_ids: &HashSet<HistoryEntryId>,
    hide_not_found: bool,
    status_classes: [bool; 4],
    hide_css_image_binary: bool,
    sort: Option<SiteMapSort>,
) -> Vec<(usize, mime::TargetRowSummary)> {
    let has_hidden_prefixes = !hidden_prefixes.is_empty();
    let has_hidden_ids = !hidden_item_ids.is_empty();
    let mut entries: Vec<(usize, mime::TargetRowSummary, u64, u16)> =
        Vec::with_capacity(items.len());
    for (idx, row) in items.iter().enumerate() {
        let row_id = row.id();
        if has_hidden_ids && hidden_item_ids.contains(&row_id) {
            continue;
        }
        if has_hidden_prefixes && is_url_hidden(&row_url(row), hidden_prefixes) {
            continue;
        }
        let sel_resp = loaded_response.filter(|_| selected_id == Some(row_id));
        let meta = row_metadata.and_then(|map| map.get(&row_id));
        if !site_map_row_passes_ui_filters(
            row,
            sel_resp,
            meta,
            hide_not_found,
            status_classes,
            hide_css_image_binary,
        ) {
            continue;
        }
        let summary = target_row_summary_with_metadata(row, sel_resp, meta);
        let len = meta
            .and_then(|m| m.response_length)
            .or_else(|| sel_resp.filter(|b| !b.is_empty()).map(|b| b.len() as u64))
            .unwrap_or_else(|| row_response_length(row));
        let status = row_status_code(row).unwrap_or(0);
        entries.push((idx, summary, len, status));
    }
    if let Some(sort) = sort {
        entries.sort_by(|left, right| compare_precomputed_target_entries(left, right, sort));
    }
    entries
        .into_iter()
        .map(|(idx, summary, _, _)| (idx, summary))
        .collect()
}

pub(in crate::workbench) fn sort_site_map_indices_with_metadata(
    items: &[HistoryRow],
    selected_id: Option<HistoryEntryId>,
    loaded_response: Option<&[u8]>,
    row_metadata: Option<&HashMap<HistoryEntryId, HttpHistoryRowMetadata>>,
    hidden_prefixes: &[String],
    sort: Option<SiteMapSort>,
) -> Vec<usize> {
    let has_hidden_prefixes = !hidden_prefixes.is_empty();
    let Some(sort) = sort else {
        return items
            .iter()
            .enumerate()
            .filter(|(_, row)| !has_hidden_prefixes || !is_url_hidden(&row_url(row), hidden_prefixes))
            .map(|(idx, _)| idx)
            .collect();
    };
    let mut entries: Vec<(usize, mime::TargetRowSummary, u64, u16)> = items
        .iter()
        .enumerate()
        .filter(|(_, row)| !has_hidden_prefixes || !is_url_hidden(&row_url(row), hidden_prefixes))
        .map(|(idx, row)| {
            let row_id = row.id();
            let loaded = loaded_response.filter(|_| selected_id == Some(row_id));
            let meta = row_metadata.and_then(|map| map.get(&row_id));
            let summary = target_row_summary_with_metadata(row, loaded, meta);
            let len = meta
                .and_then(|m| m.response_length)
                .or_else(|| loaded.filter(|b| !b.is_empty()).map(|b| b.len() as u64))
                .unwrap_or_else(|| row_response_length(row));
            let status = row_status_code(row).unwrap_or(0);
            (idx, summary, len, status)
        })
        .collect();
    entries.sort_by(|left, right| compare_precomputed_target_entries(left, right, sort));
    entries.into_iter().map(|(idx, _, _, _)| idx).collect()
}

pub(in crate::workbench) fn collect_branch_urls(
    items: &[HistoryRow],
    nodes: &[SiteMapNode],
    hidden_prefixes: &[String],
    prefix: &str,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for row in items {
        let url = row_url(row);
        if !url.is_empty()
            && !is_url_hidden(&url, hidden_prefixes)
            && url_matches_node_prefix(&url, prefix)
            && seen.insert(url.clone())
        {
            urls.push(url);
        }
    }
    if urls.is_empty() {
        for node in nodes {
            if !node.url_prefix.is_empty()
                && !is_url_hidden(&node.url_prefix, hidden_prefixes)
                && (url_matches_node_prefix(&node.key, prefix)
                    || url_matches_node_prefix(&node.url_prefix, prefix))
                && seen.insert(node.url_prefix.clone())
            {
                urls.push(node.url_prefix.clone());
            }
        }
    }
    urls
}

pub(in crate::workbench) fn filter_branch_findings<'a>(
    findings: &'a [AuditFinding],
    selected_key: Option<&str>,
    hidden_prefixes: &[String],
) -> Vec<&'a AuditFinding> {
    findings
        .iter()
        .filter(|finding| !is_url_hidden(&finding.url, hidden_prefixes))
        .filter(|finding| {
            selected_key.is_none_or(|prefix| url_matches_node_prefix(&finding.url, prefix))
        })
        .collect()
}

pub(in crate::workbench) fn export_target_scope_json(
    scope: &TargetScope,
) -> Result<String, String> {
    core_export_target_scope_json(scope)
}

pub(in crate::workbench) fn parse_target_scope_json(raw: &str) -> Result<TargetScope, String> {
    core_parse_target_scope_json(raw)
}

mod discovery;
mod engagement;
mod menus;
mod mime;
mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;
mod part_06;
mod scope_ops;
mod site_map_ext;

pub(in crate::workbench) use engagement::*;
pub(in crate::workbench) use menus::*;
pub(in crate::workbench) use part_01::*;
pub(in crate::workbench) use part_05::*;
