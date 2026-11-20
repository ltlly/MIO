use super::*;

fn builder() -> Builder {
    let mut builder = Builder::new();
    builder.bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
    builder.bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
    builder
}

fn annotations(builder: &mut Builder, units: &[u16], length: i32) -> u64 {
    let bytes: Vec<_> = units.iter().flat_map(|unit| unit.to_be_bytes()).collect();
    let backing = builder.array(2, &bytes);
    let node = builder.object(
        (0, 0),
        vec![
            reference(1, backing),
            integer(2, length),
            (3, i64::MIN.to_be_bytes().to_vec()),
        ],
    );
    builder.object(
        (0, 0),
        vec![(1, i64::MIN.to_be_bytes().to_vec()), reference(2, node)],
    )
}

fn decode(source: &mut Container, item: u64) -> Decoded<ProxyHttp229> {
    proxy_http_229(source, item, MAX_BYTE_BUDGET, &CancellationToken::new()).unwrap()
}

#[test]
fn http_note_reads_only_logical_prefix_and_preserves_raw_highlight_and_extra_descriptors() {
    for text in ["HTTP备注中文🙂\nsecond-line229", "**格式备注229**"] {
        let prefix: Vec<_> = text.encode_utf16().collect();
        let mut read_counts = Vec::new();
        for (capacity, highlight) in [(100, 0), (1000, 3), (2000, 255)] {
            let mut builder = builder();
            let mut units = prefix.clone();
            units.resize(capacity, 0xd800);
            let parent = annotations(&mut builder, &units, prefix.len() as i32);
            let item = builder.object((1, 0), vec![(13, vec![highlight]), reference(27, parent)]);
            let fixture = builder.finish(item);
            let original = fs::read(&fixture.path).unwrap();
            let mut source = fixture.open();
            let before = source.bytes_read();
            let decoded = decode(&mut source, item);
            read_counts.push(source.bytes_read() - before);
            let fields = value(&decoded.value);
            assert_eq!(fields.highlight.state, Observation::Value(highlight));
            let parent = value(&fields.annotations.state);
            assert!(parent.object.descriptors.iter().any(|field| field.id == 1));
            let node = value(&value(&parent.value).note.state);
            assert!(node.object.descriptors.iter().any(|field| field.id == 3));
            let note = value(&node.value);
            assert_eq!(note.text, Observation::Value(text.into()));
            assert_eq!(note.length.state, Observation::Value(prefix.len() as i32));
            assert_eq!(value(&note.backing.state).element_count, capacity as u64);
            assert_eq!(fs::read(&fixture.path).unwrap(), original);
        }
        assert_eq!(read_counts, vec![read_counts[0]; 3]);
        assert!(read_counts[0] < 256);
    }
}

