use super::*;
use crate::{HttpVersion, intruder::AttackPlan};

fn spec() -> IntruderSpec {
    IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:1".into(),
            raw: b"POST / HTTP/1.1\r\n\r\na".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 19, end: 20 }],
        payload_sets: vec![vec![vec![0, 255], vec![]]],
        max_concurrent_requests: 2,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
    }
}

#[test]
fn draft_validation_preserves_unfinished_inputs_without_making_them_runnable() {
    let mut unfinished = spec();
    unfinished.draft.target.clear();
    unfinished.draft.raw = vec![0, 255, 0];
    unfinished.positions.clear();
    unfinished.payload_sets.clear();
    unfinished.max_concurrent_requests = 0;
    unfinished.request_timeout_ms = 0;
    unfinished.validate_draft().unwrap();
    assert!(unfinished.validate().is_err());
    let restored: IntruderSpec =
        serde_json::from_slice(&serde_json::to_vec(&unfinished).unwrap()).unwrap();
    assert_eq!(restored, unfinished);

    // Template edits can leave markers temporarily out of range or out of order.
    unfinished.positions = vec![
        Position {
            start: usize::MAX,
            end: 0,
        },
        Position { start: 0, end: 2 },
    ];
    unfinished.payload_sets = vec![vec![vec![], vec![255], vec![255]]; 2];
    unfinished.validate_draft().unwrap();
    assert!(unfinished.validate().is_err());
    let restored: IntruderSpec =
        serde_json::from_slice(&serde_json::to_vec(&unfinished).unwrap()).unwrap();
    assert_eq!(restored, unfinished);
}

#[test]
fn draft_validation_keeps_identity_and_input_resource_limits() {
    use crate::intruder::{MAX_PAYLOAD_SETS, MAX_PAYLOADS, MAX_POSITIONS};

    let mut invalid = spec();
    invalid.revision = 0;
    assert!(invalid.validate_draft().is_err());
    invalid = spec();
    invalid.draft.revision = u64::MAX;
    assert!(invalid.validate_draft().is_err());
    invalid = spec();
    invalid.positions = vec![Position { start: 0, end: 0 }; MAX_POSITIONS + 1];
    assert!(invalid.validate_draft().is_err());
    invalid = spec();
    invalid.payload_sets = vec![vec![]; MAX_PAYLOAD_SETS + 1];
    assert!(invalid.validate_draft().is_err());
    invalid = spec();
    invalid.payload_sets = vec![vec![vec![]; MAX_PAYLOADS + 1]];
    assert!(invalid.validate_draft().is_err());
}

#[test]
fn draft_pool_inputs_bound_each_text_without_parsing_unfinished_controls() {
    use crate::{
        IntruderAutoThrottle, IntruderPoolDelay, IntruderPoolDelayMode, IntruderPoolInputs,
        IntruderPoolSelection, IntruderResourcePool,
    };
    let mut input = spec();
    input.resource_pool = Some(IntruderPoolSelection::Create(IntruderResourcePool {
        id: Uuid::new_v4(),
        name: "draft".into(),
        max_concurrent_requests: Some(10),
        delay: Some(IntruderPoolDelay::Fixed { milliseconds: 500 }),
        automatic_throttling: IntruderAutoThrottle::default(),
        retained_inputs: Some(IntruderPoolInputs {
            cap: "incomplete cap".into(),
            delay: "incomplete delay".into(),
            increment: "incomplete increment".into(),
            delay_mode: IntruderPoolDelayMode::Increment,
        }),
    }));
    input.validate_draft().unwrap();
    assert!(
        input.validate().is_err(),
        "Start still checks active values and mode"
    );
    for field in 0..3 {
        let mut boundary = input.clone();
        let Some(IntruderPoolSelection::Create(pool)) = &mut boundary.resource_pool else {
            unreachable!()
        };
        let retained = pool.retained_inputs.as_mut().unwrap();
        let text = match field {
            0 => &mut retained.cap,
            1 => &mut retained.delay,
            _ => &mut retained.increment,
        };
        *text = "x".repeat(4096);
        boundary.validate_draft().unwrap();
        let Some(IntruderPoolSelection::Create(pool)) = &mut boundary.resource_pool else {
            unreachable!()
        };
        let retained = pool.retained_inputs.as_mut().unwrap();
        match field {
            0 => &mut retained.cap,
            1 => &mut retained.delay,
            _ => &mut retained.increment,
        }
        .push('x');
        assert!(
            boundary.validate_draft().is_err(),
            "field {field} exceeds its independent limit"
        );
    }
}

