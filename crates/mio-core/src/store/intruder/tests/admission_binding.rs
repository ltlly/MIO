use super::*;

#[test]
fn task_title_columns_belong_to_each_run_without_rewriting_the_frozen_spec() {
    let mut store = store();
    let attack = run(&mut store, 1);
    let columns: usize = store.connection.query_row(
        "SELECT count(*) FROM pragma_table_info('intruder_runs') WHERE name IN ('title','title_revision')",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(
        columns, 2,
        "attack names need their own persisted value and revision"
    );
    assert_eq!(store.read_intruder_run(attack.id).unwrap(), attack);
    assert_eq!(store.read_intruder_spec(attack.id).unwrap(), spec(1));
}

#[test]
fn another_connection_inserting_after_the_precheck_cannot_be_admitted_again() {
    let mut store = store();
    let input = spec(1);
    let tab = bind(&mut store, &input);
    let id = Uuid::new_v4();
    assert!(store.find_intruder_run(id).unwrap().is_none());
    let mut competing = ProjectStore::open(&store.test_actual_path()).unwrap();
    let existing = competing.create_intruder_run(id, tab, &input, 12).unwrap();
    competing.close().unwrap();
    assert!(
        store.create_intruder_run(id, tab, &input, 12).is_err(),
        "a committed run inserted after admission's precheck is not a fresh run"
    );
    assert_eq!(store.read_intruder_run(id).unwrap(), existing);
    assert_eq!(store.read_intruder_spec(id).unwrap(), input);
    assert_eq!(store.intruder_runs(None, 10).unwrap().rows, [existing]);
}

#[test]
fn new_runs_bind_to_the_entire_saved_open_intruder_spec() {
    let mut store = store();
    let input = spec(1);
    let missing = Uuid::new_v4();
    assert!(
        store
            .create_intruder_run(missing, Uuid::new_v4(), &input, 12)
            .is_err()
    );
    assert!(store.find_intruder_run(missing).unwrap().is_none());
    let tab = bind(&mut store, &input);

    let mutations: &[fn(&mut IntruderSpec)] = &[
        |spec| spec.revision += 1,
        |spec| spec.draft.revision += 1,
        |spec| spec.draft.target = "http://127.0.0.1:2".into(),
        |spec| spec.draft.raw[0] = b'G',
        |spec| spec.draft.version = HttpVersion::Http2,
        |spec| spec.attack_type = AttackType::BatteringRam,
        |spec| spec.positions[0].start += 1,
        |spec| spec.payload_sets[0].push(vec![1]),
        |spec| spec.max_concurrent_requests += 1,
        |spec| spec.request_timeout_ms += 1,
        |spec| spec.update_host_header = !spec.update_host_header,
    ];
    for mutate in mutations {
        let mut changed = input.clone();
        mutate(&mut changed);
        changed.validate().unwrap();
        let id = Uuid::new_v4();
        let error = store
            .create_intruder_run(id, tab, &changed, 12)
            .unwrap_err();
        assert!(error.to_string().contains("已保存 revision"), "{error}");
        assert!(store.find_intruder_run(id).unwrap().is_none());
    }

    let mut workspace = store.workspace().unwrap();
    let repeater = workspace.open_request(None).unwrap();
    store.save_workspace(&workspace).unwrap();
    assert!(
        store
            .create_intruder_run(Uuid::new_v4(), repeater, &input, 12)
            .is_err()
    );
    let id = Uuid::new_v4();
    let created = store.create_intruder_run(id, tab, &input, 12).unwrap();
    workspace.intruder.tabs[0].spec.draft.raw[0] = b'G';
    workspace.intruder.tabs[0].spec.draft.revision += 1;
    workspace.intruder.tabs[0].spec.revision += 1;
    workspace.revision += 1;
    store.save_workspace(&workspace).unwrap();
    assert_eq!(store.read_intruder_spec(id).unwrap(), input);
    assert!(
        store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 12)
            .is_err()
    );

    workspace
        .intruder
        .closed
        .push(workspace.intruder.tabs.remove(0));
    workspace.intruder.active = None;
    workspace.revision += 1;
    store.save_workspace(&workspace).unwrap();
    assert!(
        store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 12)
            .is_err()
    );
    assert!(store.create_intruder_run(id, tab, &input, 12).is_err());
    assert_eq!(store.read_intruder_run(id).unwrap(), created);
    assert_eq!(store.intruder_runs(None, 10).unwrap().rows, [created]);
}