#[test]
fn http_note_preserves_missing_null_unavailable_and_unsupported_nodes() {
    for case in [
        "missing_parent",
        "null_parent",
        "failed_parent",
        "missing_note",
        "null_note",
        "failed_note",
        "missing_backing",
        "null_backing",
        "failed_backing",
        "missing_length",
        "wrong_parent",
        "wrong_node",
    ] {
        let mut builder = builder();
        let backing = builder.array(2, &65_u16.to_be_bytes());
        let mut node_fields = if case == "missing_backing" {
            vec![]
        } else {
            vec![reference(
                1,
                match case {
                    "null_backing" => 0,
                    "failed_backing" => i64::MIN as u64,
                    _ => backing,
                },
            )]
        };
        if case != "missing_length" {
            node_fields.push(integer(2, 1));
        }
        let node = builder.object(
            if case == "wrong_node" { (0, 1) } else { (0, 0) },
            node_fields,
        );
        let parent = builder.object(
            if case == "wrong_parent" {
                (0, 1)
            } else {
                (0, 0)
            },
            if case == "missing_note" {
                vec![]
            } else {
                vec![reference(
                    2,
                    match case {
                        "null_note" => 0,
                        "failed_note" => i64::MIN as u64,
                        _ => node,
                    },
                )]
            },
        );
        let item = builder.object(
            (1, 0),
            if case == "missing_parent" {
                vec![]
            } else {
                vec![reference(
                    27,
                    match case {
                        "null_parent" => 0,
                        "failed_parent" => i64::MIN as u64,
                        _ => parent,
                    },
                )]
            },
        );
        let fixture = builder.finish(item);
        let decoded = decode(&mut fixture.open(), item);
        let fields = value(&decoded.value);
        assert_eq!(fields.highlight.state, Observation::Missing);
        match case {
            "missing_parent" => assert_eq!(fields.annotations.state, Observation::Missing),
            "null_parent" => assert_eq!(fields.annotations.state, Observation::Null),
            "failed_parent" => failed(&fields.annotations.state, io::ErrorKind::InvalidData),
            "wrong_parent" => failed(
                &value(&fields.annotations.state).value,
                io::ErrorKind::Unsupported,
            ),
            _ => {
                let parent = value(&value(&fields.annotations.state).value);
                match case {
                    "missing_note" => assert_eq!(parent.note.state, Observation::Missing),
                    "null_note" => assert_eq!(parent.note.state, Observation::Null),
                    "failed_note" => failed(&parent.note.state, io::ErrorKind::InvalidData),
                    "wrong_node" => {
                        failed(&value(&parent.note.state).value, io::ErrorKind::Unsupported)
                    }
                    _ => {
                        let note = value(&value(&parent.note.state).value);
                        assert!(matches!(note.text, Observation::Unavailable { .. }));
                        match case {
                            "missing_backing" => {
                                assert_eq!(note.backing.state, Observation::Missing)
                            }
                            "null_backing" => assert_eq!(note.backing.state, Observation::Null),
                            "failed_backing" => {
                                failed(&note.backing.state, io::ErrorKind::InvalidData)
                            }
                            _ => assert_eq!(note.length.state, Observation::Missing),
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn http_note_rejects_invalid_or_oversized_text_without_truncation() {
    for (units, length, expected) in [
        (vec![0xd800; 100], 0, Some(String::new())),
        (vec![65; 100], -1, None),
        (vec![65; 100], 101, None),
        (vec![65; 32_769], 32_769, None),
        (vec![0xd800], 1, None),
        (vec![0x4e2d; 21_846], 21_846, None),
        (vec![0x80; 32_768], 32_768, Some("\u{80}".repeat(32_768))),
    ] {
        let mut builder = builder();
        let parent = annotations(&mut builder, &units, length);
        let item = builder.object((1, 0), vec![reference(27, parent)]);
        let fixture = builder.finish(item);
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded = decode(&mut source, item);
        let parent = value(&value(&value(&decoded.value).annotations.state).value);
        let note = value(&value(&parent.note.state).value);
        if let Some(expected) = expected {
            assert_eq!(note.text, Observation::Value(expected));
        } else {
            failed(&note.text, io::ErrorKind::InvalidData);
        }
        if length < 0 || length as usize > units.len() || length > 32_768 {
            assert!(
                source.bytes_read() - before < 128,
                "invalid length read payload"
            );
        }
    }
}

#[test]
fn http_note_budget_cancel_and_schema_guards_prevent_extra_reads() {
    let mut builder = builder();
    let parent = annotations(&mut builder, &[65; 32_768], 32_768);
    let item = builder.object((1, 0), vec![reference(27, parent)]);
    let fixture = builder.finish(item);
    let mut source = fixture.open();
    let before = source.bytes_read();
    let cancel = CancellationToken::new();
    for budget in [0, MAX_BYTE_BUDGET + 1, COMPACT_READ_BOUND - 1] {
        assert!(proxy_http_229(&mut source, item, budget, &cancel).is_err());
        assert_eq!(source.bytes_read(), before);
    }
    cancel.cancel();
    assert_eq!(
        proxy_http_229(&mut source, item, MAX_BYTE_BUDGET, &cancel)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(source.bytes_read(), before);

    let fresh_cancel = CancellationToken::new();
    let decoded = proxy_http_229(&mut source, item, 4096, &fresh_cancel).unwrap();
    let parent = value(&value(&value(&decoded.value).annotations.state).value);
    let note = value(&value(&parent.note.state).value);
    failed(&note.text, io::ErrorKind::Other);
    assert!(source.bytes_read() - before < 128);

    let mut old = Builder::new();
    let item = old.object((1, 0), vec![]);
    let fixture = old.finish(item);
    let mut source = fixture.open();
    let before = source.bytes_read();
    assert_eq!(
        proxy_http_229(&mut source, item, MAX_BYTE_BUDGET, &fresh_cancel)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
    assert_eq!(source.bytes_read(), before);
}
