use super::*;

#[test]
fn retained_pool_inputs_are_optional_verbatim_and_only_validate_effective_controls() {
    let mut pool = IntruderResourcePool {
        id: Uuid::new_v4(),
        name: "inputs".into(),
        max_concurrent_requests: None,
        delay: None,
        automatic_throttling: IntruderAutoThrottle::default(),
        retained_inputs: None,
    };
    let legacy = serde_json::to_value(&pool).unwrap();
    assert!(legacy.get("retained_inputs").is_none());
    assert_eq!(
        serde_json::from_value::<IntruderResourcePool>(legacy).unwrap(),
        pool
    );
    pool.retained_inputs = Some(IntruderPoolInputs {
        cap: "disabled cap".into(),
        delay: "disabled delay".into(),
        increment: "未完成增量".into(),
        delay_mode: IntruderPoolDelayMode::Increment,
    });
    pool.validate().unwrap();
    let bytes = serde_json::to_vec(&pool).unwrap();
    assert_eq!(
        serde_json::from_slice::<IntruderResourcePool>(&bytes).unwrap(),
        pool
    );
    pool.max_concurrent_requests = Some(10);
    assert!(pool.validate().is_err());
    pool.retained_inputs.as_mut().unwrap().cap = "0010".into();
    pool.validate().unwrap();
    for (mode, policy) in [
        (
            IntruderPoolDelayMode::Fixed,
            IntruderPoolDelay::Fixed { milliseconds: 500 },
        ),
        (
            IntruderPoolDelayMode::Random,
            IntruderPoolDelay::Random { milliseconds: 500 },
        ),
        (
            IntruderPoolDelayMode::Increment,
            IntruderPoolDelay::Increment {
                initial_ms: 500,
                increment_ms: 7,
            },
        ),
    ] {
        pool.delay = Some(policy);
        let inputs = pool.retained_inputs.as_mut().unwrap();
        inputs.delay_mode = mode;
        inputs.delay = "00500".into();
        inputs.increment = if mode == IntruderPoolDelayMode::Increment {
            "0007"
        } else {
            "inactive text"
        }
        .into();
        pool.validate().unwrap();
        pool.retained_inputs.as_mut().unwrap().delay = "501".into();
        assert!(pool.validate().is_err());
        pool.retained_inputs.as_mut().unwrap().delay = "500".into();
        pool.retained_inputs.as_mut().unwrap().delay_mode = match mode {
            IntruderPoolDelayMode::Fixed => IntruderPoolDelayMode::Random,
            _ => IntruderPoolDelayMode::Fixed,
        };
        assert!(pool.validate().is_err());
    }
    pool.retained_inputs.as_mut().unwrap().delay_mode = IntruderPoolDelayMode::Increment;
    pool.retained_inputs.as_mut().unwrap().increment = "8".into();
    assert!(pool.validate().is_err());
    pool.max_concurrent_requests = None;
    pool.delay = None;
    for field in ["cap", "delay", "increment"] {
        let inputs = pool.retained_inputs.as_mut().unwrap();
        let value = match field {
            "cap" => &mut inputs.cap,
            "delay" => &mut inputs.delay,
            _ => &mut inputs.increment,
        };
        *value = "x".repeat(4096);
        pool.validate().unwrap();
        let mut too_long = pool.clone();
        let inputs = too_long.retained_inputs.as_mut().unwrap();
        match field {
            "cap" => &mut inputs.cap,
            "delay" => &mut inputs.delay,
            _ => &mut inputs.increment,
        }
        .push('x');
        assert!(too_long.validate().is_err());
    }
    let mut malformed = serde_json::to_value(&pool).unwrap();
    malformed["retained_inputs"]["future"] = serde_json::json!(true);
    assert!(serde_json::from_value::<IntruderResourcePool>(malformed).is_err());
    let mut malformed = serde_json::to_value(&pool).unwrap();
    malformed["retained_inputs"]["delay_mode"] = serde_json::json!("Future");
    assert!(serde_json::from_value::<IntruderResourcePool>(malformed).is_err());
}

