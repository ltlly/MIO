mod source;

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::{
    ExtCollectionReport, ExtImportPhase, ExtImportReport, HttpRequest, HttpResponse,
    HttpVersion, TabGroup, Uuid,
    ext_archive::import::check_cancel,
    http::{parse_imported_request, parse_imported_response},
};

use source::ExtSource;

const MAX_LIVE_NOTE_BYTES: usize = 64 * 1024;
const MAX_TITLE_BYTES: usize = 4096;
pub(crate) const MAX_REPEATER_TITLE_UNITS: usize = 4096;
const MAX_WORK_TABS: usize = 512;
const MAX_WORK_GROUPS: usize = 512;
const MAX_DRAFT_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_DRAFT_SINGLE_BYTES: usize = crate::http::MAX_BODY + 256 * 1024;
const MAX_HISTORY_RAW_BYTES: usize = crate::http::MAX_BODY + 256 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct ImportedProxyFlow {
    pub index: u64,
    pub started_unix_ms: i64,
    pub time_known: bool,
    pub elapsed_ms: u64,
    pub response_received_ms: Option<u64>,
    pub listener_port: Option<u16>,
    pub remote_ip: String,
    pub original_request: HttpRequest,
    pub edited_request: Option<HttpRequest>,
    pub original_response: Option<(HttpResponse, Option<Vec<u8>>)>,
    pub edited_response: Option<(HttpResponse, Option<Vec<u8>>)>,
    pub notes: String,
    pub highlight: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct ImportedRepeaterHistoryItem {
    pub raw_request: Vec<u8>,
    pub request: HttpRequest,
    pub target: String,
    pub response: Option<(HttpResponse, Option<Vec<u8>>)>,
    pub started_unix_ms: i64,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ImportedRepeaterTab {
    pub title: String,
    pub group: Option<Uuid>,
    pub draft_target: String,
    pub draft_version: HttpVersion,
    pub draft_raw: Vec<u8>,
    pub history: Vec<ImportedRepeaterHistoryItem>,
    pub selected_history: Option<usize>,
}

pub(crate) fn inspect_report(
    path: &Path,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(ExtImportPhase, u64, Option<u64>),
) -> Result<ExtImportReport> {
    check_cancel(cancel)?;
    let mut src = ExtSource::open(path)?;
    let file_size = src.header.file_size;
    progress(ExtImportPhase::Snapshot, 0, Some(file_size));
    let schema_floor = src.header.schema_floor;
    let schema_revision = src.header.schema_revision;
    let mut collections = Vec::new();
    let mut repeater_active_tab = None;

    if src.header.project_root != 0 {
        let root = src.object(src.header.project_root)?;
        check_cancel(cancel)?;
        if let Some(Some(f1_addr)) = src.opt_ref(&root, 1)?
            && let Ok(f1) = src.object(f1_addr)
        {
            for (fid, col_path) in [(0u8, "root.f1.f0"), (1u8, "root.f1.f1")] {
                collections.push(inspect_collection(&mut src, &f1, fid, col_path)?);
            }
        }
        check_cancel(cancel)?;
        if let Some(Some(f2_addr)) = src.opt_ref(&root, 2)?
            && let Ok(f2) = src.object(f2_addr)
        {
            let tabs_report = inspect_collection(&mut src, &f2, 1, "root.f2.f1")?;
            let tab_len = tabs_report.logical_items;
            collections.push(tabs_report);
            collections.push(inspect_collection(&mut src, &f2, 4, "root.f2.f4")?);
            if let Some(active_raw) = src.opt_i32(&f2, 0)?
                && let Ok(active_u64) = u64::try_from(active_raw)
                && tab_len.is_some_and(|len| active_u64 < len)
            {
                repeater_active_tab = Some(active_u64);
            }
        }
    }

    progress(ExtImportPhase::Snapshot, file_size, Some(file_size));
    Ok(ExtImportReport {
        schema_floor,
        schema_revision,
        source_bytes: file_size,
        indexed_items: 0,
        indexed_messages: 0,
        collections,
        repeater_active_tab,
        gaps: Vec::new(),
        complete_archive: false,
    })
}

fn inspect_collection(
    src: &mut ExtSource,
    parent: &source::RawObject,
    field_id: u8,
    path: &str,
) -> Result<ExtCollectionReport> {
    if !parent.fields.iter().any(|f| f.id == field_id) {
        return Ok(ExtCollectionReport {
            path: path.into(),
            logical_items: None,
            indexed_items: 0,
            message_count: 0,
            state: "missing".into(),
        });
    }
    Ok(match src.opt_ref(parent, field_id)? {
        Some(None) => ExtCollectionReport {
            path: path.into(),
            logical_items: None,
            indexed_items: 0,
            message_count: 0,
            state: "null".into(),
        },
        Some(Some(addr)) => {
            let obj = src.object(addr)?;
            match src.opt_i32(&obj, 0)?.and_then(|n| u64::try_from(n).ok()) {
                Some(len) => ExtCollectionReport {
                    path: path.into(),
                    logical_items: Some(len),
                    indexed_items: 0,
                    message_count: 0,
                    state: if len == 0 {
                        "empty".into()
                    } else {
                        "available".into()
                    },
                },
                None => ExtCollectionReport {
                    path: path.into(),
                    logical_items: None,
                    indexed_items: 0,
                    message_count: 0,
                    state: "failed".into(),
                },
            }
        }
        None => ExtCollectionReport {
            path: path.into(),
            logical_items: None,
            indexed_items: 0,
            message_count: 0,
            state: "failed".into(),
        },
    })
}

pub(crate) enum StreamExtEvent {
    ProxyFlow(ImportedProxyFlow),
    RepeaterGroups(Vec<TabGroup>),
    RepeaterTab {
        index: usize,
        tab: ImportedRepeaterTab,
    },
}

pub(crate) fn stream_ext_file(
    path: &Path,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(ExtImportPhase, u64, Option<u64>),
    on_event: &mut dyn FnMut(StreamExtEvent) -> Result<()>,
) -> Result<(ExtImportReport, Option<usize>)> {
    check_cancel(cancel)?;
    let mut src = ExtSource::open(path)?;
    let file_size = src.header.file_size;
    let schema_floor = src.header.schema_floor;
    let schema_revision = src.header.schema_revision;
    progress(ExtImportPhase::Archive, 0, Some(file_size));

    let mut collections = Vec::new();
    let mut proxy_count = 0u64;
    let mut tab_count = 0usize;
    let mut repeater_active_raw: Option<i32> = None;

    if src.header.project_root != 0 {
        let root = src.object(src.header.project_root)?;
        check_cancel(cancel)?;

        if let Some(Some(f1_addr)) = src.opt_ref(&root, 1)? {
            let f1 = src.object(f1_addr)?;
            let mut seen_proxy_addrs = HashSet::new();
            for (fid, col_path) in [(0u8, "root.f1.f0"), (1u8, "root.f1.f1")] {
                let mut col_report = inspect_collection(&mut src, &f1, fid, col_path)?;
                if let Some(Some(col_addr)) = src.opt_ref(&f1, fid)? {
                    let slots = src.read_collection_slots(col_addr)?;
                    let total_slots = slots.len() as u64;
                    col_report.logical_items = Some(total_slots);
                    let mut col_indexed = 0u64;
                    let mut col_messages = 0u64;
                    for (slot_idx, slot_addr) in slots.into_iter().enumerate() {
                        if slot_idx % 64 == 0 {
                            check_cancel(cancel)?;
                            progress(ExtImportPhase::Index, proxy_count, None);
                        }
                        let Some(raw_addr) = slot_addr else {
                            continue;
                        };
                        let item_obj = src.object(raw_addr)?;
                        let is_new = seen_proxy_addrs.insert(item_obj.address);
                        if !is_new {
                            continue;
                        }
                        if let Some((flow, msg_cnt)) =
                            parse_proxy_item(&mut src, &item_obj, proxy_count)?
                        {
                            proxy_count += 1;
                            col_indexed += 1;
                            col_messages += msg_cnt;
                            on_event(StreamExtEvent::ProxyFlow(flow))?;
                        }
                    }
                    col_report.indexed_items = col_indexed;
                    col_report.message_count = col_messages;
                }
                collections.push(col_report);
            }
        }

        check_cancel(cancel)?;
        if let Some(Some(f2_addr)) = src.opt_ref(&root, 2)? {
            let f2 = src.object(f2_addr)?;
            repeater_active_raw = src.opt_i32(&f2, 0)?;
            let mut group_by_addr: BTreeMap<u64, Uuid> = BTreeMap::new();
            let mut repeater_groups = Vec::new();

            let mut groups_report = inspect_collection(&mut src, &f2, 4, "root.f2.f4")?;
            if let Some(Some(groups_addr)) = src.opt_ref(&f2, 4)? {
                let group_slots = src.read_collection_slots(groups_addr)?;
                groups_report.logical_items = Some(group_slots.len() as u64);
                let mut grp_indexed = 0u64;
                for slot_addr in group_slots.into_iter().flatten() {
                    check_cancel(cancel)?;
                    if repeater_groups.len() >= MAX_WORK_GROUPS {
                        break;
                    }
                    let grp_obj = src.object(slot_addr)?;
                    if group_by_addr.contains_key(&grp_obj.address) {
                        continue;
                    }
                    if let Some(group) = parse_repeater_group(&mut src, &grp_obj)? {
                        group_by_addr.insert(grp_obj.address, group.id);
                        repeater_groups.push(group);
                        grp_indexed += 1;
                    }
                }
                groups_report.indexed_items = grp_indexed;
            }
            on_event(StreamExtEvent::RepeaterGroups(repeater_groups))?;

            let mut tabs_report = inspect_collection(&mut src, &f2, 1, "root.f2.f1")?;
            let mut tab_history_reports = Vec::new();
            let mut total_draft_bytes = 0usize;
            if let Some(Some(tabs_addr)) = src.opt_ref(&f2, 1)? {
                let tab_slots = src.read_collection_slots(tabs_addr)?;
                tabs_report.logical_items = Some(tab_slots.len() as u64);
                let mut tab_indexed = 0u64;
                let mut tab_messages = 0u64;
                for (tab_idx, slot_opt) in tab_slots.into_iter().enumerate() {
                    check_cancel(cancel)?;
                    if tab_count >= MAX_WORK_TABS {
                        break;
                    }
                    let Some(slot_addr) = slot_opt else {
                        continue;
                    };
                    let tab_obj = src.object(slot_addr)?;
                    if let Some((tab, tab_msg_cnt, hist_report_opt)) = parse_repeater_tab(
                        &mut src,
                        &tab_obj,
                        tab_idx,
                        &group_by_addr,
                        &mut total_draft_bytes,
                        cancel,
                    )? {
                        let tab_idx_out = tab_count;
                        tab_count += 1;
                        tab_indexed += 1;
                        tab_messages += tab_msg_cnt;
                        if let Some(hr) = hist_report_opt {
                            tab_history_reports.push(hr);
                        }
                        on_event(StreamExtEvent::RepeaterTab {
                            index: tab_idx_out,
                            tab,
                        })?;
                    }
                }
                tabs_report.indexed_items = tab_indexed;
                tabs_report.message_count = tab_messages;
            }
            collections.push(tabs_report);
            collections.push(groups_report);
            collections.extend(tab_history_reports);
        }
    }

    let active_tab_index = repeater_active_raw
        .and_then(|v| usize::try_from(v).ok())
        .filter(|idx| *idx < tab_count);
    let repeater_active_tab = if (schema_floor, schema_revision) == (229, 229) {
        active_tab_index.map(|idx| idx as u64)
    } else {
        None
    };
    let indexed_items: u64 = collections.iter().map(|c| c.indexed_items).sum();
    let indexed_messages: u64 = collections.iter().map(|c| c.message_count).sum();

    progress(ExtImportPhase::Archive, file_size, Some(file_size));
    progress(ExtImportPhase::Index, indexed_items, Some(indexed_items));

    Ok((
        ExtImportReport {
            schema_floor,
            schema_revision,
            source_bytes: file_size,
            indexed_items,
            indexed_messages,
            collections,
            repeater_active_tab,
            gaps: Vec::new(),
            complete_archive: true,
        },
        active_tab_index,
    ))
}

fn parse_proxy_item(
    src: &mut ExtSource,
    item: &source::RawObject,
    seq_idx: u64,
) -> Result<Option<(ImportedProxyFlow, u64)>> {
    let service_origin = match src.opt_ref(item, 2)? {
        Some(Some(p_addr)) => {
            let parsed_req = src.object(p_addr)?;
            match src.opt_ref(&parsed_req, 0)? {
                Some(Some(s_addr)) => src.read_service_origin(s_addr)?,
                _ => None,
            }
        }
        _ => None,
    };

    let raw_f15 = match src.opt_ref(item, 15)? {
        Some(Some(addr)) => src.read_bytes(addr)?,
        _ => None,
    };
    let raw_f17 = match src.opt_ref(item, 17)? {
        Some(Some(addr)) => src.read_bytes(addr)?,
        _ => None,
    };

    let parsed_orig_req = raw_f15
        .as_deref()
        .and_then(|bytes| parse_imported_request(bytes, service_origin.as_deref()));
    let parsed_edit_req = raw_f17
        .as_deref()
        .and_then(|bytes| parse_imported_request(bytes, service_origin.as_deref()));

    let (original_request, edited_request, req_msg_cnt) = match (parsed_orig_req, parsed_edit_req) {
        (Some((orig, _)), Some((edit, _))) => {
            let diff = (edit != orig).then_some(edit);
            let cnt = if diff.is_some() { 2 } else { 1 };
            (orig, diff, cnt)
        }
        (Some((orig, _)), None) => (orig, None, 1),
        (None, Some((edit, _))) => (edit, None, 1),
        (None, None) => return Ok(None),
    };

    let raw_f18 = match src.opt_ref(item, 18)? {
        Some(Some(addr)) => src.read_bytes(addr)?,
        _ => None,
    };
    let raw_f20 = match src.opt_ref(item, 20)? {
        Some(Some(addr)) => src.read_bytes(addr)?,
        _ => None,
    };

    let parsed_orig_resp = raw_f18.as_deref().and_then(parse_imported_response);
    let parsed_edit_resp = raw_f20.as_deref().and_then(parse_imported_response);

    let (original_response, edited_response, resp_msg_cnt) =
        match (parsed_orig_resp, parsed_edit_resp) {
            (Some(orig), Some(edit)) => {
                let diff = (edit.0 != orig.0 || edit.1 != orig.1).then_some(edit);
                let cnt = if diff.is_some() { 2 } else { 1 };
                (Some(orig), diff, cnt)
            }
            (Some(orig), None) => (Some(orig), None, 1),
            (None, Some(edit)) => (Some(edit), None, 1),
            (None, None) => (None, None, 0),
        };

    let observed_start = src.opt_i64(item, 11)?;
    let time_known = observed_start.is_some();
    let started_unix_ms = observed_start.unwrap_or(0);

    let timer_f28 = src
        .opt_i64(item, 28)?
        .and_then(|v| u64::try_from(v).ok())
        .filter(|&v| v > 0)
        .map(|v| v.min(86_400_000));
    let timer_f29 = src
        .opt_i64(item, 29)?
        .and_then(|v| u64::try_from(v).ok())
        .filter(|&v| v > 0)
        .map(|v| v.min(86_400_000));
    let end_ms = src.opt_i64(item, 30)?.unwrap_or(0);
    let elapsed_ms = if timer_f28.is_some() || timer_f29.is_some() {
        timer_f29.unwrap_or(0).max(timer_f28.unwrap_or(0))
    } else if time_known && end_ms > started_unix_ms {
        u64::try_from(end_ms - started_unix_ms)
            .unwrap_or(0)
            .min(86_400_000)
    } else {
        0
    };
    let response_received_ms = if timer_f28.is_some() || timer_f29.is_some() {
        timer_f28.or(timer_f29)
    } else if time_known && original_response.is_some() && end_ms >= started_unix_ms {
        Some(
            u64::try_from(end_ms - started_unix_ms)
                .unwrap_or(0)
                .min(86_400_000),
        )
    } else {
        None
    };

    let listener_port = src
        .opt_i32(item, 14)?
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0);

    let highlight = src.opt_u8(item, 13)?.filter(|c| *c <= 9).unwrap_or(0);

    let notes = match src.opt_ref(item, 27)? {
        Some(Some(ann_addr)) => {
            let ann_obj = src.object(ann_addr)?;
            match src.opt_ref(&ann_obj, 2)? {
                Some(Some(note_addr)) => src
                    .read_utf16_string(note_addr, 32_768)?
                    .map(|s| truncate_utf8_clean(&s, MAX_LIVE_NOTE_BYTES))
                    .unwrap_or_default(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    };

    Ok(Some((
        ImportedProxyFlow {
            index: seq_idx,
            started_unix_ms,
            time_known,
            elapsed_ms,
            response_received_ms,
            listener_port,
            remote_ip: String::new(),
            original_request,
            edited_request,
            original_response,
            edited_response,
            notes,
            highlight,
        },
        req_msg_cnt + resp_msg_cnt,
    )))
}

fn parse_repeater_group(src: &mut ExtSource, obj: &source::RawObject) -> Result<Option<TabGroup>> {
    let Some(Some(addr)) = src.opt_ref(obj, 0)? else {
        return Ok(None);
    };
    let Some(raw_title) = src.read_utf16_string(addr, 4096)? else {
        return Ok(None);
    };
    let trimmed = raw_title.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let title = truncate_utf8_clean(trimmed, MAX_TITLE_BYTES);
    let expanded = src.opt_bool(obj, 2)?.unwrap_or(true);
    let color = src.opt_u8(obj, 1)?.filter(|c| *c <= 14);
    Ok(Some(TabGroup {
        id: Uuid::new_v4(),
        title,
        collapsed: !expanded,
        color,
    }))
}

fn parse_repeater_tab(
    src: &mut ExtSource,
    tab: &source::RawObject,
    tab_idx: usize,
    group_by_addr: &BTreeMap<u64, Uuid>,
    total_draft_bytes: &mut usize,
    cancel: &CancellationToken,
) -> Result<Option<(ImportedRepeaterTab, u64, Option<ExtCollectionReport>)>> {
    let raw_title = match src.opt_ref(tab, 0)? {
        Some(Some(addr)) => src.read_utf16_string(addr, 4096)?.unwrap_or_default(),
        _ => String::new(),
    };
    let clean_title = truncate_utf8_clean(raw_title.trim(), MAX_TITLE_BYTES);
    let title = if clean_title.is_empty() {
        (tab_idx + 1).to_string()
    } else {
        clean_title
    };

    let group = match src.opt_ref(tab, 32)? {
        Some(Some(g_addr)) => {
            let g_obj = src.object(g_addr)?;
            group_by_addr.get(&g_obj.address).copied()
        }
        _ => None,
    };

    let tab_service_origin = match src.opt_ref(tab, 3)? {
        Some(Some(s_addr)) => src.read_service_origin(s_addr)?,
        _ => None,
    };

    let mut history_items = Vec::new();
    let mut raw_to_valid_index = Vec::new();
    let mut history_report = None;

    if let Some(Some(hist_addr)) = src.opt_ref(tab, 2)? {
        let pair_slots = src.read_collection_slots(hist_addr)?;
        let hist_logical = pair_slots.len() as u64;
        let mut hist_indexed = 0u64;
        let mut hist_messages = 0u64;
        raw_to_valid_index.reserve(pair_slots.len());
        for (p_idx, pair_opt) in pair_slots.into_iter().enumerate() {
            if p_idx % 32 == 0 {
                check_cancel(cancel)?;
            }
            let Some(pair_addr) = pair_opt else {
                raw_to_valid_index.push(None);
                continue;
            };
            let pair_obj = src.object(pair_addr)?;
            let pair_service = match src.opt_ref(&pair_obj, 0)? {
                Some(Some(s_addr)) => src.read_service_origin(s_addr)?,
                _ => None,
            }
            .or_else(|| tab_service_origin.clone());
            let Some(mut raw_req) = (match src.opt_ref(&pair_obj, 2)? {
                Some(Some(r_addr)) => src.read_bytes(r_addr)?.filter(|b| !b.is_empty()),
                _ => None,
            }) else {
                raw_to_valid_index.push(None);
                continue;
            };
            if raw_req.len() > MAX_HISTORY_RAW_BYTES {
                raw_req.truncate(MAX_HISTORY_RAW_BYTES);
            }
            let Some((mut request, resolved_target)) =
                parse_imported_request(&raw_req, pair_service.as_deref())
            else {
                raw_to_valid_index.push(None);
                continue;
            };
            let response = match src.opt_ref(&pair_obj, 3)? {
                Some(Some(resp_addr)) => src
                    .read_bytes(resp_addr)?
                    .and_then(|bytes| parse_imported_response(&bytes)),
                _ => None,
            };
            let pair_started = src.opt_i64(&pair_obj, 4)?.unwrap_or(0);
            let pair_elapsed = src
                .opt_i64(&pair_obj, 5)?
                .and_then(|v| u64::try_from(v).ok())
                .filter(|&v| v <= 86_400_000)
                .unwrap_or(0);
            let pair_is_http2 = src.opt_u8(&pair_obj, 11)? == Some(2)
                || response
                    .as_ref()
                    .is_some_and(|(r, _)| r.version == HttpVersion::Http2);
            if pair_is_http2 {
                request.version = HttpVersion::Http2;
                upgrade_request_line_to_http2(&mut raw_req);
            }
            let msg_cnt = if response.is_some() { 2 } else { 1 };
            hist_indexed += 1;
            hist_messages += msg_cnt;
            let valid_idx = history_items.len();
            raw_to_valid_index.push(Some(valid_idx));
            history_items.push(ImportedRepeaterHistoryItem {
                raw_request: raw_req,
                request,
                target: resolved_target,
                response,
                started_unix_ms: pair_started,
                elapsed_ms: pair_elapsed,
            });
        }
        history_report = Some(ExtCollectionReport {
            path: format!("root.f2.f1[{tab_idx}].f2"),
            logical_items: Some(hist_logical),
            indexed_items: hist_indexed,
            message_count: hist_messages,
            state: if hist_logical == 0 {
                "empty".into()
            } else {
                "available".into()
            },
        });
    }

    let selected_raw = src.opt_i32(tab, 1)?.unwrap_or(-1);
    let selected_from_f1 = usize::try_from(selected_raw)
        .ok()
        .and_then(|idx| raw_to_valid_index.get(idx).copied().flatten());

    let mut current_f4 = match src.opt_ref(tab, 4)? {
        Some(Some(addr)) => src.read_bytes(addr)?.unwrap_or_default(),
        _ => Vec::new(),
    };
    if current_f4.len() > MAX_DRAFT_SINGLE_BYTES {
        current_f4.truncate(MAX_DRAFT_SINGLE_BYTES);
    }
    let current_f5_resp = match src.opt_ref(tab, 5)? {
        Some(Some(addr)) => src
            .read_bytes(addr)?
            .and_then(|bytes| parse_imported_response(&bytes)),
        _ => None,
    };

    let mut tab_msg_cnt = 0u64;
    if !current_f4.is_empty() {
        tab_msg_cnt += 1;
    }

    let selected_history = if let Some(sel) = selected_from_f1 {
        Some(sel)
    } else if !current_f4.is_empty()
        && history_items.is_empty()
        && current_f5_resp.is_some()
        && let Some((req, tgt)) = parse_imported_request(&current_f4, tab_service_origin.as_deref())
    {
        history_items.push(ImportedRepeaterHistoryItem {
            raw_request: current_f4.clone(),
            request: req,
            target: tgt,
            response: current_f5_resp,
            started_unix_ms: 0,
            elapsed_ms: 0,
        });
        tab_msg_cnt += 1;
        Some(0)
    } else if current_f4.is_empty() && !history_items.is_empty() {
        Some(history_items.len() - 1)
    } else {
        None
    };

    let (mut draft_raw, draft_target, draft_version) = if !current_f4.is_empty() {
        let parsed = parse_imported_request(&current_f4, tab_service_origin.as_deref());
        let target = parsed
            .as_ref()
            .map(|(_, t)| t.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| tab_service_origin.clone())
            .unwrap_or_default();
        let selected_h2 = selected_history
            .and_then(|idx| history_items.get(idx))
            .is_some_and(|item| item.request.version == HttpVersion::Http2);
        let mut version = parsed
            .map(|(r, _)| r.version)
            .unwrap_or(HttpVersion::Http11);
        if selected_h2 {
            version = HttpVersion::Http2;
            upgrade_request_line_to_http2(&mut current_f4);
        }
        (current_f4, target, version)
    } else if let Some(sel_idx) = selected_history {
        let item = &history_items[sel_idx];
        (
            item.raw_request.clone(),
            item.target.clone(),
            item.request.version,
        )
    } else {
        (
            Vec::new(),
            tab_service_origin.unwrap_or_default(),
            HttpVersion::Http11,
        )
    };

    if *total_draft_bytes + draft_raw.len() > MAX_DRAFT_TOTAL_BYTES {
        let remain = MAX_DRAFT_TOTAL_BYTES.saturating_sub(*total_draft_bytes);
        draft_raw.truncate(remain);
    }
    *total_draft_bytes += draft_raw.len();

    Ok(Some((
        ImportedRepeaterTab {
            title,
            group,
            draft_target,
            draft_version,
            draft_raw,
            history: history_items,
            selected_history,
        },
        tab_msg_cnt,
        history_report,
    )))
}

fn upgrade_request_line_to_http2(raw: &mut Vec<u8>) {
    let line_end = raw.iter().position(|&b| b == b'\n').unwrap_or(raw.len());
    let has_cr = line_end > 0 && raw[line_end - 1] == b'\r';
    let text_end = if has_cr { line_end - 1 } else { line_end };
    let suffix = b" HTTP/1.1";
    if text_end >= suffix.len() && &raw[text_end - suffix.len()..text_end] == suffix {
        raw.splice(text_end - suffix.len()..text_end, *b" HTTP/2");
    }
}

fn truncate_utf8_clean(text: &str, max_bytes: usize) -> String {
    let clean: String = text.chars().filter(|&c| c != '\0').collect();
    if clean.len() <= max_bytes {
        return clean;
    }
    let mut end = max_bytes;
    while end > 0 && !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExtJobOutcome, Client, Command, Event, HistoryRow, HttpHistoryFilter, HttpHistoryQuery,
        HttpHistorySort, ProjectKey, Runtime, SiteMapQuery, TargetScope, ext_archive::import,
        store::ProjectStore,
    };
    use std::{fs, path::PathBuf};

    struct SyntheticExt {
        bytes: Vec<u8>,
    }

    impl SyntheticExt {
        fn new(schema: u16) -> Self {
            let mut bytes = vec![0u8; 72];
            bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
            bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
            bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
            bytes[12..14].copy_from_slice(&schema.to_be_bytes());
            bytes[14..16].copy_from_slice(&schema.to_be_bytes());
            bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
            Self { bytes }
        }

        fn raw(&mut self, data: &[u8]) -> u64 {
            if !self.bytes.len().is_multiple_of(2) {
                self.bytes.push(0);
            }
            let addr = self.bytes.len() as u64;
            self.bytes.extend_from_slice(data);
            addr
        }

        fn array(&mut self, width: usize, payload: &[u8]) -> u64 {
            let mut out = Vec::new();
            out.extend_from_slice(&((8 + payload.len().max(1)) as i32).to_be_bytes());
            out.extend_from_slice(&((payload.len() / width) as i32).to_be_bytes());
            out.extend_from_slice(payload);
            if payload.is_empty() {
                out.push(0);
            }
            self.raw(&out)
        }

        fn byte_array(&mut self, payload: &[u8]) -> u64 {
            self.array(1, payload)
        }

        fn chunked_bytes(&mut self, parts: &[&[u8]]) -> u64 {
            let total_len: usize = parts.iter().map(|p| p.len()).sum();
            let chunk_addrs: Vec<i64> = parts
                .iter()
                .map(|part| self.byte_array(part) as i64)
                .collect();
            let refs = self.ref_array(&chunk_addrs);
            let wrapper = self.object(
                (0, 0),
                vec![
                    (0, (chunk_addrs.len() as i32).to_be_bytes().to_vec()),
                    (1, (refs as i64).to_be_bytes().to_vec()),
                ],
            );
            self.object(
                (0, 0),
                vec![
                    (0, (total_len as i32).to_be_bytes().to_vec()),
                    (1, (wrapper as i64).to_be_bytes().to_vec()),
                ],
            )
        }

        fn utf16_direct(&mut self, text: &str) -> u64 {
            let payload: Vec<u8> = text
                .encode_utf16()
                .flat_map(|u| u.to_be_bytes())
                .collect();
            let arr = self.array(2, &payload);
            self.object((0, 0), vec![(0, (arr as i64).to_be_bytes().to_vec())])
        }

        fn utf16_chunked(&mut self, text: &str) -> u64 {
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut chunk_addrs = Vec::new();
            for part in units.chunks(32) {
                let mut padded = part.to_vec();
                padded.resize(32, 0);
                let raw_bytes: Vec<u8> = padded.into_iter().flat_map(|u| u.to_be_bytes()).collect();
                chunk_addrs.push(self.array(2, &raw_bytes) as i64);
            }
            let refs = self.ref_array(&chunk_addrs);
            let wrapper = self.object(
                (0, 0),
                vec![
                    (0, (chunk_addrs.len() as i32).to_be_bytes().to_vec()),
                    (1, (refs as i64).to_be_bytes().to_vec()),
                ],
            );
            self.object(
                (0, 0),
                vec![
                    (0, (units.len() as i32).to_be_bytes().to_vec()),
                    (3, (wrapper as i64).to_be_bytes().to_vec()),
                ],
            )
        }

        fn ref_array(&mut self, refs: &[i64]) -> u64 {
            let payload: Vec<u8> = refs.iter().flat_map(|r| r.to_be_bytes()).collect();
            self.array(8, &payload)
        }

        fn object(&mut self, tag: (u8, u8), fields: Vec<(u8, Vec<u8>)>) -> u64 {
            let mut raw = vec![0, tag.0, tag.1, fields.len() as u8];
            let mut offset = 4 + fields.len() * 3;
            for (id, val) in &fields {
                raw.push(*id);
                raw.extend_from_slice(&(offset as u16).to_be_bytes());
                offset += val.len();
            }
            for (_, val) in fields {
                raw.extend_from_slice(&val);
            }
            self.raw(&raw)
        }

        fn direct_list(&mut self, items: &[i64]) -> u64 {
            let arr = self.ref_array(items);
            self.object(
                (0, 0),
                vec![
                    (0, (items.len() as i32).to_be_bytes().to_vec()),
                    (1, (arr as i64).to_be_bytes().to_vec()),
                ],
            )
        }

        fn chunked_list(&mut self, items: &[i64]) -> u64 {
            let arr = self.ref_array(items);
            let chunks = self.direct_list(&[arr as i64]);
            self.object(
                (0, 0),
                vec![
                    (0, (items.len() as i32).to_be_bytes().to_vec()),
                    (1, (items.len().max(1) as i32).to_be_bytes().to_vec()),
                    (2, (chunks as i64).to_be_bytes().to_vec()),
                    (3, 0_i32.to_be_bytes().to_vec()),
                ],
            )
        }

        fn service(&mut self, host: &str, port: i32, tls: bool) -> u64 {
            let h = self.utf16_direct(host);
            self.object(
                (0, 0),
                vec![
                    (0, (h as i64).to_be_bytes().to_vec()),
                    (1, port.to_be_bytes().to_vec()),
                    (2, vec![u8::from(tls)]),
                ],
            )
        }

        fn note_annotations(&mut self, text: &str) -> u64 {
            let units: Vec<u16> = text.encode_utf16().collect();
            let raw_bytes: Vec<u8> = units.iter().flat_map(|u| u.to_be_bytes()).collect();
            let arr = self.array(2, &raw_bytes);
            let node = self.object(
                (0, 0),
                vec![
                    (1, (arr as i64).to_be_bytes().to_vec()),
                    (2, (units.len() as i32).to_be_bytes().to_vec()),
                ],
            );
            self.object((0, 0), vec![(2, (node as i64).to_be_bytes().to_vec())])
        }

        fn finish(mut self, root: u64, path: &Path) {
            let len = self.bytes.len() as i64;
            self.bytes[56..64].copy_from_slice(&len.to_be_bytes());
            self.bytes[64..72].copy_from_slice(&(root as i64).to_be_bytes());
            fs::write(path, self.bytes).unwrap();
        }
    }

    #[test]
    fn direct_import_recovers_proxy_history_streaming_chunked_bytes_blank_and_populated_repeater_tabs_across_schemas()
     {
        for schema in [226u16, 228, 229, 231] {
            let dir = tempfile::tempdir().unwrap();
            let ext_path = dir.path().join(format!("sample-{schema}.archive"));
            let mio_path = dir.path().join(format!("converted-{schema}.mio"));
            let mut b = SyntheticExt::new(schema);

            let svc = b.service("api.example.com", 443, true);
            let parsed_req = b.object((0, 0), vec![(0, (svc as i64).to_be_bytes().to_vec())]);
            let req1 = b.byte_array(
                b"POST /v1/chat HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 4\r\n\r\nping",
            );
            let edit_req1 = b.byte_array(
                b"POST /v1/chat HTTP/1.1\r\nHost: api.example.com\r\nX-Edited: 1\r\nContent-Length: 4\r\n\r\npong",
            );
            let resp1_chunked = b.chunked_bytes(&[
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: part1\n\n",
                b"data: part2\n\n",
            ]);
            let note1 = b.note_annotations("核心接口备注");
            let proxy_item_streaming = b.object(
                (2, 0),
                vec![
                    (2, (parsed_req as i64).to_be_bytes().to_vec()),
                    (11, 1_794_278_900_000_i64.to_be_bytes().to_vec()),
                    (13, vec![3]),
                    (14, 8080_i32.to_be_bytes().to_vec()),
                    (15, (req1 as i64).to_be_bytes().to_vec()),
                    (17, (edit_req1 as i64).to_be_bytes().to_vec()),
                    (18, (resp1_chunked as i64).to_be_bytes().to_vec()),
                    (27, (note1 as i64).to_be_bytes().to_vec()),
                    (30, 1_794_278_900_125_i64.to_be_bytes().to_vec()),
                ],
            );

            let f0_col = b.chunked_list(&[proxy_item_streaming as i64]);
            let f1_col = b.chunked_list(&[proxy_item_streaming as i64]);
            let bad_ws_col = b.object((9, 9), vec![(0, (-99_i32).to_be_bytes().to_vec())]);
            let http_parent = b.object(
                (0, 0),
                vec![
                    (0, (f0_col as i64).to_be_bytes().to_vec()),
                    (1, (f1_col as i64).to_be_bytes().to_vec()),
                    (2, (bad_ws_col as i64).to_be_bytes().to_vec()),
                ],
            );

            let grp_title = b.utf16_direct("鉴权分组");
            let grp_obj = b.object(
                (0, 1),
                vec![
                    (0, (grp_title as i64).to_be_bytes().to_vec()),
                    (1, vec![2]),
                    (2, vec![1]),
                ],
            );
            let groups_list = b.direct_list(&[grp_obj as i64]);

            let blank_title = b.utf16_chunked("1");
            let empty_hist = b.direct_list(&[]);
            let blank_tab = b.object(
                (0, 2),
                vec![
                    (0, (blank_title as i64).to_be_bytes().to_vec()),
                    (1, (-1_i32).to_be_bytes().to_vec()),
                    (2, (empty_hist as i64).to_be_bytes().to_vec()),
                    (4, 0_i64.to_be_bytes().to_vec()),
                ],
            );

            let active_title = b.utf16_chunked("登录重放");
            let pair_req =
                b.byte_array(b"GET /me HTTP/1.1\r\nHost: api.example.com\r\n\r\n");
            let pair_resp = b.byte_array(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            let pair = b.object(
                (1, 0),
                vec![
                    (0, (svc as i64).to_be_bytes().to_vec()),
                    (2, (pair_req as i64).to_be_bytes().to_vec()),
                    (3, (pair_resp as i64).to_be_bytes().to_vec()),
                ],
            );
            let tab_hist = b.direct_list(&[pair as i64]);
            let populated_tab = b.object(
                (0, 2),
                vec![
                    (0, (active_title as i64).to_be_bytes().to_vec()),
                    (1, 0_i32.to_be_bytes().to_vec()),
                    (2, (tab_hist as i64).to_be_bytes().to_vec()),
                    (3, (svc as i64).to_be_bytes().to_vec()),
                    (32, (grp_obj as i64).to_be_bytes().to_vec()),
                ],
            );

            let tabs_list = b.direct_list(&[blank_tab as i64, populated_tab as i64]);
            let repeater_parent = b.object(
                (0, 2),
                vec![
                    (0, 1_i32.to_be_bytes().to_vec()),
                    (1, (tabs_list as i64).to_be_bytes().to_vec()),
                    (4, (groups_list as i64).to_be_bytes().to_vec()),
                ],
            );

            let bad_settings = b.object((7, 7), vec![(0, (-1_i64).to_be_bytes().to_vec())]);
            let root = b.object(
                (0, 7),
                vec![
                    (1, (http_parent as i64).to_be_bytes().to_vec()),
                    (2, (repeater_parent as i64).to_be_bytes().to_vec()),
                    (4, (bad_settings as i64).to_be_bytes().to_vec()),
                ],
            );
            b.finish(root, &ext_path);

            let cancel = CancellationToken::new();
            let inspection = import::inspect(ext_path.clone(), &cancel, &mut |_, _, _| {}).unwrap();
            let (outcome, cleanup) =
                import::convert(inspection, mio_path.clone(), &cancel, &mut |_, _, _| {});
            assert!(cleanup.is_none());
            let ExtJobOutcome::Imported { report, .. } = outcome else {
                panic!("expected import success for schema {schema}: {outcome:?}");
            };
            assert_eq!((report.schema_floor, report.schema_revision), (schema, schema));

            let store = ProjectStore::open(&mio_path).unwrap();
            let ws = store.workspace().unwrap();
            assert_eq!(ws.groups.len(), 1);
            assert_eq!(ws.groups[0].title, "鉴权分组");
            assert_eq!(ws.tabs.len(), 2);
            assert_eq!(ws.tabs[0].title, "1");
            assert!(ws.tabs[0].draft.raw.is_empty());
            assert_eq!(ws.tabs[1].title, "登录重放");
            assert_eq!(ws.tabs[1].group, Some(ws.groups[0].id));
            assert_eq!(ws.active, Some(ws.tabs[1].id));
            assert!(ws.tabs[1].last_flow.is_some());

            let project_key = ProjectKey {
                id: store.id,
                generation: 1,
            };
            let query = HttpHistoryQuery {
                project: project_key,
                query_revision: 1,
                offset: 0,
                limit: 50,
                url_filter: String::new(),
                filter: HttpHistoryFilter::default(),
                sort: HttpHistorySort::default(),
                selected_ids: Vec::new(),
            };
            let hist = store.query_http_history(&query, &cancel).unwrap();
            assert_eq!(hist.total, 1);
            assert_eq!(hist.live_annotations.len(), 1);
            assert_eq!(hist.live_annotations[0].2, "核心接口备注");
            assert_eq!(
                hist.live_annotations[0].3,
                crate::ExtHttpHighlight {
                    revision: 1,
                    color: 3,
                }
            );
            let HistoryRow::Live(summary) = &hist.rows[0] else {
                panic!("expected native Live flow row");
            };
            assert_eq!(summary.elapsed_ms, 125);
            let full_flow = store.read(summary.id).unwrap();
            let full_resp = full_flow.response.as_ref().unwrap();
            assert_eq!(full_resp.reason_phrase.as_deref(), Some(b"OK".as_slice()));
            assert!(crate::response_to_raw(full_resp).starts_with(b"HTTP/1.1 200 OK\r\n"));
            assert_eq!(
                full_flow.response.unwrap().body,
                b"data: part1\n\ndata: part2\n\n"
            );
            let orig = store.original(summary.id).unwrap();
            assert_eq!(orig.request.unwrap().body, b"ping");

            let sitemap = store
                .target_site_map_snapshot(&TargetScope::default(), &SiteMapQuery::default(), &[])
                .unwrap();
            assert!(sitemap.total_items >= 1);
            store.close().unwrap();

            let direct_opened = ProjectStore::open_ext(&ext_path, &cancel).unwrap();
            assert!(direct_opened.path.is_none());
            assert_eq!(direct_opened.workspace().unwrap().tabs.len(), 2);
            direct_opened.close().unwrap();
        }
    }

    #[test]
    fn direct_import_preserves_item_index_capture_numbers_under_out_of_order_timestamps_deletions_timers_and_archive_queries()
     {
        let dir = tempfile::tempdir().unwrap();
        let ext_path = dir.path().join("out-of-order.archive");
        let mut b = SyntheticExt::new(229);
        let svc = b.service("order.example.com", 443, true);
        let parsed_req = b.object((0, 0), vec![(0, (svc as i64).to_be_bytes().to_vec())]);

        let req1 = b.byte_array(b"GET /first HTTP/1.1\r\nHost: order.example.com\r\n\r\n");
        let resp1 = b.byte_array(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n5\r\nalpha\r\n0\r\n\r\n",
        );
        let item0 = b.object(
            (2, 0),
            vec![
                (2, (parsed_req as i64).to_be_bytes().to_vec()),
                (11, 1_794_278_950_000_i64.to_be_bytes().to_vec()),
                (15, (req1 as i64).to_be_bytes().to_vec()),
                (18, (resp1 as i64).to_be_bytes().to_vec()),
                (28, 134_i64.to_be_bytes().to_vec()),
                (29, 135_i64.to_be_bytes().to_vec()),
            ],
        );

        let req2 = b.byte_array(b"GET /second HTTP/1.1\r\nHost: order.example.com\r\n\r\n");
        let resp2 = b.byte_array(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nbravo");
        let item1 = b.object(
            (2, 0),
            vec![
                (2, (parsed_req as i64).to_be_bytes().to_vec()),
                (11, 1_794_278_910_000_i64.to_be_bytes().to_vec()),
                (15, (req2 as i64).to_be_bytes().to_vec()),
                (18, (resp2 as i64).to_be_bytes().to_vec()),
                (30, 1_794_278_910_040_i64.to_be_bytes().to_vec()),
            ],
        );

        let req3 = b.byte_array(b"GET /third HTTP/1.1\r\nHost: order.example.com\r\n\r\n");
        let resp3 = b.byte_array(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\ncharlie");
        let large_pad = vec![b'P'; 33_000];
        let item2 = b.object(
            (2, 0),
            vec![
                (1, large_pad),
                (2, (parsed_req as i64).to_be_bytes().to_vec()),
                (15, (req3 as i64).to_be_bytes().to_vec()),
                (18, (resp3 as i64).to_be_bytes().to_vec()),
            ],
        );

        let f0_col = b.direct_list(&[item0 as i64, item1 as i64, item2 as i64]);
        let http_parent = b.object((0, 0), vec![(0, (f0_col as i64).to_be_bytes().to_vec())]);
        let root = b.object((0, 7), vec![(1, (http_parent as i64).to_be_bytes().to_vec())]);
        b.finish(root, &ext_path);

        let cancel = CancellationToken::new();
        let mut store = ProjectStore::open_ext(&ext_path, &cancel).unwrap();
        let sources = store.ext_sources().unwrap();
        assert_eq!(sources.len(), 1);
        let source_id = sources[0].id;
        let ws_page = store.ext_websocket_page(source_id, 0, 50).unwrap();
        assert_eq!(ws_page.total, 0);
        assert!(ws_page.rows.is_empty());
        let coverage = store.ext_coverage_report(source_id).unwrap();
        assert_eq!(
            coverage
                .category(crate::ExtCategory::ProxyHttpHistory)
                .unwrap()
                .recovered_items,
            3
        );
        assert!(
            store
                .ext_item_detail(source_id, Uuid::new_v4(), 0, 8)
                .unwrap_err()
                .to_string()
                .contains("外部归档条目不存在或不属于当前来源")
        );
        assert_eq!(
            crate::service::websocket::ext_archive::completed_source_id(&store).unwrap(),
            None
        );

        let mut query = HttpHistoryQuery {
            project: ProjectKey {
                id: store.id,
                generation: 1,
            },
            query_revision: 1,
            offset: 0,
            limit: 50,
            url_filter: String::new(),
            filter: HttpHistoryFilter::default(),
            sort: HttpHistorySort {
                column: crate::HttpHistorySortColumn::Number,
                ascending: true,
            },
            selected_ids: Vec::new(),
        };
        let hist = store.query_http_history(&query, &cancel).unwrap();
        assert_eq!(hist.total, 3);
        let urls_and_numbers: Vec<(String, u64, u64, Option<u64>, bool)> = hist
            .rows
            .iter()
            .map(|row| {
                let flow = row.live().unwrap();
                let num = hist
                    .live_numbers
                    .iter()
                    .find(|(id, _)| *id == flow.id)
                    .unwrap()
                    .1;
                let meta = hist
                    .row_metadata_for(crate::HistoryEntryId::Live(flow.id))
                    .unwrap();
                (
                    flow.url.clone(),
                    num,
                    flow.elapsed_ms,
                    meta.response_received_ms,
                    meta.time_known,
                )
            })
            .collect();
        assert_eq!(
            urls_and_numbers,
            vec![
                (
                    "https://order.example.com:443/first".into(),
                    1,
                    135,
                    Some(134),
                    true
                ),
                (
                    "https://order.example.com:443/second".into(),
                    2,
                    40,
                    Some(40),
                    true
                ),
                (
                    "https://order.example.com:443/third".into(),
                    3,
                    0,
                    None,
                    false
                ),
            ]
        );
        let first_id = hist.rows[0].live().unwrap().id;
        let second_id = hist.rows[1].live().unwrap().id;
        let third_id = hist.rows[2].live().unwrap().id;
        assert_eq!(
            store.read(first_id).unwrap().response.unwrap().body,
            b"alpha"
        );

        store.delete_live_proxy_flows(&[second_id]).unwrap();
        query.query_revision += 1;
        let after_delete = store.query_http_history(&query, &cancel).unwrap();
        assert_eq!(after_delete.total, 2);
        assert_eq!(
            after_delete.live_numbers,
            vec![(first_id, 1), (third_id, 3)]
        );
    }

    async fn wait_project_opened(
        client: &mut Client,
    ) -> (ProjectKey, Option<PathBuf>, crate::Workspace) {
        loop {
            match client.events.recv().await.unwrap() {
                Event::ProjectOpened {
                    project,
                    path,
                    workspace,
                } => return (project, path, workspace),
                Event::Error {
                    operation, message, ..
                } => panic!("unexpected runtime error in {operation}: {message}"),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn real_ext_project_files_import_and_open_cleanly_when_present() {
        let candidates: Vec<PathBuf> = std::env::var_os("MIO_REAL_ARCHIVE_SAMPLE")
            .into_iter()
            .map(PathBuf::from)
            .collect();
        for ext_path in candidates {
            if !ext_path.is_file() {
                continue;
            }
            let cancel = CancellationToken::new();
            let (runtime, mut client) = Runtime::start().unwrap();
            let _ = wait_project_opened(&mut client).await;
            client
                .commands
                .send(Command::OpenProject {
                    path: ext_path.clone(),
                })
                .await
                .unwrap();
            let (project, opened_path, workspace) = wait_project_opened(&mut client).await;
            assert!(opened_path.is_none());
            assert!(!workspace.tabs.is_empty());

            let query = HttpHistoryQuery {
                project,
                query_revision: 1,
                offset: 0,
                limit: 100,
                url_filter: String::new(),
                filter: HttpHistoryFilter::default(),
                sort: HttpHistorySort::default(),
                selected_ids: Vec::new(),
            };
            client
                .commands
                .send(Command::QueryHttpHistory { query })
                .await
                .unwrap();
            let total = loop {
                match client.events.recv().await.unwrap() {
                    Event::HttpHistoryPage { result, .. } => break result.unwrap().total,
                    Event::Error {
                        operation, message, ..
                    } => panic!("history error in {operation}: {message}"),
                    _ => {}
                }
            };
            assert!(total > 0, "expected imported proxy flows from {}", ext_path.display());

            let dir = tempfile::tempdir().unwrap();
            let converted_mio = dir.path().join("converted.mio");
            let inspect_job = Uuid::new_v4();
            client
                .commands
                .send(Command::InspectExternal {
                    project,
                    job_id: inspect_job,
                    path: ext_path.clone(),
                    cancel: cancel.clone(),
                })
                .await
                .unwrap();
            let inspection_id = loop {
                match client.events.recv().await.unwrap() {
                    Event::ExtJob {
                        job_id,
                        outcome: ExtJobOutcome::Inspected(info),
                        ..
                    } if job_id == inspect_job => break info.id,
                    Event::ExtJob {
                        job_id, outcome, ..
                    } if job_id == inspect_job => {
                        panic!("inspect failed for {}: {outcome:?}", ext_path.display())
                    }
                    _ => {}
                }
            };
            let convert_job = Uuid::new_v4();
            client
                .commands
                .send(Command::ConvertExternal {
                    project,
                    job_id: convert_job,
                    inspection_id,
                    destination: converted_mio.clone(),
                    cancel: cancel.clone(),
                })
                .await
                .unwrap();
            loop {
                match client.events.recv().await.unwrap() {
                    Event::ExtJob {
                        job_id,
                        outcome: ExtJobOutcome::Imported { path, .. },
                        ..
                    } if job_id == convert_job => {
                        assert_eq!(path, converted_mio);
                        break;
                    }
                    Event::ExtJob {
                        job_id, outcome, ..
                    } if job_id == convert_job => {
                        panic!("convert failed for {}: {outcome:?}", ext_path.display())
                    }
                    _ => {}
                }
            }
            client.shutdown.cancel();
            drop(client);
            tokio::task::spawn_blocking(move || runtime.shutdown())
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[test]
    fn frozen_sync_v11_row_1402_preserves_http11_200_ok_reason_phrase_in_flow_record_and_raw() {
        let Some(ext_path) = std::env::var_os("MIO_FROZEN_ARCHIVE_SAMPLE").map(PathBuf::from) else {
            return;
        };
        if !ext_path.is_file() {
            return;
        }
        let cancel = CancellationToken::new();
        let store = ProjectStore::open_ext(&ext_path, &cancel).unwrap();
        let query = HttpHistoryQuery {
            project: ProjectKey {
                id: store.id,
                generation: 1,
            },
            query_revision: 1,
            offset: 0,
            limit: 50,
            url_filter: String::new(),
            filter: HttpHistoryFilter::default(),
            sort: HttpHistorySort::default(),
            selected_ids: Vec::new(),
        };
        let page = store.query_http_history(&query, &cancel).unwrap();
        let row_1402 = page
            .rows
            .iter()
            .filter_map(HistoryRow::live)
            .find(|row| {
                page.live_numbers
                    .iter()
                    .find_map(|&(id, num)| (id == row.id).then_some(num))
                    == Some(1402)
            })
            .expect("row #1402 must be present in frozen archive sample");
        let flow = store.read(row_1402.id).unwrap();
        let resp = flow.response.as_ref().expect("row #1402 must have response");
        assert_eq!(resp.version, HttpVersion::Http11);
        assert_eq!(resp.status, 200);
        assert_eq!(resp.reason_phrase.as_deref(), Some(b"OK".as_slice()));
        let raw = crate::response_to_raw(resp);
        assert!(
            raw.starts_with(b"HTTP/1.1 200 OK\r\n"),
            "expected HTTP/1.1 200 OK status line, got {:?}",
            String::from_utf8_lossy(&raw[..raw.len().min(64)])
        );
        let meta = page
            .row_metadata_for(crate::HistoryEntryId::Live(row_1402.id))
            .expect("row #1402 metadata must be present");
        assert_eq!(meta.response_length, Some(raw.len() as u64));

        let asc_query = HttpHistoryQuery {
            project: ProjectKey {
                id: store.id,
                generation: 1,
            },
            query_revision: 2,
            offset: 0,
            limit: 10,
            url_filter: String::new(),
            filter: HttpHistoryFilter::default(),
            sort: HttpHistorySort {
                column: crate::HttpHistorySortColumn::Number,
                ascending: true,
            },
            selected_ids: Vec::new(),
        };
        let asc_page = store.query_http_history(&asc_query, &cancel).unwrap();
        let row_1 = asc_page
            .rows
            .iter()
            .filter_map(HistoryRow::live)
            .find(|row| {
                asc_page
                    .live_numbers
                    .iter()
                    .find_map(|&(id, num)| (id == row.id).then_some(num))
                    == Some(1)
            })
            .expect("row #1 must be present in frozen archive sample");
        let flow_1 = store.read(row_1.id).unwrap();
        let resp_1 = flow_1.response.as_ref().expect("row #1 must have response");
        assert!(resp_1.body.len() > 512);
        assert!(
            !resp_1
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-type"))
        );
        let meta_1 = asc_page
            .row_metadata_for(crate::HistoryEntryId::Live(row_1.id))
            .expect("row #1 metadata must be present");
        assert_eq!(meta_1.mime_type, "JSON");
        assert_eq!(meta_1.response_length, Some(2364));
        store.close().unwrap();
    }

    #[test]
    fn streaming_ext_import_handles_multi_batch_flows_repeater_and_cancel() {
        let mut b = SyntheticExt::new(229);
        let service = b.service("stream.example", 443, true);
        let parsed_req = b.object((2, 0), vec![(0, service.to_be_bytes().to_vec())]);
        let mut flow_refs = Vec::new();
        for i in 0..270usize {
            let orig_req = b.byte_array(
                format!("GET /batch/{i} HTTP/1.1\r\nHost: stream.example\r\n\r\n").as_bytes(),
            );
            let orig_resp = b.byte_array(
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nitem-{i}")
                    .as_bytes(),
            );
            let mut fields = vec![
                (2, (parsed_req as i64).to_be_bytes().to_vec()),
                (11, ((1_700_000_000_000i64 + i as i64 * 10) as i64).to_be_bytes().to_vec()),
                (14, 8080i32.to_be_bytes().to_vec()),
                (15, (orig_req as i64).to_be_bytes().to_vec()),
                (18, (orig_resp as i64).to_be_bytes().to_vec()),
                (28, 12i64.to_be_bytes().to_vec()),
            ];
            if i == 10 {
                let edit_req = b.byte_array(
                    b"POST /batch/10 HTTP/1.1\r\nHost: stream.example\r\nContent-Length: 4\r\n\r\nedit",
                );
                let ann = b.note_annotations("batch-note-10");
                fields.push((13, vec![3]));
                fields.push((17, (edit_req as i64).to_be_bytes().to_vec()));
                fields.push((27, (ann as i64).to_be_bytes().to_vec()));
            }
            fields.sort_by_key(|(fid, _)| *fid);
            let item = b.object((1, 0), fields);
            flow_refs.push(item as i64);
        }
        let f0 = b.chunked_list(&flow_refs);
        let f1_empty = b.direct_list(&[]);
        let proxy_col = b.object(
            (1, 0),
            vec![
                (0, (f0 as i64).to_be_bytes().to_vec()),
                (1, (f1_empty as i64).to_be_bytes().to_vec()),
            ],
        );

        let r_title = b.utf16_direct("Streamed Tab");
        let r_draft = b.byte_array(b"GET /rep HTTP/1.1\r\nHost: stream.example\r\n\r\n");
        let rep_tab = b.object(
            (1, 0),
            vec![
                (0, (r_title as i64).to_be_bytes().to_vec()),
                (1, (-1i32).to_be_bytes().to_vec()),
                (3, (service as i64).to_be_bytes().to_vec()),
                (4, (r_draft as i64).to_be_bytes().to_vec()),
            ],
        );
        let tabs_col = b.direct_list(&[rep_tab as i64]);
        let groups_col = b.direct_list(&[]);
        let rep_section = b.object(
            (1, 0),
            vec![
                (0, 0i32.to_be_bytes().to_vec()),
                (1, (tabs_col as i64).to_be_bytes().to_vec()),
                (4, (groups_col as i64).to_be_bytes().to_vec()),
            ],
        );
        let root = b.object(
            (1, 0),
            vec![
                (1, (proxy_col as i64).to_be_bytes().to_vec()),
                (2, (rep_section as i64).to_be_bytes().to_vec()),
            ],
        );
        let tmp = tempfile::tempdir().unwrap();
        let ext_path = tmp.path().join("streaming.archive");
        b.finish(root, &ext_path);

        let pre_cancel = CancellationToken::new();
        pre_cancel.cancel();
        assert!(ProjectStore::open_ext(&ext_path, &pre_cancel).is_err());

        let cancel = CancellationToken::new();
        let store = ProjectStore::open_ext(&ext_path, &cancel).unwrap();
        let src_info = store.read_direct_ext_source().unwrap().unwrap();
        assert_eq!(src_info.report.indexed_items, 271);
        let ws = store.workspace().unwrap();
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].title, "Streamed Tab");

        let query = HttpHistoryQuery {
            project: ProjectKey {
                id: store.id,
                generation: 1,
            },
            query_revision: 1,
            offset: 0,
            limit: 50,
            url_filter: String::new(),
            filter: HttpHistoryFilter::default(),
            sort: HttpHistorySort {
                column: crate::HttpHistorySortColumn::Number,
                ascending: true,
            },
            selected_ids: Vec::new(),
        };
        let page = store.query_http_history(&query, &cancel).unwrap();
        assert_eq!(page.total, 270);
        assert_eq!(page.rows.len(), 50);
        let tenth = page.rows[10].live().unwrap();
        assert_eq!(tenth.method, "GET");
        assert_eq!(tenth.revision, 2);
        assert_eq!(store.read(tenth.id).unwrap().request.method, "POST");
        let tenth_orig = store.original(tenth.id).unwrap();
        assert_eq!(tenth_orig.request.as_ref().unwrap().method, "GET");
        let tenth_meta = page
            .row_metadata_for(crate::HistoryEntryId::Live(tenth.id))
            .unwrap();
        assert_eq!(tenth_meta.capture_number, Some(11));
        assert_eq!(tenth_meta.response_received_ms, Some(12));
        let ann = page
            .live_annotations
            .iter()
            .find(|(id, _, _, _)| *id == tenth.id)
            .unwrap();
        assert_eq!(ann.2, "batch-note-10");
        assert_eq!(ann.3.color, 3);
        store.close().unwrap();
    }
}
