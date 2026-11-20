use super::*;

#[test]
fn sniper_replaces_each_member_once_and_keeps_unassigned_positions() {
    let plan = AttackPlan::new(
        AttackType::Sniper,
        b"left=aa&right=bbb".to_vec(),
        vec![position(5, 7), position(14, 17)],
        sets(&[&[b"x", b"\0\xff"]]),
    )
    .unwrap();
    // Position-major is the documented MIO policy, not a native External order assertion.
    assert_eq!(plan.request_count(), 4);
    assert_eq!(
        requests(&plan),
        [
            b"left=x&right=bbb".to_vec(),
            b"left=\0\xff&right=bbb".to_vec(),
            b"left=aa&right=x".to_vec(),
            b"left=aa&right=\0\xff".to_vec(),
        ]
    );
    assert_eq!(plan.template, b"left=aa&right=bbb");
    assert_eq!(plan.positions, [position(5, 7), position(14, 17)]);
    assert_eq!(plan.payload_sets, sets(&[&[b"x", b"\0\xff"]]));
}

#[test]
fn battering_ram_uses_the_same_binary_payload_at_all_positions() {
    let plan = AttackPlan::new(
        AttackType::BatteringRam,
        b"[a]:[bb]".to_vec(),
        vec![position(1, 2), position(5, 7)],
        sets(&[&[b"\0\xfe", b""]]),
    )
    .unwrap();
    assert_eq!(plan.request_count(), 2);
    assert_eq!(
        requests(&plan),
        [b"[\0\xfe]:[\0\xfe]".to_vec(), b"[]:[]".to_vec()]
    );
}

#[test]
fn pitchfork_uses_aligned_rows_and_stops_at_the_shortest_set() {
    let plan = AttackPlan::new(
        AttackType::Pitchfork,
        b"a/b/c".to_vec(),
        vec![position(0, 1), position(2, 3), position(4, 5)],
        sets(&[
            &[b"A", b"B", b"unused"],
            &[b"1", b"2"],
            &[b"X", b"", b"tail"],
        ]),
    )
    .unwrap();
    assert_eq!(plan.request_count(), 2);
    assert_eq!(requests(&plan), [b"A/1/X".to_vec(), b"B/2/".to_vec()]);
}

#[test]
fn cluster_bomb_two_set_prefix_follows_the_official_example() {
    let plan = AttackPlan::new(
        AttackType::ClusterBomb,
        b"a/b".to_vec(),
        vec![position(0, 1), position(2, 3)],
        sets(&[&[b"A", b"B"], &[b"1", b"2", b"3"]]),
    )
    .unwrap();
    assert_eq!(plan.request_count(), 6);
    assert_eq!(
        requests(&plan),
        [b"A/1", b"A/2", b"A/3", b"B/1", b"B/2", b"B/3"].map(|value| value.to_vec())
    );
}

#[test]
fn cluster_bomb_mio_mixed_radix_policy_carries_from_the_last_set() {
    let plan = AttackPlan::new(
        AttackType::ClusterBomb,
        b"a/b/c".to_vec(),
        vec![position(0, 1), position(2, 3), position(4, 5)],
        sets(&[&[b"A", b"B"], &[b"1", b"2"], &[b"X", b"Y", b"Z"]]),
    )
    .unwrap();
    assert_eq!(plan.request_count(), 12);
    assert_eq!(plan.request(11).unwrap(), b"B/2/Z");
    assert_eq!(plan.request(3).unwrap(), b"A/2/X");
    assert_eq!(plan.request(6).unwrap(), b"B/1/X");
    assert_eq!(plan.request(0).unwrap(), b"A/1/X");
    assert_eq!(plan.request(3).unwrap(), b"A/2/X");
}

#[test]
fn adjacent_and_zero_length_ranges_use_original_byte_offsets() {
    let plan = AttackPlan::new(
        AttackType::Pitchfork,
        b"AB".to_vec(),
        vec![
            position(0, 0),
            position(0, 1),
            position(1, 1),
            position(1, 2),
            position(2, 2),
        ],
        sets(&[&[b"<"], &[b"123"], &[b"|"], &[b""], &[b">"]]),
    )
    .unwrap();
    assert_eq!(plan.request(0).unwrap(), b"<123|>");
    let same_offset = AttackPlan::new(
        AttackType::Pitchfork,
        Vec::new(),
        vec![position(0, 0), position(0, 0)],
        sets(&[&[b"\xff"], &[b"\0"]]),
    )
    .unwrap();
    assert_eq!(same_offset.request(0).unwrap(), b"\xff\0");
}

