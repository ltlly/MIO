use super::*;
use crate::intruder::{
    PayloadPosition, payload_positions, payload_request_count, validate_positions,
};

pub(in crate::store::intruder) const PREVIEW_BYTES: usize = 1024;
const PREVIEW_CHARACTERS: usize = 256;

/// A bounded prefix plus exact coordinates into the immutable payload table.
/// Binary values are not decoded lossily; consumers choose a text or hex presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PayloadSummary {
    pub selected: PayloadPosition,
    pub raw_bytes: u64,
    pub preview_bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResultPayloads {
    pub ordinal: u64,
    pub payloads: Vec<PayloadSummary>,
}

pub(in crate::store::intruder) struct PayloadOrder {
    pub(in crate::store::intruder) attack_type: AttackType,
    pub(in crate::store::intruder) position_count: usize,
    pub(in crate::store::intruder) lengths: Vec<usize>,
    pub(in crate::store::intruder) ranks: Vec<Vec<u32>>,
    pub(in crate::store::intruder) baseline: Option<u64>,
}

impl PayloadOrder {
    pub(in crate::store::intruder) fn request_index(&self, ordinal: u64) -> Result<Option<usize>> {
        let count = payload_request_count(self.attack_type, self.position_count, &self.lengths)?;
        let index = match self.baseline {
            Some(baseline) => {
                ensure!(
                    baseline <= count && count < MAX_REQUESTS && ordinal <= count,
                    "Intruder 基准位置或请求索引超出已验证的生成数量"
                );
                if ordinal == baseline {
                    return Ok(None);
                }
                ordinal - u64::from(ordinal > baseline)
            }
            None => ordinal,
        };
        ensure!(index < count, "Intruder 请求索引超出已验证的生成数量");
        Ok(Some(
            usize::try_from(index).context("Intruder 请求索引超出本机范围")?,
        ))
    }

    pub(super) fn compare_indices(
        &self,
        left: Option<usize>,
        right: Option<usize>,
        ascending: bool,
    ) -> std::cmp::Ordering {
        if left == right {
            return std::cmp::Ordering::Equal;
        }
        // shortcut: empty baseline sorts before empty payload bytes until External tie order is confirmed.
        let ordering = match (left, right) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(l), Some(r)) => match self.attack_type {
                AttackType::Sniper => {
                    let len = self.lengths[0];
                    self.ranks[0][l % len].cmp(&self.ranks[0][r % len])
                }
                AttackType::BatteringRam => self.ranks[0][l].cmp(&self.ranks[0][r]),
                AttackType::Pitchfork => (0..self.position_count)
                    .map(|set| self.ranks[set][l])
                    .cmp((0..self.position_count).map(|set| self.ranks[set][r])),
                AttackType::ClusterBomb => {
                    let mut left_coords = [0usize; MAX_PAYLOAD_SETS];
                    let mut right_coords = [0usize; MAX_PAYLOAD_SETS];
                    let mut l_rem = l;
                    let mut r_rem = r;
                    for (set, &len) in self.lengths.iter().enumerate().rev() {
                        left_coords[set] = l_rem % len;
                        right_coords[set] = r_rem % len;
                        l_rem /= len;
                        r_rem /= len;
                    }
                    (0..self.position_count)
                        .map(|set| self.ranks[set][left_coords[set]])
                        .cmp((0..self.position_count).map(|set| self.ranks[set][right_coords[set]]))
                }
            },
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    }

    #[cfg(test)]
    pub(super) fn compare(
        &self,
        left: u64,
        right: u64,
        ascending: bool,
    ) -> Result<std::cmp::Ordering> {
        Ok(self.compare_indices(
            self.request_index(left)?,
            self.request_index(right)?,
            ascending,
        ))
    }
}

pub(super) fn payload_order(
    connection: &Connection,
    run: &IntruderRunSummary,
    check: &dyn Fn() -> Result<()>,
) -> Result<PayloadOrder> {
    check()?;
    let (meta, lengths) = payload_shape(connection, run.id, check)?;
    let count = payload_request_count(meta.attack_type, meta.positions.len(), &lengths)?;
    ensure!(
        count.checked_add(u64::from(run.baseline.is_some())) == Some(run.total)
            && (!meta.baseline_request
                || run.baseline.is_some_and(|baseline| baseline.ordinal == 0)),
        "Intruder payload 组合数量与运行不一致"
    );
    let mut ranks: Vec<Vec<u32>> = lengths.iter().map(|&len| vec![0; len]).collect();
    let mut query = connection.prepare(
        "SELECT set_index,ordinal,dense_rank() OVER (PARTITION BY set_index ORDER BY value ASC)
         FROM intruder_payloads WHERE run=?1",
    )?;
    let mut rows = query.query([run.id.to_string()])?;
    while let Some(row) = rows.next()? {
        check()?;
        let set = row.get::<_, usize>(0)?;
        let ordinal = row.get::<_, usize>(1)?;
        let rank = row.get::<_, u32>(2)?;
        let slot = ranks
            .get_mut(set)
            .and_then(|values| values.get_mut(ordinal))
            .context("Intruder payload 排序坐标越界")?;
        ensure!(*slot == 0 && rank > 0, "Intruder payload 排序秩重复或无效");
        *slot = rank;
    }
    ensure!(
        ranks.iter().flatten().all(|&rank| rank > 0),
        "Intruder payload 排序秩缺失"
    );
    Ok(PayloadOrder {
        attack_type: meta.attack_type,
        position_count: meta.positions.len(),
        lengths,
        ranks,
        baseline: run.baseline.map(|baseline| baseline.ordinal),
    })
}

