use super::tests_runtime::{identity, request};
use super::*;

#[test]
fn broker_denies_ungranted_forged_read_only_and_stale_requests() {
    let identity = identity();
    let envelope = Envelope::new(
        identity,
        1,
        1,
        envelope::Body::Ready(panel::Empty {}),
        MAX_OPERATION,
    );
    let send = HostCall {
        operation: Some(host_call::Operation::NetworkSend(request().into())),
    };
    let mut broker = PermissionBroker::new(identity, [Permission::DraftWrite]);
    assert!(broker.authorize(&envelope, send.clone()).is_err());
    let mut forged = envelope.clone();
    forged.instance_id = Uuid::new_v4().as_bytes().to_vec();
    assert!(broker.authorize(&forged, send).is_err());
    let id = Uuid::new_v4();
    broker.bind_resource(id, 3, true).unwrap();
    let update = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: id.as_bytes().to_vec(),
            expected_revision: 3,
            request: Some(request().into()),
        })),
    };
    assert!(broker.authorize(&envelope, update.clone()).is_err());
    broker.bind_resource(id, 4, false).unwrap();
    assert!(broker.authorize(&envelope, update).is_err());
    let valid = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: id.as_bytes().to_vec(),
            expected_revision: 4,
            request: Some(request().into()),
        })),
    };
    assert!(matches!(
        broker.authorize(&envelope, valid),
        Ok(ServiceOperation::UpdateDraft {
            expected_revision: 4,
            ..
        })
    ));
}

#[test]
fn late_draft_completion_cannot_reauthorize_read_only_or_newer_resources() {
    let mut broker = PermissionBroker::new(identity(), [Permission::DraftWrite]);
    let id = Uuid::new_v4();
    broker.bind_resource(id, 3, false).unwrap();
    let pending = PendingResource::UpdateDraft { id, revision: 3 };
    broker.bind_resource(id, 3, true).unwrap();
    assert!(
        broker.complete_resource(pending, id, 4).is_err(),
        "late completion cannot restore revoked write access"
    );
    assert!(broker.resources[&id].read_only);
    assert_eq!(broker.resources[&id].revision, 3);
    broker.bind_resource(id, 5, false).unwrap();
    assert!(
        broker.complete_resource(pending, id, 4).is_err(),
        "late completion cannot roll back a newer resource revision"
    );
    assert_eq!(broker.resources[&id].revision, 5);
    assert!(
        broker
            .complete_resource(PendingResource::UpdateDraft { id, revision: 5 }, id, 6)
            .is_ok()
    );
    assert_eq!(broker.resources[&id].revision, 6);
}
