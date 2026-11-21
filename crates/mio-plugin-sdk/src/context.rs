use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
};

use mio_plugin_api::{
    FlowRecord, HttpRequest, HttpResponse, Uuid,
    panel::Empty,
    protocol::{
        AnnotationsDto, Cancel, CollaboratorGenerateDto, CollaboratorInteractionDto,
        CollaboratorPayloadDto, CollaboratorPollDto, DraftUpdate, Envelope, ErrorCode, Failure,
        HistoryPage, HistoryQuery, HostCall, OpenOrganiserDto, PANEL_ID, Permission,
        PluginFindingDto, ScopeUpdateDto, SiteMapPageDto, SiteMapQueryDto, StorageEntry,
        SubmitAuditIssueDto, UpdateAnnotationsDto, WorkerSession, envelope, host_call, host_result,
    },
};

use crate::PanelUpdate;

/// Maximum number of interleaved host envelopes buffered while a synchronous `HostCall` waits.
pub const MAX_DEFERRED_ENVELOPES: usize = 256;

pub(crate) const MAX_ABANDONED_HOST_CALLS: usize = 64;

pub trait HostClient {
    fn grants(&self) -> &[Permission];
    fn has_permission(&self, permission: Permission) -> bool {
        self.grants().contains(&permission)
    }
    fn current_panel_revision(&self) -> u64;
    fn next_panel_revision(&mut self) -> u64;
    fn panel_id(&self) -> u64 {
        PANEL_ID
    }
    fn call_host(&mut self, operation: host_call::Operation)
    -> Result<host_result::Outcome, String>;

    /// Start an asynchronous `HostCall` whose `HostResult` arrives later in `MioPlugin::on_host_result`.
    fn start_host_call(&mut self, _operation: host_call::Operation) -> Result<u64, String> {
        Err("当前 HostClient 不支持异步 start_host_call".into())
    }

    /// Ask the host to cancel an in-flight asynchronous `HostCall` by its request ID.
    fn cancel_host_call(&mut self, _request_id: u64) -> Result<(), String> {
        Err("当前 HostClient 不支持 cancel_host_call".into())
    }

    /// Emit a panel update immediately (consuming the pending reply slot on the first panel reply).
    fn publish_panel(&mut self, _update: PanelUpdate) -> Result<(), String> {
        Ok(())
    }

    /// Queue an unsolicited panel update to be flushed immediately after the primary reply is sent.
    fn enqueue_panel_update(&mut self, _update: PanelUpdate) {}

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

pub(crate) fn is_prunable_expired(envelope: &Envelope) -> bool {
    envelope.is_expired()
        && !matches!(
            envelope.body,
            Some(
                envelope::Body::Shutdown(_)
                    | envelope::Body::Cancel(_)
                    | envelope::Body::CloseRequest(_)
                    | envelope::Body::HostResult(_)
            )
        )
}

pub(crate) fn send_deadline_error<R: Read, W: Write>(
    session: &mut WorkerSession<R, W>,
    envelope: &Envelope,
    panel_revisions: &HashMap<u64, PanelRevisionState>,
) -> io::Result<()> {
    let panel_id = envelope.panel_id.max(PANEL_ID);
    let msg = if panel_id != PANEL_ID {
        "子面板请求已超时"
    } else {
        "插件请求已超时"
    };
    let rev = panel_revisions
        .get(&panel_id)
        .map_or(envelope.revision, |s| s.published);
    session.send_on_panel(
        panel_id,
        envelope::Body::Error(Failure::new(ErrorCode::Deadline, msg)),
        rev,
        envelope.request_id,
    )?;
    Ok(())
}

pub(crate) fn emit_panel_update<R: Read, W: Write>(
    session: &mut WorkerSession<R, W>,
    panel_id: u64,
    panel_revisions: &mut HashMap<u64, PanelRevisionState>,
    reply_to: u64,
    update: PanelUpdate,
) -> io::Result<()> {
    let state = panel_revisions
        .entry(panel_id)
        .or_insert_with(|| PanelRevisionState::new(0));
    match update {
        PanelUpdate::None => {
            if reply_to != 0 {
                session.send_on_panel(
                    panel_id,
                    envelope::Body::Ack(Empty {}),
                    state.published,
                    reply_to,
                )?;
            }
        }
        PanelUpdate::Snapshot(snapshot) => {
            let rev = snapshot.revision;
            state.published = rev;
            state.pending = state.pending.max(rev);
            session.send_on_panel(panel_id, envelope::Body::Snapshot(snapshot), rev, reply_to)?;
        }
        PanelUpdate::Patch(patch) => {
            let rev = patch.revision;
            state.published = rev;
            state.pending = state.pending.max(rev);
            session.send_on_panel(panel_id, envelope::Body::Patch(patch), rev, reply_to)?;
        }
    }
    Ok(())
}

/// Bounded priority buffer for interleaved host frames received during synchronous `call_host` waits.
pub(crate) struct DeferredQueue {
    urgent: VecDeque<Envelope>,
    normal: VecDeque<Envelope>,
    max_len: usize,
    pub(crate) shutdown_requested: bool,
    abandoned_host_calls: VecDeque<u64>,
}

impl DeferredQueue {
    pub(crate) fn new(max_len: usize) -> Self {
        Self {
            urgent: VecDeque::new(),
            normal: VecDeque::new(),
            max_len: max_len.max(1),
            shutdown_requested: false,
            abandoned_host_calls: VecDeque::new(),
        }
    }

