use super::*;

mod cancellation;
mod deadline;

fn handle(runner: JoinHandle<Result<(), ProxyError>>, stop_timeout: Duration) -> ProxyHandle {
    let (_, dynamic_init, _) = ProxyConfig::new(MitmAuthority::generate().unwrap()).into_parts();
    let (dynamic, _) = watch::channel(dynamic_init);
    ProxyHandle {
        address: (Ipv4Addr::LOCALHOST, 0).into(),
        cancel: CancellationToken::new(),
        dynamic,
        runner: Some(runner),
        stop_timeout,
        terminal_records: Default::default(),
        injectors: websocket::WebSocketInjectorRegistry::new(),
    }
}

#[tokio::test]
async fn stop_deadline_keeps_a_live_join_until_a_retry_really_finishes() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let finished = CancellationToken::new();
        let runner_finished = finished.clone();
        let (entered, entered_receiver) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let runner = tokio::spawn(async move {
            let _finished = runner_finished.drop_guard();
            entered.send(()).unwrap();
            released.await.unwrap();
            Ok(())
        });
        let id = runner.id();
        let mut handle = handle(runner, Duration::from_millis(2));
        entered_receiver.await.unwrap();
        let first = handle.stop().await;
        let second = handle.stop().await;
        let cancelled = handle.cancellation_token().is_cancelled();
        let retained = !handle.is_joined()
            && handle
                .runner
                .as_ref()
                .is_some_and(|runner| runner.id() == id);
        let finished_early = finished.is_cancelled();
        release.send(()).unwrap();
        finished.cancelled().await;
        let joined = handle.stop().await;
        assert!(finished.is_cancelled());
        assert!(handle.is_joined());
        handle.stop().await.unwrap();
        assert!(matches!(first, Err(ProxyError::StopDeadline)));
        assert!(matches!(second, Err(ProxyError::StopDeadline)));
        assert!(cancelled);
        assert!(retained, "both deadlines must retain the original runner");
        assert!(!finished_early, "a deadline must not abort cleanup");
        joined.unwrap();
    })
    .await
    .expect("deadline retries and fixture cleanup are bounded");
}

#[tokio::test]
async fn completion_wait_is_passive_cancel_safe_and_preserves_the_runner_error() {
    use std::future::Future;

    tokio::time::timeout(Duration::from_secs(8), async {
        let finished = CancellationToken::new();
        let runner_finished = finished.clone();
        let (entered, entered_receiver) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let runner = tokio::spawn(async move {
            let _finished = runner_finished.drop_guard();
            entered.send(()).unwrap();
            released.await.unwrap();
            Err(ProxyError::Network("runner failure root cause".into()))
        });
        let id = runner.id();
        let mut handle = handle(runner, Duration::from_secs(3));
        entered_receiver.await.unwrap();
        let pending = {
            let mut waiting = Box::pin(handle.join());
            std::future::poll_fn(|cx| std::task::Poll::Ready(waiting.as_mut().poll(cx)))
                .await
                .is_pending()
        };
        let cancelled = handle.cancellation_token().is_cancelled();
        let retained = !handle.is_joined()
            && handle.runner.as_ref().is_some_and(|runner| runner.id() == id);
        release.send(()).unwrap();
        let result = handle.join().await;
        assert!(finished.is_cancelled());
        assert!(handle.is_joined());
        assert!(pending && retained);
        assert!(!cancelled, "observing completion must not cancel a live Proxy");
        assert!(matches!(result, Err(ProxyError::Network(message)) if message == "runner failure root cause"));
    })
    .await
    .expect("a cancelled completion wait still joins its original runner");
}

#[tokio::test]
async fn stop_propagates_a_join_error_instead_of_reporting_success() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let finished = CancellationToken::new();
        let runner_finished = finished.clone();
        let runner = tokio::spawn(async move {
            let _finished = runner_finished.drop_guard();
            panic!("Proxy runner panic fixture");
        });
        let mut handle = handle(runner, Duration::from_secs(3));
        let result = handle.stop().await;
        assert!(finished.is_cancelled());
        assert!(handle.is_joined(), "an error still consumes the actual join");
        assert!(matches!(result, Err(ProxyError::Network(message)) if message.contains("Proxy runner panic fixture")));
    })
    .await
    .expect("a failed runner is actually joined");
}
