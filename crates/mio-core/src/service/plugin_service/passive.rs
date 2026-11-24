use url::Url;

use super::hooks::plugin_hook_depth;
use super::*;
use crate::{store::audit::merge_audit_findings, target_issues::IssueSeverity};

pub(super) const PLUGIN_PASSIVE_ISSUE_ID: &str = "0x00900100";
pub(in crate::service) const PLUGIN_ACTIVE_ISSUE_ID: &str = "0x00900101";

fn map_plugin_severity(raw: i32) -> IssueSeverity {
    match protocol::FindingSeverity::try_from(raw) {
        Ok(protocol::FindingSeverity::High) => IssueSeverity::High,
        Ok(protocol::FindingSeverity::Medium) => IssueSeverity::Medium,
        Ok(protocol::FindingSeverity::Low) => IssueSeverity::Low,
        _ => IssueSeverity::Information,
    }
}

fn map_plugin_confidence(raw: i32) -> IssueConfidence {
    match protocol::FindingConfidence::try_from(raw) {
        Ok(protocol::FindingConfidence::Certain) => IssueConfidence::Certain,
        Ok(protocol::FindingConfidence::Tentative) => IssueConfidence::Tentative,
        _ => IssueConfidence::Firm,
    }
}

fn evidence_spans(raw: &[u8], needle: &[u8]) -> Vec<EvidenceSpan> {
    if needle.is_empty() || raw.len() < needle.len() {
        return Vec::new();
    }
    raw.windows(needle.len())
        .position(|w| w == needle)
        .map(|offset| {
            vec![EvidenceSpan {
                offset,
                length: needle.len(),
            }]
        })
        .unwrap_or_default()
}

pub(in crate::service) fn convert_plugin_findings(
    flow_id: FlowId,
    url: &str,
    response: &HttpResponse,
    dtos: Vec<protocol::PluginFindingDto>,
) -> Vec<AuditFinding> {
    convert_plugin_findings_for_phase(
        flow_id,
        url,
        response,
        dtos,
        AuditPhase::Passive,
        PLUGIN_PASSIVE_ISSUE_ID,
    )
}

pub(in crate::service) fn convert_plugin_findings_for_phase(
    flow_id: FlowId,
    url: &str,
    response: &HttpResponse,
    dtos: Vec<protocol::PluginFindingDto>,
    phase: AuditPhase,
    issue_id: &str,
) -> Vec<AuditFinding> {
    let host = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    let raw_response = response_to_raw(response);
    let now_unix_ms = crate::codec_util::now_unix_ms();
    dtos.into_iter()
        .map(|dto| {
            let sev = map_plugin_severity(dto.severity);
            let confidence = map_plugin_confidence(dto.confidence);
            let title = if dto.title.trim().is_empty() {
                if phase == AuditPhase::Active {
                    "插件主动扫描检查告警".to_owned()
                } else {
                    "插件被动合规检查告警".to_owned()
                }
            } else {
                dto.title.trim().to_owned()
            };
            let summary = if dto.detail.trim().is_empty() {
                title.clone()
            } else {
                dto.detail.trim().to_owned()
            };
            let remediation = if dto.remediation.trim().is_empty() {
                "请根据插件合规检查建议修复响应暴露面。".to_owned()
            } else {
                dto.remediation.trim().to_owned()
            };
            let evidence: String = dto.evidence.chars().take(512).collect();
            let response_highlights = evidence_spans(&raw_response, evidence.as_bytes());
            let parameter = (!dto.parameter.trim().is_empty()).then(|| dto.parameter.trim().into());
            AuditFinding {
                id: Uuid::new_v4(),
                issue_id: issue_id.to_owned(),
                title,
                severity_label: sev.label().to_owned(),
                severity_rank: sev.rank(),
                confidence,
                phase,
                host: host.clone(),
                url: url.to_owned(),
                parameter,
                flow_id: Some(flow_id),
                summary,
                evidence,
                request_highlights: Vec::new(),
                response_highlights,
                remediation,
                detected_unix_ms: now_unix_ms,
            }
        })
        .collect()
}

