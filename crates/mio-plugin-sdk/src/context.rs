use std::{
    collections::VecDeque,
    io::{Read, Write},
};

use mio_plugin_api::{
    FlowRecord, HttpRequest, HttpResponse, Uuid,
    protocol::{
        AnnotationsDto, CollaboratorGenerateDto, CollaboratorInteractionDto,
        CollaboratorPayloadDto, CollaboratorPollDto, DraftUpdate, Envelope, HistoryPage,
        HistoryQuery, HostCall, OpenOrganiserDto, Permission, PluginFindingDto, ScopeUpdateDto,
        SiteMapPageDto, SiteMapQueryDto, StorageEntry, SubmitAuditIssueDto, UpdateAnnotationsDto,
        WorkerSession, envelope, host_call, host_result,
    },
};

pub trait HostClient {
    fn grants(&self) -> &[Permission];
    fn has_permission(&self, permission: Permission) -> bool {
        self.grants().contains(&permission)
    }
    fn current_panel_revision(&self) -> u64;
    fn next_panel_revision(&mut self) -> u64;
    fn panel_id(&self) -> u64 {
        mio_plugin_api::protocol::PANEL_ID
    }
    fn call_host(&mut self, operation: host_call::Operation)
    -> Result<host_result::Outcome, String>;

