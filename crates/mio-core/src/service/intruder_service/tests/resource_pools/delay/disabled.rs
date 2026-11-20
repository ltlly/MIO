use super::*;

#[test]
fn retained_disabled_text_does_not_change_pool_policy_or_cached_timing() {
    let (mut state, _events) = state();
    let mut inactive = pool("inactive", None);
    inactive.retained_inputs = Some(IntruderPoolInputs {
        cap: "10".into(),
        delay: "500".into(),
        increment: String::new(),
        delay_mode: IntruderPoolDelayMode::Fixed,
    });
    let mut random = pool("random", None);
    random.delay = Some(IntruderPoolDelay::Random { milliseconds: 100 });
    random.retained_inputs = Some(IntruderPoolInputs {
        cap: String::new(),
        delay: "00100".into(),
        increment: "unused".into(),
        delay_mode: IntruderPoolDelayMode::Random,
    });
    install(&mut state, vec![inactive.clone(), random.clone()]);
    let sent = PoolInstant::now();
    let inactive_clock = PoolTiming {
        last_send: Some(sent),
        sent_count: 3,
        not_before: None,
    };
    let random_clock = PoolTiming {
        last_send: Some(sent),
        sent_count: 1,
        not_before: Some(sent + Duration::from_millis(100) + Duration::from_nanos(1)),
    };
    state
        .intruder
        .pool_timings
        .insert(inactive.id, inactive_clock);
    state.intruder.pool_timings.insert(random.id, random_clock);
    let inputs = inactive.retained_inputs.as_mut().unwrap();
    inputs.cap = "inactive non-number".into();
    inputs.delay = "retained off delay".into();
    inputs.increment = "incomplete increment".into();
    inputs.delay_mode = IntruderPoolDelayMode::Increment;
    let inputs = random.retained_inputs.as_mut().unwrap();
    inputs.cap = "disabled cap".into();
    inputs.increment = "new inactive increment".into();
    install(&mut state, vec![inactive.clone(), random.clone()]);
    assert_eq!(state.intruder.pool_timings[&inactive.id], inactive_clock);
    assert_eq!(state.intruder.pool_timings[&random.id], random_clock);
    assert_eq!(
        state
            .intruder
            .resource_pools
            .find(inactive.id)
            .unwrap()
            .delay,
        None
    );
    assert_eq!(
        state
            .intruder
            .resource_pools
            .find(inactive.id)
            .unwrap()
            .max_concurrent_requests,
        None
    );
    assert_eq!(
        state.intruder.resource_pools.find(random.id).unwrap().delay,
        random.delay
    );
    assert!(state.intruder.requests.is_empty() && state.intruder.jobs.is_empty());
}