impl State {
    pub(in crate::service) fn spawn_plugin_passive_checks(&mut self, record: &FlowRecord) {
        let handles = self.passive_check_hook_entries();
        if handles.is_empty() {
            return;
        }
        if self.extensions_disabled
            || self.quiesced
            || self.plugins.stopping
            || self.unsaved_count() != 0
            || record.summary.state != FlowState::Complete
            || plugin_hook_depth(&record.request) > 1
        {
            return;
        }
        let persisted = self.store.flows().read_optional(record.summary.id).ok().flatten();
        if persisted
            .as_ref()
            .is_none_or(|stored| stored.summary.revision < record.summary.revision)
        {
            return;
        }
        let Some(response) = record.response.as_ref() else {
            return;
        };
        let Ok(audit_state) = self.store.audit().read_state() else {
            return;
        };
        let scope = self
            .store
            .metadata()
            .workspace_arc()
            .ok()
            .map(|ws| ws.scope.clone());
        if !audit_state.live_passive_enabled
            || (audit_state.live_passive_suite_scope_only
                && scope
                    .as_ref()
                    .is_some_and(|s| !s.is_url_in_scope(&record.summary.url)))
        {
            return;
        }
        let resp_ct = super::hooks::response_content_type(response);
        let inline_fits = super::hooks::http_request_fits_inline_fast(&record.request)
            && super::hooks::http_response_fits_inline_fast(response);
        let flow_id = record.summary.id;
        let url = record.request.url.clone();
        let wire_call = if inline_fits {
            let call = protocol::PassiveCheckCall {
                flow_id: flow_id.as_bytes().to_vec(),
                url: url.clone(),
                request: Some(record.request.clone().into()),
                response: Some(response.clone().into()),
            };
            match protocol::try_into_wire_body(
                protocol::envelope::Body::PassiveCheckCall(call),
                protocol::PROTOCOL_MINOR,
            ) {
                Ok(protocol::envelope::Body::PassiveCheckCall(call)) => Some(call),
                _ => None,
            }
        } else {
            None
        };
        let project = self.project;
        for (identity, handle, filter, counters) in handles {
            if handle.protocol_minor() < protocol::V16_COMPAT_MINOR {
                continue;
            }
            if !filter.should_invoke_http(&record.request.url, true, resp_ct, scope.as_ref()) {
                counters.inc_filtered_skipped();
                continue;
            }
            let (call, spill) = if let Some(call) = wire_call.as_ref() {
                (call.clone(), super::hooks::large_body::HookSpillScope::default())
            } else if let Some(prepared) = super::hooks::large_body::prepare_passive_check_call(
                flow_id,
                &url,
                &record.request,
                response,
                handle.protocol_minor(),
                handle.supports_large_body(),
            ) {
                prepared
            } else {
                counters.inc_oversized_bypassed();
                continue;
            };
            if self.plugins.operations.len() >= MAX_OPERATIONS {
                break;
            }
            let (instance_id, url) = (identity.instance_id, url.clone());
            let resp_copy = Box::new(response.clone());
            let op_id = Uuid::new_v4();
            let cancel = CancellationToken::new();
            let task_cancel = cancel.clone();
            counters.inc_invoked();
            let job_counters = std::sync::Arc::clone(&counters);
            self.plugin_job_start(op_id, Some(instance_id), cancel, false, async move {
                let _spill = spill;
                let result = match handle
                    .call_passive_check_with_cancel(call, task_cancel.clone())
                    .await
                {
                    Ok(res) => res,
                    Err(err) => {
                        if !task_cancel.is_cancelled() {
                            job_counters.inc_error_or_timeout();
                        }
                        return Err(err).context("被动合规检查调用失败");
                    }
                };
                Ok(Output::PassiveChecked {
                    project,
                    flow_id,
                    url,
                    response: resp_copy,
                    findings: result.findings,
                })
            });
            if let Some(op) = self.plugins.operations.get_mut(&op_id) {
                op.background_passive = true;
            }
        }
    }

    pub(super) async fn finish_passive_checked(
        &mut self,
        project: ProjectKey,
        flow_id: FlowId,
        url: String,
        response: HttpResponse,
        findings: Vec<protocol::PluginFindingDto>,
    ) {
        if project != self.project
            || self.quiesced
            || self.plugins.stopping
            || self.unsaved_count() != 0
            || findings.is_empty()
        {
            return;
        }
        let Ok(mut state) = self.store.audit().read_state() else {
            return;
        };
        if !state.live_passive_enabled
            || (state.live_passive_suite_scope_only
                && self
                    .store
                    .metadata()
                    .workspace_arc()
                    .is_ok_and(|ws| !ws.scope.is_url_in_scope(&url)))
        {
            return;
        }
        let incoming = convert_plugin_findings(flow_id, &url, &response, findings);
        let added = merge_audit_findings(&mut state.findings, incoming);
        if added.is_empty() {
            return;
        }
        state.revision += 1;
        if let Err(error) = self
            .store
            .audit_mut()
            .write_state(&state, &CancellationToken::new())
        {
            self.error("audit", error).await;
            return;
        }
        self.emit(Event::Audit(AuditEvent::State {
            project: self.project,
            query_revision: 0,
            result: Ok(Box::new(state)),
        }))
        .await;
    }
}
