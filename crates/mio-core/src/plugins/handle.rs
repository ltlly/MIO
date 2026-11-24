use super::*;

#[derive(Default)]
pub(super) struct ControlOrderState {
    next_seq: u64,
    next_waiter_id: u64,
    pending_waiters: Vec<PendingControlWaiter>,
}

struct PendingControlWaiter {
    waiter_id: u64,
    panel_id: u64,
    resource_id: Option<Uuid>,
    cancel: CancellationToken,
}

struct WaiterGuard {
    order: Arc<std::sync::Mutex<ControlOrderState>>,
    waiter_id: u64,
    cancel: CancellationToken,
    active: bool,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        if self.active {
            let mut state = self.order.lock().unwrap_or_else(|e| e.into_inner());
            state.pending_waiters.retain(|w| w.waiter_id != self.waiter_id);
        }
    }
}

pub(crate) struct PendingOpen {
    handle: PluginHandle,
    panel_id: u64,
    id: Uuid,
    guard: WaiterGuard,
}

pub(crate) struct PendingCommit {
    handle: PluginHandle,
    panel_id: u64,
    guard: WaiterGuard,
    expected_snapshot: Option<Arc<PanelSnapshot>>,
}

enum RevokeDispatchState {
    Immediate(Result<()>),
    Submitted(Vec<oneshot::Receiver<Result<()>>>),
}

pub(crate) struct PendingRevokeHandle {
    handle: PluginHandle,
    state: RevokeDispatchState,
}

#[derive(Clone)]
pub struct PluginHandle {
    pub identity: ChannelIdentity,
    pub(super) control: mpsc::Sender<SequencedControl>,
    pub(super) urgent_control: mpsc::UnboundedSender<SequencedControl>,
    pub(super) control_order: Arc<std::sync::Mutex<ControlOrderState>>,
    pub(super) hook_slots: Arc<tokio::sync::Semaphore>,
    pub(super) cancel: CancellationToken,
}

fn commit_permit(
    order: &std::sync::Mutex<ControlOrderState>,
    permit: mpsc::Permit<'_, SequencedControl>,
    command: Control,
) {
    let mut state = order.lock().unwrap_or_else(|e| e.into_inner());
    state.next_seq = state.next_seq.saturating_add(1);
    let seq = state.next_seq;
    permit.send(SequencedControl { seq, command });
}

fn register_waiter(
    order: &Arc<std::sync::Mutex<ControlOrderState>>,
    panel_id: u64,
    resource_id: Option<Uuid>,
) -> WaiterGuard {
    let cancel = CancellationToken::new();
    let mut state = order.lock().unwrap_or_else(|e| e.into_inner());
    state.next_waiter_id = state.next_waiter_id.saturating_add(1);
    let waiter_id = state.next_waiter_id;
    state.pending_waiters.push(PendingControlWaiter {
        waiter_id,
        panel_id,
        resource_id,
        cancel: cancel.clone(),
    });
    WaiterGuard {
        order: order.clone(),
        waiter_id,
        cancel,
        active: true,
    }
}

fn cancel_matching_waiters_batch(
    state: &mut ControlOrderState,
    panel_id: u64,
    resource_ids: &[Uuid],
    remove_panel: bool,
) {
    let remove_panel = remove_panel && panel_id != protocol::PANEL_ID;
    state.pending_waiters.retain(|waiter| {
        let matched = waiter.panel_id == panel_id
            && (remove_panel
                || waiter
                    .resource_id
                    .is_some_and(|rid| resource_ids.contains(&rid)));
        if matched {
            waiter.cancel.cancel();
            false
        } else {
            true
        }
    });
}

fn cancel_matching_waiters(
    state: &mut ControlOrderState,
    panel_id: u64,
    resource_id: Uuid,
    remove_panel: bool,
) {
    cancel_matching_waiters_batch(state, panel_id, &[resource_id], remove_panel);
}

fn commit_waiter_permit(
    guard: &mut WaiterGuard,
    permit: mpsc::Permit<'_, SequencedControl>,
    command: Control,
) -> bool {
    let mut state = guard.order.lock().unwrap_or_else(|e| e.into_inner());
    state.pending_waiters.retain(|w| w.waiter_id != guard.waiter_id);
    guard.active = false;
    if guard.cancel.is_cancelled() {
        drop(permit);
        return false;
    }
    state.next_seq = state.next_seq.saturating_add(1);
    let seq = state.next_seq;
    permit.send(SequencedControl { seq, command });
    true
}

