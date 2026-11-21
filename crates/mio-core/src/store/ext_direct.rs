use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use anyhow::Result;
use rusqlite::{TransactionBehavior, params};
use tokio_util::sync::CancellationToken;

use super::{
    ProjectStore,
    messages::{
        ImportedFlowHeadMeta, read_next_proxy_capture_number, write_imported_capture,
        write_next_proxy_capture_number, write_record_with_reason_phrase,
    },
    metadata,
    plugin_storage::{LiveHttpAnnotation, write_live_http_annotations},
    repeater_history::insert_imported_history_entry,
    write_workspace,
};
use crate::{
    ExtImportPhase, ExtImportReport, ExtSourceInfo, Draft, FlowRecord, FlowState, FlowSummary,
    RepeaterHistory, RepeaterHistoryCursor, RepeaterHistoryEntryId, TrafficSource, Uuid, WorkTab,
    ext_archive::{
        direct_import::{StreamExtEvent, stream_ext_file},
        import::check_cancel,
    },
};

pub(in crate::store) const DIRECT_EXT_REPORT_KEY: &str = "ext_direct_report";
const PROXY_BATCH_SIZE: usize = 256;
const REPEATER_BATCH_SIZE: usize = 32;

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::ExtArchiveRepo<S> {
    pub(crate) fn read_direct_source(&self) -> Result<Option<ExtSourceInfo>> {
        let tx = self.0.connection.unchecked_transaction()?;
        let info = {
            let mut stmt = tx.prepare(
                "SELECT CASE WHEN typeof(value)='blob' AND length(value)<=262144 THEN value END
                 FROM main.metadata WHERE key=?1 COLLATE BINARY",
            )?;
            let mut rows = stmt.query([DIRECT_EXT_REPORT_KEY])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            let Some(bytes): Option<Vec<u8>> = row.get(0)? else {
                anyhow::bail!("外部导入摘要记录类型无效或超过限额");
            };
            serde_json::from_slice(&bytes)?
        };
        tx.commit()?;
        Ok(Some(info))
    }
}

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::FlowsRepo<S> {
    pub(crate) fn annotated_live_flow_ids(&self) -> Result<HashSet<Uuid>> {
        let map = super::plugin_storage::read_live_http_annotations(&self.0.connection)?;
        Ok(map.into_keys().collect())
    }
}

impl ProjectStore {
    pub(crate) fn open_ext(path: &Path, cancel: &CancellationToken) -> Result<Self> {
        check_cancel(cancel)?;
        let default_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
            .map(str::to_owned);
        let mut store = Self::create(None)?;
        store.cancel_queries_with(cancel.clone());
        let source_id = Uuid::new_v4();
        let _ = store.ext_archive_mut().import_file_streaming(
            source_id,
            path,
            default_name,
            cancel,
            &mut |_, _, _| {},
        )?;
        store.checkpoint()?;
        let _ = store.connection.execute_batch("PRAGMA shrink_memory;");
        reclaim_macos_malloc_pages();
        Ok(store)
    }
}

