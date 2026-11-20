use super::*;

mod ack_backpressure;
mod auto_throttle;
mod auto_throttle_boundaries;
mod basic;
#[path = "default_pool.rs"]
mod default_pool;
mod delay;
mod delete_partial;
mod rebind;
mod rebind_failures;
mod runtime_catalog;
mod tasks;

fn pool(name: &str, limit: Option<u16>) -> IntruderResourcePool {
    IntruderResourcePool {
        retained_inputs: None,
        automatic_throttling: IntruderAutoThrottle::default(),
        delay: None,
        id: Uuid::new_v4(),
        name: name.into(),
        max_concurrent_requests: limit,
    }
}

fn install(state: &mut State, pools: Vec<IntruderResourcePool>) -> IntruderResourcePools {
    state
        .set_intruder_resource_pools(
            state.project,
            Uuid::new_v4(),
            state.intruder.resource_pools.revision,
            pools,
            &CancellationToken::new(),
        )
        .unwrap()
}

fn rebind(state: &mut State, id: Uuid, pool: Uuid) -> IntruderRunSummary {
    let revision = state.intruder_run(id).unwrap().resource_pool_revision;
    state
        .set_intruder_resource_pool(
            state.project,
            id,
            Uuid::new_v4(),
            revision,
            pool,
            &CancellationToken::new(),
        )
        .unwrap()
}

async fn finish_next(state: &mut State) {
    let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
    state.complete_intruder(completion).await.unwrap();
}

async fn respond(stream: &mut tokio::net::TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
        .await
        .unwrap();
}

fn no_extra(listener: TcpListener) {
    assert_eq!(
        listener.into_std().unwrap().accept().unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
}
