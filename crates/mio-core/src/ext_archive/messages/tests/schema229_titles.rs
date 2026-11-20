use super::*;

fn current_229(builder: &mut Builder, units: &[u16]) -> (u64, u64, u64, u64, u64) {
    let (tab, title, wrapper, references, chunks) = chunked_current_229(builder, units, 10);
    assert_eq!(chunks.len(), 1);
    (tab, title, wrapper, references, chunks[0])
}

pub(super) fn chunked_current_229(
    builder: &mut Builder,
    units: &[u16],
    reference_capacity: usize,
) -> (u64, u64, u64, u64, Vec<u64>) {
    assert!(!units.is_empty());
    assert!(reference_capacity >= units.len().div_ceil(32));
    builder.bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
    builder.bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
    let chunks: Vec<_> = units
        .chunks(32)
        .map(|units| {
            let mut capacity = [0xd800_u16; 32];
            capacity[..units.len()].copy_from_slice(units);
            builder.array(
                2,
                &capacity
                    .iter()
                    .flat_map(|unit| unit.to_be_bytes())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut slots = vec![-1_i64; reference_capacity];
    for (slot, chunk) in slots.iter_mut().zip(&chunks) {
        *slot = *chunk as i64;
    }
    let references = builder.references(&slots);
    let wrapper = builder.direct(chunks.len() as i32, references);
    let title = builder.object(
        (0, 0),
        vec![
            integer(0, units.len() as i32),
            integer(1, 0),
            integer(2, 32),
            reference(3, wrapper),
        ],
    );
    let current = builder.bytes(b"PUT /current HTTP/1.1\r\n\r\n\0\xff");
    let tab = builder.object(
        (0, 2),
        vec![
            reference(4, current),
            reference(0, title),
            (2, (-1_i64).to_be_bytes().to_vec()),
            (33, (-1_i64).to_be_bytes().to_vec()),
        ],
    );
    (tab, title, wrapper, references, chunks)
}

pub(super) fn replace_field(builder: &mut Builder, object: u64, id: u8, value: &[u8]) {
    let object = object as usize;
    let field = (0..usize::from(builder.bytes[object + 3]))
        .map(|index| object + 4 + index * 3)
        .find(|offset| builder.bytes[*offset] == id)
        .unwrap();
    let relative =
        u16::from_be_bytes(builder.bytes[field + 1..field + 3].try_into().unwrap()) as usize;
    builder.bytes[object + relative..object + relative + value.len()].copy_from_slice(value);
}

#[test]
fn schema_229_title_reads_only_logical_utf16_and_slot_zero() {
    let full = "x".repeat(32);
    for (text, forwarded) in [
        ("5", false),
        ("mio-title-汉🙂", false),
        ("a\0汉🙂", false),
        (full.as_str(), false),
        ("mio-title-汉🙂", true),
    ] {
        let mut builder = Builder::new();
        let units: Vec<_> = text.encode_utf16().collect();
        let (tab, title, wrapper, _, _) = current_229(&mut builder, &units);
        if forwarded {
            let next = builder.forward(wrapper);
            replace_field(&mut builder, title, 3, &(next as i64).to_be_bytes());
            let next = builder.forward(title);
            replace_field(&mut builder, tab, 0, &(next as i64).to_be_bytes());
        }
        let fixture = builder.finish(tab);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded =
            repeater_current_229(&mut source, tab, 4096, &CancellationToken::new()).unwrap();
        let fields = value(&decoded.value);
        assert_eq!(fields.current_request.id, 4);
        assert!(matches!(
            fields.current_request.state,
            Observation::Value(_)
        ));
        assert_eq!(fields.history.id, 2);
        failed(&fields.history.state, io::ErrorKind::InvalidData);
        assert_eq!(fields.selected_history.id, 1);
        assert_eq!(fields.selected_history.state, Observation::Missing);
        assert_eq!(fields.title.id, 0);
        let title = value(&fields.title.state);
        assert_eq!(value(&title.value), text);
        assert_eq!(title.object.forwarded_from.len(), usize::from(forwarded));
        // Includes array revalidation and the invalid history reference; no inactive slots or f33 reads.
        assert_eq!(
            source.bytes_read() - before,
            138 + 2 * units.len() as u64 + u64::from(forwarded) * 18
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
    }
}

#[test]
fn schema_229_title_rejects_unverified_lengths_tags_chunks_and_references() {
    for case in [
        "empty",
        "negative",
        "oversized",
        "huge",
        "high surrogate",
        "low surrogate",
        "split pair",
        "tab tag",
        "title tag",
        "wrapper tag",
        "zero chunks",
        "two chunks",
        "null references",
        "negative slot",
        "null slot",
        "reference width",
        "insufficient reference capacity",
        "chunk capacity",
        "direct chunk",
        "self wrapper",
        "forward cycle",
    ] {
        let mut builder = Builder::new();
        let (tab, title, wrapper, references, chunk) = current_229(&mut builder, &[b'5' as u16]);
        match case {
            "empty" | "negative" | "oversized" | "huge" => {
                let length: i32 = match case {
                    "empty" => 0,
                    "negative" => -1,
                    "oversized" => 4097,
                    _ => i32::MAX,
                };
                replace_field(&mut builder, title, 0, &length.to_be_bytes());
            }
            "high surrogate" | "low surrogate" | "split pair" => {
                let unit: u16 = if case == "low surrogate" {
                    0xdc00
                } else {
                    0xd83d
                };
                builder.bytes[chunk as usize + 8..chunk as usize + 10]
                    .copy_from_slice(&unit.to_be_bytes());
                if case == "split pair" {
                    builder.bytes[chunk as usize + 10..chunk as usize + 12]
                        .copy_from_slice(&0xde42_u16.to_be_bytes());
                }
            }
            "tab tag" => builder.bytes[tab as usize + 2] = 7,
            "title tag" => builder.bytes[title as usize + 1] = 1,
            "wrapper tag" => builder.bytes[wrapper as usize + 2] = 2,
            "zero chunks" | "two chunks" => {
                let count: i32 = if case == "zero chunks" { 0 } else { 2 };
                replace_field(&mut builder, wrapper, 0, &count.to_be_bytes());
            }
            "null references" => replace_field(&mut builder, wrapper, 1, &0_i64.to_be_bytes()),
            "negative slot" | "null slot" => {
                let address: i64 = if case == "negative slot" { -1 } else { 0 };
                builder.bytes[references as usize + 8..references as usize + 16]
                    .copy_from_slice(&address.to_be_bytes());
            }
            "reference width" => {
                replace_field(&mut builder, wrapper, 1, &(chunk as i64).to_be_bytes())
            }
            "insufficient reference capacity" => {
                builder.bytes[references as usize..references as usize + 4]
                    .copy_from_slice(&9_i32.to_be_bytes());
                builder.bytes[references as usize + 4..references as usize + 8]
                    .copy_from_slice(&0_i32.to_be_bytes());
            }
            "chunk capacity" => {
                builder.bytes[chunk as usize..chunk as usize + 4]
                    .copy_from_slice(&70_i32.to_be_bytes());
                builder.bytes[chunk as usize + 4..chunk as usize + 8]
                    .copy_from_slice(&31_i32.to_be_bytes());
            }
            "direct chunk" => replace_field(&mut builder, title, 3, &(chunk as i64).to_be_bytes()),
            "self wrapper" => replace_field(&mut builder, title, 3, &(title as i64).to_be_bytes()),
            "forward cycle" => {
                let cycle = builder.forward(0);
                builder.bytes[cycle as usize + 1..cycle as usize + 9]
                    .copy_from_slice(&(cycle as i64).to_be_bytes());
                replace_field(&mut builder, title, 3, &(cycle as i64).to_be_bytes());
            }
            _ => unreachable!(),
        }
        let fixture = builder.finish(tab);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded =
            repeater_current_229(&mut source, tab, 4096, &CancellationToken::new()).unwrap();
        if case == "tab tag" {
            failed(&decoded.value, io::ErrorKind::Unsupported);
            assert_eq!(source.bytes_read() - before, 16);
        } else {
            let fields = value(&decoded.value);
            assert!(
                matches!(fields.current_request.state, Observation::Value(_)),
                "{case}"
            );
            let title = value(&fields.title.state);
            assert!(matches!(title.value, Observation::Failed(_)), "{case}");
            assert_eq!(title.object.descriptors.len(), 4);
        }
        assert!(source.bytes_read() - before <= 4096, "{case}");
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
    }
}

#[test]
fn schema_229_current_decoder_keeps_budget_cancel_and_schema_guards() {
    let mut builder = Builder::new();
    let (tab, title, _, _, _) = current_229(&mut builder, &[b'5' as u16]);
    let fixture = builder.finish(tab);
    let mut source = fixture.open();
    let cancel = CancellationToken::new();
    let before = source.bytes_read();
    for budget in [0, MAX_BYTE_BUDGET + 1, u64::MAX] {
        assert_eq!(
            repeater_current_229(&mut source, tab, budget, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(source.bytes_read(), before);
    }
    assert!(repeater_current_229(&mut source, tab, COMPACT_READ_BOUND - 1, &cancel).is_err());
    assert_eq!(source.bytes_read(), before);
    let decoded = repeater_current_229(&mut source, tab, COMPACT_READ_BOUND, &cancel).unwrap();
    let fields = value(&decoded.value);
    assert!(matches!(
        fields.current_request.state,
        Observation::Value(_)
    ));
    assert_eq!(fields.history.id, 2);
    failed(&fields.history.state, io::ErrorKind::InvalidData);
    assert_eq!(fields.selected_history.id, 1);
    assert_eq!(fields.selected_history.state, Observation::Missing);
    failed(&fields.title.state, io::ErrorKind::Other);
    // The independently decoded invalid history reference adds eight bytes before the title preflight.
    assert_eq!(source.bytes_read() - before, 48);
    let before = source.bytes_read();
    assert_eq!(
        Decoder::new(&mut source, 4096).err().unwrap().kind(),
        io::ErrorKind::Unsupported
    );
    assert_eq!(source.bytes_read(), before);

    let object = source.compact_object(title).unwrap();
    let mut decoder = Decoder {
        started_at: source.bytes_read(),
        source: &mut source,
        byte_budget: 4096,
        cancel: Some(&cancel),
    };
    let error = decoder
        .read(4, |source| {
            let result = source.field_i32(&object, 0);
            cancel.cancel();
            result
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(decoder.bytes_read(), 4);
    assert_eq!(
        decoder
            .read(4, |source| source.field_i32(&object, 0))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(decoder.bytes_read(), 4);
    let before = source.bytes_read();
    assert_eq!(
        repeater_current_229(&mut source, tab, 4096, &cancel)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(source.bytes_read(), before);

    for (floor, revision) in [(228_u16, 228_u16), (228, 229), (229, 228), (230, 230)] {
        let mut builder = Builder::new();
        let (tab, _, _, _, _) = current_229(&mut builder, &[b'5' as u16]);
        builder.bytes[12..14].copy_from_slice(&floor.to_be_bytes());
        builder.bytes[14..16].copy_from_slice(&revision.to_be_bytes());
        let fixture = builder.finish(tab);
        let mut source = fixture.open();
        let before = source.bytes_read();
        assert_eq!(
            repeater_current_229(&mut source, tab, 4096, &CancellationToken::new())
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(source.bytes_read(), before);
    }
}