fn send_urgent_revoke_batch(
    order: &std::sync::Mutex<ControlOrderState>,
    urgent_control: &mpsc::UnboundedSender<SequencedControl>,
    panel_id: u64,
    ids: Vec<Uuid>,
    remove_panel: bool,
) -> Result<Vec<oneshot::Receiver<Result<()>>>> {
    let mut state = order.lock().unwrap_or_else(|e| e.into_inner());
    cancel_matching_waiters_batch(&mut state, panel_id, &ids, remove_panel);
    state.next_seq = state.next_seq.saturating_add(1);
    let seq = state.next_seq;
    if ids.len() == 1 {
        let (reply, received) = oneshot::channel();
        urgent_control
            .send(SequencedControl {
                seq,
                command: Control::Revoke {
                    panel_id,
                    id: ids[0],
                    remove_panel,
                    reply,
                },
            })
            .map_err(|_| anyhow::anyhow!("插件已停止"))?;
        Ok(vec![received])
    } else {
        let mut items = Vec::with_capacity(ids.len());
        let mut receivers = Vec::with_capacity(ids.len());
        for id in ids {
            let (reply, received) = oneshot::channel();
            items.push((id, reply));
            receivers.push(received);
        }
        urgent_control
            .send(SequencedControl {
                seq,
                command: Control::RevokeBatch { panel_id, items },
            })
            .map_err(|_| anyhow::anyhow!("插件已停止"))?;
        Ok(receivers)
    }
}

