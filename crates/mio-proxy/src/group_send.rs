use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::ProxyError;

const DEFAULT_SINGLE_PACKET_BARRIER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeaterGroupSendMode {
    #[default]
    Default,
    SequentialSingleConnection,
    SequentialSeparateConnections,
    ParallelSinglePacketAttack,
    ParallelSeparateConnections,
}

#[derive(Clone)]
pub struct SinglePacketBarrier {
    barrier: Arc<tokio::sync::Barrier>,
    group_size: usize,
    timeout: Duration,
}

impl std::fmt::Debug for SinglePacketBarrier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinglePacketBarrier")
            .field("group_size", &self.group_size)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl SinglePacketBarrier {
    pub fn new(group_size: usize) -> Self {
        Self::with_timeout(group_size, DEFAULT_SINGLE_PACKET_BARRIER_TIMEOUT)
    }

    pub fn with_timeout(group_size: usize, timeout: Duration) -> Self {
        let size = group_size.max(1);
        Self {
            barrier: Arc::new(tokio::sync::Barrier::new(size)),
            group_size: size,
            timeout,
        }
    }

    pub fn group_size(&self) -> usize {
        self.group_size
    }

    pub async fn wait(&self) -> Result<(), ProxyError> {
        tokio::time::timeout(self.timeout, self.barrier.wait())
            .await
            .map(|_| ())
            .map_err(|_| ProxyError::Timeout)
    }

    pub async fn wait_or_cancel(&self, cancel: &CancellationToken) -> Result<(), ProxyError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            res = self.wait() => res,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RepeaterGroupSendTicket {
    pub mode: RepeaterGroupSendMode,
    pub connection_pool_group_id: Option<u128>,
    pub single_packet_barrier: Option<SinglePacketBarrier>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn barrier_synchronizes_participants_and_surfaces_timeout_and_cancel() {
        let barrier = SinglePacketBarrier::with_timeout(2, Duration::from_secs(2));
        let b2 = barrier.clone();
        let (res1, res2) = tokio::join!(barrier.wait(), b2.wait());
        assert!(res1.is_ok());
        assert!(res2.is_ok());

        let timeout_barrier = SinglePacketBarrier::with_timeout(2, Duration::from_millis(30));
        assert!(matches!(
            timeout_barrier.wait().await,
            Err(ProxyError::Timeout)
        ));

        let cancel_barrier = SinglePacketBarrier::with_timeout(2, Duration::from_secs(5));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            cancel_barrier.wait_or_cancel(&cancel).await,
            Err(ProxyError::Cancelled)
        ));
    }
}
