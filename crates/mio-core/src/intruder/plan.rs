use anyhow::{Context, Result, ensure};

use super::*;

/// Resolve a stored result ordinal without generating a request or loading payload bytes.
/// A baseline has no payload positions; an empty payload value still has an input index.
pub fn payload_positions(
    kind: AttackType,
    position_count: usize,
    set_lengths: &[usize],
    ordinal: u64,
    baseline_ordinal: Option<u64>,
) -> Result<Vec<PayloadPosition>> {
    let count = payload_request_count(kind, position_count, set_lengths)?;
    let index = match baseline_ordinal {
        Some(baseline) => {
            ensure!(
                baseline <= count && count < MAX_REQUESTS && ordinal <= count,
                "Intruder 基准位置或请求索引超出已验证的生成数量"
            );
            if ordinal == baseline {
                return Ok(Vec::new());
            }
            ordinal - u64::from(ordinal > baseline)
        }
        None => ordinal,
    };
    ensure!(index < count, "Intruder 请求索引超出已验证的生成数量");
    let mut index = usize::try_from(index).context("Intruder 请求索引超出本机范围")?;
    match kind {
        AttackType::Sniper => Ok(vec![PayloadPosition {
            position: index / set_lengths[0],
            set_index: 0,
            payload_index: index % set_lengths[0],
        }]),
        AttackType::BatteringRam => Ok((0..position_count)
            .map(|position| PayloadPosition {
                position,
                set_index: 0,
                payload_index: index,
            })
            .collect()),
        AttackType::Pitchfork => Ok((0..position_count)
            .map(|position| PayloadPosition {
                position,
                set_index: position,
                payload_index: index,
            })
            .collect()),
        AttackType::ClusterBomb => {
            let mut selected = Vec::with_capacity(position_count);
            for (position, length) in set_lengths.iter().enumerate().rev() {
                selected.push(PayloadPosition {
                    position,
                    set_index: position,
                    payload_index: index % length,
                });
                index /= length;
            }
            selected.reverse();
            Ok(selected)
        }
    }
}

pub(crate) fn payload_request_count(
    kind: AttackType,
    position_count: usize,
    set_lengths: &[usize],
) -> Result<u64> {
    ensure!(
        (1..=MAX_POSITIONS).contains(&position_count),
        "Intruder 需要 1..={MAX_POSITIONS} 个明确位置（MIO 资源限额）"
    );
    let expected_sets = match kind {
        AttackType::Sniper | AttackType::BatteringRam => 1,
        AttackType::Pitchfork | AttackType::ClusterBomb => position_count,
    };
    ensure!(
        set_lengths.len() == expected_sets && set_lengths.len() <= MAX_PAYLOAD_SETS,
        "此 Intruder 类型需要 {expected_sets} 个 payload set，不能遗漏或忽略额外的 set"
    );
    ensure!(
        set_lengths
            .iter()
            .try_fold(0usize, |total, length| total.checked_add(*length))
            .is_some_and(|total| total <= MAX_PAYLOADS),
        "Intruder payload 总数量超过限额"
    );
    count_request_lengths(kind, position_count, set_lengths)
}

impl AttackPlan {
    /// Requires ordered, non-overlapping positions and exactly the sets used by the type.
    /// Empty payload sets produce no requests; an empty payload *value* is a replacement.
    pub fn new(
        kind: AttackType,
        template: Vec<u8>,
        positions: Vec<Position>,
        payload_sets: Vec<Vec<Vec<u8>>>,
    ) -> Result<Self> {
        let request_count = validate_inputs(kind, &template, &positions, &payload_sets)?;
        Ok(Self {
            kind,
            template,
            positions,
            payload_sets,
            request_count,
        })
    }

    /// Payload-generated requests only; an optional unmodified baseline is not added here.
    pub fn request_count(&self) -> u64 {
        self.request_count
    }

    /// Maps a stored result ordinal without renumbering payloads admitted before a late baseline.
    /// The baseline uses the original bytes; HTTP normalization remains the send layer's job.
    pub fn request_with_baseline(
        &self,
        ordinal: u64,
        baseline_ordinal: Option<u64>,
    ) -> Result<Vec<u8>> {
        let selected = self.payload_positions(ordinal, baseline_ordinal)?;
        if selected.is_empty() {
            let mut request = Vec::new();
            request
                .try_reserve_exact(self.template.len())
                .context("无法分配 Intruder 基准请求缓冲区")?;
            request.extend_from_slice(&self.template);
            return Ok(request);
        }
        self.request_from_positions(&selected)
    }

