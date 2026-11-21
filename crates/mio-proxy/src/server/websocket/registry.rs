use super::*;
use std::time::Duration;

impl Drop for InjectorRegistrationGuard {
    fn drop(&mut self) {
        self.registry.remove_matching(self.flow_id, self.token);
    }
}

impl Default for WebSocketInjectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSocketInjectorRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InjectorInner {
                next_token: 1,
                entries: HashMap::new(),
            })),
        }
    }

    pub(super) fn register(
        &self,
        flow_id: Uuid,
        sender: mpsc::Sender<InjectRequest>,
    ) -> InjectorRegistrationGuard {
        let mut inner = crate::lock_unpoison(&self.inner);
        let token = inner.next_token;
        inner.next_token = inner.next_token.wrapping_add(1);
        inner
            .entries
            .insert(flow_id, WebSocketInjector { token, sender });
        InjectorRegistrationGuard {
            registry: self.clone(),
            flow_id,
            token,
        }
    }

    pub fn contains(&self, flow_id: Uuid) -> bool {
        crate::lock_unpoison(&self.inner)
            .entries
            .get(&flow_id)
            .is_some_and(|entry| !entry.sender.is_closed())
    }

    async fn send_inject_request(
        &self,
        flow_id: Uuid,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        message: Option<Message>,
        timeout: Duration,
    ) -> Result<WebSocketMessage, ProxyError> {
        let sender = crate::lock_unpoison(&self.inner)
            .entries
            .get(&flow_id)
            .map(|injector| injector.sender.clone())
            .ok_or_else(|| ProxyError::Network("WebSocket 连接不存在或已断开".into()))?;
        let (reply, receive) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + timeout;
        tokio::time::timeout_at(
            deadline,
            sender.send(InjectRequest {
                direction,
                kind,
                payload,
                message,
                reply,
            }),
        )
        .await
        .map_err(|_| ProxyError::Timeout)?
        .map_err(|_| ProxyError::Network("WebSocket 连接已断开".into()))?;
        match tokio::time::timeout_at(deadline, receive).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ProxyError::Network("WebSocket 连接已断开".into())),
            Err(_) => Err(ProxyError::Timeout),
        }
    }

    pub(in crate::server) async fn inject_with_limit(
        &self,
        flow_id: Uuid,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        max_body_bytes: usize,
        timeout: Duration,
    ) -> Result<WebSocketMessage, ProxyError> {
        let message = client::websocket_outgoing_message(kind, &payload, max_body_bytes)?;
        self.send_inject_request(flow_id, direction, kind, payload, Some(message), timeout)
            .await
    }

    pub async fn inject_directed(
        &self,
        flow_id: Uuid,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<WebSocketMessage, ProxyError> {
        self.send_inject_request(flow_id, direction, kind, payload, None, timeout)
            .await
    }

    pub fn remove(&self, flow_id: Uuid) {
        crate::lock_unpoison(&self.inner).entries.remove(&flow_id);
    }

    fn remove_matching(&self, flow_id: Uuid, token: u64) {
        let mut inner = crate::lock_unpoison(&self.inner);
        if inner
            .entries
            .get(&flow_id)
            .is_some_and(|entry| entry.token == token)
        {
            inner.entries.remove(&flow_id);
        }
    }
}