impl ProjectStore {
    /// Read only the immutable inputs selected by these stored result ordinals.
    /// Recovery callers may include an admitted ordinal not yet present in the result table.
    /// Returns a complete-row prefix within the page budget; continue after its actual length.
    #[cfg(test)]
    pub(crate) fn intruder_payload_summaries(
        &self,
        id: Uuid,
        ordinals: &[u64],
        cancel: &CancellationToken,
    ) -> Result<Vec<ResultPayloads>> {
        ensure!(!cancel.is_cancelled(), "Intruder payload 读取已取消");
        ensure!(
            !id.is_nil() && !ordinals.is_empty() && ordinals.len() <= 500,
            "Intruder payload 查询需要有效运行和 1–500 条结果"
        );
        let unique: BTreeSet<_> = ordinals.iter().copied().collect();
        ensure!(
            unique.len() == ordinals.len(),
            "Intruder payload 查询结果序号重复"
        );
        if let Some(dataset) = self.session_dataset(id)? {
            return dataset.data.payload_summaries(&dataset.run, ordinals, &|| {
                ensure!(!cancel.is_cancelled(), "Intruder payload 读取已取消");
                Ok(())
            });
        }
        let transaction = self.connection.unchecked_transaction()?;
        let run = read_run(&transaction, id)?;
        let output = payload_summaries(&transaction, &run, ordinals, &|| {
            ensure!(!cancel.is_cancelled(), "Intruder payload 读取已取消");
            Ok(())
        })?;
        transaction.commit()?;
        Ok(output)
    }
}

