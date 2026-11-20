use super::*;

#[test]
fn highlight_aliases_repeat_occurrences_and_clear_survive_save_as_without_touching_notes_or_source()
{
    let (mut store, source, items) = fixture(3);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let note = store
        .set_ext_http_notes(owner, source, items[0].id, 0, "# Notes kept🙂", &cancel)
        .unwrap();
    let input = [
        change(source, items[2].id, 0, 1),
        change(source, items[1].id, 0, 2),
    ];
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "ext_source_chunks",
                column_name: "data",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    assert_eq!(
        store
            .set_ext_http_highlights(owner, &input, &cancel)
            .unwrap(),
        [
            (
                source,
                items[2].id,
                ExtHttpHighlight {
                    revision: 1,
                    color: 1
                }
            ),
            (
                source,
                items[1].id,
                ExtHttpHighlight {
                    revision: 1,
                    color: 2
                }
            )
        ]
    );
    let (total, rows) = store.history_page(0, 10, "").unwrap();
    assert_eq!((total, rows.len()), (3, 3));
    for row in rows {
        let HistoryRow::ExtProxy(row) = row else {
            panic!("unexpected live row")
        };
        let expected = if row.item.id == items[0].id {
            ExtHttpHighlight {
                revision: 1,
                color: 1,
            }
        } else if row.item.id == items[1].id {
            ExtHttpHighlight {
                revision: 1,
                color: 2,
            }
        } else {
            ExtHttpHighlight {
                revision: 0,
                color: 3,
            }
        };
        assert_eq!(row.highlight, Some(expected));
        assert!(row.highlight_error.is_none());
    }
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let clear = [change(source, items[0].id, 1, 0)];
    let cleared = store
        .set_ext_http_highlights(owner, &clear, &cancel)
        .unwrap();
    assert_eq!(
        cleared,
        [(
            source,
            items[0].id,
            ExtHttpHighlight {
                revision: 2,
                color: 0
            }
        )]
    );
    assert_eq!(
        store
            .set_ext_http_highlights(owner, &[change(source, items[2].id, 2, 0)], &cancel)
            .unwrap()[0]
            .2,
        cleared[0].2
    );
    assert_eq!(
        store.ext_http_notes(owner, source, items[0].id).unwrap(),
        note
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("highlight.mio");
    let copy = directory.path().join("copy.mio");
    store.save(Some(&path)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&path, &copy] {
        let store = ProjectStore::open(path).unwrap();
        let values = store
            .ext_http_highlights(
                owner,
                &[
                    change(source, items[2].id, 0, 0),
                    change(source, items[3].id, 0, 0),
                    change(source, items[4].id, 0, 0),
                ],
            )
            .unwrap();
        assert_eq!(
            values,
            [
                (
                    source,
                    items[2].id,
                    ExtHttpHighlight {
                        revision: 2,
                        color: 0
                    }
                ),
                (
                    source,
                    items[3].id,
                    ExtHttpHighlight {
                        revision: 1,
                        color: 2
                    }
                ),
                (
                    source,
                    items[4].id,
                    ExtHttpHighlight {
                        revision: 0,
                        color: 3
                    }
                )
            ]
        );
        assert_eq!(
            store.ext_http_notes(owner, source, items[0].id).unwrap(),
            note
        );
        for item in &items {
            assert_eq!(store.ext_item(item.id).unwrap(), *item);
        }
        assert_eq!(store.read_source_range(source, 0, 1024).unwrap(), [0; 1024]);
        assert_eq!(store.page(0, 1, "").unwrap().0, 0);
        store.close().unwrap();
    }
}

#[test]
fn invalid_source_highlights_stay_visible_and_do_not_block_valid_notes() {
    for mode in ["missing", "null", "failed", "unknown", "binding", "field"] {
        let (mut store, source, items) = fixture(3);
        let owner = project(&store);
        for item in &items {
            metadata(&store, item, |value| {
                let field = &mut value["record"]["mapping"]["value"]["highlight"];
                match mode {
                    "missing" => *field = Value::Null,
                    "null" | "failed" => field["observation"] = json!({"state":mode}),
                    "unknown" => field["observation"]["value"] = json!(255),
                    "binding" => field["binding"]["item_id"] = json!(Uuid::new_v4()),
                    "field" => field["id"] = json!(12),
                    _ => unreachable!(),
                }
            });
        }
        let (total, rows) = store.history_page(0, 10, "").unwrap();
        assert_eq!((total, rows.len()), (3, 3));
        for row in rows {
            let HistoryRow::ExtProxy(row) = row else {
                unreachable!()
            };
            assert!(row.highlight.is_none());
            assert!(row.highlight_error.is_some());
            assert!(row.notes_error.is_none());
        }
        let error = store
            .set_ext_http_highlights(
                owner,
                &[change(source, items[0].id, 0, 1)],
                &CancellationToken::new(),
            )
            .unwrap_err();
        assert!(error.stored.is_empty());
        assert_eq!(
            store
                .set_ext_http_notes(
                    owner,
                    source,
                    items[2].id,
                    0,
                    "independent",
                    &CancellationToken::new()
                )
                .unwrap()
                .revision,
            1
        );
    }
    let (mut store, source, items) = fixture(0);
    let owner = project(&store);
    for item in &items {
        metadata(&store, item, |value| {
            value["record"]["mapping"]["value"]["annotations"]["binding"]["source_id"] =
                json!(Uuid::new_v4())
        });
    }
    assert!(store.ext_http_notes(owner, source, items[2].id).is_err());
    assert_eq!(
        store
            .set_ext_http_highlights(
                owner,
                &[change(source, items[2].id, 0, 9)],
                &CancellationToken::new()
            )
            .unwrap()[0]
            .2,
        ExtHttpHighlight {
            revision: 1,
            color: 9
        }
    );
}

#[test]
fn batch_conflicts_duplicates_invalid_projects_and_cancellation_never_partially_commit() {
    let (mut store, source, items) = fixture(3);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    assert!(store.set_ext_http_highlights(owner, &[], &cancel).is_err());
    for changes in [
        vec![
            change(source, items[0].id, 0, 1),
            change(source, items[2].id, 0, 1),
        ],
        vec![
            change(source, items[0].id, 0, 1),
            change(source, items[1].id, 8, 2),
        ],
        vec![
            change(source, items[0].id, 0, 1),
            change(source, items[5].id, 0, 2),
        ],
        vec![change(source, items[0].id, 0, 255)],
        vec![change(source, items[0].id, 0, 1); 501],
    ] {
        assert!(
            store
                .set_ext_http_highlights(owner, &changes, &cancel)
                .is_err()
        );
        assert_eq!(
            store
                .connection
                .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_highlights", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            0
        );
    }
    let foreign = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    assert!(
        store
            .set_ext_http_highlights(foreign, &[change(source, items[0].id, 0, 1)], &cancel)
            .unwrap_err()
            .stored
            .is_empty()
    );
    let error = store
        .set_ext_http_highlights(
            owner,
            &[
                change(source, items[0].id, 8, 1),
                change(source, Uuid::new_v4(), 0, 1),
            ],
            &cancel,
        )
        .unwrap_err();
    assert_eq!(
        error.stored,
        [(
            source,
            items[0].id,
            ExtHttpHighlight {
                revision: 0,
                color: 3
            }
        )]
    );
    let during_write = CancellationToken::new();
    let observed = during_write.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Insert {
                    table_name: "ext_http_highlights"
                }
            ) {
                observed.cancel();
            }
            Authorization::Allow
        }));
    assert!(
        store
            .set_ext_http_highlights(
                owner,
                &[
                    change(source, items[0].id, 0, 1),
                    change(source, items[1].id, 0, 2)
                ],
                &during_write
            )
            .is_err()
    );
    assert!(during_write.is_cancelled());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store
            .connection
            .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_highlights", [], |row| row
                .get(0))
            .unwrap(),
        0
    );
    metadata(&store, &items[2], |value| {
        value["record"]["mapping"]["value"]["highlight"]["observation"]["value"] = json!(2)
    });
    assert!(
        store
            .set_ext_http_highlights(owner, &[change(source, items[2].id, 0, 1)], &cancel)
            .is_err()
    );
}