#[test]
fn frozen_spec_keeps_binary_data_and_uses_the_generator_validation() {
    let original = spec();
    let restored: IntruderSpec =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(restored, original);
    assert_eq!(restored.validate().unwrap(), 2);
    let plan = AttackPlan::new(
        restored.attack_type,
        restored.draft.raw,
        restored.positions,
        restored.payload_sets,
    )
    .unwrap();
    assert_eq!(plan.request_count(), 2);
    assert_eq!(plan.request(0).unwrap(), b"POST / HTTP/1.1\r\n\r\n\0\xff");
    let mut invalid = original;
    invalid.positions[0].end += 1;
    assert!(invalid.validate().is_err());
}

#[test]
fn start_storage_policy_round_trips_and_legacy_configuration_keeps_project_semantics() {
    let mut original = spec();
    original.save_to_project = false;
    let json = serde_json::to_value(&original).unwrap();
    assert_eq!(
        serde_json::from_value::<IntruderSpec>(json.clone()).unwrap(),
        original
    );
    let mut legacy = json;
    legacy.as_object_mut().unwrap().remove("save_to_project");
    original.save_to_project = true;
    assert_eq!(
        serde_json::from_value::<IntruderSpec>(legacy.clone()).unwrap(),
        original
    );
    legacy["save_to_project"] = serde_json::json!("false");
    assert!(serde_json::from_value::<IntruderSpec>(legacy).is_err());
}

#[test]
fn baseline_start_policy_is_explicit_and_missing_legacy_flag_does_not_add_a_request() {
    let mut input = spec();
    input.baseline_request = true;
    assert_eq!(input.validate().unwrap(), 3);
    let mut json = serde_json::to_value(&input).unwrap();
    assert_eq!(
        serde_json::from_value::<IntruderSpec>(json.clone()).unwrap(),
        input
    );
    json.as_object_mut().unwrap().remove("baseline_request");
    input.baseline_request = false;
    let legacy: IntruderSpec = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(legacy, input);
    assert_eq!(legacy.validate().unwrap(), 2);
    json["baseline_request"] = serde_json::json!(1);
    assert!(serde_json::from_value::<IntruderSpec>(json).is_err());

    input.attack_type = AttackType::ClusterBomb;
    input.draft.raw = b"POST / HTTP/1.1\r\n\r\nabc".to_vec();
    input.positions = (19..22)
        .map(|start| Position {
            start,
            end: start + 1,
        })
        .collect();
    input.payload_sets = vec![vec![b"x".to_vec(); 100]; 3];
    assert_eq!(input.validate().unwrap(), crate::intruder::MAX_REQUESTS);
    input.baseline_request = true;
    assert!(
        input.validate().is_err(),
        "baseline cannot bypass the total slot budget"
    );
}