    pub(crate) fn pop_next(&mut self) -> Option<Envelope> {
        self.urgent
            .pop_front()
            .or_else(|| self.normal.pop_front())
    }

    pub(crate) fn len(&self) -> usize {
        self.urgent.len() + self.normal.len()
    }

    pub(crate) fn push_urgent(&mut self, env: Envelope) {
        self.urgent.push_back(env);
    }

    pub(crate) fn push_normal(&mut self, env: Envelope) {
        self.normal.push_back(env);
    }

    pub(crate) fn record_abandoned(&mut self, req_id: u64) {
        if self.abandoned_host_calls.len() >= MAX_ABANDONED_HOST_CALLS {
            self.abandoned_host_calls.pop_front();
        }
        self.abandoned_host_calls.push_back(req_id);
    }

    pub(crate) fn take_abandoned(&mut self, req_id: u64) -> bool {
        if let Some(pos) = self.abandoned_host_calls.iter().position(|&id| id == req_id) {
            self.abandoned_host_calls.remove(pos);
            true
        } else {
            false
        }
    }

    pub(crate) fn prune_expired<R: Read, W: Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        panel_revisions: &HashMap<u64, PanelRevisionState>,
    ) -> io::Result<usize> {
        let mut kept = VecDeque::with_capacity(self.normal.len());
        let mut pruned = 0usize;
        while let Some(env) = self.normal.pop_front() {
            if is_prunable_expired(&env) {
                pruned += 1;
                if env.request_id != 0 {
                    send_deadline_error(session, &env, panel_revisions)?;
                }
            } else {
                kept.push_back(env);
            }
        }
        self.normal = kept;
        Ok(pruned)
    }
}

pub struct HostContext<'a, R: Read, W: Write> {
    pub(crate) session: &'a mut WorkerSession<R, W>,
    pub(crate) deferred: &'a mut DeferredQueue,
    pub(crate) panel_id: u64,
    pub(crate) parent_request_id: u64,
    pub(crate) allow_panel_reply: bool,
    pub(crate) replied: bool,
    pub(crate) cancelled: bool,
    pub(crate) panel_revisions: &'a mut HashMap<u64, PanelRevisionState>,
    pub(crate) post_reply_updates: &'a mut Vec<PanelUpdate>,
}

impl<'a, R: Read, W: Write> HostContext<'a, R, W> {
    pub(crate) fn new(
        session: &'a mut WorkerSession<R, W>,
        deferred: &'a mut DeferredQueue,
        panel_id: u64,
        parent_request_id: u64,
        allow_panel_reply: bool,
        panel_revisions: &'a mut HashMap<u64, PanelRevisionState>,
        post_reply_updates: &'a mut Vec<PanelUpdate>,
    ) -> Self {
        Self {
            session,
            deferred,
            panel_id,
            parent_request_id,
            allow_panel_reply,
            replied: false,
            cancelled: false,
            panel_revisions,
            post_reply_updates,
        }
    }

    fn send_host_call(&mut self, operation: host_call::Operation) -> Result<u64, String> {
        if self.deferred.shutdown_requested {
            return Err("宿主已请求关闭".into());
        }
        let parent_request_id = if self.replied {
            0
        } else {
            self.parent_request_id
        };
        let published = self.current_panel_revision();
        self.session
            .send_on_panel(
                self.panel_id,
                envelope::Body::HostCall(HostCall {
                    operation: Some(operation),
                    parent_request_id,
                }),
                published,
                0,
            )
            .map_err(|e| format!("发送 HostCall 失败：{e}"))
    }

    fn is_urgent_frame(env: &Envelope) -> bool {
        match env.body {
            Some(envelope::Body::Cancel(_) | envelope::Body::Resync(_)) => true,
            Some(envelope::Body::CloseRequest(ref c)) => c.destroy_panel,
            _ => false,
        }
    }