impl crate::store::ExtArchiveRepo<&mut ProjectStore> {
    pub(crate) fn import_file_streaming(
        &mut self,
        source_id: Uuid,
        ext_path: &Path,
        project_name: Option<String>,
        cancel: &CancellationToken,
        progress: &mut dyn FnMut(ExtImportPhase, u64, Option<u64>),
    ) -> Result<ExtImportReport> {
        check_cancel(cancel)?;
        let now_unix_ms = crate::codec_util::now_unix_ms();
        let (
            mut final_report,
            active_tab_index,
            mut annotations,
            max_capture_number,
            mut passive_findings,
            passive_dedup,
            repeater_groups,
            imported_tabs,
        ) = {
            let conn = &self.0.connection;
            let mut active_tx: Option<rusqlite::Transaction<'_>> = None;
            let mut batch_proxy_count = 0usize;
            let mut batch_tab_count = 0usize;
            let mut annotations: BTreeMap<Uuid, LiveHttpAnnotation> = BTreeMap::new();
            let mut max_capture_number = 0u64;
            let mut passive_findings: Vec<crate::AuditFinding> = Vec::new();
            let mut passive_dedup: HashSet<(String, String, String, Option<String>)> =
                HashSet::new();
            let mut repeater_groups = Vec::new();
            let mut imported_tabs: Vec<(usize, WorkTab)> = Vec::new();

            let (final_report, active_tab_index) = stream_ext_file(
                ext_path,
                cancel,
                progress,
                &mut |event| match event {
                    StreamExtEvent::ProxyFlow(item) => {
                        if batch_proxy_count % 64 == 0 {
                            check_cancel(cancel)?;
                        }
                        let tx = match active_tx.as_mut() {
                            Some(tx) => tx,
                            None => active_tx.insert(rusqlite::Transaction::new_unchecked(
                                conn,
                                TransactionBehavior::Immediate,
                            )?),
                        };
                        let flow_id = ordered_flow_uuid(item.index);
                        let capture_number = item
                            .index
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("外部归档 HTTP 历史序号溢出"))?;
                        max_capture_number = max_capture_number.max(capture_number);
                        let head_meta = ImportedFlowHeadMeta {
                            time_known: item.time_known,
                            capture_number,
                            remote_ip: &item.remote_ip,
                            response_received_ms: item.response_received_ms,
                        };
                        let record_passive = |record: &FlowRecord,
                                              started_unix_ms: i64,
                                              findings: &mut Vec<crate::AuditFinding>,
                                              dedup: &mut HashSet<(
                            String,
                            String,
                            String,
                            Option<String>,
                        )>| {
                            if findings.len() < 500 {
                                let ts = if started_unix_ms > 0 {
                                    started_unix_ms
                                } else {
                                    now_unix_ms
                                };
                                for finding in crate::audit_engine::inspect_flow_passive(record, ts)
                                {
                                    let key = (
                                        finding.issue_id.clone(),
                                        finding.host.clone(),
                                        finding.url.clone(),
                                        finding.parameter.clone(),
                                    );
                                    if dedup.insert(key) && findings.len() < 500 {
                                        findings.push(finding);
                                    }
                                }
                            }
                        };

                        let (orig_resp, orig_reason) = match item.original_response {
                            Some((r, reason)) => (Some(r), reason),
                            None => (None, None),
                        };
                        let first_record = FlowRecord {
                            summary: FlowSummary {
                                id: flow_id,
                                revision: 1,
                                source: TrafficSource::Proxy,
                                state: FlowState::Complete,
                                started_unix_ms: item.started_unix_ms,
                                method: item.original_request.method.clone(),
                                url: item.original_request.url.clone(),
                                request_bytes: item.original_request.body.len() as u64,
                                status: orig_resp.as_ref().map(|r| r.status),
                                response_bytes: orig_resp
                                    .as_ref()
                                    .map_or(0, |r| r.body.len() as u64),
                                elapsed_ms: item.elapsed_ms,
                            },
                            request: item.original_request,
                            response: orig_resp,
                        };
                        write_imported_capture(
                            tx,
                            &first_record,
                            orig_reason.as_deref(),
                            item.listener_port,
                            &head_meta,
                        )?;
                        if item.edited_request.is_none() && item.edited_response.is_none() {
                            record_passive(
                                &first_record,
                                item.started_unix_ms,
                                &mut passive_findings,
                                &mut passive_dedup,
                            );
                        } else {
                            let final_req =
                                item.edited_request.unwrap_or(first_record.request);
                            let (final_resp, final_reason) = match item.edited_response {
                                Some((r, reason)) => (Some(r), reason),
                                None => (first_record.response, orig_reason),
                            };
                            let second_record = FlowRecord {
                                summary: FlowSummary {
                                    id: flow_id,
                                    revision: 2,
                                    source: TrafficSource::Proxy,
                                    state: FlowState::Complete,
                                    started_unix_ms: item.started_unix_ms,
                                    method: final_req.method.clone(),
                                    url: final_req.url.clone(),
                                    request_bytes: final_req.body.len() as u64,
                                    status: final_resp.as_ref().map(|r| r.status),
                                    response_bytes: final_resp
                                        .as_ref()
                                        .map_or(0, |r| r.body.len() as u64),
                                    elapsed_ms: item.elapsed_ms,
                                },
                                request: final_req,
                                response: final_resp,
                            };
                            write_imported_capture(
                                tx,
                                &second_record,
                                final_reason.as_deref(),
                                item.listener_port,
                                &head_meta,
                            )?;
                            record_passive(
                                &second_record,
                                item.started_unix_ms,
                                &mut passive_findings,
                                &mut passive_dedup,
                            );
                        }

                        if !item.notes.is_empty() || item.highlight > 0 {
                            annotations.insert(
                                flow_id,
                                LiveHttpAnnotation {
                                    notes_revision: u64::from(!item.notes.is_empty()),
                                    notes: item.notes,
                                    highlight_revision: u64::from(item.highlight > 0),
                                    highlight: item.highlight,
                                },
                            );
                        }

                        batch_proxy_count += 1;
                        if batch_proxy_count >= PROXY_BATCH_SIZE {
                            if let Some(tx) = active_tx.take() {
                                tx.commit()?;
                            }
                            batch_proxy_count = 0;
                            let _ = conn.execute_batch("PRAGMA shrink_memory;");
                            reclaim_macos_malloc_pages();
                            check_cancel(cancel)?;
                        }
                        Ok(())
                    }
                    StreamExtEvent::RepeaterGroups(groups) => {
                        if let Some(tx) = active_tx.take() {
                            tx.commit()?;
                            batch_proxy_count = 0;
                            let _ = conn.execute_batch("PRAGMA shrink_memory;");
                            reclaim_macos_malloc_pages();
                        }
                        repeater_groups = groups;
                        Ok(())
                    }
                    StreamExtEvent::RepeaterTab {
                        index: tab_idx_out,
                        tab: imported_tab,
                    } => {
                        check_cancel(cancel)?;
                        let tx = match active_tx.as_mut() {
                            Some(tx) => tx,
                            None => active_tx.insert(rusqlite::Transaction::new_unchecked(
                                conn,
                                TransactionBehavior::Immediate,
                            )?),
                        };
                        let tab_id = Uuid::new_v4();
                        let mut history_flow_ids =
                            Vec::with_capacity(imported_tab.history.len());
                        for (hist_ord, hist_item) in
                            imported_tab.history.into_iter().enumerate()
                        {
                            let flow_id = Uuid::new_v4();
                            let (resp, reason) = match hist_item.response {
                                Some((r, reason)) => (Some(r), reason),
                                None => (None, None),
                            };
                            let state = if resp.is_some() {
                                FlowState::Complete
                            } else {
                                FlowState::Cancelled
                            };
                            let record = FlowRecord {
                                summary: FlowSummary {
                                    id: flow_id,
                                    revision: 1,
                                    source: TrafficSource::Repeater,
                                    state,
                                    started_unix_ms: hist_item.started_unix_ms,
                                    method: hist_item.request.method.clone(),
                                    url: hist_item.request.url.clone(),
                                    request_bytes: hist_item.request.body.len() as u64,
                                    status: resp.as_ref().map(|r| r.status),
                                    response_bytes: resp
                                        .as_ref()
                                        .map_or(0, |r| r.body.len() as u64),
                                    elapsed_ms: hist_item.elapsed_ms,
                                },
                                request: hist_item.request,
                                response: resp,
                            };
                            write_record_with_reason_phrase(tx, &record, reason.as_deref())?;
                            insert_imported_history_entry(
                                tx,
                                tab_id,
                                hist_ord as u64,
                                flow_id,
                                &hist_item.target,
                                record.request.version,
                                &hist_item.raw_request,
                            )?;
                            history_flow_ids.push(flow_id);
                        }

                        let selected_flow = imported_tab
                            .selected_history
                            .and_then(|sel_idx| history_flow_ids.get(sel_idx).copied());
                        let history = if !history_flow_ids.is_empty() {
                            Some(RepeaterHistory {
                                source: None,
                                cursor: selected_flow.map(|flow_id| RepeaterHistoryCursor {
                                    entry: RepeaterHistoryEntryId::Live(flow_id),
                                    draft_revision: 1,
                                }),
                            })
                        } else {
                            None
                        };

                        imported_tabs.push((
                            tab_idx_out,
                            WorkTab {
                                id: tab_id,
                                title: imported_tab.title,
                                group: imported_tab.group,
                                draft: Draft {
                                    revision: 1,
                                    target: imported_tab.draft_target,
                                    raw: imported_tab.draft_raw,
                                    version: imported_tab.draft_version,
                                },
                                last_flow: selected_flow,
                                archive_response: None,
                                history,
                            },
                        ));

                        batch_tab_count += 1;
                        if batch_tab_count >= REPEATER_BATCH_SIZE {
                            if let Some(tx) = active_tx.take() {
                                tx.commit()?;
                            }
                            batch_tab_count = 0;
                            let _ = conn.execute_batch("PRAGMA shrink_memory;");
                            reclaim_macos_malloc_pages();
                            check_cancel(cancel)?;
                        }
                        Ok(())
                    }
                },
            )?;

            if let Some(tx) = active_tx.take() {
                tx.commit()?;
            }
            (
                final_report,
                active_tab_index,
                annotations,
                max_capture_number,
                passive_findings,
                passive_dedup,
                repeater_groups,
                imported_tabs,
            )
        };

