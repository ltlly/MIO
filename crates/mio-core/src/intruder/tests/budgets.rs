use super::*;

#[test]
fn empty_values_still_consume_the_payload_count_budget() {
    let limit = AttackPlan::new(
        AttackType::BatteringRam,
        Vec::new(),
        vec![position(0, 0)],
        vec![vec![Vec::new(); MAX_PAYLOADS]],
    )
    .unwrap();
    assert_eq!(limit.request_count(), MAX_PAYLOADS as u64);
    assert!(limit.request(MAX_PAYLOADS as u64 - 1).unwrap().is_empty());
    let over = AttackPlan::new(
        AttackType::BatteringRam,
        Vec::new(),
        vec![position(0, 0)],
        vec![vec![Vec::new(); MAX_PAYLOADS + 1]],
    );
    assert!(over.err().unwrap().to_string().contains("payload 总数量"));
}

#[test]
fn payload_byte_budget_rejects_the_first_byte_over_the_limit() {
    let mut payloads = vec![vec![vec![0; MAX_PAYLOAD_BYTES]]];
    assert!(validate_payloads(&payloads).is_ok());
    payloads[0].push(vec![0]);
    assert!(
        validate_payloads(&payloads)
            .unwrap_err()
            .to_string()
            .contains("payload 总字节数")
    );
}

#[test]
fn cluster_count_overflow_and_total_budget_fail_without_materializing_combinations() {
    let overflow = AttackPlan::new(
        AttackType::ClusterBomb,
        Vec::new(),
        vec![position(0, 0); 20],
        vec![vec![Vec::new(); 10]; 20],
    );
    assert!(overflow.err().unwrap().to_string().contains("生成数量溢出"));
    let budget = AttackPlan::new(
        AttackType::ClusterBomb,
        Vec::new(),
        vec![position(0, 0); 7],
        vec![vec![Vec::new(); 10]; 7],
    );
    assert!(budget.err().unwrap().to_string().contains("1000000"));
    let mut empty = vec![vec![Vec::new(); 100]; 20];
    empty[19].clear();
    let plan = AttackPlan::new(
        AttackType::ClusterBomb,
        Vec::new(),
        vec![position(0, 0); 20],
        empty,
    )
    .unwrap();
    assert_eq!(plan.request_count(), 0);
}

#[test]
fn a_million_combinations_support_direct_last_index_and_repeated_reads() {
    let plan = AttackPlan::new(
        AttackType::ClusterBomb,
        Vec::new(),
        vec![position(0, 0); 6],
        vec![(b'0'..=b'9').map(|byte| vec![byte]).collect(); 6],
    )
    .unwrap();
    assert_eq!(plan.request_count(), MAX_REQUESTS);
    assert_eq!(plan.request(999_999).unwrap(), b"999999");
    assert_eq!(plan.request(123_456).unwrap(), b"123456");
    assert_eq!(plan.request(0).unwrap(), b"000000");
    assert_eq!(plan.request(999_999).unwrap(), b"999999");
    assert!(plan.request(1_000_000).is_err());
    assert!(plan.request(u64::MAX).is_err());
    assert_eq!(plan.payload_sets.iter().map(Vec::len).sum::<usize>(), 60);
}

#[test]
fn checked_expanded_lengths_reject_overflow_and_do_not_saturate_or_truncate() {
    assert_eq!(expanded_len(0, 0, 0).unwrap(), 0);
    assert_eq!(
        expanded_len(MAX_REQUEST_BYTES, 0, 0).unwrap(),
        MAX_REQUEST_BYTES
    );
    assert_eq!(
        expanded_len(MAX_REQUEST_BYTES, 1, 1).unwrap(),
        MAX_REQUEST_BYTES
    );
    assert!(expanded_len(MAX_REQUEST_BYTES, 0, 1).is_err());
    assert!(expanded_len(MAX_REQUEST_BYTES + 1, 0, 0).is_err());
    assert!(expanded_len(0, 1, 0).is_err());
    assert!(expanded_len(usize::MAX, 0, 1).is_err());
    assert!(expanded_len(1, 0, usize::MAX).is_err());
}

