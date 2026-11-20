use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use super::*;

const MAX_TRACKED_PROVENANCE_FLOWS: usize = 512;

#[derive(Clone, Default)]
pub(crate) struct ProvenanceTracker {
    inner: Arc<Mutex<HashMap<FlowId, FlowProvenance>>>,
}

impl ProvenanceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn record_request(
        &self,
        flow_id: FlowId,
        initial_original: &HttpRequest,
        actor: ProvenanceActor,
        next: &HttpRequest,
    ) {
        if initial_original == next && matches!(actor, ProvenanceActor::Original) {
            return;
        }
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !guard.contains_key(&flow_id)
            && guard.len() >= MAX_TRACKED_PROVENANCE_FLOWS
            && let Some(&victim) = guard.keys().next()
        {
            guard.remove(&victim);
        }
        let entry = guard.entry(flow_id).or_default();
        entry.push_request_step(initial_original, actor, next);
        if entry.is_empty() {
            guard.remove(&flow_id);
        }
    }

    pub fn record_response(
        &self,
        flow_id: FlowId,
        initial_original: &HttpResponse,
        actor: ProvenanceActor,
        next: &HttpResponse,
    ) {
        if initial_original == next && matches!(actor, ProvenanceActor::Original) {
            return;
        }
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !guard.contains_key(&flow_id)
            && guard.len() >= MAX_TRACKED_PROVENANCE_FLOWS
            && let Some(&victim) = guard.keys().next()
        {
            guard.remove(&victim);
        }
        let entry = guard.entry(flow_id).or_default();
        entry.push_response_step(initial_original, actor, next);
        if entry.is_empty() {
            guard.remove(&flow_id);
        }
    }

    pub fn set(&self, flow_id: FlowId, provenance: FlowProvenance) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if provenance.is_empty() {
            guard.remove(&flow_id);
            return;
        }
        if !guard.contains_key(&flow_id)
            && guard.len() >= MAX_TRACKED_PROVENANCE_FLOWS
            && let Some(&victim) = guard.keys().next()
        {
            guard.remove(&victim);
        }
        guard.insert(flow_id, provenance);
    }

    pub fn get(&self, flow_id: FlowId) -> Option<FlowProvenance> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&flow_id)
            .cloned()
    }

    pub fn remove(&self, flow_id: FlowId) -> Option<FlowProvenance> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&flow_id)
    }

    pub fn snapshot_all(&self) -> Vec<(FlowId, FlowProvenance)> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(&k, v)| (k, v.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlimSaveOptions {
    pub drop_scanner_and_unreferenced_tool_flows: bool,
    pub drop_static_asset_flows: bool,
    pub max_unpinned_body_bytes: Option<usize>,
    pub drop_finished_intruder_results: bool,
    pub compact_ext_source_chunks: bool,
}

impl Default for SlimSaveOptions {
    fn default() -> Self {
        Self {
            drop_scanner_and_unreferenced_tool_flows: true,
            drop_static_asset_flows: true,
            max_unpinned_body_bytes: Some(256 * 1024),
            drop_finished_intruder_results: false,
            compact_ext_source_chunks: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlimSaveReport {
    pub old_bytes: u64,
    pub new_bytes: u64,
    pub removed_flows: usize,
    pub compacted_ext_chunks: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvenanceActor {
    Original,
    UnpackCompressed,
    ResponseModification,
    MatchAndReplace { rule_id: Uuid, comment: String },
    SessionRule { rule_name: String },
    PluginHook { plugin_id: String, hook: String },
    ManualInterceptEdit,
    InspectorEdit,
    ProtocolNormalization { description: String },
    ExtArchiveEdited,
    Final,
}

impl ProvenanceActor {
    pub fn default_label(&self) -> String {
        match self {
            Self::Original => "原始报文 (Original)".into(),
            Self::UnpackCompressed => "自动解压 (Unpack Compressed)".into(),
            Self::ResponseModification => "HTML 响应修改 (Response Modification)".into(),
            Self::MatchAndReplace { rule_id, comment } => {
                let trimmed = comment.trim();
                if trimmed.is_empty() {
                    format!("Match & Replace ({rule_id})")
                } else {
                    format!("Match & Replace: {trimmed}")
                }
            }
            Self::SessionRule { rule_name } => {
                let trimmed = rule_name.trim();
                if trimmed.is_empty() {
                    "Session 规则".into()
                } else {
                    format!("Session 规则: {trimmed}")
                }
            }
            Self::PluginHook { plugin_id, hook } => {
                if hook.is_empty() {
                    format!("插件: {plugin_id}")
                } else {
                    format!("插件: {plugin_id} ({hook})")
                }
            }
            Self::ManualInterceptEdit => "手动拦截修改 (Intercept Edit)".into(),
            Self::InspectorEdit => "Inspector 修改".into(),
            Self::ProtocolNormalization { description } => {
                format!("协议规范化: {description}")
            }
            Self::ExtArchiveEdited => "归档已编辑报文 (Archive Edited)".into(),
            Self::Final => "最终报文 (Final)".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestProvenanceStep {
    pub label: String,
    pub actor: ProvenanceActor,
    pub request: HttpRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseProvenanceStep {
    pub label: String,
    pub actor: ProvenanceActor,
    pub response: HttpResponse,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowProvenance {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_steps: Vec<RequestProvenanceStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_steps: Vec<ResponseProvenanceStep>,
}

impl FlowProvenance {
    pub fn is_empty(&self) -> bool {
        self.request_steps.is_empty() && self.response_steps.is_empty()
    }

    pub fn push_request_step(
        &mut self,
        initial_original: &HttpRequest,
        actor: ProvenanceActor,
        next: &HttpRequest,
    ) {
        if self.request_steps.is_empty() {
            if initial_original == next {
                return;
            }
            self.request_steps.push(RequestProvenanceStep {
                label: ProvenanceActor::Original.default_label(),
                actor: ProvenanceActor::Original,
                request: initial_original.clone(),
            });
            self.request_steps.push(RequestProvenanceStep {
                label: actor.default_label(),
                actor,
                request: next.clone(),
            });
        } else if self.request_steps.last().map(|s| &s.request) != Some(next) {
            self.request_steps.push(RequestProvenanceStep {
                label: actor.default_label(),
                actor,
                request: next.clone(),
            });
        }
    }

    pub fn push_response_step(
        &mut self,
        initial_original: &HttpResponse,
        actor: ProvenanceActor,
        next: &HttpResponse,
    ) {
        if self.response_steps.is_empty() {
            if initial_original == next {
                return;
            }
            self.response_steps.push(ResponseProvenanceStep {
                label: ProvenanceActor::Original.default_label(),
                actor: ProvenanceActor::Original,
                response: initial_original.clone(),
            });
            self.response_steps.push(ResponseProvenanceStep {
                label: actor.default_label(),
                actor,
                response: next.clone(),
            });
        } else if self.response_steps.last().map(|s| &s.response) != Some(next) {
            self.response_steps.push(ResponseProvenanceStep {
                label: actor.default_label(),
                actor,
                response: next.clone(),
            });
        }
    }
}

impl OriginalMessages {
    pub fn synthesize_provenance(
        &mut self,
        final_request: Option<&HttpRequest>,
        final_response: Option<&HttpResponse>,
    ) {
        if !self.provenance.request_steps.is_empty()
            && let Some(fin) = final_request
        {
            if self
                .provenance
                .request_steps
                .first()
                .is_some_and(|first| &first.request == fin)
                && self.request.is_none()
            {
                self.provenance.request_steps.clear();
            } else if self
                .provenance
                .request_steps
                .last()
                .is_some_and(|last| &last.request != fin)
            {
                self.provenance.request_steps.push(RequestProvenanceStep {
                    label: ProvenanceActor::ManualInterceptEdit.default_label(),
                    actor: ProvenanceActor::ManualInterceptEdit,
                    request: fin.clone(),
                });
            }
        }

        if !self.provenance.response_steps.is_empty()
            && let Some(fin) = final_response
        {
            if self.response_body_complete.is_some() {
                for step in &mut self.provenance.response_steps {
                    if step.response.body.is_empty() && !fin.body.is_empty() {
                        step.response.body = fin.body.clone();
                        step.response.trailers = fin.trailers.clone();
                    }
                }
            }
            if self
                .provenance
                .response_steps
                .first()
                .is_some_and(|first| &first.response == fin)
                && self.response.is_none()
            {
                self.provenance.response_steps.clear();
            } else if self
                .provenance
                .response_steps
                .last()
                .is_some_and(|last| &last.response != fin)
            {
                self.provenance.response_steps.push(ResponseProvenanceStep {
                    label: ProvenanceActor::ManualInterceptEdit.default_label(),
                    actor: ProvenanceActor::ManualInterceptEdit,
                    response: fin.clone(),
                });
            }
        }
    }
}