    fn defer_and_check_limit(&mut self, env: Envelope, req_id: u64) -> Result<(), String> {
        if self.deferred.len() + 1 >= self.deferred.max_len {
            let _ = self
                .deferred
                .prune_expired(self.session, self.panel_revisions);
        }
        if Self::is_urgent_frame(&env) {
            self.deferred.push_urgent(env);
        } else {
            self.deferred.push_normal(env);
        }
        if self.deferred.len() >= self.deferred.max_len {
            self.deferred.record_abandoned(req_id);
            return Err("延迟消息队列达到上限；已中止同步 HostCall 以避免阻塞".into());
        }
        Ok(())
    }
}

impl<R: Read, W: Write> HostClient for HostContext<'_, R, W> {
    fn grants(&self) -> &[Permission] {
        &self.session.grants
    }

    fn current_panel_revision(&self) -> u64 {
        self.panel_revisions
            .get(&self.panel_id)
            .map_or(0, |s| s.published)
    }

    fn next_panel_revision(&mut self) -> u64 {
        let state = self
            .panel_revisions
            .entry(self.panel_id)
            .or_insert_with(|| PanelRevisionState::new(0));
        state.pending = state
            .pending
            .max(state.published)
            .saturating_add(1)
            .max(1);
        state.pending
    }

    fn panel_id(&self) -> u64 {
        self.panel_id
    }

    fn start_host_call(&mut self, operation: host_call::Operation) -> Result<u64, String> {
        self.send_host_call(operation)
    }

    fn cancel_host_call(&mut self, request_id: u64) -> Result<(), String> {
        let published = self.current_panel_revision();
        self.session
            .send_on_panel(
                self.panel_id,
                envelope::Body::Cancel(Cancel { request_id }),
                published,
                0,
            )
            .map(|_| ())
            .map_err(|e| format!("发送 Cancel 失败：{e}"))
    }

    fn publish_panel(&mut self, update: PanelUpdate) -> Result<(), String> {
        let reply_to = if !self.replied && self.allow_panel_reply && self.parent_request_id != 0 {
            self.replied = true;
            self.parent_request_id
        } else {
            0
        };
        if matches!(update, PanelUpdate::None) && reply_to == 0 {
            return Ok(());
        }
        emit_panel_update(
            self.session,
            self.panel_id,
            self.panel_revisions,
            reply_to,
            update,
        )
        .map_err(|e| format!("发布面板更新失败：{e}"))
    }

    fn enqueue_panel_update(&mut self, update: PanelUpdate) {
        if !matches!(update, PanelUpdate::None) {
            self.post_reply_updates.push(update);
        }
    }

    fn call_host(
        &mut self,
        operation: host_call::Operation,
    ) -> Result<host_result::Outcome, String> {
        if self.cancelled {
            return Err("操作已被宿主取消".into());
        }
        if self.deferred.len() >= self.deferred.max_len {
            let _ = self
                .deferred
                .prune_expired(self.session, self.panel_revisions);
            if self.deferred.len() >= self.deferred.max_len {
                return Err("延迟消息队列已满；请先处理待办消息".into());
            }
        }
        let req_id = self.send_host_call(operation)?;
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
            if matches!(incoming.body, Some(envelope::Body::HostResult(_)))
                && self.deferred.take_abandoned(incoming.in_reply_to)
            {
                continue;
            }
            if matches!(incoming.body, Some(envelope::Body::Shutdown(_))) {
                self.deferred.shutdown_requested = true;
                self.deferred.record_abandoned(req_id);
                return Err("宿主已请求关闭".into());
            }
            if let Some(envelope::Body::Cancel(ref cancel)) = incoming.body {
                let inc_panel_id = incoming.panel_id.max(PANEL_ID);
                if inc_panel_id != PANEL_ID && !self.panel_revisions.contains_key(&inc_panel_id) {
                    self.session
                        .send_on_panel(
                            inc_panel_id,
                            envelope::Body::Ack(Empty {}),
                            incoming.revision,
                            incoming.request_id,
                        )
                        .map_err(|e| format!("发送过期 Cancel Ack 失败：{e}"))?;
                    continue;
                }
                if inc_panel_id == self.panel_id
                    && (cancel.request_id == 0
                        || cancel.request_id == self.parent_request_id
                        || cancel.request_id == req_id)
                {
                    self.deferred.push_urgent(incoming);
                    self.deferred.record_abandoned(req_id);
                    self.cancelled = true;
                    return Err("操作已被宿主取消".into());
                }
                self.defer_and_check_limit(incoming, req_id)?;
                continue;
            }
            if is_prunable_expired(&incoming) {
                if incoming.request_id != 0 {
                    send_deadline_error(self.session, &incoming, self.panel_revisions)
                        .map_err(|e| format!("发送超时响应失败：{e}"))?;
                }
                continue;
            }
            self.defer_and_check_limit(incoming, req_id)?;
        }
    }
}
