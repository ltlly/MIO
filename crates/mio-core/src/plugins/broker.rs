use super::*;

#[derive(Debug)]
pub enum ServiceOperation {
    QueryHistory {
        offset: usize,
        limit: usize,
        filter: String,
    },
    ReadFlow {
        flow_id: FlowId,
    },
    UpdateDraft {
        tab_id: Uuid,
        expected_revision: u64,
        request: HttpRequest,
    },
    OpenRepeater {
        request: HttpRequest,
    },
    NetworkSend {
        request: HttpRequest,
    },
    StorageGet {
        key: String,
    },
    StorageSet {
        key: String,
        value: Vec<u8>,
    },
    PreferencesGet {
        key: String,
    },
    PreferencesSet {
        key: String,
        value: Vec<u8>,
    },
    OpenIntruder {
        request: HttpRequest,
    },
    ScopeCheck {
        url: String,
    },
    ScopeUpdate {
        url_prefix: String,
        exclude: bool,
        include_subdomains: bool,
    },
    QuerySiteMap {
        prefix: String,
        in_scope_only: bool,
        offset: usize,
        limit: usize,
    },
    ReadAnnotations {
        flow_id: FlowId,
    },
    UpdateAnnotations {
        flow_id: FlowId,
        notes: Option<String>,
        highlight_color: Option<u8>,
    },
    OpenOrganiser {
        request: HttpRequest,
        response: Option<HttpResponse>,
        notes: String,
        highlight_color: Option<u8>,
    },
    SubmitAuditIssue {
        flow_id: Option<FlowId>,
        url: String,
        finding: protocol::PluginFindingDto,
    },
    CollaboratorGenerate {
        label: String,
    },
    CollaboratorPoll {
        token_filter: String,
        limit: usize,
    },
}

#[derive(Clone, Debug)]
pub enum ServiceResult {
    History {
        offset: usize,
        total: usize,
        rows: Vec<FlowSummary>,
    },
    Record(Box<FlowRecord>),
    Response(HttpResponse),
    Resource {
        id: Uuid,
        revision: u64,
    },
    Done,
    StorageValue {
        present: bool,
        value: Vec<u8>,
    },
    ScopeStatus {
        in_scope: bool,
    },
    SiteMap {
        offset: usize,
        total: usize,
        rows: Vec<protocol::SiteMapRowDto>,
    },
    Annotations {
        flow_id: FlowId,
        notes: String,
        highlight_color: u8,
        notes_revision: u64,
        highlight_revision: u64,
    },
    CollaboratorPayload(crate::oast::OastPayloadToken),
    CollaboratorInteractions(Vec<crate::oast::OastInteraction>),
}