#[test]
fn maximum_request_lengths_match_only_reachable_combinations() {
    for (kind, payloads, expected) in [
        (AttackType::Sniper, sets(&[&[b"", b"123456"]]), 11),
        (AttackType::BatteringRam, sets(&[&[b"", b"123456"]]), 13),
        (
            AttackType::Pitchfork,
            sets(&[
                &[b"123456", b""],
                &[b"", b"abcdefgh", b"unused-longer-value"],
            ]),
            9,
        ),
        (
            AttackType::ClusterBomb,
            sets(&[&[b"123456", b""], &[b"", b"abcdefgh"]]),
            15,
        ),
    ] {
        let plan = AttackPlan::new(
            kind,
            b"aa/bbbb".to_vec(),
            vec![position(0, 2), position(3, 7)],
            payloads,
        )
        .unwrap();
        assert_eq!(plan.maximum_request_len(6).unwrap(), expected, "{kind:?}");
        assert_eq!(requests(&plan).iter().map(Vec::len).max(), Some(expected));
    }
    let empty = AttackPlan::new(
        AttackType::Sniper,
        b"template".to_vec(),
        vec![position(0, 0)],
        vec![Vec::new()],
    )
    .unwrap();
    assert_eq!(empty.maximum_request_len(0).unwrap(), 0);
}

#[test]
fn sniper_request_budget_is_checked_before_generating_any_requests() {
    let limit = AttackPlan::new(
        AttackType::Sniper,
        Vec::new(),
        vec![position(0, 0); 100],
        vec![vec![Vec::new(); 10_000]],
    )
    .unwrap();
    assert_eq!(limit.request_count(), MAX_REQUESTS);
    assert!(limit.request(MAX_REQUESTS - 1).unwrap().is_empty());
    let over = AttackPlan::new(
        AttackType::Sniper,
        Vec::new(),
        vec![position(0, 0); 100],
        vec![vec![Vec::new(); 10_001]],
    );
    assert!(over.err().unwrap().to_string().contains("1000000"));
}

#[test]
fn expanded_request_limit_is_rejected_during_plan_construction() {
    // Repetition reaches the boundary using a 65 KiB payload, without allocating the output.
    assert_eq!(MAX_REQUEST_BYTES % MAX_POSITIONS, 0);
    let payload_length = MAX_REQUEST_BYTES / MAX_POSITIONS;
    let limit = AttackPlan::new(
        AttackType::BatteringRam,
        Vec::new(),
        vec![position(0, 0); MAX_POSITIONS],
        vec![vec![vec![0; payload_length]]],
    )
    .unwrap();
    assert_eq!(limit.maximum_request_len(0).unwrap(), MAX_REQUEST_BYTES);
    let over = AttackPlan::new(
        AttackType::BatteringRam,
        Vec::new(),
        vec![position(0, 0); MAX_POSITIONS],
        vec![vec![vec![0; payload_length + 1]]],
    );
    assert!(over.err().unwrap().to_string().contains("单条请求"));
}
#[test]
fn baseline_mapping_preserves_original_bytes_and_existing_payload_ordinals() {
    let template = "x=base&literal=§".as_bytes().to_vec();
    let plan = super::AttackPlan::new(
        super::AttackType::Sniper,
        template.clone(),
        vec![super::Position { start: 2, end: 6 }],
        vec![vec![vec![0, 255], b"B".to_vec()]],
    )
    .unwrap();
    assert_eq!(plan.request_count(), 2);
    for slot in 0..=2 {
        assert_eq!(
            plan.request_with_baseline(slot, Some(slot)).unwrap(),
            template
        );
        for payload in 0..2 {
            let ordinal = payload + u64::from(payload >= slot);
            assert_eq!(
                plan.request_with_baseline(ordinal, Some(slot)).unwrap(),
                plan.request(payload).unwrap()
            );
        }
    }
    assert_eq!(
        plan.request_with_baseline(1, None).unwrap(),
        plan.request(1).unwrap()
    );
    assert!(plan.request_with_baseline(3, Some(0)).is_err());
    assert!(plan.request_with_baseline(0, Some(3)).is_err());
}

