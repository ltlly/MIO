use super::*;

const OBSERVED_TIMES: [i64; 7] = [
    1794116727080,
    1794116789362,
    1794117404346,
    1794120041500,
    1794125602482,
    1794125656179,
    1794126317491,
];

fn builder() -> Builder {
    let mut builder = Builder::new();
    builder.bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
    builder.bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
    builder
}

#[test]
fn proxy_time_reads_observed_signed_milliseconds_without_dereferencing_or_sentinel_guessing() {
    let mut read_counts = Vec::new();
    for timestamp in OBSERVED_TIMES
        .into_iter()
        .chain([0, -1, i64::MIN, i64::MAX])
    {
        let mut builder = builder();
        let item = builder.object(
            (1, 0),
            vec![(11, timestamp.to_be_bytes().to_vec()), (13, vec![0])],
        );
        let fixture = builder.finish(item);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let before = source.bytes_read();
        let decoded = proxy_http_229(
            &mut source,
            item,
            MAX_BYTE_BUDGET,
            &CancellationToken::new(),
        )
        .unwrap();
        let fields = value(&decoded.value);
        assert_eq!(fields.time.id, 11);
        assert_eq!(fields.time.state, Observation::Value(timestamp));
        assert_eq!(fields.highlight.state, Observation::Value(0));
        read_counts.push(source.bytes_read() - before);
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
    }
    assert!(
        read_counts
            .iter()
            .all(|count| *count == read_counts[0] && *count < 128)
    );
}

#[test]
fn missing_or_short_time_keeps_its_observation_and_does_not_hide_original_request() {
    for short in [false, true] {
        let mut builder = builder();
        let request = builder.bytes(b"GET /time HTTP/1.1\r\n\r\n");
        let mut fields = Vec::new();
        if short {
            fields.push((11, 1_i32.to_be_bytes().to_vec()));
        }
        fields.push((13, vec![0]));
        fields.push(reference(15, request));
        let item = builder.object((1, 0), fields);
        let fixture = builder.finish(item);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let decoded = proxy_http_229(
            &mut source,
            item,
            MAX_BYTE_BUDGET,
            &CancellationToken::new(),
        )
        .unwrap();
        let fields = value(&decoded.value);
        if short {
            assert!(
                matches!(&fields.time.state, Observation::Failed(error) if error.kind == io::ErrorKind::InvalidData && error.message.contains("overlaps"))
            );
        } else {
            assert_eq!(fields.time.state, Observation::Missing);
        }
        assert_eq!(value(&fields.request.state).address, request);
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
    }
}

#[test]
fn time_mapping_keeps_existing_metadata_budget_and_cancel_guard() {
    let mut builder = builder();
    let item = builder.object((1, 0), vec![(11, OBSERVED_TIMES[0].to_be_bytes().to_vec())]);
    let fixture = builder.finish(item);
    let mut source = fixture.open();
    let before = source.bytes_read();
    assert!(proxy_http_229(&mut source, item, 1, &CancellationToken::new()).is_err());
    assert_eq!(source.bytes_read(), before);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        proxy_http_229(&mut source, item, MAX_BYTE_BUDGET, &cancelled)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(source.bytes_read(), before);
}