async fn wait_revoke_receivers(
    cancel: &CancellationToken,
    receivers: Vec<oneshot::Receiver<Result<()>>>,
) -> Result<()> {
    let mut first_error = None;
    for received in receivers {
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("插件已停止；资源不会自动重新授权"),
            result = received => result.context("插件未确认关闭；资源不会自动重新授权")?,
        };
        if let Err(err) = outcome {
            first_error.get_or_insert(err);
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

impl PendingRevokeHandle {
    pub(crate) async fn wait(self) -> Result<()> {
        match self.state {
            RevokeDispatchState::Immediate(res) => res,
            RevokeDispatchState::Submitted(receivers) => {
                wait_revoke_receivers(&self.handle.cancel, receivers).await
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_hook(
    control: &mpsc::Sender<SequencedControl>,
    control_order: &std::sync::Mutex<ControlOrderState>,
    hook_slots: &Arc<tokio::sync::Semaphore>,
    handle_cancel: &CancellationToken,
    cancel: CancellationToken,
    permission: Permission,
    body: envelope::Body,
) -> Result<HookReply> {
    let _hook_permit = tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("插件 Hook 调用已取消；未发送"),
        _ = handle_cancel.cancelled() => bail!("插件已停止"),
        _ = control.closed() => bail!("插件已停止"),
        permit = hook_slots.clone().acquire_owned() => {
            permit.map_err(|_| anyhow::anyhow!("插件已停止"))?
        }
    };
    let (reply, received) = oneshot::channel();
    tokio::select! {
    biased;
    _ = cancel.cancelled() => bail!("插件 Hook 调用已取消；未发送"),
    _ = handle_cancel.cancelled() => bail!("插件已停止"),
    _ = control.closed() => bail!("插件已停止"),
    result = control.reserve() => {
        let permit = result.map_err(|_| anyhow::anyhow!("插件已停止"))?;
        commit_permit(
            control_order,
            permit,
            Control::Hook {
                permission,
                body,
                cancelled: cancel.clone(),
                reply,
            },
        );
    }
    }
    tokio::select! {
    biased;
    _ = cancel.cancelled() => bail!("插件 Hook 调用已取消"),
    _ = handle_cancel.cancelled() => bail!("插件已停止"),
    result = received => result.context("插件未完成 Hook 调用")?,
    }
}
impl PluginHandle {
    pub async fn call_http_hook(
        &self,
        call: protocol::HttpHookCall,
        cancel: CancellationToken,
    ) -> Result<protocol::HttpHookResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::TrafficRead,
            envelope::Body::HttpHookCall(call),
        )
        .await?
        {
            HookReply::Http(res) => Ok(res),
            _ => bail!("插件返回了不匹配的 Hook 结果类型"),
        }
    }

    pub async fn call_ws_hook(
        &self,
        call: protocol::WebSocketHookCall,
        cancel: CancellationToken,
    ) -> Result<protocol::WebSocketHookResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::TrafficRead,
            envelope::Body::WebSocketHookCall(call),
        )
        .await?
        {
            HookReply::Ws(res) => Ok(res),
            _ => bail!("插件返回了不匹配的 Hook 结果类型"),
        }
    }

    pub async fn call_session_action(
        &self,
        call: protocol::SessionActionCall,
        cancel: CancellationToken,
    ) -> Result<protocol::SessionActionResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::SessionHook,
            envelope::Body::SessionActionCall(call),
        )
        .await?
        {
            HookReply::Session(res) => Ok(res),
            _ => bail!("插件返回了不匹配的 Hook 结果类型"),
        }
    }

    pub async fn call_payload(
        &self,
        call: protocol::PayloadCall,
        cancel: CancellationToken,
    ) -> Result<protocol::PayloadResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::PayloadProcess,
            envelope::Body::PayloadCall(call),
        )
        .await?
        {
            HookReply::Payload(res) => Ok(res),
            _ => bail!("插件返回了不匹配的 Hook 结果类型"),
        }
    }

    pub async fn call_context_menu(
        &self,
        call: protocol::ContextMenuCall,
        cancel: CancellationToken,
    ) -> Result<protocol::ContextMenuResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::TrafficRead,
            envelope::Body::ContextMenuCall(call),
        )
        .await?
        {
            HookReply::ContextMenu(res) => Ok(res),
            _ => bail!("插件返回了不匹配的上下文菜单结果类型"),
        }
    }

    pub async fn call_passive_check(
        &self,
        call: protocol::PassiveCheckCall,
    ) -> Result<protocol::PassiveCheckResult> {
        self.call_passive_check_with_cancel(call, self.cancel.clone())
            .await
    }

    pub async fn call_passive_check_with_cancel(
        &self,
        call: protocol::PassiveCheckCall,
        cancel: CancellationToken,
    ) -> Result<protocol::PassiveCheckResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::TrafficRead,
            envelope::Body::PassiveCheckCall(call),
        )
        .await?
        {
            HookReply::PassiveCheck(res) => Ok(res),
            _ => bail!("插件返回了不匹配的被动检查结果类型"),
        }
    }

    pub async fn call_active_check_with_cancel(
        &self,
        call: protocol::ActiveCheckCall,
        cancel: CancellationToken,
    ) -> Result<protocol::ActiveCheckResult> {
        match send_hook(
            &self.control,
            &self.control_order,
            &self.hook_slots,
            &self.cancel,
            cancel,
            Permission::TrafficRead,
            envelope::Body::ActiveCheckCall(call),
        )
        .await?
        {
            HookReply::ActiveCheck(res) => Ok(res),
            _ => bail!("插件返回了不匹配的被动检查结果类型"),
        }
    }

    pub fn try_event(&self, event: UiEvent) -> Result<()> {
        self.try_event_on_panel(protocol::PANEL_ID, event)
    }

    pub fn try_event_on_panel(&self, panel_id: u64, event: UiEvent) -> Result<()> {
        let permit = self
            .control
            .try_reserve()
            .map_err(|_| anyhow::anyhow!("插件事件队列已满或插件已停止"))?;
        commit_permit(
            &self.control_order,
            permit,
            Control::Event {
                panel_id,
                event,
                expected_snapshot: None,
                event_seq: 0,
            },
        );
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn try_event_on_panel_with_snapshot(
        &self,
        panel_id: u64,
        expected_snapshot: Arc<PanelSnapshot>,
        event: UiEvent,
    ) -> Result<()> {
        self.try_event_on_panel_with_snapshot_seq(panel_id, expected_snapshot, 0, event)
    }

    pub fn try_event_on_panel_with_snapshot_seq(
        &self,
        panel_id: u64,
        expected_snapshot: Arc<PanelSnapshot>,
        event_seq: u64,
        event: UiEvent,
    ) -> Result<()> {
        let permit = self
            .control
            .try_reserve()
            .map_err(|_| anyhow::anyhow!("插件事件队列已满或插件已停止"))?;
        commit_permit(
            &self.control_order,
            permit,
            Control::Event {
                panel_id,
                event,
                expected_snapshot: Some(expected_snapshot),
                event_seq,
            },
        );
        Ok(())
    }
}

