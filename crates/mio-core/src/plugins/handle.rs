use super::*;

#[derive(Clone)]
pub struct PluginHandle {
    pub identity: ChannelIdentity,
    pub(super) control: mpsc::Sender<Control>,
    pub(super) cancel: CancellationToken,
}
async fn send_hook(
    control: &mpsc::Sender<Control>,
    cancel: CancellationToken,
    permission: Permission,
    body: envelope::Body,
) -> Result<HookReply> {
    let (reply, received) = oneshot::channel();
    tokio::select! {
    biased;
    _ = cancel.cancelled() => bail!("插件 Hook 调用已取消；未发送"),
    _ = control.closed() => bail!("插件已停止"),
    result = control.send(Control::Hook {
        permission,
        body,
        cancelled: cancel.clone(),
        reply,
    }) => result.context("插件已停止")?,
    }
    tokio::select! {
    biased;
    _ = cancel.cancelled() => bail!("插件 Hook 调用已取消"),
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
            cancel,
            Permission::TrafficRead,
            envelope::Body::ActiveCheckCall(call),
        )
        .await?
        {
            HookReply::ActiveCheck(res) => Ok(res),
            _ => bail!("插件返回了不匹配的主动检查结果类型"),
        }
    }

    pub fn try_event(&self, event: UiEvent) -> Result<()> {
        self.control
            .try_send(Control::Event(event))
            .map_err(|_| anyhow::anyhow!("插件事件队列已满或插件已停止"))
    }

    /// Returns the correlated, verified snapshot without depending on the separate event queue.
    pub async fn open_request(
        &self,
        id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    ) -> Result<Arc<PanelSnapshot>> {
        let (reply, received) = oneshot::channel();
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("插件已停止"),
            result = self.control.send(Control::Open { id, revision, read_only, request, reply }) => result.context("插件已停止")?,
        }
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("插件已停止；未确认打开请求"),
            result = received => result.context("插件未完成打开请求")?,
        }
    }

    pub async fn open_response(
        &self,
        id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    ) -> Result<Arc<PanelSnapshot>> {
        let (reply, received) = oneshot::channel();
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("插件已停止"),
            result = self.control.send(Control::OpenResponse { id, revision, read_only, response, reply }) => result.context("插件已停止")?,
        }
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("插件已停止；未确认打开响应"),
            result = received => result.context("插件未完成打开响应")?,
        }
    }

    pub async fn commit(
        &self,
        request: CommitEditor,
        cancel: CancellationToken,
    ) -> Result<HttpRequest> {
        let (reply, received) = oneshot::channel();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("插件编辑提交已取消；未发送"),
            _ = self.cancel.cancelled() => bail!("插件已停止"),
            result = self.control.send(Control::Commit { request, reply }) => result.context("插件已停止")?,
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { self.cancel.cancel(); bail!("插件编辑提交已取消；插件实例正在回收，未发送") },
            _ = self.cancel.cancelled() => bail!("插件已停止；未发送旧稿"),
            result = received => result.context("插件未完成编辑提交")?,
        }
    }

    pub async fn commit_response(
        &self,
        request: protocol::CommitResponseEditor,
        cancel: CancellationToken,
    ) -> Result<HttpResponse> {
        let (reply, received) = oneshot::channel();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("插件响应编辑提交已取消"),
            _ = self.cancel.cancelled() => bail!("插件已停止"),
            result = self.control.send(Control::CommitResponse { request, reply }) => result.context("插件已停止")?,
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { self.cancel.cancel(); bail!("插件响应编辑提交已取消；实例正在回收") },
            _ = self.cancel.cancelled() => bail!("插件已停止；未应用旧响应修改"),
            result = received => result.context("插件未完成响应编辑提交")?,
        }
    }

    /// Revocation is final even if the worker fails to acknowledge it; explicitly open to regrant.
    pub async fn revoke_resource(&self, id: Uuid) -> Result<()> {
        ensure!(!id.is_nil(), "资源身份无效");
        let (reply, received) = oneshot::channel();
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("插件已停止"),
            result = self.control.send(Control::Revoke { id, reply }) => result.context("插件已停止")?,
        }
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("插件已停止；资源不会自动重新授权"),
            result = received => result.context("插件未确认关闭；资源不会自动重新授权")?,
        }
    }

    /// Cancels associated host I/O and retires this instance; reload requires a fresh trust decision.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
