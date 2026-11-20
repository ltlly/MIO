use super::*;

#[test]
fn delay_candidates_share_pool_history_and_keep_ready_pool_round_robin() {
    let (mut state, _events) = state();
    let mut first_pool = pool("first", None);
    first_pool.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 100 });
    let mut other_pool = pool("other", None);
    other_pool.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 200 });
    install(&mut state, vec![first_pool.clone(), other_pool.clone()]);
    let mut input = spec("http://127.0.0.1:1".into(), 1);
    input.resource_pool = Some(IntruderPoolSelection::Existing(first_pool.id));
    let first = admit(&mut state, input.clone());
    let second = admit(&mut state, input.clone());
    let mut other_input = input.clone();
    other_input.resource_pool = Some(IntruderPoolSelection::Existing(other_pool.id));
    let other = admit(&mut state, other_input);
    let now = PoolInstant::now();
    let first_timing = PoolTiming::default()
        .after_send(now, first_pool.delay)
        .unwrap();
    state
        .intruder
        .pool_timings
        .insert(first_pool.id, first_timing);
    assert_eq!(
        state.intruder.dispatch_candidate_at(now),
        Some((other, now)),
        "a ready independent pool does not wait behind the delayed pool"
    );
    state.intruder.pool_timings.insert(
        other_pool.id,
        PoolTiming::default()
            .after_send(now, other_pool.delay)
            .unwrap(),
    );
    state.intruder.last_dispatched = Some(first);
    for offset in [0, 99, 100] {
        assert_eq!(
            state
                .intruder
                .dispatch_candidate_at(now + Duration::from_millis(offset)),
            Some((second, now + Duration::from_millis(100)))
        );
    }
    assert_eq!(
        state.intruder.pool_timings[&first_pool.id], first_timing,
        "candidate queries never consume a send or shift its deadline"
    );
    first_pool.name = "renamed without resetting history".into();
    install(&mut state, vec![first_pool.clone(), other_pool.clone()]);
    assert_eq!(state.intruder.pool_timings[&first_pool.id], first_timing);
    let third = admit(&mut state, input);
    assert_eq!(
        state.intruder.pool_timings[&first_pool.id], first_timing,
        "another run Start shares the existing timing state"
    );
    rebind(&mut state, third, other_pool.id);
    assert_eq!(
        state
            .intruder
            .pool_ready_at(&state.intruder.runs[&third], now),
        now + Duration::from_millis(200)
    );
    first_pool.delay = Some(IntruderPoolDelay::Increment {
        initial_ms: 100,
        increment_ms: 100,
    });
    install(&mut state, vec![first_pool.clone(), other_pool.clone()]);
    assert_eq!(
        state.intruder.pool_timings[&first_pool.id].not_before,
        Some(now + Duration::from_millis(200))
    );
    first_pool.delay = None;
    install(&mut state, vec![first_pool.clone(), other_pool]);
    assert_eq!(
        (
            state.intruder.pool_timings[&first_pool.id].last_send,
            state.intruder.pool_timings[&first_pool.id].sent_count
        ),
        (Some(now), 1)
    );
    assert_eq!(
        state.intruder.dispatch_candidate_at(now),
        Some((second, now))
    );
    assert_eq!(
        state.intruder.pool_timings.len(),
        state.intruder.resource_pools.pools.len()
    );
}