impl PendingOpen {
    async fn submit_open(
        mut self,
        revoked_msg: &'static str,
        wait_cancel_msg: &'static str,
        wait_drop_msg: &'static str,
        build: impl FnOnce(u64, Uuid, oneshot::Sender<Result<Arc<PanelSnapshot>>>) -> Control,
    ) -> Result<Arc<PanelSnapshot>> {
        if self.handle.cancel.is_cancelled() {
            bail!("插件已停止");
        }
        if self.guard.cancel.is_cancelled() {
            bail!(revoked_msg);
        }
        let (reply, received) = oneshot::channel();
        let handle_cancel = self.handle.cancel.clone();
        let waiter_cancel = self.guard.cancel.clone();
        tokio::select! {
            biased;
            _ = handle_cancel.cancelled() => bail!("插件已停止"),
            _ = waiter_cancel.cancelled() => bail!(revoked_msg),
            result = self.handle.control.reserve() => {
                let permit = result.map_err(|_| anyhow::anyhow!("插件已停止"))?;
                let command = build(self.panel_id, self.id, reply);
                if !commit_waiter_permit(&mut self.guard, permit, command) {
                    bail!(revoked_msg);
                }
            }
        }
        tokio::select! {
            biased;
            _ = self.handle.cancel.cancelled() => bail!(wait_cancel_msg),
            result = received => result.context(wait_drop_msg)?,
        }
    }

    pub(crate) async fn open_request(
        self,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    ) -> Result<Arc<PanelSnapshot>> {
        self.submit_open(
            "资源已撤销；未打开请求",
            "插件已停止；未确认打开请求",
            "插件未完成打开请求",
            move |panel_id, id, reply| Control::Open {
                panel_id,
                id,
                revision,
                read_only,
                request,
                reply,
            },
        )
        .await
    }

    pub(crate) async fn open_response(
        self,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    ) -> Result<Arc<PanelSnapshot>> {
        self.submit_open(
            "资源已撤销；未打开响应",
            "插件已停止；未确认打开响应",
            "插件未完成打开响应",
            move |panel_id, id, reply| Control::OpenResponse {
                panel_id,
                id,
                revision,
                read_only,
                response,
                reply,
            },
        )
        .await
    }
}