        let tx = rusqlite::Transaction::new_unchecked(
            &self.0.connection,
            TransactionBehavior::Immediate,
        )?;

        if !annotations.is_empty() {
            write_live_http_annotations(&tx, &mut annotations, cancel)?;
        }
        drop(annotations);
        if max_capture_number > 0 {
            let next_capture = max_capture_number
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("外部归档 HTTP 历史序号计数器溢出"))?;
            let current_next = read_next_proxy_capture_number(&tx)?.unwrap_or(1);
            write_next_proxy_capture_number(&tx, current_next.max(next_capture))?;
        }

        let mut workspace = metadata::workspace(&tx)?;
        let mut workspace_changed = false;
        if !workspace.upstream_tls.insecure_skip_verify {
            workspace.upstream_tls.insecure_skip_verify = true;
            workspace_changed = true;
        }
        if let Some(name) = project_name
            && workspace.project_name.as_ref() != Some(&name)
        {
            workspace.project_name = Some(name);
            workspace_changed = true;
        }
        if !repeater_groups.is_empty() {
            workspace.groups = repeater_groups;
            workspace_changed = true;
        }
        for (tab_idx, work_tab) in imported_tabs {
            let tab_id = work_tab.id;
            workspace.tabs.push(work_tab);
            if active_tab_index == Some(tab_idx) {
                workspace.active = Some(tab_id);
            }
            workspace_changed = true;
        }

        if workspace.active.is_none()
            && let Some(first) = workspace.tabs.first()
        {
            workspace.active = Some(first.id);
        }
        if workspace_changed {
            workspace.revision = workspace.revision.saturating_add(1);
            write_workspace(&tx, &workspace)?;
        }

        final_report.complete_archive = true;
        super::ext_archive::encode_report(&final_report)?;
        let source_info = ExtSourceInfo {
            id: source_id,
            report: final_report.clone(),
        };
        let encoded_source = serde_json::to_vec(&source_info)?;
        tx.execute(
            "INSERT INTO main.metadata(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![DIRECT_EXT_REPORT_KEY, encoded_source],
        )?;
        check_cancel(cancel)?;
        tx.commit()?;
        if !passive_findings.is_empty() {
            passive_findings.sort_by(|a, b| {
                b.severity_rank
                    .cmp(&a.severity_rank)
                    .then_with(|| b.detected_unix_ms.cmp(&a.detected_unix_ms))
            });
            let mut audit_state = self.0.audit().read_state()?;
            let mut existing: HashSet<(String, String, String, Option<String>)> = audit_state
                .findings
                .iter()
                .map(|f| {
                    (
                        f.issue_id.clone(),
                        f.host.clone(),
                        f.url.clone(),
                        f.parameter.clone(),
                    )
                })
                .collect();
            let mut added = false;
            for finding in passive_findings {
                let key = (
                    finding.issue_id.clone(),
                    finding.host.clone(),
                    finding.url.clone(),
                    finding.parameter.clone(),
                );
                if existing.insert(key) {
                    audit_state.findings.push(finding);
                    added = true;
                }
            }
            if added {
                audit_state.revision = audit_state.revision.saturating_add(1);
                let _ = self.0.audit_mut().write_state(&audit_state, cancel);
            }
        }
        drop(passive_dedup);
        self.0.invalidate_workspace_cache();
        let _ = self.0.connection.execute_batch("PRAGMA shrink_memory;");
        reclaim_macos_malloc_pages();
        Ok(final_report)
    }
}

#[cfg(target_os = "macos")]
fn reclaim_macos_malloc_pages() {
    unsafe extern "C" {
        fn malloc_default_zone() -> *mut std::ffi::c_void;
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    // SAFETY: Passing NULL examines all malloc zones; explicitly relieving the default
    // zone as well returns empty small/tiny magazines back to the OS on Apple Silicon.
    unsafe {
        let _ = malloc_zone_pressure_relief(malloc_default_zone(), 0);
        let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn reclaim_macos_malloc_pages() {}

fn ordered_flow_uuid(index: u64) -> Uuid {
    let high = (u64::MAX - index).max(1) as u128;
    let low = (Uuid::new_v4().as_u128() & 0xffff_ffff_ffff_ffff).max(1);
    Uuid::from_u128((high << 64) | low)
}