#[test]
fn frozen_inputs_use_blobs_and_preserve_binary_duplicates_and_empty_sets() {
    let mut store = store();
    let mut input = spec(2);
    input.payload_sets[0].push(Vec::new());
    let id = Uuid::new_v4();
    let tab = bind(&mut store, &input);
    let created = store.create_intruder_run(id, tab, &input, 12).unwrap();
    assert_eq!(
        (created.total, created.spec_revision, created.revision),
        (3, 3, 1)
    );
    assert_eq!(store.read_intruder_spec(id).unwrap(), input);
    assert!(store.create_intruder_run(id, tab, &input, 12).is_err());
    assert_eq!(store.read_intruder_run(id).unwrap(), created);
    input.draft.raw[0] = b'X';
    assert!(store.create_intruder_run(id, tab, &input, 12).is_err());
    let (kind, bytes): (String, usize) = store
        .connection
        .query_row(
            "SELECT typeof(template),length(template) FROM intruder_runs WHERE id=?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((kind.as_str(), bytes), ("blob", 20));
    let (values, bytes): (usize, usize) = store
        .connection
        .query_row(
            "SELECT count(*),sum(length(value)) FROM intruder_payloads WHERE run=?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((values, bytes), (3, 6));

    let mut empty = spec(1);
    empty.attack_type = AttackType::Pitchfork;
    empty.positions.push(Position { start: 20, end: 20 });
    empty.payload_sets.push(Vec::new());
    let tab = bind(&mut store, &empty);
    let zero = store
        .create_intruder_run(Uuid::new_v4(), tab, &empty, 13)
        .unwrap();
    assert_eq!(zero.total, 0);
    assert_eq!(store.read_intruder_spec(zero.id).unwrap(), empty);
    assert_eq!(
        store
            .set_intruder_state(zero.id, IntruderRunState::Completed)
            .unwrap()
            .completed,
        0
    );
}

#[test]
fn dispatch_is_sequential_but_completions_can_arrive_out_of_order() {
    let mut store = store();
    let run = run(&mut store, 3);
    let first = pending();
    let second = pending();
    assert!(store.put_intruder_result(run.id, 1, Ok(&second)).is_err());
    let admitted = store.put_intruder_result(run.id, 0, Ok(&first)).unwrap();
    assert_eq!((admitted.dispatched, admitted.completed), (1, 0));
    assert_eq!(
        store.put_intruder_result(run.id, 0, Ok(&first)).unwrap(),
        admitted
    );
    store.put_intruder_result(run.id, 1, Ok(&second)).unwrap();
    let rejected = store
        .put_intruder_result(run.id, 2, Err("invalid generated request"))
        .unwrap();
    assert_eq!((rejected.dispatched, rejected.completed), (3, 1));
    assert_eq!(
        store
            .put_intruder_result(run.id, 2, Err("invalid generated request"))
            .unwrap(),
        rejected
    );
    assert!(
        store
            .put_intruder_result(run.id, 2, Err("different error"))
            .is_err()
    );
    assert!(
        store
            .put_intruder_result(run.id, 2, Ok(&pending()))
            .is_err()
    );
    assert!(
        store
            .put_intruder_result(run.id, 3, Err("out of range"))
            .is_err()
    );
    assert!(
        store
            .put_intruder_result(run.id, u64::MAX, Err("overflow"))
            .is_err()
    );
    let finished_second = complete(&second);
    let partial = store
        .put_intruder_result(run.id, 1, Ok(&finished_second))
        .unwrap();
    assert_eq!(partial.completed, 2);
    let finished = store
        .put_intruder_result(run.id, 0, Ok(&complete(&first)))
        .unwrap();
    assert_eq!((finished.dispatched, finished.completed), (3, 3));
    assert_eq!(
        store
            .put_intruder_result(run.id, 1, Ok(&finished_second))
            .unwrap(),
        finished
    );
    let done = store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();
    assert_eq!(
        store
            .put_intruder_result(run.id, 1, Ok(&finished_second))
            .unwrap(),
        done
    );
    assert_eq!(
        store.read(first.summary.id).unwrap().response.unwrap().body,
        [255, 0, 128]
    );
}

#[test]
fn flow_identity_requests_and_terminal_results_cannot_be_replaced() {
    let mut store = store();
    let run = run(&mut store, 2);
    let first = pending();
    store.put_intruder_result(run.id, 0, Ok(&first)).unwrap();
    assert!(store.put_intruder_result(run.id, 1, Ok(&first)).is_err());
    assert!(
        store
            .put_intruder_result(run.id, 0, Ok(&complete(&pending())))
            .is_err()
    );
    let mut changed_request = complete(&first);
    changed_request.request.body[0] = 123;
    assert!(
        store
            .put_intruder_result(run.id, 0, Ok(&changed_request))
            .is_err()
    );
    assert_eq!(store.read(first.summary.id).unwrap(), first);
    let finished = complete(&first);
    store.put_intruder_result(run.id, 0, Ok(&finished)).unwrap();
    let mut changed_body = finished.clone();
    changed_body.response.as_mut().unwrap().body[0] = 12;
    assert!(
        store
            .put_intruder_result(run.id, 0, Ok(&changed_body))
            .is_err()
    );
    changed_body.summary.revision += 1;
    assert!(
        store
            .put_intruder_result(run.id, 0, Ok(&changed_body))
            .is_err()
    );
    assert!(store.put_intruder_result(run.id, 0, Ok(&first)).is_err());
    assert_eq!(store.read(first.summary.id).unwrap(), finished);
    let mut foreign = pending();
    foreign.summary.source = TrafficSource::Repeater;
    assert!(store.put_intruder_result(run.id, 1, Ok(&foreign)).is_err());
    assert!(store.read_optional(foreign.summary.id).unwrap().is_none());
}