/// The caller owns the read transaction and any SQL progress or deadline handling.
/// One extra ordinal is reserved for the current selection outside a 500-row page.
pub(super) fn payload_summaries(
    connection: &Connection,
    run: &IntruderRunSummary,
    ordinals: &[u64],
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<ResultPayloads>> {
    check()?;
    let id = run.id;
    ensure!(
        !id.is_nil() && !ordinals.is_empty() && ordinals.len() <= 501,
        "Intruder payload 查询需要有效运行及至多 500 行和一个页外选择"
    );
    let unique: BTreeSet<_> = ordinals.iter().copied().collect();
    ensure!(
        unique.len() == ordinals.len(),
        "Intruder payload 查询结果序号重复"
    );
    ensure!(
        ordinals.iter().all(|ordinal| *ordinal < run.total),
        "Intruder payload 结果序号超出计划"
    );
    let (meta, lengths) = payload_shape(connection, id, check)?;
    let count = payload_request_count(meta.attack_type, meta.positions.len(), &lengths)?;
    ensure!(
        count.checked_add(u64::from(run.baseline.is_some())) == Some(run.total)
            && (!meta.baseline_request
                || run.baseline.is_some_and(|baseline| baseline.ordinal == 0)),
        "Intruder payload 组合数量与运行不一致"
    );
    let mut page_bytes = 0usize;
    let mut cache = BTreeMap::new();
    let mut output = Vec::with_capacity(ordinals.len());
    let mut query = connection.prepare(
        "SELECT typeof(value),length(value),
        CASE WHEN typeof(value)='blob' AND length(value)<=?4
            THEN CASE WHEN length(value)=0 THEN X'' ELSE substr(value,1,?5) END END
         FROM intruder_payloads WHERE run=?1 AND set_index=?2 AND ordinal=?3",
    )?;
    for &ordinal in ordinals {
        check()?;
        let selected = payload_positions(
            meta.attack_type,
            meta.positions.len(),
            &lengths,
            ordinal,
            run.baseline.map(|baseline| baseline.ordinal),
        )?;
        let mut payloads = Vec::with_capacity(selected.len());
        let mut inputs = BTreeSet::new();
        let mut row_bytes = 32usize;
        for selected in selected {
            check()?;
            let key = (selected.set_index, selected.payload_index);
            if !inputs.insert(key) {
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(key) {
                let mut rows = query.query(params![
                    id.to_string(),
                    selected.set_index,
                    selected.payload_index,
                    MAX_PAYLOAD_BYTES,
                    PREVIEW_BYTES,
                ])?;
                let row = rows.next()?.context("Intruder payload 输入不存在")?;
                ensure!(
                    row.get::<_, String>(0)? == "blob",
                    "Intruder payload 类型无效"
                );
                let raw_bytes = row.get::<_, u64>(1)?;
                let prefix = row
                    .get::<_, Option<Vec<u8>>>(2)?
                    .context("Intruder payload 超出读取限额")?;
                ensure!(
                    prefix.len() == usize::try_from(raw_bytes)?.min(PREVIEW_BYTES),
                    "Intruder payload 前缀长度不一致"
                );
                let prefix = preview(prefix, raw_bytes <= PREVIEW_BYTES as u64);
                ensure!(rows.next()?.is_none(), "Intruder payload 输入身份重复");
                entry.insert((raw_bytes, prefix));
            }
            let (raw_bytes, preview_bytes) = &cache[&key];
            row_bytes = row_bytes
                .checked_add(preview_bytes.len() + 64)
                .context("Intruder payload 结果行长度溢出")?;
            ensure!(
                row_bytes <= PAGE_BYTES,
                "Intruder payload 单行超过 1 MiB；未返回部分输入"
            );
            payloads.push(PayloadSummary {
                selected,
                raw_bytes: *raw_bytes,
                preview_bytes: preview_bytes.clone(),
            });
        }
        if row_bytes > PAGE_BYTES - page_bytes {
            break;
        }
        page_bytes += row_bytes;
        output.push(ResultPayloads { ordinal, payloads });
    }
    drop(query);
    check()?;
    Ok(output)
}

fn payload_shape(
    connection: &Connection,
    id: Uuid,
    check: &dyn Fn() -> Result<()>,
) -> Result<(SpecMeta, Vec<usize>)> {
    check()?;
    let (meta, template_bytes): (Vec<u8>, usize) = connection.query_row(
        "SELECT CASE WHEN typeof(spec_meta)='blob' AND length(spec_meta)<=?2 THEN spec_meta END,
            CASE WHEN typeof(template)='blob' AND length(template)<=?3 THEN length(template) END
         FROM intruder_runs WHERE id=?1",
        params![id.to_string(), MAX_SPEC_META, MAX_REQUEST_BYTES],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let meta: SpecMeta = serde_json::from_slice(&meta)?;
    ensure!(
        (1..=MAX_PAYLOAD_SETS).contains(&meta.set_count),
        "Intruder payload set 数量无效"
    );
    validate_positions(template_bytes, &meta.positions)?;
    let mut lengths = vec![0usize; meta.set_count];
    let mut count = 0usize;
    let mut bytes = 0usize;
    // Visit only indexed coordinates and lengths; neither payload data nor request bodies are read.
    let mut query = connection.prepare(
        "SELECT set_index,ordinal,typeof(value),length(value) FROM intruder_payloads
         WHERE run=?1 ORDER BY set_index,ordinal LIMIT ?2",
    )?;
    let mut rows = query.query(params![id.to_string(), MAX_PAYLOADS + 1])?;
    while let Some(row) = rows.next()? {
        check()?;
        let set = row.get::<_, usize>(0)?;
        let ordinal = row.get::<_, usize>(1)?;
        ensure!(
            set < lengths.len() && ordinal == lengths[set],
            "Intruder payload 位置缺失、重复或越界"
        );
        ensure!(
            row.get::<_, String>(2)? == "blob",
            "Intruder payload 不是二进制"
        );
        lengths[set] += 1;
        count += 1;
        bytes = bytes
            .checked_add(row.get::<_, usize>(3)?)
            .context("Intruder payload 字节数溢出")?;
        ensure!(
            count <= MAX_PAYLOADS && bytes <= MAX_PAYLOAD_BYTES,
            "Intruder payload 输入超过限额"
        );
    }
    Ok((meta, lengths))
}

pub(in crate::store::intruder) fn preview(mut bytes: Vec<u8>, complete: bool) -> Vec<u8> {
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => Some(text),
        Err(error) if !complete && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    };
    let length = text.map_or(bytes.len().min(PREVIEW_CHARACTERS), |text| {
        text.char_indices()
            .nth(PREVIEW_CHARACTERS)
            .map_or(text.len(), |(offset, _)| offset)
    });
    bytes.truncate(length);
    bytes
}

#[cfg(test)]
mod tests;
