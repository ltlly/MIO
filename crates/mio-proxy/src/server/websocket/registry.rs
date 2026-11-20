use super::*;
use rama::http::ws::Message;
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
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&flow_id)
            .is_some_and(|entry| !entry.sender.is_closed())
    }

    pub(in crate::server) async fn inject(
        &self,
        flow_id: Uuid,
        message: Message,
        timeout: Duration,
    ) -> Result<(), ProxyError> {
        let (kind, payload) = client::websocket_received_message(message)?;
        self.inject_directed(
            flow_id,
            WebSocketDirection::ClientToServer,
            kind,
            payload,
            timeout,
        )
        .await
        .map(|_| ())
    }

    pub async fn inject_directed(
        &self,
        flow_id: Uuid,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<WebSocketMessage, ProxyError> {
        let sender = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    pub fn remove(&self, flow_id: Uuid) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .remove(&flow_id);
    }

    fn remove_matching(&self, flow_id: Uuid, token: u64) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner
            .entries
            .get(&flow_id)
            .is_some_and(|entry| entry.token == token)
        {
            inner.entries.remove(&flow_id);
        }
    }
}