#[test]
fn untouched_request_bytes_headers_and_literal_markers_are_not_normalized() {
    let raw = b"POST /literal/\xc2\xa7 HTTP/1.1\nHost: example.invalid\nX-A: one\nX-A:\ttwo\nContent-Length: 1\n\n\0";
    let start = raw.len() - 1;
    let plan = AttackPlan::new(
        AttackType::Sniper,
        raw.to_vec(),
        vec![position(start, start + 1)],
        sets(&[&[b"\xff\0\n"]]),
    )
    .unwrap();
    let generated = plan.request(0).unwrap();
    assert_eq!(&generated[..start], &raw[..start]);
    assert_eq!(&generated[start..], b"\xff\0\n");
    assert_eq!(plan.request_count(), 1, "no automatic baseline request");
}

#[test]
fn empty_sets_mean_no_requests_but_empty_values_and_duplicates_are_retained() {
    for kind in [
        AttackType::Sniper,
        AttackType::BatteringRam,
        AttackType::Pitchfork,
        AttackType::ClusterBomb,
    ] {
        let plan =
            AttackPlan::new(kind, b"a".to_vec(), vec![position(0, 1)], vec![Vec::new()]).unwrap();
        assert_eq!(plan.request_count(), 0);
        assert!(plan.request(0).is_err());
        let empty = AttackPlan::new(
            kind,
            b"a".to_vec(),
            vec![position(0, 1)],
            sets(&[&[b"", b""]]),
        )
        .unwrap();
        assert_eq!(empty.request_count(), 2);
        assert_eq!(requests(&empty), [Vec::<u8>::new(), Vec::new()]);
    }
    for kind in [AttackType::Pitchfork, AttackType::ClusterBomb] {
        let empty_factor = AttackPlan::new(
            kind,
            b"ab".to_vec(),
            vec![position(0, 1), position(1, 2)],
            vec![vec![b"x".to_vec()], Vec::new()],
        )
        .unwrap();
        assert_eq!(empty_factor.request_count(), 0);
    }
}

#[test]
fn range_validation_rejects_unsorted_overlap_reversed_and_out_of_bounds_inputs() {
    for invalid in [
        vec![],
        vec![position(1, 0)],
        vec![position(0, 4)],
        vec![position(4, 4)],
        vec![position(usize::MAX, usize::MAX)],
        vec![position(1, 2), position(0, 1)],
        vec![position(0, 2), position(1, 3)],
        vec![position(0, 2), position(1, 1)],
        vec![position(0, 1), position(0, 1)],
        vec![position(0, 0); MAX_POSITIONS + 1],
    ] {
        assert!(
            AttackPlan::new(
                AttackType::Sniper,
                b"abc".to_vec(),
                invalid,
                sets(&[&[b"x"]])
            )
            .is_err()
        );
    }
    assert!(
        AttackPlan::new(
            AttackType::Sniper,
            Vec::new(),
            vec![position(0, 0)],
            sets(&[&[b"x"]])
        )
        .is_ok()
    );
}

#[test]
fn set_count_is_exact_and_the_twenty_set_limit_is_not_a_position_limit() {
    for kind in [AttackType::Sniper, AttackType::BatteringRam] {
        for values in [vec![], sets(&[&[b"a"], &[b"b"]])] {
            assert!(
                AttackPlan::new(
                    kind,
                    b"xx".to_vec(),
                    vec![position(0, 1), position(1, 2)],
                    values
                )
                .is_err()
            );
        }
        let plan = AttackPlan::new(
            kind,
            Vec::new(),
            vec![position(0, 0); MAX_POSITIONS],
            sets(&[&[b"x"]]),
        )
        .unwrap();
        assert_eq!(
            plan.request_count(),
            if kind == AttackType::Sniper {
                MAX_POSITIONS as u64
            } else {
                1
            }
        );
    }
    for kind in [AttackType::Pitchfork, AttackType::ClusterBomb] {
        for values in [vec![], sets(&[&[b"a"]]), sets(&[&[b"a"], &[b"b"], &[b"c"]])] {
            assert!(
                AttackPlan::new(
                    kind,
                    b"xx".to_vec(),
                    vec![position(0, 1), position(1, 2)],
                    values
                )
                .is_err()
            );
        }
        let limit = AttackPlan::new(
            kind,
            Vec::new(),
            vec![position(0, 0); MAX_PAYLOAD_SETS],
            vec![vec![Vec::new()]; MAX_PAYLOAD_SETS],
        )
        .unwrap();
        assert_eq!(limit.request_count(), 1);
        let too_many = AttackPlan::new(
            kind,
            Vec::new(),
            vec![position(0, 0); MAX_PAYLOAD_SETS + 1],
            vec![vec![Vec::new()]; MAX_PAYLOAD_SETS + 1],
        );
        assert!(too_many.err().unwrap().to_string().contains("20"));
    }
}