#[test]
fn resource_pool_catalog_keeps_identity_and_strict_known_settings() {
    let pool = IntruderResourcePool {
        retained_inputs: None,
        automatic_throttling: IntruderAutoThrottle::default(),
        delay: None,
        id: Uuid::new_v4(),
        name: "共享请求".into(),
        max_concurrent_requests: Some(2),
    };
    let catalog = IntruderResourcePools {
        revision: 1,
        pools: vec![pool.clone()],
        default_pool_id: None,
    };
    catalog.validate().unwrap();
    assert_eq!(catalog.find(pool.id), Some(&pool));
    let mut json = serde_json::to_value(&catalog).unwrap();
    assert_eq!(
        serde_json::from_value::<IntruderResourcePools>(json.clone()).unwrap(),
        catalog
    );
    json["pools"][0]["future_throttle"] = serde_json::json!(true);
    assert!(serde_json::from_value::<IntruderResourcePools>(json).is_err());
    let mut duplicated = catalog.clone();
    duplicated.pools.push(pool.clone());
    assert!(duplicated.validate().is_err());
    duplicated.pools[1].id = Uuid::new_v4();
    duplicated.validate().unwrap();
    for limit in [None, Some(1), Some(16)] {
        IntruderResourcePool {
            max_concurrent_requests: limit,
            ..pool.clone()
        }
        .validate()
        .unwrap();
    }
    for limit in [0, 17, u16::MAX] {
        assert!(
            IntruderResourcePool {
                max_concurrent_requests: Some(limit),
                ..pool.clone()
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn default_pool_identity_is_explicit_and_legacy_catalogs_stay_unassigned() {
    let pool = IntruderResourcePool {
        retained_inputs: None,
        id: Uuid::new_v4(),
        name: "默认资源池".into(),
        max_concurrent_requests: Some(10),
        delay: None,
        automatic_throttling: IntruderAutoThrottle {
            enabled: true,
            ..IntruderAutoThrottle::default()
        },
    };
    let mut catalog = IntruderResourcePools {
        revision: 1,
        default_pool_id: Some(pool.id),
        pools: vec![pool],
    };
    catalog.validate().unwrap();
    let mut legacy = serde_json::to_value(&catalog).unwrap();
    legacy.as_object_mut().unwrap().remove("default_pool_id");
    let legacy: IntruderResourcePools = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.default_pool_id, None);
    assert_eq!(legacy.pools, catalog.pools);
    catalog.pools[0].name = "改名后仍是默认池".into();
    catalog.validate().unwrap();
    for id in [Uuid::nil(), Uuid::new_v4()] {
        catalog.default_pool_id = Some(id);
        assert!(catalog.validate().is_err());
    }
}

#[test]
fn resource_pool_delay_is_explicit_bounded_and_absent_in_legacy_json() {
    let mut pool = IntruderResourcePool {
        retained_inputs: None,
        id: Uuid::new_v4(),
        name: "间隔".into(),
        max_concurrent_requests: None,
        delay: None,
        automatic_throttling: IntruderAutoThrottle::default(),
    };
    let mut legacy = serde_json::to_value(&pool).unwrap();
    legacy.as_object_mut().unwrap().remove("delay");
    assert_eq!(
        serde_json::from_value::<IntruderResourcePool>(legacy).unwrap(),
        pool
    );
    for delay in [
        IntruderPoolDelay::Fixed { milliseconds: 0 },
        IntruderPoolDelay::Random { milliseconds: 0 },
        IntruderPoolDelay::Random {
            milliseconds: 300_000,
        },
        IntruderPoolDelay::Fixed {
            milliseconds: 300_000,
        },
        IntruderPoolDelay::Increment {
            initial_ms: 0,
            increment_ms: 1,
        },
        IntruderPoolDelay::Increment {
            initial_ms: 300_000,
            increment_ms: 300_000,
        },
    ] {
        pool.delay = Some(delay);
        pool.validate().unwrap();
        assert_eq!(
            serde_json::from_slice::<IntruderResourcePool>(&serde_json::to_vec(&pool).unwrap())
                .unwrap(),
            pool
        );
    }
    for delay in [
        IntruderPoolDelay::Random {
            milliseconds: 300_001,
        },
        IntruderPoolDelay::Fixed {
            milliseconds: 300_001,
        },
        IntruderPoolDelay::Increment {
            initial_ms: 0,
            increment_ms: 0,
        },
        IntruderPoolDelay::Increment {
            initial_ms: 1,
            increment_ms: u64::MAX,
        },
    ] {
        pool.delay = Some(delay);
        assert!(pool.validate().is_err());
    }
    for invalid in [
        serde_json::json!({"mode":"Fixed","milliseconds":"100"}),
        serde_json::json!({"mode":"Fixed","milliseconds":100,"future":1}),
        serde_json::json!({"mode":"FutureDelay","milliseconds":100}),
    ] {
        assert!(serde_json::from_value::<IntruderPoolDelay>(invalid).is_err());
    }
}

#[test]
fn automatic_throttling_keeps_explicit_switches_and_validates_active_csv_without_normalizing() {
    let mut config = IntruderAutoThrottle::default();
    assert!(config.response_codes().unwrap().is_empty());
    config.enabled = true;
    assert_eq!(config.response_codes().unwrap(), [429]);
    config.status_503 = true;
    config.other_enabled = true;
    config.other_codes = " 504, 505,504,200, 429 ".into();
    assert_eq!(config.response_codes().unwrap(), [200, 429, 503, 504, 505]);
    let bytes = serde_json::to_vec(&config).unwrap();
    assert_eq!(
        serde_json::from_slice::<IntruderAutoThrottle>(&bytes).unwrap(),
        config
    );
    assert_eq!(config.other_codes, " 504, 505,504,200, 429 ");
    for invalid in ["", "oops", "99", "600", "4xx", "+200", "200,", "200,,503"] {
        config.other_codes = invalid.into();
        assert!(config.validate().is_err(), "{invalid}");
    }
    config.other_enabled = false;
    config.validate().unwrap();
    config.other_enabled = true;
    config.enabled = false;
    config.validate().unwrap();
    config.other_codes = "x".repeat(4097);
    assert!(config.validate().is_err());
    let mut unknown = serde_json::to_value(IntruderAutoThrottle::default()).unwrap();
    unknown["future_behavior"] = serde_json::json!(true);
    assert!(serde_json::from_value::<IntruderAutoThrottle>(unknown).is_err());
}