#[test]
fn baseline_display_number_and_progress_follow_its_slot_and_actual_completion() {
    let mut run = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        id: Uuid::new_v4(),
        tab_id: Uuid::new_v4(),
        revision: 1,
        spec_revision: 1,
        total: 2,
        dispatched: 2,
        completed: 2,
        state: IntruderRunState::Completed,
        started_unix_ms: 0,
        retention: IntruderRetention::Project,
        title: String::new(),
        title_revision: 0,
        baseline: None,
    };
    assert_eq!(run.payload_progress(), Some((2, 2)));
    assert_eq!(
        (run.request_number(0), run.request_number(1)),
        (Some(1), Some(2))
    );
    run.total = 3;
    run.state = IntruderRunState::Running;
    run.baseline = Some(IntruderBaseline {
        ordinal: 2,
        completed: false,
    });
    assert_eq!(run.payload_progress(), Some((2, 2)));
    assert_eq!(run.request_number(2), Some(0));
    assert_eq!(run.request_number(3), None);
    run.dispatched = 3;
    run.baseline = Some(IntruderBaseline {
        ordinal: 0,
        completed: false,
    });
    assert_eq!(
        run.payload_progress(),
        Some((2, 2)),
        "pending baseline is not subtracted from completions"
    );
    assert_eq!(
        (
            run.request_number(0),
            run.request_number(1),
            run.request_number(2)
        ),
        (Some(0), Some(1), Some(2))
    );
    run.completed = 3;
    assert_eq!(
        run.payload_progress(),
        None,
        "completed count cannot include a pending baseline"
    );
    run.baseline.as_mut().unwrap().completed = true;
    assert_eq!(run.payload_progress(), Some((2, 2)));
    run.completed = 0;
    assert_eq!(run.payload_progress(), None);
    run.baseline = Some(IntruderBaseline {
        ordinal: 3,
        completed: false,
    });
    assert_eq!(run.payload_progress(), None);
    run.baseline = Some(IntruderBaseline {
        ordinal: 0,
        completed: false,
    });
    run.dispatched = 0;
    run.state = IntruderRunState::Stopped;
    assert_eq!(
        run.payload_progress(),
        Some((0, 2)),
        "stopping before baseline dispatch is valid"
    );
    let mut legacy = serde_json::to_value(&run).unwrap();
    legacy.as_object_mut().unwrap().remove("baseline");
    assert!(
        serde_json::from_value::<IntruderRunSummary>(legacy)
            .unwrap()
            .baseline
            .is_none()
    );
}

#[test]
fn admission_rejects_invalid_identity_target_and_resource_settings() {
    for target in [
        "",
        "ftp://127.0.0.1/",
        "http://u:p@127.0.0.1/",
        "http://127.0.0.1/#fragment",
    ] {
        let mut invalid = spec();
        invalid.draft.target = target.into();
        assert!(invalid.validate().is_err());
    }
    for revision in [0, u64::MAX] {
        let mut invalid = spec();
        invalid.revision = revision;
        assert!(invalid.validate().is_err());
        invalid = spec();
        invalid.draft.revision = revision;
        assert!(invalid.validate().is_err());
    }
    for concurrent in [0, crate::MAX_INTRUDER_CONCURRENT_REQUESTS + 1, u16::MAX] {
        let mut invalid = spec();
        invalid.max_concurrent_requests = concurrent;
        assert!(invalid.validate().is_err());
    }
    for timeout in [0, 300_001, u64::MAX] {
        let mut invalid = spec();
        invalid.request_timeout_ms = timeout;
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn input_and_output_limits_are_shared_with_plan_construction() {
    let mut boundary = spec();
    boundary.max_concurrent_requests = crate::MAX_INTRUDER_CONCURRENT_REQUESTS;
    boundary.request_timeout_ms = 300_000;
    boundary.revision = i64::MAX as u64;
    boundary.draft.revision = i64::MAX as u64;
    assert_eq!(boundary.validate().unwrap(), 2);
    boundary.payload_sets[0].clear();
    assert_eq!(boundary.validate().unwrap(), 0);
    boundary.positions.push(Position { start: 0, end: 0 });
    assert!(
        boundary.validate().is_err(),
        "empty sets cannot hide invalid positions"
    );
}

#[test]
fn summary_retention_round_trips_and_defaults_legacy_json_to_project() {
    let mut run = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: Uuid::new_v4(),
        tab_id: Uuid::new_v4(),
        revision: 7,
        spec_revision: 3,
        total: 2,
        dispatched: 2,
        completed: 2,
        state: IntruderRunState::Completed,
        started_unix_ms: 1,
        retention: IntruderRetention::Project,
        title: String::new(),
        title_revision: 0,
    };
    assert_eq!(IntruderRetention::default(), IntruderRetention::Project);
    for (retention, name) in [
        (IntruderRetention::Project, "Project"),
        (IntruderRetention::Session, "Session"),
    ] {
        run.retention = retention;
        let bytes = serde_json::to_vec(&run).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["retention"], name);
        assert_eq!(
            serde_json::from_slice::<IntruderRunSummary>(&bytes).unwrap(),
            run
        );
    }
    let mut legacy = serde_json::to_value(&run).unwrap();
    legacy.as_object_mut().unwrap().remove("retention");
    run.retention = IntruderRetention::Project;
    assert_eq!(
        serde_json::from_value::<IntruderRunSummary>(legacy.clone()).unwrap(),
        run
    );
    legacy["retention"] = serde_json::json!("Unknown");
    assert!(serde_json::from_value::<IntruderRunSummary>(legacy).is_err());
}

