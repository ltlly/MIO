use super::*;

#[test]
fn regression_discard_retirement_survives_uppercase_uuid_key_on_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("uppercase-retirement.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let input = spec(1);
    let tab = bind(&mut store, &input);
    let id = Uuid::from_u128(0xaabbccddeeff);
    store.create_intruder_run(id, tab, &input, 50).unwrap();
    let flow = complete(&pending());
    store.put_intruder_result(id, 0, Ok(&flow)).unwrap();
    let attack = store
        .set_intruder_state(id, IntruderRunState::Completed)
        .unwrap();
    let proposal = discard_proposal(&store);
    store
        .discard_intruder_run(id, attack.revision, proposal, &CancellationToken::new())
        .unwrap();
    assert!(store.find_intruder_run(id).unwrap().is_none());
    assert!(store.read_optional(flow.summary.id).unwrap().is_none());
    let marker: Vec<u8> = store
        .connection
        .query_row(
            "SELECT value FROM main.metadata WHERE key=?1",
            [retirement_key(id)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker.len(), 16);
    store.close().unwrap();

    let edit = Connection::open(&path).unwrap();
    let uppercase = format!("intruder_discarded:{}", id.to_string().to_ascii_uppercase());
    assert_ne!(uppercase, retirement_key(id));
    assert_eq!(
        edit.execute(
            "UPDATE main.metadata SET key=?1 WHERE key=?2",
            params![uppercase, retirement_key(id)],
        )
        .unwrap(),
        1
    );
    let changed: Vec<u8> = edit
        .query_row(
            "SELECT value FROM main.metadata WHERE key=?1",
            [&uppercase],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(changed, marker);
    assert_eq!(
        edit.pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        crate::store::SCHEMA
    );
    edit.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    edit.close().unwrap();
    let before_open = std::fs::read(&path).unwrap();
    let refused = match ProjectStore::open(&path) {
        Err(_) => {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before_open,
                "reopen rejection must not rewrite the source"
            );
            None
        }
        Ok(mut reopened) => {
            let admission = reopened.create_intruder_run(id, tab, &input, 50);
            let replay = reopened.put_intruder_result(id, 0, Ok(&flow));
            reopened.close().unwrap();
            Some((admission.is_err(), replay.is_err()))
        }
    };
    let read =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let counts: (u64, u64, u64, u64) = read
        .query_row(
            "SELECT (SELECT count(*) FROM intruder_runs WHERE id=?1),
                (SELECT count(*) FROM intruder_results WHERE run=?1),
                (SELECT count(*) FROM flows WHERE id=?2),
                (SELECT count(*) FROM messages WHERE flow=?2)",
            params![id.to_string(), flow.summary.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let markers: Vec<Vec<u8>> = read
        .prepare("SELECT value FROM main.metadata WHERE key COLLATE NOCASE=?1")
        .unwrap()
        .query_map([retirement_key(id)], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    read.close().unwrap();

    if let Some((admission, replay)) = refused {
        assert!(
            admission,
            "a differently cased retirement key must still block the same run UUID"
        );
        assert!(replay, "retired result records must not be replayed");
    }
    assert_eq!(counts, (0, 0, 0, 0));
    assert_eq!(
        markers,
        [marker],
        "the valid retirement marker must not be lost"
    );
}

#[test]
fn regression_discard_retirement_rejects_blob_key_without_replay() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("blob-retirement.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let input = spec(1);
    let tab = bind(&mut store, &input);
    let id = Uuid::new_v4();
    store.create_intruder_run(id, tab, &input, 50).unwrap();
    let flow = complete(&pending());
    store.put_intruder_result(id, 0, Ok(&flow)).unwrap();
    let run = store
        .set_intruder_state(id, IntruderRunState::Completed)
        .unwrap();
    let proposal = discard_proposal(&store);
    store
        .discard_intruder_run(id, run.revision, proposal, &CancellationToken::new())
        .unwrap();
    store.close().unwrap();
    let edit = Connection::open(&path).unwrap();
    assert_eq!(
        edit.execute(
            "UPDATE metadata SET key=CAST(key AS BLOB) WHERE key=?1",
            [retirement_key(id)],
        )
        .unwrap(),
        1
    );
    let marker: (Vec<u8>, Vec<u8>) = edit
        .query_row(
            "SELECT key,value FROM metadata WHERE typeof(key)='blob'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(marker.0, retirement_key(id).as_bytes());
    assert_eq!(marker.1.len(), 16);
    edit.close().unwrap();
    let before = std::fs::read(&path).unwrap();
    match ProjectStore::open(&path) {
        Err(_) => assert_eq!(std::fs::read(&path).unwrap(), before),
        Ok(mut reopened) => {
            let admitted = reopened.create_intruder_run(id, tab, &input, 50).is_ok();
            let replayed = reopened.put_intruder_result(id, 0, Ok(&flow)).is_ok();
            reopened.close().unwrap();
            assert!(
                !admitted && !replayed,
                "BLOB-typed retirement key must block same-ID admission and replay"
            );
        }
    }
    let read =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let after: (Vec<u8>, Vec<u8>) = read
        .query_row(
            "SELECT key,value FROM metadata WHERE typeof(key)='blob'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, marker);
    let count: u64 = read
        .query_row(
            "SELECT count(*) FROM intruder_runs WHERE id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    read.close().unwrap();
}

#[test]
fn regression_discard_refuses_nullable_payload_ordinal_without_orphaning_data() {
    fn payload_rows(connection: &Connection, id: Uuid) -> Vec<(u64, Option<u64>, Vec<u8>)> {
        let mut query = connection.prepare(
            "SELECT set_index,ordinal,value FROM intruder_payloads WHERE run=?1 ORDER BY set_index,ordinal",
        ).unwrap();
        query
            .query_map([id.to_string()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nullable-payload.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let attack = run(&mut store, 1);
    let flow = complete(&pending());
    store.put_intruder_result(attack.id, 0, Ok(&flow)).unwrap();
    let attack = store
        .set_intruder_state(attack.id, IntruderRunState::Completed)
        .unwrap();
    store.close().unwrap();
    let edit = Connection::open(&path).unwrap();
    edit.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE replacement_payloads (
             run TEXT NOT NULL,set_index INTEGER NOT NULL,ordinal INTEGER,value BLOB NOT NULL,
             PRIMARY KEY(run,set_index,ordinal));
         INSERT INTO replacement_payloads SELECT run,set_index,ordinal,value FROM intruder_payloads;
         DROP TABLE intruder_payloads;
         ALTER TABLE replacement_payloads RENAME TO intruder_payloads;",
    )
    .unwrap();
    edit.execute(
        "INSERT INTO intruder_payloads(run,set_index,ordinal,value) VALUES(?1,0,NULL,?2)",
        params![attack.id.to_string(), [0_u8, 255, b'N'].as_slice()],
    )
    .unwrap();
    let key: (u64, u64) = edit.query_row(
        "SELECT \"notnull\",pk FROM pragma_table_xinfo('intruder_payloads') WHERE name='ordinal'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(key, (0, 3));
    let without_rowid: bool = edit
        .query_row(
            "SELECT wr FROM pragma_table_list WHERE schema='main' AND name='intruder_payloads'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!without_rowid);
    assert!(
        !edit
            .prepare("PRAGMA foreign_key_list('intruder_payloads')")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert!(
        !edit
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert_eq!(
        edit.query_row::<String, _, _>("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap(),
        "ok"
    );
    edit.close().unwrap();

    let mut store = ProjectStore::open(&path)
        .expect("ordinary nullable composite-key table must reach discard");
    assert_eq!(store.read_intruder_run(attack.id).unwrap(), attack);
    assert_eq!(
        store
            .intruder_results(attack.id, None, 1)
            .unwrap()
            .rows
            .len(),
        1
    );
    let original_payloads = payload_rows(&store.connection, attack.id);
    assert_eq!(
        original_payloads,
        [(0, None, vec![0, 255, b'N']), (0, Some(0), vec![0, 255, 1])]
    );
    let original_workspace = discard_workspace_bytes(&store.connection);
    let proposal = discard_proposal(&store);
    let result = store.discard_intruder_run(
        attack.id,
        attack.revision,
        proposal,
        &CancellationToken::new(),
    );
    let after_payloads = payload_rows(&store.connection, attack.id);
    let after_workspace = discard_workspace_bytes(&store.connection);
    let after_run = store.find_intruder_run(attack.id).unwrap();
    let after_flow = store.read_optional(flow.summary.id).unwrap();
    let marker = retired_run(&store.connection, attack.id).unwrap();
    store.close().unwrap();

    assert!(
        result.is_err(),
        "discard must refuse nullable payload keys rather than leave live orphan rows"
    );
    assert_eq!(after_payloads, original_payloads);
    assert_eq!(after_workspace, original_workspace);
    assert_eq!(after_run, Some(attack));
    assert_eq!(after_flow, Some(flow));
    assert!(marker.is_none());
}