    pub fn payload_positions(
        &self,
        ordinal: u64,
        baseline_ordinal: Option<u64>,
    ) -> Result<Vec<PayloadPosition>> {
        let mut lengths = [0usize; MAX_PAYLOAD_SETS];
        for (length, set) in lengths.iter_mut().zip(&self.payload_sets) {
            *length = set.len();
        }
        payload_positions(
            self.kind,
            self.positions.len(),
            &lengths[..self.payload_sets.len()],
            ordinal,
            baseline_ordinal,
        )
    }

    /// Generates one zero-based index without visiting earlier combinations.
    /// Sniper: exhaust one position's set before advancing to the next position.
    /// Cluster bomb: the last set varies fastest, then carries toward the first set.
    /// These ordering contracts are MIO policies; see the module's evidence boundary.
    pub fn request(&self, index: u64) -> Result<Vec<u8>> {
        self.request_from_positions(&self.payload_positions(index, None)?)
    }

    fn request_from_positions(&self, positions: &[PayloadPosition]) -> Result<Vec<u8>> {
        let mut removed = 0_usize;
        let mut inserted = 0_usize;
        for selected in positions {
            let position = self.positions[selected.position];
            let payload = &self.payload_sets[selected.set_index][selected.payload_index];
            removed = removed
                .checked_add(position.end - position.start)
                .context("Intruder 替换位置总长度溢出")?;
            inserted = inserted
                .checked_add(payload.len())
                .context("Intruder 展开 payload 总长度溢出")?;
        }
        let length = expanded_len(self.template.len(), removed, inserted)?;
        let mut request = Vec::new();
        request
            .try_reserve_exact(length)
            .context("无法分配 Intruder 单条请求缓冲区")?;
        let mut cursor = 0;
        for selected in positions {
            let position = self.positions[selected.position];
            let payload = &self.payload_sets[selected.set_index][selected.payload_index];
            request.extend_from_slice(&self.template[cursor..position.start]);
            request.extend_from_slice(payload);
            cursor = position.end;
        }
        request.extend_from_slice(&self.template[cursor..]);
        debug_assert_eq!(request.len(), length);
        Ok(request)
    }

    #[cfg(test)]
    pub(super) fn maximum_request_len(&self, removed: usize) -> Result<usize> {
        maximum_request_len(
            self.kind,
            self.template.len(),
            &self.positions,
            &self.payload_sets,
            self.request_count,
            removed,
        )
    }
}

pub(crate) fn validate_input_limits(
    template: &[u8],
    positions: &[Position],
    payload_sets: &[Vec<Vec<u8>>],
) -> Result<()> {
    expanded_len(template.len(), 0, 0)?;
    ensure!(
        positions.len() <= MAX_POSITIONS,
        "Intruder 最多支持 {MAX_POSITIONS} 个位置（MIO 资源限额）"
    );
    ensure!(
        payload_sets.len() <= MAX_PAYLOAD_SETS,
        "Intruder 最多支持 {MAX_PAYLOAD_SETS} 个 payload set"
    );
    validate_payloads(payload_sets)
}

// Draft storage and admission share resource limits without copying large payload sets.
pub(crate) fn validate_inputs(
    kind: AttackType,
    template: &[u8],
    positions: &[Position],
    payload_sets: &[Vec<Vec<u8>>],
) -> Result<u64> {
    validate_input_limits(template, positions, payload_sets)?;
    let removed = validate_positions(template.len(), positions)?;
    let expected_sets = match kind {
        AttackType::Sniper | AttackType::BatteringRam => 1,
        AttackType::Pitchfork | AttackType::ClusterBomb => positions.len(),
    };
    ensure!(
        payload_sets.len() == expected_sets,
        "此 Intruder 类型需要 {expected_sets} 个 payload set，不能遗漏或忽略额外的 set"
    );
    let count = count_requests(kind, positions.len(), payload_sets)?;
    maximum_request_len(
        kind,
        template.len(),
        positions,
        payload_sets,
        count,
        removed,
    )?;
    Ok(count)
}