#[test]
fn task_title_round_trip_keeps_a_revision_independent_of_lifecycle() {
    let json = serde_json::json!({
        "id": Uuid::new_v4(), "tab_id": Uuid::new_v4(),
        "revision": 17, "spec_revision": 3,
        "total": 2, "dispatched": 2, "completed": 2,
        "state": "Completed", "started_unix_ms": 1,
        "retention": "Session", "title": "独立攻击名称🙂", "title_revision": 2,
        "baseline": null, "resource_pool": null, "resource_pool_revision": 0
    });
    let restored: IntruderRunSummary = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), json);
    let mut legacy = json;
    legacy.as_object_mut().unwrap().remove("title");
    legacy.as_object_mut().unwrap().remove("title_revision");
    let restored: IntruderRunSummary = serde_json::from_value(legacy).unwrap();
    assert_eq!((restored.title.as_str(), restored.title_revision), ("", 0));
}

#[test]
fn resource_pool_selection_is_explicit_and_legacy_concurrency_stays_independent() {
    let input = spec();
    let mut legacy = serde_json::to_value(&input).unwrap();
    legacy.as_object_mut().unwrap().remove("resource_pool");
    let restored: IntruderSpec = serde_json::from_value(legacy.clone()).unwrap();
    assert!(restored.resource_pool.is_none());
    assert_eq!(
        restored.max_concurrent_requests,
        input.max_concurrent_requests
    );
    assert_eq!(restored.validate().unwrap(), input.validate().unwrap());
    let pool = super::super::IntruderResourcePool {
        retained_inputs: None,
        automatic_throttling: super::super::IntruderAutoThrottle::default(),
        delay: None,
        id: Uuid::new_v4(),
        name: "同项目共享".into(),
        max_concurrent_requests: Some(3),
    };
    for selection in [
        super::super::IntruderPoolSelection::Existing(pool.id),
        super::super::IntruderPoolSelection::Create(pool),
    ] {
        let mut selected = input.clone();
        selected.resource_pool = Some(selection);
        assert_eq!(selected.validate().unwrap(), 2);
        assert_eq!(
            serde_json::from_slice::<IntruderSpec>(&serde_json::to_vec(&selected).unwrap())
                .unwrap(),
            selected
        );
    }
    legacy["resource_pool"] = serde_json::json!({"Existing": Uuid::nil()});
    assert!(
        serde_json::from_value::<IntruderSpec>(legacy)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn only_acknowledged_terminal_states_are_terminal() {
    assert!(!IntruderRunState::Running.is_terminal());
    assert!(!IntruderRunState::Paused.is_terminal());
    assert!(!IntruderRunState::Stopping.is_terminal());
    for state in [
        IntruderRunState::Stopped,
        IntruderRunState::Completed,
        IntruderRunState::Interrupted,
        IntruderRunState::Failed("storage failed".into()),
    ] {
        assert!(state.is_terminal());
    }
}