#[test]
fn payload_positions_identify_frozen_inputs_used_by_every_attack_type() {
    let tuples = |positions: Vec<PayloadPosition>| {
        positions
            .into_iter()
            .map(|selected| {
                (
                    selected.position,
                    selected.set_index,
                    selected.payload_index,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        tuples(payload_positions(AttackType::Sniper, 3, &[2], 4, None).unwrap()),
        [(2, 0, 0)]
    );
    assert_eq!(
        tuples(payload_positions(AttackType::BatteringRam, 3, &[2], 1, None).unwrap()),
        [(0, 0, 1), (1, 0, 1), (2, 0, 1)]
    );
    assert_eq!(
        tuples(payload_positions(AttackType::Pitchfork, 3, &[3, 2, 4], 1, None).unwrap()),
        [(0, 0, 1), (1, 1, 1), (2, 2, 1)]
    );
    assert_eq!(
        tuples(payload_positions(AttackType::ClusterBomb, 3, &[2, 2, 3], 7, None).unwrap()),
        [(0, 0, 1), (1, 1, 0), (2, 2, 1)]
    );
    let plan = AttackPlan::new(
        AttackType::ClusterBomb,
        b"<a>/<b>/<c>".to_vec(),
        vec![position(1, 2), position(5, 6), position(9, 10)],
        sets(&[&[b"A", b""], &[b"\0\xff", b"2"], &[b"X", b"Y", b"Z"]]),
    )
    .unwrap();
    let selection = plan.payload_positions(7, None).unwrap();
    assert_eq!(
        selection
            .iter()
            .map(|selected| {
                plan.payload_sets[selected.set_index][selected.payload_index].as_slice()
            })
            .collect::<Vec<_>>(),
        [b"".as_slice(), b"\0\xff".as_slice(), b"Y".as_slice()]
    );
    assert_eq!(plan.request(7).unwrap(), b"<>/<\0\xff>/<Y>");
}

#[test]
fn payload_positions_preserve_late_baseline_and_direct_large_index_without_data() {
    for baseline in 0..=6 {
        assert!(
            payload_positions(AttackType::Sniper, 3, &[2], baseline, Some(baseline))
                .unwrap()
                .is_empty()
        );
        for index in 0..6 {
            let ordinal = index + u64::from(index >= baseline);
            assert_eq!(
                payload_positions(AttackType::Sniper, 3, &[2], ordinal, Some(baseline)).unwrap(),
                payload_positions(AttackType::Sniper, 3, &[2], index, None).unwrap()
            );
        }
    }
    assert!(
        payload_positions(AttackType::Sniper, 1, &[0], 0, Some(0))
            .unwrap()
            .is_empty()
    );
    let positions = payload_positions(AttackType::ClusterBomb, 6, &[10; 6], 999_999, None).unwrap();
    assert_eq!(positions.len(), 6);
    assert!(positions.iter().enumerate().all(|(index, selected)| {
        (
            selected.position,
            selected.set_index,
            selected.payload_index,
        ) == (index, index, 9)
    }));
    assert!(payload_positions(AttackType::ClusterBomb, 6, &[10; 6], 0, Some(0)).is_err());
}

#[test]
fn payload_positions_reject_invalid_shape_counts_and_result_ordinals() {
    for (kind, count, lengths, ordinal, baseline) in [
        (AttackType::Sniper, 0, vec![1], 0, None),
        (AttackType::Sniper, MAX_POSITIONS + 1, vec![1], 0, None),
        (AttackType::Sniper, 1, vec![], 0, None),
        (AttackType::BatteringRam, 2, vec![1, 1], 0, None),
        (AttackType::Pitchfork, 2, vec![1], 0, None),
        (AttackType::Pitchfork, 2, vec![3, 2], 2, None),
        (
            AttackType::ClusterBomb,
            MAX_PAYLOAD_SETS + 1,
            vec![1; MAX_PAYLOAD_SETS + 1],
            0,
            None,
        ),
        (AttackType::Sniper, 1, vec![MAX_PAYLOADS + 1], 0, None),
        (AttackType::ClusterBomb, 2, vec![usize::MAX, 1], 0, None),
        (AttackType::ClusterBomb, 7, vec![10; 7], 0, None),
        (AttackType::Sniper, 1, vec![0], 0, None),
        (AttackType::Sniper, 1, vec![2], 2, None),
        (AttackType::Sniper, 1, vec![2], u64::MAX, None),
        (AttackType::Sniper, 1, vec![2], 0, Some(3)),
        (AttackType::Sniper, 1, vec![2], 3, Some(0)),
    ] {
        assert!(
            payload_positions(kind, count, &lengths, ordinal, baseline).is_err(),
            "{kind:?} {count} {lengths:?} {ordinal} {baseline:?}"
        );
    }
}