fn maximum_request_len(
    kind: AttackType,
    template_bytes: usize,
    positions: &[Position],
    payload_sets: &[Vec<Vec<u8>>],
    request_count: u64,
    removed: usize,
) -> Result<usize> {
    if request_count == 0 {
        return Ok(0);
    }
    let maximum = |set: &Vec<Vec<u8>>| set.iter().map(Vec::len).max().unwrap_or(0);
    match kind {
        AttackType::Sniper => {
            let shortest_position = positions
                .iter()
                .map(|position| position.end - position.start)
                .min()
                .context("Intruder 需要至少一个 payload 位置")?;
            expanded_len(template_bytes, shortest_position, maximum(&payload_sets[0]))
        }
        AttackType::BatteringRam => {
            let inserted = maximum(&payload_sets[0])
                .checked_mul(positions.len())
                .context("Intruder 展开 payload 总长度溢出")?;
            expanded_len(template_bytes, removed, inserted)
        }
        AttackType::Pitchfork => {
            // Only aligned rows are possible; maxima from different rows are not a request.
            let mut maximum = 0;
            for index in 0..usize::try_from(request_count)? {
                let inserted = payload_sets.iter().try_fold(0_usize, |sum, set| {
                    sum.checked_add(set[index].len())
                        .context("Intruder 展开 payload 总长度溢出")
                })?;
                maximum = maximum.max(expanded_len(template_bytes, removed, inserted)?);
            }
            Ok(maximum)
        }
        AttackType::ClusterBomb => {
            let inserted = payload_sets.iter().try_fold(0_usize, |sum, set| {
                sum.checked_add(maximum(set))
                    .context("Intruder 展开 payload 总长度溢出")
            })?;
            expanded_len(template_bytes, removed, inserted)
        }
    }
}

pub(crate) fn validate_positions(template_bytes: usize, positions: &[Position]) -> Result<usize> {
    ensure!(
        !positions.is_empty() && positions.len() <= MAX_POSITIONS,
        "Intruder 需要 1..={MAX_POSITIONS} 个明确位置（MIO 资源限额）"
    );
    let mut previous_end = 0;
    let mut removed = 0_usize;
    for (index, position) in positions.iter().enumerate() {
        ensure!(
            position.start <= position.end && position.end <= template_bytes,
            "Intruder 位置 {index} 超出原模板 byte range"
        );
        ensure!(
            position.start >= previous_end,
            "Intruder 位置必须按原模板排序且不能重叠"
        );
        removed = removed
            .checked_add(position.end - position.start)
            .context("Intruder 替换位置总长度溢出")?;
        previous_end = position.end;
    }
    Ok(removed)
}

pub(super) fn validate_payloads(payload_sets: &[Vec<Vec<u8>>]) -> Result<()> {
    let mut count = 0_usize;
    let mut bytes = 0_usize;
    for set in payload_sets {
        count = count
            .checked_add(set.len())
            .context("Intruder payload 数量溢出")?;
        ensure!(
            count <= MAX_PAYLOADS,
            "Intruder payload 总数量超过 {MAX_PAYLOADS}（MIO 资源限额，未截断）"
        );
        for payload in set {
            bytes = bytes
                .checked_add(payload.len())
                .context("Intruder payload 总字节数溢出")?;
            ensure!(
                bytes <= MAX_PAYLOAD_BYTES,
                "Intruder payload 总字节数超过 {MAX_PAYLOAD_BYTES}（MIO 资源限额，未截断）"
            );
        }
    }
    Ok(())
}

fn count_requests(
    kind: AttackType,
    position_count: usize,
    payload_sets: &[Vec<Vec<u8>>],
) -> Result<u64> {
    let lengths: Vec<_> = payload_sets.iter().map(Vec::len).collect();
    count_request_lengths(kind, position_count, &lengths)
}

fn count_request_lengths(
    kind: AttackType,
    position_count: usize,
    lengths: &[usize],
) -> Result<u64> {
    // Any empty factor means zero even when the other factors' product would overflow.
    if lengths.contains(&0) {
        return Ok(0);
    }
    let count = match kind {
        AttackType::Sniper => u64::try_from(position_count)?
            .checked_mul(u64::try_from(lengths[0])?)
            .context("Intruder 生成数量溢出")?,
        AttackType::BatteringRam => u64::try_from(lengths[0])?,
        AttackType::Pitchfork => u64::try_from(
            lengths
                .iter()
                .copied()
                .min()
                .context("Intruder 缺少 payload set")?,
        )?,
        AttackType::ClusterBomb => lengths.iter().try_fold(1_u64, |count, length| {
            count
                .checked_mul(u64::try_from(*length)?)
                .context("Intruder 生成数量溢出")
        })?,
    };
    ensure!(
        count <= MAX_REQUESTS,
        "Intruder 生成数量超过 {MAX_REQUESTS}（MIO 资源限额，未截断）"
    );
    Ok(count)
}

pub(super) fn expanded_len(template: usize, removed: usize, inserted: usize) -> Result<usize> {
    let length = template
        .checked_sub(removed)
        .and_then(|base| base.checked_add(inserted))
        .context("Intruder 展开请求长度溢出或替换范围无效")?;
    ensure!(
        length <= MAX_REQUEST_BYTES,
        "Intruder 单条请求超过 {MAX_REQUEST_BYTES} 字节（MIO 资源限额，未截断）"
    );
    Ok(length)
}