impl ServiceResult {
    pub(super) fn into_wire(self) -> HostResult {
        let outcome = match self {
            Self::History {
                offset,
                total,
                rows,
            } => host_result::Outcome::History(protocol::HistoryPage {
                offset: offset as u64,
                total: total as u64,
                rows: rows.into_iter().map(Into::into).collect(),
            }),
            Self::Record(record) => host_result::Outcome::Record(Box::new((*record).into())),
            Self::Response(response) => host_result::Outcome::Response(response.into()),
            Self::Resource { id, revision } => {
                host_result::Outcome::Resource(protocol::ResourceRevision {
                    resource_id: id.as_bytes().to_vec(),
                    revision,
                })
            }
            Self::Done => host_result::Outcome::Done(panel::Empty {}),
            Self::StorageValue { present, value } => {
                host_result::Outcome::StorageValue(protocol::StorageValue { present, value })
            }
            Self::ScopeStatus { in_scope } => {
                host_result::Outcome::ScopeStatus(protocol::ScopeStatusDto { in_scope })
            }
            Self::SiteMap {
                offset,
                total,
                rows,
            } => host_result::Outcome::SiteMap(protocol::SiteMapPageDto {
                offset: offset as u64,
                total: total as u64,
                rows,
            }),
            Self::Annotations {
                flow_id,
                notes,
                highlight_color,
                notes_revision,
                highlight_revision,
            } => host_result::Outcome::Annotations(protocol::AnnotationsDto {
                flow_id: flow_id.as_bytes().to_vec(),
                notes,
                highlight_color: u32::from(highlight_color),
                notes_revision,
                highlight_revision,
            }),
            Self::CollaboratorPayload(token) => {
                host_result::Outcome::CollaboratorPayload(protocol::CollaboratorPayloadDto {
                    token: token.token,
                    label: token.label,
                    http_callback_url: token.http_callback_url,
                    dns_qname: token.dns_qname,
                    created_unix_ms: token.created_unix_ms,
                })
            }
            Self::CollaboratorInteractions(interactions) => {
                host_result::Outcome::CollaboratorInteractions(
                    protocol::CollaboratorInteractionsDto {
                        interactions: interactions
                            .into_iter()
                            .map(|item| {
                                let mut raw_request = item.raw_request;
                                if raw_request.len() > protocol::MAX_INLINE_BODY {
                                    raw_request.truncate(protocol::MAX_INLINE_BODY);
                                }
                                protocol::CollaboratorInteractionDto {
                                    id: item.id.as_bytes().to_vec(),
                                    token: item.token.unwrap_or_default(),
                                    protocol: match item.protocol {
                                        crate::oast::OastProtocol::Http => "http".into(),
                                        crate::oast::OastProtocol::Dns => "dns".into(),
                                    },
                                    peer_addr: item.peer_addr,
                                    summary: item.summary,
                                    raw_request,
                                    observed_unix_ms: item.observed_unix_ms,
                                }
                            })
                            .collect(),
                    },
                )
            }
        };
        HostResult {
            outcome: Some(outcome),
        }
    }
}
/// The service implementation must observe cancel/deadline and join its actual I/O task.
pub struct ServiceRequest {
    pub identity: ChannelIdentity,
    pub plugin_id: String,
    pub panel_id: u64,
    pub request_id: u64,
    pub deadline: Instant,
    pub operation: ServiceOperation,
    pub cancel: CancellationToken,
    pub reply: oneshot::Sender<std::result::Result<ServiceResult, Failure>>,
}
#[derive(Clone, Debug)]
pub enum PluginEvent {
    Panel {
        identity: ChannelIdentity,
        panel_id: u64,
        snapshot: Arc<PanelSnapshot>,
        /// Native input associated with this exact response, never inferred from worker revision.
        editor_bases: Vec<EditorSnapshot>,
        synced_ui_seq: u64,
        rejected_ui_seqs: Vec<u64>,
    },
    /// Same-revision optimistic rollback sync after a UI request is rejected or retracted.
    PanelSynced {
        identity: ChannelIdentity,
        panel_id: u64,
        snapshot: Arc<PanelSnapshot>,
        synced_ui_seq: u64,
        rejected_ui_seqs: Vec<u64>,
    },
    Error {
        identity: ChannelIdentity,
        panel_id: u64,
        failure: Failure,
    },
    PanelClosed {
        identity: ChannelIdentity,
        panel_id: u64,
    },
    Stopped {
        identity: ChannelIdentity,
        reason: String,
    },
}
pub struct PermissionBroker {
    identity: ChannelIdentity,
    pub(super) grants: BTreeSet<Permission>,
    pub(super) resources: HashMap<Uuid, ResourceBinding>,
    pub(super) panel_resources: HashMap<(u64, Uuid), ResourceBinding>,
    reserved_resources: usize,
    reserved_panel_resources: usize,
}

impl PermissionBroker {
    pub fn new(identity: ChannelIdentity, grants: impl IntoIterator<Item = Permission>) -> Self {
        Self {
            identity,
            grants: grants.into_iter().collect(),
            resources: HashMap::new(),
            panel_resources: HashMap::new(),
            reserved_resources: 0,
            reserved_panel_resources: 0,
        }
    }

