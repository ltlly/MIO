use std::time::Duration;

use mio_plugin_api::HttpRequest;
use mio_proxy::{PreparedSend, ProxyError, SendConfig, prepare_send};
use tokio_util::sync::CancellationToken;

const MAX_RETRIES: u8 = 5;

fn retry_delay(status: u16, codes: &[u16], retries: u8) -> Option<Duration> {
    (retries < MAX_RETRIES && codes.contains(&status))
        .then(|| Duration::from_secs(1_u64 << retries))
}

async fn wait_for_retry(delay: Duration, cancel: &CancellationToken) -> Result<(), ProxyError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

/// One logical Intruder job may explicitly send up to six times. The caller retains its pool
/// slot until this future joins; only the final outcome is published as the logical result.
pub(super) async fn send(
    mut prepared: PreparedSend,
    request: &HttpRequest,
    config: SendConfig,
    codes: Vec<u16>,
    cancel: CancellationToken,
) -> Result<mio_proxy::SendMetrics, ProxyError> {
    let mut retries = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        // Each single-send future drains its SDK workers before another attempt is considered.
        let metrics = prepared.send_with_metrics(cancel.clone()).await?;
        let Some(delay) = retry_delay(metrics.response.status, &codes, retries) else {
            return Ok(metrics);
        };
        // Intermediate responses are internal retry decisions, not additional stored flows.
        // Drop their bodies before waiting or preparing the next one-request SDK body.
        drop(metrics);
        wait_for_retry(delay, &cancel).await?;
        if cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        prepared = prepare_send(request, config.clone())?;
        retries += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, task::Poll};

    #[test]
    fn five_matching_responses_allow_exactly_five_exponential_retries() {
        let intervals: Vec<_> = (0..=MAX_RETRIES)
            .map(|retries| retry_delay(429, &[429], retries).map(|delay| delay.as_secs()))
            .collect();
        assert_eq!(
            intervals,
            [Some(1), Some(2), Some(4), Some(8), Some(16), None]
        );
        assert_eq!(1 + intervals.iter().flatten().count(), 6);
        assert_eq!(retry_delay(200, &[429], 0), None);
        assert_eq!(retry_delay(503, &[429], 0), None);
        assert_eq!(retry_delay(429, &[], 0), None);
        assert_eq!(
            retry_delay(503, &[429, 503], 0),
            Some(Duration::from_secs(1))
        );
        assert_eq!(retry_delay(418, &[418], 4), Some(Duration::from_secs(16)));
    }

    #[tokio::test(start_paused = true)]
    async fn virtual_retry_waits_keep_all_five_deadlines_and_cancel_before_a_ready_timer() {
        tokio::time::timeout(Duration::from_secs(60), async {
            let cancel = CancellationToken::new();
            for retries in 0..MAX_RETRIES {
                let delay = retry_delay(429, &[429], retries).unwrap();
                let started = tokio::time::Instant::now();
                let mut waiting = Box::pin(wait_for_retry(delay, &cancel));
                std::future::poll_fn(|cx| {
                    assert!(waiting.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                tokio::time::advance(delay - Duration::from_millis(1)).await;
                std::future::poll_fn(|cx| {
                    assert!(waiting.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                tokio::time::advance(Duration::from_millis(1)).await;
                waiting.await.unwrap();
                assert!(started.elapsed() >= delay);
            }
            let mut waiting = Box::pin(wait_for_retry(Duration::from_secs(1), &cancel));
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            tokio::time::advance(Duration::from_secs(2)).await;
            cancel.cancel();
            assert!(matches!(waiting.await, Err(ProxyError::Cancelled)));
            let cancel = CancellationToken::new();
            let mut waiting = Box::pin(wait_for_retry(Duration::from_secs(16), &cancel));
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            cancel.cancel();
            assert!(
                matches!(
                    tokio::time::timeout(Duration::from_millis(1), waiting)
                        .await
                        .unwrap(),
                    Err(ProxyError::Cancelled)
                ),
                "cancellation must wake before the remaining sixteen-second timer"
            );
        })
        .await
        .expect("five virtual waits and their cancellation remain bounded without real waiting");
    }
}