    fn query_history(&mut self, offset: u64, limit: u32, filter: &str) -> Result<HistoryPage, String> {
        match self.call_host(host_call::Operation::QueryHistory(HistoryQuery {
            offset,
            limit,
            filter: filter.to_owned(),
        }))? {
            host_result::Outcome::History(page) => Ok(page),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn read_flow(&mut self, flow_id: Uuid) -> Result<FlowRecord, String> {
        match self.call_host(host_call::Operation::ReadFlow(flow_id.as_bytes().to_vec()))? {
            host_result::Outcome::Record(record) => (*record).try_into(),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn update_draft(
        &mut self,
        resource_id: Uuid,
        expected_revision: u64,
        request: HttpRequest,
    ) -> Result<(Uuid, u64), String> {
        match self.call_host(host_call::Operation::UpdateDraft(DraftUpdate {
            resource_id: resource_id.as_bytes().to_vec(),
            expected_revision,
            request: Some(request.into()),
        }))? {
            host_result::Outcome::Resource(res) => {
                Ok((mio_plugin_api::protocol::uuid(&res.resource_id)?, res.revision))
            }
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn open_repeater(&mut self, request: HttpRequest) -> Result<(Uuid, u64), String> {
        match self.call_host(host_call::Operation::OpenRepeater(request.into()))? {
            host_result::Outcome::Resource(res) => {
                Ok((mio_plugin_api::protocol::uuid(&res.resource_id)?, res.revision))
            }
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn open_intruder(&mut self, request: HttpRequest) -> Result<(), String> {
        match self.call_host(host_call::Operation::OpenIntruder(request.into()))? {
            host_result::Outcome::Done(_) => Ok(()),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn network_send(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        match self.call_host(host_call::Operation::NetworkSend(request.into()))? {
            host_result::Outcome::Response(resp) => resp.try_into(),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn storage_get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self.call_host(host_call::Operation::StorageGet(key.to_owned()))? {
            host_result::Outcome::StorageValue(val) => {
                Ok(if val.present { Some(val.value) } else { None })
            }
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn storage_set(&mut self, key: &str, value: Vec<u8>) -> Result<(), String> {
        match self.call_host(host_call::Operation::StorageSet(StorageEntry {
            key: key.to_owned(),
            value,
        }))? {
            host_result::Outcome::Done(_) => Ok(()),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn preferences_get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self.call_host(host_call::Operation::PreferencesGet(key.to_owned()))? {
            host_result::Outcome::StorageValue(val) => {
                Ok(if val.present { Some(val.value) } else { None })
            }
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn preferences_set(&mut self, key: &str, value: Vec<u8>) -> Result<(), String> {
        match self.call_host(host_call::Operation::PreferencesSet(StorageEntry {
            key: key.to_owned(),
            value,
        }))? {
            host_result::Outcome::Done(_) => Ok(()),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn is_in_scope(&mut self, url: &str) -> Result<bool, String> {
        match self.call_host(host_call::Operation::ScopeCheck(url.to_owned()))? {
            host_result::Outcome::ScopeStatus(status) => Ok(status.in_scope),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn add_scope_include(
        &mut self,
        url_prefix: &str,
        include_subdomains: bool,
    ) -> Result<(), String> {
        match self.call_host(host_call::Operation::ScopeUpdate(ScopeUpdateDto {
            url_prefix: url_prefix.to_owned(),
            exclude: false,
            include_subdomains,
        }))? {
            host_result::Outcome::Done(_) => Ok(()),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn add_scope_exclude(
        &mut self,
        url_prefix: &str,
        include_subdomains: bool,
    ) -> Result<(), String> {
        match self.call_host(host_call::Operation::ScopeUpdate(ScopeUpdateDto {
            url_prefix: url_prefix.to_owned(),
            exclude: true,
            include_subdomains,
        }))? {
            host_result::Outcome::Done(_) => Ok(()),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn query_site_map(
        &mut self,
        prefix: &str,
        in_scope_only: bool,
        offset: u64,
        limit: u32,
    ) -> Result<SiteMapPageDto, String> {
        match self.call_host(host_call::Operation::QuerySiteMap(SiteMapQueryDto {
            prefix: prefix.to_owned(),
            in_scope_only,
            offset,
            limit,
        }))? {
            host_result::Outcome::SiteMap(page) => Ok(page),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn read_annotations(&mut self, flow_id: Uuid) -> Result<AnnotationsDto, String> {
        match self.call_host(host_call::Operation::ReadAnnotations(
            flow_id.as_bytes().to_vec(),
        ))? {
            host_result::Outcome::Annotations(ann) => Ok(ann),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn update_annotations(
        &mut self,
        flow_id: Uuid,
        notes: Option<String>,
        highlight_color: Option<u32>,
    ) -> Result<AnnotationsDto, String> {
        match self.call_host(host_call::Operation::UpdateAnnotations(
            UpdateAnnotationsDto {
                flow_id: flow_id.as_bytes().to_vec(),
                notes,
                highlight_color,
            },
        ))? {
            host_result::Outcome::Annotations(ann) => Ok(ann),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn open_organiser(
        &mut self,
        request: HttpRequest,
        response: Option<HttpResponse>,
        notes: &str,
        highlight_color: Option<u32>,
    ) -> Result<(Uuid, u64), String> {
        match self.call_host(host_call::Operation::OpenOrganiser(OpenOrganiserDto {
            request: Some(request.into()),
            response: response.map(Into::into),
            notes: notes.to_owned(),
            highlight_color,
        }))? {
            host_result::Outcome::Resource(res) => {
                Ok((mio_plugin_api::protocol::uuid(&res.resource_id)?, res.revision))
            }
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn submit_audit_issue(
        &mut self,
        flow_id: Option<Uuid>,
        url: &str,
        finding: PluginFindingDto,
    ) -> Result<(), String> {
        match self.call_host(host_call::Operation::SubmitAuditIssue(SubmitAuditIssueDto {
            flow_id: flow_id.map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
            url: url.to_owned(),
            finding: Some(finding),
        }))? {
            host_result::Outcome::Done(_) => Ok(()),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn collaborator_generate(&mut self, label: &str) -> Result<CollaboratorPayloadDto, String> {
        match self.call_host(host_call::Operation::CollaboratorGenerate(
            CollaboratorGenerateDto {
                label: label.to_owned(),
            },
        ))? {
            host_result::Outcome::CollaboratorPayload(payload) => Ok(payload),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }

    fn collaborator_poll(
        &mut self,
        token_filter: &str,
        limit: u32,
    ) -> Result<Vec<CollaboratorInteractionDto>, String> {
        match self.call_host(host_call::Operation::CollaboratorPoll(CollaboratorPollDto {
            token_filter: token_filter.to_owned(),
            limit,
        }))? {
            host_result::Outcome::CollaboratorInteractions(list) => Ok(list.interactions),
            other => Err(format!("意外宿主响应：{other:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PanelRevisionState {
    pub(crate) published: u64,
    pub(crate) pending: u64,
}

impl PanelRevisionState {
    pub(crate) fn new(published: u64) -> Self {
        Self {
            published,
            pending: published,
        }
    }
}

pub struct HostContext<'a, R: Read, W: Write> {
    pub(crate) session: &'a mut WorkerSession<R, W>,
    pub(crate) deferred: &'a mut VecDeque<Envelope>,
    pub(crate) panel_id: u64,
    pub(crate) parent_request_id: u64,
    pub(crate) panel_revision: &'a mut PanelRevisionState,
}

impl<R: Read, W: Write> HostClient for HostContext<'_, R, W> {
    fn grants(&self) -> &[Permission] {
        &self.session.grants
    }

    fn current_panel_revision(&self) -> u64 {
        self.panel_revision.published
    }

    fn next_panel_revision(&mut self) -> u64 {
        self.panel_revision.pending = self
            .panel_revision
            .pending
            .max(self.panel_revision.published)
            .saturating_add(1)
            .max(1);
        self.panel_revision.pending
    }

    fn panel_id(&self) -> u64 {
        self.panel_id
    }

    fn call_host(
        &mut self,
        operation: host_call::Operation,
    ) -> Result<host_result::Outcome, String> {
        let req_id = self
            .session
            .send_on_panel(
                self.panel_id,
                envelope::Body::HostCall(HostCall {
                    operation: Some(operation),
                    parent_request_id: self.parent_request_id,
                }),
                self.panel_revision.published,
                0,
            )
            .map_err(|e| format!("发送 HostCall 失败：{e}"))?;
        loop {
            let incoming = self
                .session
                .receive()
                .map_err(|e| format!("接收 HostResult 失败：{e}"))?;
            if incoming.in_reply_to == req_id
                && let Some(envelope::Body::HostResult(res)) = incoming.body
            {
                let outcome = res.outcome.ok_or_else(|| "HostResult 为空".to_owned())?;
                if let host_result::Outcome::Error(failure) = outcome {
                    return Err(failure.message);
                }
                return Ok(outcome);
            }
            self.deferred.push_back(incoming);
        }
    }
}
