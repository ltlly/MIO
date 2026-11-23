use std::collections::BTreeSet;

use anyhow::{Result, ensure};

use super::*;

impl IntruderPoolInputs {
    fn validate(&self, pool: &IntruderResourcePool) -> Result<()> {
        // Match the existing name/CSV bound; inactive input remains text, not a policy.
        ensure!(
            [&self.cap, &self.delay, &self.increment]
                .iter()
                .all(|value| value.len() <= 4096),
            "Intruder 资源池保留输入单项超过 4096 字节"
        );
        if let Some(cap) = pool.max_concurrent_requests {
            ensure!(
                self.cap.parse::<u16>().ok() == Some(cap),
                "Intruder 资源池并发输入与生效值不一致"
            );
        }
        if let Some(delay) = pool.delay {
            let (mode, milliseconds, increment) = match delay {
                IntruderPoolDelay::Fixed { milliseconds } => {
                    (IntruderPoolDelayMode::Fixed, milliseconds, None)
                }
                IntruderPoolDelay::Random { milliseconds } => {
                    (IntruderPoolDelayMode::Random, milliseconds, None)
                }
                IntruderPoolDelay::Increment {
                    initial_ms,
                    increment_ms,
                } => (
                    IntruderPoolDelayMode::Increment,
                    initial_ms,
                    Some(increment_ms),
                ),
            };
            ensure!(
                self.delay_mode == mode && self.delay.parse::<u64>().ok() == Some(milliseconds),
                "Intruder 资源池间隔输入与生效策略不一致"
            );
            if let Some(increment) = increment {
                ensure!(
                    self.increment.parse::<u64>().ok() == Some(increment),
                    "Intruder 资源池增量输入与生效值不一致"
                );
            }
        }
        Ok(())
    }
}

impl Default for IntruderAutoThrottle {
    fn default() -> Self {
        Self {
            enabled: false,
            status_429: true,
            status_503: false,
            other_enabled: false,
            other_codes: String::new(),
        }
    }
}

impl IntruderAutoThrottle {
    pub fn response_codes(&self) -> Result<Vec<u16>> {
        ensure!(
            self.other_codes.len() <= 4096,
            "Intruder 自动节流状态码列表超过 4096 字节"
        );
        if !self.enabled {
            return Ok(Vec::new());
        }
        let mut codes = BTreeSet::new();
        if self.status_429 {
            codes.insert(429);
        }
        if self.status_503 {
            codes.insert(503);
        }
        if self.other_enabled {
            for item in self.other_codes.split(',') {
                let item = item.trim();
                ensure!(
                    item.len() == 3 && item.bytes().all(|byte| byte.is_ascii_digit()),
                    "Intruder 自动节流状态码应为逗号分隔的 100–599 数字"
                );
                let code: u16 = item.parse()?;
                ensure!(
                    (100..=599).contains(&code),
                    "Intruder 自动节流状态码必须为 100–599"
                );
                codes.insert(code);
            }
        }
        Ok(codes.into_iter().collect())
    }

    pub fn validate(&self) -> Result<()> {
        self.response_codes().map(|_| ())
    }
}

impl IntruderPoolDelay {
    pub fn validate(&self) -> Result<()> {
        let (initial, increment) = match *self {
            Self::Fixed { milliseconds } | Self::Random { milliseconds } => (milliseconds, None),
            Self::Increment {
                initial_ms,
                increment_ms,
            } => (initial_ms, Some(increment_ms)),
        };
        ensure!(
            initial <= 300_000,
            "Intruder 资源池初始间隔超过 300000 ms（MIO 资源限额）"
        );
        ensure!(
            increment.is_none_or(|step| (1..=300_000).contains(&step)),
            "Intruder 资源池间隔增量必须为 1–300000 ms（MIO 资源限额）"
        );
        Ok(())
    }
}

impl IntruderResourcePool {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.id.is_nil(), "Intruder 资源池 ID 无效");
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 4096,
            "Intruder 资源池名称不能为空或超过 4096 字节"
        );
        ensure!(
            self.max_concurrent_requests
                .is_none_or(|limit| (1..=crate::MAX_INTRUDER_CONCURRENT_REQUESTS).contains(&limit)),
            "Intruder 资源池并发上限必须为 1–256（MIO 资源限额）"
        );
        if let Some(delay) = self.delay {
            delay.validate()?;
        }
        self.automatic_throttling.validate()?;
        if let Some(inputs) = &self.retained_inputs {
            inputs.validate(self)?;
        }
        Ok(())
    }
}

impl IntruderResourcePools {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.revision <= i64::MAX as u64,
            "Intruder 资源池目录 revision 无效"
        );
        ensure!(
            self.pools.len() <= MAX_INTRUDER_RESOURCE_POOLS,
            "Intruder 资源池数量超过上限"
        );
        let mut ids = BTreeSet::new();
        for pool in &self.pools {
            pool.validate()?;
            ensure!(ids.insert(pool.id), "Intruder 资源池 ID 重复");
        }
        ensure!(
            self.default_pool_id.is_none_or(|id| ids.contains(&id)),
            "Intruder 默认资源池不存在"
        );
        Ok(())
    }

    pub fn find(&self, id: Uuid) -> Option<&IntruderResourcePool> {
        self.pools.iter().find(|pool| pool.id == id)
    }
}

impl IntruderPoolSelection {
    pub fn id(&self) -> Uuid {
        match self {
            Self::Existing(id) => *id,
            Self::Create(pool) => pool.id,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Existing(id) => ensure!(!id.is_nil(), "Intruder 资源池 ID 无效"),
            Self::Create(pool) => pool.validate()?,
        }
        Ok(())
    }
}
