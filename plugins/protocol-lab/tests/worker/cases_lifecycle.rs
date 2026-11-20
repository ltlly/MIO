use std::future::Future;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[tokio::test]
async fn dropping_a_polled_stop_future_does_not_detach_the_worker_join() {
    let (_directory, package) = fixture_package("cancel-hang");
    let (mut worker, mut calls) = start(package, &[Permission::TrafficRead]).await;
    let (inflight, pid) = entered_hang(&worker, &mut calls).await;
    {
        let stopping = worker.stop();
        tokio::pin!(stopping);
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(stopping.as_mut().poll(cx)))
                .await
                .is_pending()
        );
    }
    let returned_without_joining;
    {
        let stopping = worker.stop();
        tokio::pin!(stopping);
        match std::future::poll_fn(|cx| Poll::Ready(stopping.as_mut().poll(cx))).await {
            Poll::Ready(result) => {
                result.unwrap();
                returned_without_joining = true;
            }
            Poll::Pending => {
                returned_without_joining = false;
                timeout(QUICK, stopping).await.unwrap().unwrap();
            }
        }
    }
    timeout(QUICK, async {
        loop {
            match worker.events.recv().await {
                Some(PluginEvent::Stopped { .. }) => break,
                Some(_) => {}
                None => panic!("missing terminal event"),
            }
        }
    })
    .await
    .unwrap();
    assert!(inflight.cancel.is_cancelled());
    assert_reaped(pid);
    assert!(
        !returned_without_joining,
        "dropping stop lost the still-running JoinHandle"
    );
}

#[tokio::test]
async fn cooperative_worker_exit_still_terminates_its_own_process_group() {
    let (directory, package) = fixture_package("cooperative-group");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    fs::write(
        directory.path().join("descendant.addr"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let (mut worker, mut calls) = start(package, &[Permission::TrafficRead]).await;
    let notification = next_call(&mut calls).await;
    let ServiceOperation::QueryHistory { filter, .. } = &notification.operation else {
        panic!("missing descendant identity");
    };
    let mut ids = filter.strip_prefix("group-child:").unwrap().split(':');
    let parent: u32 = ids.next().unwrap().parse().unwrap();
    let child: u32 = ids.next().unwrap().parse().unwrap();
    assert!(parent > 1 && child > 1 && parent != child);
    let (mut probe, _) = timeout(QUICK, listener.accept()).await.unwrap().unwrap();
    let mut ready = [0];
    timeout(QUICK, probe.read_exact(&mut ready))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready, [1]);
    notification.reply.send(Ok(ServiceResult::Done)).unwrap();
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
    let mut byte = [0];
    let closed = timeout(QUICK, probe.read(&mut byte)).await;
    if closed.is_err() {
        // The test owns this socket; release its child without addressing any process by PID.
        probe.write_all(&[1]).await.unwrap();
        assert_eq!(
            timeout(QUICK, probe.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
    assert_reaped(parent);
    assert_reaped(child);
    assert!(
        matches!(closed, Ok(Ok(0)))
            || matches!(
                &closed,
                Ok(Err(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                    )
            ),
        "own descendant {child} survived successful stop: {closed:?}"
    );
}

#[tokio::test]
async fn cancellation_revokes_host_tasks_and_joins_stalled_worker() {
    let (_directory, package) = fixture_package("cancel-hang");
    let (mut worker, mut calls) = start(package, &[Permission::TrafficRead]).await;
    let (inflight, pid) = entered_hang(&worker, &mut calls).await;
    worker.handle().cancel();
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
    assert!(inflight.cancel.is_cancelled());
    assert_reaped(pid);
}

#[tokio::test]
async fn operation_deadline_kills_and_joins_stalled_worker_without_sleep_polling() {
    let (_directory, package) = fixture_package("deadline-hang");
    let (mut worker, mut calls) = start(package, &[Permission::TrafficRead]).await;
    let (inflight, pid) = entered_hang(&worker, &mut calls).await;
    timeout(Duration::from_secs(7), async {
        loop {
            match worker.events.recv().await {
                Some(PluginEvent::Stopped { reason, .. }) => {
                    assert!(reason.contains("超时"));
                    break;
                }
                Some(_) => {}
                None => panic!("missing terminal event"),
            }
        }
    })
    .await
    .unwrap();
    assert!(worker.stop().await.is_err());
    assert!(inflight.cancel.is_cancelled());
    assert_reaped(pid);
}

#[tokio::test]
async fn reload_validates_candidate_before_replacing_identity() {
    let (_directory, package) = real_package();
    let (mut worker, _calls) = start(package.clone(), &[]).await;
    let original = worker.identity;
    let old_handle = worker.handle();
    let current = panel(&mut worker, |p| p.revision >= 2).await;
    let (_bad_directory, bad) = fixture_package("bad-frame");
    let trust = NativeTrust::confirmed(&bad, []).unwrap();
    let (services, _calls) = mpsc::channel(16);
    assert!(worker.reload(bad, trust, services).await.is_err());
    assert_eq!(worker.identity, original);
    old_handle
        .try_event(click(10, snapshots(&current)))
        .unwrap();
    panel(&mut worker, |p| p.revision > current.revision).await;
    let trust = NativeTrust::confirmed(&package, []).unwrap();
    let (services, _calls) = mpsc::channel(16);
    timeout(QUICK, worker.reload(package, trust, services))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(worker.identity.instance_id, original.instance_id);
    assert_eq!(worker.identity.project, original.project);
    assert!(old_handle.try_event(click(10, vec![])).is_err());
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[test]
fn discovering_manifest_does_not_allow_entry_to_escape_package() {
    let (_directory, mut package) = real_package();
    package.manifest.entry = Path::new("../outside").into();
    fs::write(
        package.directory.join("manifest.json"),
        serde_json::to_vec(&package.manifest).unwrap(),
    )
    .unwrap();
    assert!(VerifiedPackage::read(&package.directory).is_err());
}
