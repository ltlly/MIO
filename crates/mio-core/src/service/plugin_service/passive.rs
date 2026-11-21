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
    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
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
        let handles = self.passive_check_hooks();
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
        let persisted = self.store.read_optional(record.summary.id).ok().flatten();
        if persisted
            .as_ref()
            .is_none_or(|stored| stored.summary.revision < record.summary.revision)
        {
            return;
        }
        let Some(response) = record.response.as_ref() else {
            return;
        };
        if record.request.url.is_empty()
            || record.request.url.len() > 16 * 1024
            || record.request.body.len() > protocol::MAX_INLINE_BODY
            || response.body.len() > protocol::MAX_INLINE_BODY
        {
            return;
        }
        if !self
            .store
            .read_audit_state()
            .is_ok_and(|s| s.live_passive_enabled)
        {
            return;
        }
        let request_dto: protocol::HttpRequestDto = record.request.clone().into();
        let response_dto: protocol::HttpResponseDto = response.clone().into();
        if request_dto.validate().is_err() || response_dto.validate().is_err() {
            return;
        }
        let flow_id = record.summary.id;
        let url = record.request.url.clone();
        let call = protocol::PassiveCheckCall {
            flow_id: flow_id.as_bytes().to_vec(),
            url: url.clone(),
            request: Some(request_dto),
            response: Some(response_dto),
        };
        let project = self.project;
        for handle in handles {
            if self.plugins.operations.len() >= MAX_OPERATIONS {
                break;
            }
            let (instance_id, call, url) = (handle.identity.instance_id, call.clone(), url.clone());
            let resp_copy = Box::new(response.clone());
            let op_id = Uuid::new_v4();
            let cancel = CancellationToken::new();
            let task_cancel = cancel.clone();
            self.plugin_job_start(op_id, Some(instance_id), cancel, false, async move {
                let result = handle
                    .call_passive_check_with_cancel(call, task_cancel)
                    .await
                    .with_context(|| "被动合规检查调用失败")?;
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
        let Ok(mut state) = self.store.read_audit_state() else {
            return;
        };
        if !state.live_passive_enabled {
            return;
        }
        let incoming = convert_plugin_findings(flow_id, &url, &response, findings);
        let added = merge_audit_findings(&mut state.findings, incoming);
        if added.is_empty() {
            return;
        }
        let Some(next_rev) = state
            .revision
            .checked_add(1)
            .filter(|rev| *rev <= i64::MAX as u64)
        else {
            return;
        };
        state.revision = next_rev;
        if let Err(error) = self
            .store
            .write_audit_state(&state, &CancellationToken::new())
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
