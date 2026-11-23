use anyhow::{Context, Result, ensure};

use crate::intruder::{validate_input_limits, validate_inputs};

use super::*;

impl IntruderSpec {
    /// Storage accepts unfinished targets, positions and settings without discarding input.
    pub fn validate_draft(&self) -> Result<()> {
        ensure!(
            (1..=i64::MAX as u64).contains(&self.revision)
                && (1..=i64::MAX as u64).contains(&self.draft.revision),
            "Intruder 配置或草稿 revision 无效"
        );
        ensure!(
            self.draft.target.len() <= 64 * 1024,
            "Intruder 连接目标过长"
        );
        if let Some(super::IntruderPoolSelection::Create(pool)) = &self.resource_pool {
            ensure!(pool.name.len() <= 4096, "Intruder 资源池名称超过 4096 字节");
            ensure!(
                pool.automatic_throttling.other_codes.len() <= 4096,
                "Intruder 自动节流状态码列表超过 4096 字节"
            );
            if let Some(inputs) = &pool.retained_inputs {
                ensure!(
                    [&inputs.cap, &inputs.delay, &inputs.increment]
                        .iter()
                        .all(|value| value.len() <= 4096),
                    "Intruder 资源池保留输入单项超过 4096 字节"
                );
            }
        }
        validate_input_limits(&self.draft.raw, &self.positions, &self.payload_sets)
    }

    /// Returns the initial result-slot count, including baseline, without copying inputs.
    pub fn validate(&self) -> Result<u64> {
        self.validate_draft()?;
        if let Some(pool) = &self.resource_pool {
            pool.validate()?;
        }
        ensure!(
            (1..=crate::MAX_INTRUDER_CONCURRENT_REQUESTS).contains(&self.max_concurrent_requests),
            "Intruder 并发数必须为 1–256（MIO 资源限额）"
        );
        ensure!(
            (1..=300_000).contains(&self.request_timeout_ms),
            "Intruder 单请求超时必须为 1–300000 ms（MIO 资源限额）"
        );
        let target = url::Url::parse(&self.draft.target)
            .context("Intruder 连接目标必须为完整 HTTP/HTTPS URL")?;
        ensure!(
            matches!(target.scheme(), "http" | "https") && target.host_str().is_some(),
            "Intruder 连接目标必须使用 HTTP 或 HTTPS"
        );
        ensure!(
            target.username().is_empty() && target.password().is_none(),
            "Intruder 连接目标不能包含凭据"
        );
        ensure!(
            target.fragment().is_none(),
            "Intruder 连接目标不能包含 fragment"
        );
        let payloads = validate_inputs(
            self.attack_type,
            &self.draft.raw,
            &self.positions,
            &self.payload_sets,
        )?;
        let total = payloads
            .checked_add(u64::from(self.baseline_request))
            .context("Intruder 请求总数溢出")?;
        ensure!(
            total <= crate::intruder::MAX_REQUESTS,
            "Intruder 请求总数（含基准请求）超过资源上限"
        );
        Ok(total)
    }
}

impl IntruderRunState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Stopped | Self::Completed | Self::Interrupted | Self::Failed(_)
        )
    }
}

impl IntruderRunSummary {
    pub fn payload_progress(&self) -> Option<(u64, u64)> {
        if self.completed > self.dispatched || self.dispatched > self.total {
            return None;
        }
        if let Some(baseline) = self.baseline {
            if baseline.ordinal >= self.total
                || baseline.ordinal > self.dispatched
                || (baseline.completed && baseline.ordinal >= self.dispatched)
                || (!baseline.completed
                    && baseline.ordinal < self.dispatched
                    && self.completed == self.dispatched)
            {
                return None;
            }
            let completed = self.completed.checked_sub(u64::from(baseline.completed))?;
            let total = self.total.checked_sub(1)?;
            (completed <= total).then_some((completed, total))
        } else {
            Some((self.completed, self.total))
        }
    }

    pub fn request_number(&self, ordinal: u64) -> Option<u64> {
        self.payload_progress()?;
        if ordinal >= self.total {
            return None;
        }
        if self
            .baseline
            .is_some_and(|baseline| baseline.ordinal == ordinal)
        {
            Some(0)
        } else {
            ordinal.checked_add(1)?.checked_sub(u64::from(
                self.baseline
                    .is_some_and(|baseline| baseline.ordinal < ordinal),
            ))
        }
    }
}
