use super::*;
use crate::{
    Draft, HttpVersion, IntruderAutoThrottle, IntruderPoolDelay, IntruderRetention,
    IntruderRunState, IntruderSpec, IntruderTab,
    intruder::{AttackType, Position},
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[path = "tests/catalog_and_runs.rs"]
mod catalog_and_runs;
mod default_pool_tests;
#[path = "tests/guards_and_extensions.rs"]
mod guards_and_extensions;
#[path = "tests/migrations_and_policies.rs"]
mod migrations_and_policies;
#[path = "tests/random_tests.rs"]
mod random_tests;
#[path = "tests/retained_inputs.rs"]
mod retained_inputs;
#[path = "tests/tasks.rs"]
mod tasks;

pub(super) fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

pub(super) fn pool() -> IntruderResourcePool {
    IntruderResourcePool {
        retained_inputs: None,
        automatic_throttling: IntruderAutoThrottle::default(),
        delay: None,
        id: Uuid::new_v4(),
        name: "共享池".into(),
        max_concurrent_requests: Some(2),
    }
}

pub(super) fn spec(selection: Option<IntruderPoolSelection>, persistent: bool) -> IntruderSpec {
    IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:1".into(),
            raw: b"GET /?x=X HTTP/1.1\r\n\r\n".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 8, end: 9 }],
        payload_sets: vec![vec![b"A".to_vec(), b"B".to_vec()]],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: persistent,
        baseline_request: false,
        resource_pool: selection,
    }
}

pub(super) fn bind(store: &mut ProjectStore, spec: &IntruderSpec) -> Uuid {
    let id = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.tabs.push(IntruderTab {
        id,
        title: "pool".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(id);
    store.save_workspace(&workspace).unwrap();
    id
}