    fn check_resource_on_panel(&self, panel_id: u64, id: Uuid, revision: u64) -> Result<()> {
        ensure!(!id.is_nil() && revision != 0, "资源身份或 revision 无效");
        if panel_id == protocol::PANEL_ID {
            ensure!(
                self.resources.contains_key(&id) || self.resources.len() + self.reserved_resources < 64,
                "插件资源 handle 超过限额"
            );
        } else {
            ensure!(
                self.panel_resources.contains_key(&(panel_id, id))
                    || self.panel_resources.len() + self.reserved_panel_resources < 128,
                "多路复用子面板资源 handle 超过限额"
            );
        }
        Ok(())
    }

    pub(super) fn reserve_panel_resource(
        &mut self,
        panel_id: u64,
        id: Option<Uuid>,
    ) -> Result<bool> {
        if panel_id == protocol::PANEL_ID {
            if id.is_some_and(|id| self.resources.contains_key(&id)) {
                return Ok(false);
            }
            ensure!(
                self.resources.len() + self.reserved_resources < 64,
                "插件资源 handle 超过 64 个限额；未创建标签"
            );
            self.reserved_resources += 1;
            return Ok(true);
        }
        if id.is_some_and(|id| self.panel_resources.contains_key(&(panel_id, id))) {
            return Ok(false);
        }
        ensure!(
            self.panel_resources.len() + self.reserved_panel_resources < 128,
            "多路复用子面板资源 handle 超过 128 个限额；未打开标签"
        );
        self.reserved_panel_resources += 1;
        Ok(true)
    }

    pub(super) fn release_panel_resource(&mut self, panel_id: u64, reserved: bool) {
        if !reserved {
            return;
        }
        if panel_id == protocol::PANEL_ID {
            self.reserved_resources -= 1;
        } else {
            self.reserved_panel_resources -= 1;
        }
    }

    pub(super) fn authorize_open_panel(&self, panel_id: u64, id: Uuid, revision: u64) -> Result<()> {
        ensure!(
            self.grants.contains(&Permission::TrafficRead),
            "插件尚未获准 traffic.read；未提供请求内容"
        );
        self.check_resource_on_panel(panel_id, id, revision)
    }

    pub fn bind_resource(&mut self, id: Uuid, revision: u64, read_only: bool) -> Result<()> {
        self.bind_panel_resource(protocol::PANEL_ID, id, revision, read_only)
    }

    pub fn bind_panel_resource(
        &mut self,
        panel_id: u64,
        id: Uuid,
        revision: u64,
        read_only: bool,
    ) -> Result<()> {
        self.check_resource_on_panel(panel_id, id, revision)?;
        let binding = ResourceBinding {
            revision,
            read_only,
        };
        if panel_id == protocol::PANEL_ID {
            self.resources.insert(id, binding);
        } else {
            self.panel_resources.insert((panel_id, id), binding);
        }
        Ok(())
    }

    pub(super) fn remove_panel_resource(&mut self, panel_id: u64, id: Uuid) {
        if panel_id == protocol::PANEL_ID {
            self.resources.remove(&id);
        } else {
            self.panel_resources.remove(&(panel_id, id));
        }
    }

    pub(super) fn has_panel_resource(&self, panel_id: u64, id: Uuid) -> bool {
        if panel_id == protocol::PANEL_ID {
            self.resources.contains_key(&id)
        } else {
            self.panel_resources.contains_key(&(panel_id, id))
        }
    }