impl PendingCommit {
    #[allow(clippy::too_many_arguments)]
    async fn submit_commit<T>(
        mut self,
        cancel: CancellationToken,
        cancelled_msg: &'static str,
        cancelled_main_msg: &'static str,
        cancelled_child_msg: &'static str,
        stop_wait_msg: &'static str,
        drop_msg: &'static str,
        build: impl FnOnce(u64, Uuid, oneshot::Sender<Result<T>>) -> Control,
    ) -> Result<T> {
        let revoked_msg = "资源已撤销；未提交或发送，保留现有编辑内容";
        if cancel.is_cancelled() {
            bail!(cancelled_msg);
        }
        if self.handle.cancel.is_cancelled() {
            bail!("插件已停止");
        }
        if self.guard.cancel.is_cancelled() {
            bail!(revoked_msg);
        }
        let op_id = Uuid::new_v4();
        let (reply, received) = oneshot::channel();
        let handle_cancel = self.handle.cancel.clone();
        let waiter_cancel = self.guard.cancel.clone();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!(cancelled_msg),
            _ = handle_cancel.cancelled() => bail!("插件已停止"),
            _ = waiter_cancel.cancelled() => bail!(revoked_msg),
            result = self.handle.control.reserve() => {
                let permit = result.map_err(|_| anyhow::anyhow!("插件已停止"))?;
                if !commit_waiter_permit(&mut self.guard, permit, build(self.panel_id, op_id, reply)) {
                    bail!(revoked_msg);
                }
            }
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if self.panel_id == protocol::PANEL_ID {
                    self.handle.cancel.cancel();
                    bail!(cancelled_main_msg);
                }
                self.handle.send_urgent_cancel_op(self.panel_id, op_id);
                bail!(cancelled_child_msg);
            }
            _ = self.handle.cancel.cancelled() => bail!(stop_wait_msg),
            result = received => result.context(drop_msg)?,
        }
    }

    pub(crate) async fn commit_request(
        self,
        request: CommitEditor,
        cancel: CancellationToken,
    ) -> Result<HttpRequest> {
        let cancel_clone = cancel.clone();
        let expected_snapshot = self.expected_snapshot.clone();
        self.submit_commit(
            cancel,
            "插件编辑提交已取消；未发送",
            "插件编辑提交已取消；插件实例正在回收，未发送",
            "插件子面板编辑提交已取消；未发送",
            "插件已停止；未发送旧稿",
            "插件未完成编辑提交",
            move |panel_id, op_id, reply| Control::Commit {
                panel_id,
                op_id,
                request,
                cancelled: cancel_clone,
                expected_snapshot,
                reply,
            },
        )
        .await
    }

    pub(crate) async fn commit_response(
        self,
        request: protocol::CommitResponseEditor,
        cancel: CancellationToken,
    ) -> Result<HttpResponse> {
        let cancel_clone = cancel.clone();
        let expected_snapshot = self.expected_snapshot.clone();
        self.submit_commit(
            cancel,
            "插件响应编辑提交已取消",
            "插件响应编辑提交已取消；实例正在回收",
            "插件子面板响应编辑提交已取消",
            "插件已停止；未应用旧响应修改",
            "插件未完成响应编辑提交",
            move |panel_id, op_id, reply| Control::CommitResponse {
                panel_id,
                op_id,
                request,
                cancelled: cancel_clone,
                expected_snapshot,
                reply,
            },
        )
        .await
    }
}

impl PluginHandle {
    pub(crate) fn begin_open(&self, panel_id: u64, id: Uuid) -> PendingOpen {
        let guard = register_waiter(&self.control_order, panel_id, Some(id));
        PendingOpen {
            handle: self.clone(),
            panel_id,
            id,
            guard,
        }
    }

    pub(crate) fn begin_commit(&self, panel_id: u64, resource_id: Option<Uuid>) -> PendingCommit {
        self.begin_commit_with_snapshot(panel_id, resource_id, None)
    }

    pub(crate) fn begin_commit_with_snapshot(
        &self,
        panel_id: u64,
        resource_id: Option<Uuid>,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
    ) -> PendingCommit {
        let guard = register_waiter(&self.control_order, panel_id, resource_id);
        PendingCommit {
            handle: self.clone(),
            panel_id,
            guard,
            expected_snapshot,
        }
    }

    pub(crate) fn cancel_pending_resource_ops(&self, panel_id: u64, id: Uuid, remove_panel: bool) {
        let mut state = self.control_order.lock().unwrap_or_else(|e| e.into_inner());
        cancel_matching_waiters(&mut state, panel_id, id, remove_panel);
    }

    /// Returns the correlated, verified snapshot without depending on the separate event queue.
    pub async fn open_request(
        &self,
        id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    ) -> Result<Arc<PanelSnapshot>> {
        self.open_request_on_panel(protocol::PANEL_ID, id, revision, read_only, request)
            .await
    }

    pub async fn open_request_on_panel(
        &self,
        panel_id: u64,
        id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    ) -> Result<Arc<PanelSnapshot>> {
        self.begin_open(panel_id, id)
            .open_request(revision, read_only, request)
            .await
    }

    pub async fn open_response(
        &self,
        id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    ) -> Result<Arc<PanelSnapshot>> {
        self.open_response_on_panel(protocol::PANEL_ID, id, revision, read_only, response)
            .await
    }

    pub async fn open_response_on_panel(
        &self,
        panel_id: u64,
        id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    ) -> Result<Arc<PanelSnapshot>> {
        self.begin_open(panel_id, id)
            .open_response(revision, read_only, response)
            .await
    }

