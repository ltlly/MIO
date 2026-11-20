use super::tests_runtime::{request, running};
use super::*;

#[tokio::test]
async fn open_repeater_capacity_is_reserved_before_dispatching_side_effects() {
    let (mut running, mut calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    for _ in 0..63 {
        running
            .broker
            .bind_resource(Uuid::new_v4(), 1, false)
            .unwrap();
    }
    for id in [1, 2] {
        let call = HostCall {
            operation: Some(host_call::Operation::OpenRepeater(request().into())),
        };
        let envelope = Envelope::new(
            running.identity,
            id,
            1,
            envelope::Body::HostCall(call.clone()),
            MAX_OPERATION,
        );
        running.host_call(&envelope, call, &outgoing).await.unwrap();
    }
    let first = calls.try_recv().unwrap();
    assert!(
        calls.try_recv().is_err(),
        "the 65th resource must be rejected before a tab is created"
    );
    let denied = wire.try_recv().unwrap();
    assert_eq!(denied.in_reply_to, 2);
    assert!(matches!(
        denied.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(_))
        }))
    ));
    first
        .reply
        .send(Err(Failure::new(ErrorCode::Cancelled, "test cancelled")))
        .unwrap();
    let completed = timeout(MAX_OPERATION, running.service_tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    running.complete_service(completed, &outgoing).unwrap();
    let call = HostCall {
        operation: Some(host_call::Operation::OpenRepeater(request().into())),
    };
    let envelope = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::HostCall(call.clone()),
        MAX_OPERATION,
    );
    running.host_call(&envelope, call, &outgoing).await.unwrap();
    assert_eq!(
        calls.try_recv().unwrap().request_id,
        3,
        "failed opens must release their reservation"
    );
}

#[tokio::test]
async fn a_closed_new_tab_cannot_be_bound_by_a_late_open_repeater_result() {
    let (mut running, mut calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let resource = Uuid::new_v4();
    let call = HostCall {
        operation: Some(host_call::Operation::OpenRepeater(request().into())),
    };
    let envelope = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::HostCall(call.clone()),
        MAX_OPERATION,
    );
    running.host_call(&envelope, call, &outgoing).await.unwrap();
    calls
        .try_recv()
        .unwrap()
        .reply
        .send(Ok(ServiceResult::Resource {
            id: resource,
            revision: 1,
        }))
        .unwrap();
    let completed = timeout(MAX_OPERATION, running.service_tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (reply, _closed) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                id: resource,
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(matches!(
        wire.try_recv().unwrap().body,
        Some(envelope::Body::CloseRequest(_))
    ));
    running.complete_service(completed, &outgoing).unwrap();
    assert!(
        !running.broker.resources.contains_key(&resource),
        "closing a just-created tab must win over the earlier service reply"
    );
    assert!(
        running.revoked_resources.is_empty(),
        "finished creates must release close barriers"
    );
    let result = wire.try_recv().unwrap();
    assert_eq!(result.in_reply_to, 1);
    assert!(matches!(
        result.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(_))
        }))
    ));
}

#[tokio::test]
async fn oversized_open_is_a_correlated_error_without_a_wire_write() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let mut large = request();
    large.body = vec![0; protocol::MAX_INLINE_BODY + 1];
    let (reply, mut opened) = oneshot::channel();
    running
        .control(
            Control::Open {
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: large,
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(
        matches!(opened.try_recv(), Ok(Err(_))),
        "an oversized open must fail its own reply, not kill the pipe writer"
    );
    assert!(wire.try_recv().is_err());
    assert!(running.opening_request.is_none());
    assert!(running.broker.resources.is_empty());
}

#[tokio::test]
async fn oversized_host_results_fail_only_the_correlated_call() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    for size in [protocol::MAX_INLINE_BODY + 1, protocol::MAX_FRAME_BYTES + 1] {
        running
            .complete_service(
                ServiceCompletion {
                    request_id: 7,
                    resource_binding: None,
                    result: Ok(ServiceResult::Response(HttpResponse {
                        status: 200,
                        version: mio_plugin_api::HttpVersion::Http11,
                        reason_phrase: None,
                        headers: vec![],
                        body: vec![0; size],
                        trailers: vec![],
                    })),
                },
                &outgoing,
            )
            .unwrap();
        let result = wire.try_recv().unwrap();
        assert_eq!(result.in_reply_to, 7);
        assert!(
            matches!(
                result.body,
                Some(envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Error(_))
                }))
            ),
            "oversized results need a per-call error, not oversized pipe data"
        );
        result.validate().unwrap();
    }
}