    pub(super) fn writable_panel(
        &self,
        panel_id: u64,
        id: Uuid,
        revision: u64,
    ) -> std::result::Result<(), Failure> {
        let resource = if panel_id == protocol::PANEL_ID {
            self.resources.get(&id)
        } else {
            self.panel_resources.get(&(panel_id, id))
        }
        .ok_or_else(|| Failure::new(ErrorCode::Denied, "资源不属于此插件实例"))?;
        if resource.read_only {
            return Err(Failure::new(ErrorCode::Denied, "只读资源拒绝修改"));
        }
        if revision != resource.revision {
            return Err(Failure::new(ErrorCode::Stale, "资源 revision 已改变"));
        }
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn complete_resource(
        &mut self,
        expected: PendingResource,
        id: Uuid,
        revision: u64,
    ) -> std::result::Result<(), Failure> {
        self.complete_panel_resource(protocol::PANEL_ID, expected, id, revision)
    }

    pub(super) fn complete_panel_resource(
        &mut self,
        panel_id: u64,
        expected: PendingResource,
        id: Uuid,
        revision: u64,
    ) -> std::result::Result<(), Failure> {
        match expected {
            PendingResource::UpdateDraft {
                id: expected_id,
                revision: expected_revision,
            } => {
                if id != expected_id {
                    return Err(Failure::new(
                        ErrorCode::Internal,
                        "宿主返回了错误的资源 handle",
                    ));
                }
                self.writable_panel(panel_id, id, expected_revision)?;
                if revision <= expected_revision {
                    return Err(Failure::new(
                        ErrorCode::Stale,
                        "草稿回写结果没有更新 revision",
                    ));
                }
            }
            PendingResource::OpenRepeater if self.has_panel_resource(panel_id, id) => {
                return Err(Failure::new(
                    ErrorCode::Internal,
                    "新的 Repeater 资源复用了已有 handle",
                ));
            }
            PendingResource::OpenRepeater => {}
        }
        self.bind_panel_resource(panel_id, id, revision, false)
            .map_err(|error| Failure::new(ErrorCode::Internal, error.to_string()))
    }

    pub fn authorize(
        &self,
        envelope: &Envelope,
        call: HostCall,
    ) -> std::result::Result<ServiceOperation, Failure> {
        if envelope.identity().ok() != Some(self.identity) {
            return Err(Failure::new(ErrorCode::Denied, "插件通道身份不匹配"));
        }
        if envelope.deadline_unix_ms <= protocol::unix_ms() {
            return Err(Failure::new(ErrorCode::Deadline, "服务请求已超时"));
        }
        let invalid = |message| Failure::new(ErrorCode::Invalid, message);
        let operation = call
            .operation
            .ok_or_else(|| invalid("缺少服务操作".into()))?;
        let permission = match &operation {
            host_call::Operation::QueryHistory(_)
            | host_call::Operation::ReadFlow(_)
            | host_call::Operation::ScopeCheck(_)
            | host_call::Operation::QuerySiteMap(_)
            | host_call::Operation::ReadAnnotations(_)
            | host_call::Operation::SubmitAuditIssue(_) => Permission::TrafficRead,
            host_call::Operation::UpdateDraft(_) | host_call::Operation::UpdateAnnotations(_) => {
                Permission::DraftWrite
            }
            host_call::Operation::OpenRepeater(_)
            | host_call::Operation::OpenIntruder(_)
            | host_call::Operation::OpenOrganiser(_) => Permission::RepeaterOpen,
            host_call::Operation::NetworkSend(_)
            | host_call::Operation::CollaboratorGenerate(_)
            | host_call::Operation::CollaboratorPoll(_) => Permission::NetworkSend,
            host_call::Operation::ScopeUpdate(_) => Permission::TrafficModify,
            host_call::Operation::StorageGet(_)
            | host_call::Operation::StorageSet(_)
            | host_call::Operation::PreferencesGet(_)
            | host_call::Operation::PreferencesSet(_) => Permission::Storage,
        };
        if !self.grants.contains(&permission) {
            return Err(Failure::new(ErrorCode::Denied, "插件尚未获准此宿主能力"));
        }
        Ok(match operation {
            host_call::Operation::QueryHistory(q) => {
                if q.limit == 0 || q.limit as usize > panel::MAX_TABLE_ROWS || q.filter.len() > 1024
                {
                    return Err(invalid("History 查询超过限额".into()));
                }
                ServiceOperation::QueryHistory {
                    offset: usize::try_from(q.offset)
                        .map_err(|_| invalid("History offset 无效".into()))?,
                    limit: q.limit as usize,
                    filter: q.filter,
                }
            }
            host_call::Operation::ReadFlow(id) => ServiceOperation::ReadFlow {
                flow_id: protocol::uuid(&id).map_err(invalid)?,
            },
            host_call::Operation::UpdateDraft(update) => {
                let id = protocol::uuid(&update.resource_id).map_err(invalid)?;
                self.writable_panel(envelope.panel_id, id, update.expected_revision)?;
                ServiceOperation::UpdateDraft {
                    tab_id: id,
                    expected_revision: update.expected_revision,
                    request: update
                        .request
                        .ok_or_else(|| invalid("缺少草稿请求".into()))?
                        .try_into()
                        .map_err(invalid)?,
                }
            }
            host_call::Operation::OpenRepeater(request) => ServiceOperation::OpenRepeater {
                request: request.try_into().map_err(invalid)?,
            },
            host_call::Operation::NetworkSend(request) => ServiceOperation::NetworkSend {
                request: request.try_into().map_err(invalid)?,
            },
            host_call::Operation::StorageGet(key) => {
                if key.is_empty() || key.len() > protocol::MAX_STORAGE_KEY_BYTES {
                    return Err(invalid("插件 KV key 为空或超过限额".into()));
                }
                ServiceOperation::StorageGet { key: key.clone() }
            }
            host_call::Operation::StorageSet(entry) => {
                if entry.key.is_empty()
                    || entry.key.len() > protocol::MAX_STORAGE_KEY_BYTES
                    || entry.value.len() > protocol::MAX_STORAGE_VALUE_BYTES
                {
                    return Err(invalid("插件 KV key/value 为空或超过限额".into()));
                }
                ServiceOperation::StorageSet {
                    key: entry.key.clone(),
                    value: entry.value.clone(),
                }
            }
            host_call::Operation::PreferencesGet(key) => {
                if key.is_empty() || key.len() > protocol::MAX_STORAGE_KEY_BYTES {
                    return Err(invalid("插件偏好 key 为空或超过限额".into()));
                }
                ServiceOperation::PreferencesGet { key }
            }
            host_call::Operation::PreferencesSet(entry) => {
                if entry.key.is_empty()
                    || entry.key.len() > protocol::MAX_STORAGE_KEY_BYTES
                    || entry.value.len() > protocol::MAX_STORAGE_VALUE_BYTES
                {
                    return Err(invalid("插件偏好 key/value 为空或超过限额".into()));
                }
                ServiceOperation::PreferencesSet {
                    key: entry.key,
                    value: entry.value,
                }
            }
            host_call::Operation::OpenIntruder(request) => ServiceOperation::OpenIntruder {
                request: request.try_into().map_err(invalid)?,
            },
            host_call::Operation::ScopeCheck(url) => {
                let trimmed = url.trim();
                if trimmed.is_empty() || url.len() > 16 * 1024 {
                    return Err(invalid("ScopeCheck URL 为空或超过限额".into()));
                }
                ServiceOperation::ScopeCheck {
                    url: trimmed.to_owned(),
                }
            }
            host_call::Operation::ScopeUpdate(update) => {
                let prefix = update.url_prefix.trim();
                if prefix.is_empty()
                    || update.url_prefix.len() > 2048
                    || update.url_prefix.chars().any(char::is_control)
                {
                    return Err(invalid("ScopeUpdate 前缀无效或超过 2048 字节".into()));
                }
                ServiceOperation::ScopeUpdate {
                    url_prefix: prefix.to_owned(),
                    exclude: update.exclude,
                    include_subdomains: update.include_subdomains,
                }
            }
            host_call::Operation::QuerySiteMap(q) => {
                if q.limit == 0 || q.limit as usize > panel::MAX_TABLE_ROWS || q.prefix.len() > 1024
                {
                    return Err(invalid("SiteMap 查询超过限额".into()));
                }
                ServiceOperation::QuerySiteMap {
                    prefix: q.prefix,
                    in_scope_only: q.in_scope_only,
                    offset: usize::try_from(q.offset)
                        .map_err(|_| invalid("SiteMap offset 无效".into()))?,
                    limit: q.limit as usize,
                }
            }
            host_call::Operation::ReadAnnotations(id) => ServiceOperation::ReadAnnotations {
                flow_id: protocol::uuid(&id).map_err(invalid)?,
            },
            host_call::Operation::UpdateAnnotations(update) => {
                let flow_id = protocol::uuid(&update.flow_id).map_err(invalid)?;
                if let Some(notes) = &update.notes
                    && notes.len() > 64 * 1024
                {
                    return Err(invalid("注解文本超过 64 KiB 限额".into()));
                }
                if let Some(color) = update.highlight_color
                    && color > 9
                {
                    return Err(invalid("高亮颜色编码必须为 0..=9".into()));
                }
                ServiceOperation::UpdateAnnotations {
                    flow_id,
                    notes: update.notes,
                    highlight_color: update.highlight_color.map(|c| c as u8),
                }
            }
            host_call::Operation::OpenOrganiser(dto) => {
                if dto.notes.len() > 64 * 1024 {
                    return Err(invalid("Organiser 备注超过 64 KiB 限额".into()));
                }
                if let Some(color) = dto.highlight_color
                    && color > 9
                {
                    return Err(invalid("Organiser 高亮颜色编码必须为 0..=9".into()));
                }
                let request: HttpRequest = dto
                    .request
                    .ok_or_else(|| invalid("Organiser 缺少请求".into()))?
                    .try_into()
                    .map_err(invalid)?;
                let response: Option<HttpResponse> = dto
                    .response
                    .map(TryInto::try_into)
                    .transpose()
                    .map_err(invalid)?;
                ServiceOperation::OpenOrganiser {
                    request,
                    response,
                    notes: dto.notes,
                    highlight_color: dto.highlight_color.map(|c| c as u8),
                }
            }
            host_call::Operation::SubmitAuditIssue(dto) => {
                let trimmed = dto.url.trim();
                if trimmed.is_empty() || dto.url.len() > 16 * 1024 {
                    return Err(invalid("AuditIssue URL 为空或超过限额".into()));
                }
                let flow_id = if dto.flow_id.is_empty() {
                    None
                } else {
                    Some(protocol::uuid(&dto.flow_id).map_err(invalid)?)
                };
                let finding = dto
                    .finding
                    .ok_or_else(|| invalid("SubmitAuditIssue 缺少 finding".into()))?;
                protocol::validate_finding(&finding).map_err(invalid)?;
                ServiceOperation::SubmitAuditIssue {
                    flow_id,
                    url: trimmed.to_owned(),
                    finding,
                }
            }
            host_call::Operation::CollaboratorGenerate(dto) => {
                if dto.label.len() > 256 || dto.label.chars().any(char::is_control) {
                    return Err(invalid("Collaborator 标签无效或超过 256 字节".into()));
                }
                ServiceOperation::CollaboratorGenerate { label: dto.label }
            }
            host_call::Operation::CollaboratorPoll(dto) => {
                if dto.token_filter.len() > 256 {
                    return Err(invalid("Collaborator 过滤条件超过 256 字节".into()));
                }
                let limit = if dto.limit == 0 {
                    panel::MAX_TABLE_ROWS
                } else if dto.limit as usize > panel::MAX_TABLE_ROWS {
                    return Err(invalid("Collaborator 轮询条数超过限额".into()));
                } else {
                    dto.limit as usize
                };
                ServiceOperation::CollaboratorPoll {
                    token_filter: dto.token_filter,
                    limit,
                }
            }
        })
    }
}
