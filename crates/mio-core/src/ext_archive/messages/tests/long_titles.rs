use super::*;

fn decoded_title(decoded: &Decoded<RepeaterCurrent229>) -> &str {
    let fields = value(&decoded.value);
    assert!(matches!(
        fields.current_request.state,
        Observation::Value(_)
    ));
    value(&value(&fields.title.state).value)
}

#[test]
fn schema_229_chunked_titles_join_utf16_before_decode_and_read_only_active_windows() {
    let texts = [
        ("1".to_owned(), 10),
        (format!("{}🙂", "a".repeat(31)), 10),
        (format!("{}🙂{}", "a".repeat(31), "b".repeat(32)), 10),
        (format!("{}🙂{}", "a".repeat(31), "b".repeat(320)), 20),
        ("1".to_owned(), 9),
    ];
    for (text, reference_capacity) in texts {
        let units: Vec<_> = text.encode_utf16().collect();
        let mut builder = Builder::new();
        let (tab, _, _, _, chunks) = chunked_current_229(&mut builder, &units, reference_capacity);
        let fixture = builder.finish(tab);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded =
            repeater_current_229(&mut source, tab, 16 * 1024, &CancellationToken::new()).unwrap();
        assert_eq!(decoded_title(&decoded), text, "{} chunks", chunks.len());
        // Poisoned inactive references and final-array characters are excluded from the read count.
        assert_eq!(
            source.bytes_read() - before,
            114 + 24 * chunks.len() as u64 + 2 * units.len() as u64
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
    }
}

#[test]
fn schema_229_chunked_title_units_and_decoded_utf8_each_have_a_4096_limit() {
    for (text, accepted) in [
        ("a".repeat(4096), true),
        (format!("{}a", "汉".repeat(1365)), true),
        ("🙂".repeat(1024), true),
        ("汉".repeat(1366), false),
        ("🙂".repeat(1025), false),
    ] {
        let units: Vec<_> = text.encode_utf16().collect();
        let mut builder = Builder::new();
        let capacity = units.len().div_ceil(32).div_ceil(10) * 10;
        let (tab, _, _, _, _) = chunked_current_229(&mut builder, &units, capacity);
        let fixture = builder.finish(tab);
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded =
            repeater_current_229(&mut source, tab, 16 * 1024, &CancellationToken::new()).unwrap();
        if accepted {
            assert_eq!(decoded_title(&decoded), text);
        } else {
            let title = value(&value(&decoded.value).title.state);
            failed(&title.value, io::ErrorKind::InvalidData);
        }
        assert!(source.bytes_read() - before <= 16 * 1024);
    }
}

#[test]
fn schema_229_chunked_titles_reject_inconsistent_counts_references_and_unit_bounds() {
    for case in [
        "empty",
        "negative length",
        "length limit",
        "huge length",
        "minimum length",
        "negative chunks",
        "zero chunks",
        "short chunks",
        "extra chunks",
        "huge chunks",
        "short references",
        "negative reference count",
        "huge reference count",
        "null middle",
        "negative middle",
        "outside middle",
        "short utf16 capacity",
        "long utf16 capacity",
        "broken surrogate",
    ] {
        let mut builder = Builder::new();
        let text = format!("{}🙂{}", "a".repeat(31), "b".repeat(32));
        let units: Vec<_> = text.encode_utf16().collect();
        let (tab, title, wrapper, references, chunks) =
            chunked_current_229(&mut builder, &units, 10);
        match case {
            "empty" | "negative length" | "length limit" | "huge length" | "minimum length" => {
                let length: i32 = match case {
                    "empty" => 0,
                    "negative length" => -1,
                    "length limit" => 4097,
                    "huge length" => i32::MAX,
                    _ => i32::MIN,
                };
                replace_field(&mut builder, title, 0, &length.to_be_bytes());
            }
            "negative chunks" | "zero chunks" | "short chunks" | "extra chunks" | "huge chunks" => {
                let count: i32 = match case {
                    "negative chunks" => -1,
                    "zero chunks" => 0,
                    "short chunks" => 2,
                    "extra chunks" => 4,
                    _ => i32::MAX,
                };
                replace_field(&mut builder, wrapper, 0, &count.to_be_bytes());
            }
            "short references" => {
                builder.bytes[references as usize..references as usize + 4]
                    .copy_from_slice(&24_i32.to_be_bytes());
                builder.bytes[references as usize + 4..references as usize + 8]
                    .copy_from_slice(&2_i32.to_be_bytes());
            }
            "negative reference count" | "huge reference count" => {
                let count = if case == "negative reference count" {
                    -1_i32
                } else {
                    i32::MAX
                };
                builder.bytes[references as usize + 4..references as usize + 8]
                    .copy_from_slice(&count.to_be_bytes());
            }
            "null middle" | "negative middle" | "outside middle" => {
                let address: i64 = match case {
                    "null middle" => 0,
                    "negative middle" => -1,
                    _ => i64::MAX,
                };
                builder.bytes[references as usize + 16..references as usize + 24]
                    .copy_from_slice(&address.to_be_bytes());
            }
            "short utf16 capacity" | "long utf16 capacity" => {
                let count = if case == "short utf16 capacity" {
                    31_i32
                } else {
                    33_i32
                };
                let chunk = chunks[1] as usize;
                builder.bytes[chunk..chunk + 4].copy_from_slice(&(8 + 2 * count).to_be_bytes());
                builder.bytes[chunk + 4..chunk + 8].copy_from_slice(&count.to_be_bytes());
            }
            "broken surrogate" => {
                let second = chunks[1] as usize;
                builder.bytes[second + 8..second + 10].copy_from_slice(&0x0062_u16.to_be_bytes());
            }
            _ => unreachable!(),
        }
        let fixture = builder.finish(tab);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded =
            repeater_current_229(&mut source, tab, 4096, &CancellationToken::new()).unwrap();
        let fields = value(&decoded.value);
        assert!(
            matches!(fields.current_request.state, Observation::Value(_)),
            "{case}"
        );
        assert!(
            matches!(value(&fields.title.state).value, Observation::Failed(_)),
            "{case}"
        );
        assert!(source.bytes_read() - before <= 4096, "{case}");
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
    }
}

#[test]
fn schema_229_chunked_title_budget_failure_keeps_the_current_request() {
    let units: Vec<_> = format!("{}🙂{}", "a".repeat(31), "b".repeat(320))
        .encode_utf16()
        .collect();
    let mut builder = Builder::new();
    let (tab, _, _, _, _) = chunked_current_229(&mut builder, &units, 20);
    let fixture = builder.finish(tab);
    let mut source = fixture.open();
    let before = source.bytes_read();
    let decoded = repeater_current_229(&mut source, tab, 1000, &CancellationToken::new()).unwrap();
    let fields = value(&decoded.value);
    assert!(matches!(
        fields.current_request.state,
        Observation::Value(_)
    ));
    let title_failed = match &fields.title.state {
        Observation::Failed(_) => true,
        Observation::Value(title) => matches!(title.value, Observation::Failed(_)),
        _ => false,
    };
    assert!(title_failed);
    assert!(source.bytes_read() - before <= 1000);
}