#[test]
fn notes_and_highlights_keep_independent_revisions_and_share_project_transaction_rollback() {
    let (mut store, source, items) = fixture(3);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let note = ExtHttpNotesChange {
        source_id: source,
        item_id: items[0].id,
        expected_notes_revision: 0,
        text: "first note".into(),
    };
    {
        let tx = store.connection.transaction().unwrap();
        super::super::super::http_notes::write_changes(&tx, std::slice::from_ref(&note), &cancel)
            .unwrap();
        assert!(write_changes(&tx, &[change(source, items[0].id, 8, 1)], &cancel).is_err());
    }
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[0].id)
            .unwrap()
            .revision,
        0
    );
    {
        let tx = store.connection.transaction().unwrap();
        assert_eq!(
            write_changes(&tx, &[change(source, items[2].id, 0, 1)], &cancel).unwrap(),
            [(source, items[2].id, 1)]
        );
        assert_eq!(
            super::super::super::http_notes::write_changes(
                &tx,
                std::slice::from_ref(&note),
                &cancel
            )
            .unwrap(),
            [(source, items[0].id, 1)]
        );
        tx.commit().unwrap();
    }
    store
        .set_ext_http_notes(owner, source, items[0].id, 1, "newer note", &cancel)
        .unwrap();
    let result = store
        .set_ext_http_highlights(owner, &[change(source, items[0].id, 1, 2)], &cancel)
        .unwrap();
    assert_eq!(
        result[0].2,
        ExtHttpHighlight {
            revision: 2,
            color: 2
        }
    );
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[0].id)
            .unwrap()
            .text,
        "newer note"
    );
    {
        let tx = store.connection.transaction().unwrap();
        write_changes(&tx, &[change(source, items[0].id, 2, 0)], &cancel).unwrap();
        assert!(super::super::super::http_notes::write_changes(&tx, &[note], &cancel).is_err());
    }
    assert_eq!(
        store
            .ext_http_highlights(owner, &[change(source, items[0].id, 0, 0)])
            .unwrap()[0]
            .2,
        result[0].2
    );
}