    fn send_urgent_cancel_op(&self, panel_id: u64, op_id: Uuid) {
        let mut state = self.control_order.lock().unwrap_or_else(|e| e.into_inner());
        state.next_seq = state.next_seq.saturating_add(1);
        let seq = state.next_seq;
        let _ = self.urgent_control.send(SequencedControl {
            seq,
            command: Control::CancelPanelOp { panel_id, op_id },
        });
    }

    pub async fn commit(
        &self,
        request: CommitEditor,
        cancel: CancellationToken,
    ) -> Result<HttpRequest> {
        self.commit_on_panel(protocol::PANEL_ID, request, cancel)
            .await
    }

    pub async fn commit_on_panel(
        &self,
        panel_id: u64,
        request: CommitEditor,
        cancel: CancellationToken,
    ) -> Result<HttpRequest> {
        let resource_id = protocol::uuid(&request.resource_id).ok();
        self.begin_commit(panel_id, resource_id)
            .commit_request(request, cancel)
            .await
    }

    pub async fn commit_response(
        &self,
        request: protocol::CommitResponseEditor,
        cancel: CancellationToken,
    ) -> Result<HttpResponse> {
        self.commit_response_on_panel(protocol::PANEL_ID, request, cancel)
            .await
    }

    pub async fn commit_response_on_panel(
        &self,
        panel_id: u64,
        request: protocol::CommitResponseEditor,
        cancel: CancellationToken,
    ) -> Result<HttpResponse> {
        let resource_id = protocol::uuid(&request.resource_id).ok();
        self.begin_commit(panel_id, resource_id)
            .commit_response(request, cancel)
            .await
    }

    fn begin_revoke_inner(
        &self,
        panel_id: u64,
        ids: Vec<Uuid>,
        remove_panel: bool,
    ) -> PendingRevokeHandle {
        if ids.iter().any(Uuid::is_nil) {
            return PendingRevokeHandle {
                handle: self.clone(),
                state: RevokeDispatchState::Immediate(Err(anyhow::anyhow!("资源身份无效"))),
            };
        }
        let mut unique = Vec::with_capacity(ids.len());
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        if unique.is_empty() {
            return PendingRevokeHandle {
                handle: self.clone(),
                state: RevokeDispatchState::Immediate(Ok(())),
            };
        }
        if self.cancel.is_cancelled() {
            let mut state = self.control_order.lock().unwrap_or_else(|e| e.into_inner());
            cancel_matching_waiters_batch(&mut state, panel_id, &unique, remove_panel);
            return PendingRevokeHandle {
                handle: self.clone(),
                state: RevokeDispatchState::Immediate(Err(anyhow::anyhow!("插件已停止"))),
            };
        }
        let state = match send_urgent_revoke_batch(
            &self.control_order,
            &self.urgent_control,
            panel_id,
            unique,
            remove_panel,
        ) {
            Ok(receivers) => RevokeDispatchState::Submitted(receivers),
            Err(err) => RevokeDispatchState::Immediate(Err(err)),
        };
        PendingRevokeHandle {
            handle: self.clone(),
            state,
        }
    }

    pub(crate) fn begin_revoke_panel_resource(
        &self,
        panel_id: u64,
        id: Uuid,
        remove_panel: bool,
    ) -> PendingRevokeHandle {
        self.begin_revoke_inner(panel_id, vec![id], remove_panel)
    }

    pub(crate) fn begin_revoke_panel_resources(
        &self,
        panel_id: u64,
        ids: Vec<Uuid>,
    ) -> PendingRevokeHandle {
        self.begin_revoke_inner(panel_id, ids, false)
    }

    /// Revocation is final even if the worker fails to acknowledge it; explicitly open to regrant.
    pub async fn revoke_resource(&self, id: Uuid) -> Result<()> {
        self.revoke_panel_resource(protocol::PANEL_ID, id, false)
            .await
    }

    pub async fn revoke_panel_resource(
        &self,
        panel_id: u64,
        id: Uuid,
        remove_panel: bool,
    ) -> Result<()> {
        self.begin_revoke_panel_resource(panel_id, id, remove_panel)
            .wait()
            .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn revoke_panel_resources(&self, panel_id: u64, ids: Vec<Uuid>) -> Result<()> {
        self.begin_revoke_panel_resources(panel_id, ids).wait().await
    }

    /// Cancels associated host I/O and retires this instance; reload requires a fresh trust decision.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
