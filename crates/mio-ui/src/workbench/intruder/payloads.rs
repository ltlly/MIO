use std::io::Read;
use std::rc::Rc;

use gpui_kit::{Global, RenderOnce};
use mio_core::intruder::{
    AttackType, IntruderPayloadType, MAX_PAYLOAD_BYTES, MAX_PAYLOAD_SETS, MAX_PAYLOADS,
    PayloadProcessingRule, apply_payload_processing_rules,
};
use mio_core::{IntruderPayloadProcessingEntry, IntruderPayloadSetConfig};

use super::*;

#[cfg(test)]
mod generators_qa_tests;
mod generators_ui;
#[cfg(test)]
mod generators_ui_tests;
mod list_ops;
mod render;
mod render_sections;
#[cfg(test)]
mod unit_tests;

pub(super) const DEFAULT_PAYLOAD_ENCODE_CHARS: &str = "!#$&'()*+,/:;=?@[] ";

#[derive(Default)]
pub(super) struct PayloadEncodingInputs(pub(super) HashMap<TabId, Entity<InputState>>);

impl Global for PayloadEncodingInputs {}

type WindowElementRender = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

#[derive(IntoElement)]
pub(super) struct WithWindow(pub(super) WindowElementRender);

impl RenderOnce for WithWindow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        (self.0)(window, cx)
    }
}

pub(super) fn ensure_payload_encode_input(
    id: TabId,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    if let Some(existing) = cx
        .try_global::<PayloadEncodingInputs>()
        .and_then(|inputs| inputs.0.get(&id).cloned())
    {
        return existing;
    }
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(DEFAULT_PAYLOAD_ENCODE_CHARS)
            .default_value(DEFAULT_PAYLOAD_ENCODE_CHARS)
    });
    let mut map = cx
        .try_global::<PayloadEncodingInputs>()
        .map(|inputs| inputs.0.clone())
        .unwrap_or_default();
    map.insert(id, input.clone());
    cx.set_global(PayloadEncodingInputs(map));
    input
}

fn payload_set_count(spec: &IntruderSpec, positions: usize) -> usize {
    match spec.attack_type {
        AttackType::Sniper | AttackType::BatteringRam => 1,
        AttackType::Pitchfork | AttackType::ClusterBomb => positions,
    }
}

fn expected_payload_sets(spec: &IntruderSpec) -> usize {
    payload_set_count(spec, spec.positions.len())
}

fn payload_lines(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err("Payload 文件超过 32 MiB 限额；未截断或加载任何条目。".into());
    }
    let mut values = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let value = if let Some(line) = line.strip_suffix(b"\n") {
            line.strip_suffix(b"\r").unwrap_or(line)
        } else {
            line
        };
        if values.len() == MAX_PAYLOADS {
            return Err("Payload 条目超过 100000 个限额；未截断或加载任何条目。".into());
        }
        values.push(value.to_vec());
    }
    Ok(values)
}

fn read_payload_file(path: &std::path::Path) -> Result<Vec<Vec<u8>>, String> {
    #[cfg(test)]
    unit_tests::before_payload_open();
    #[cfg(not(unix))]
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && (!meta.is_file() || meta.len() > MAX_PAYLOAD_BYTES as u64)
    {
        return Err("请选择不超过 32 MiB 的普通 payload 文件；列表未改变。".into());
    }
    // Nonblocking open lets the same fd reject a replacement FIFO before reading.
    let mut opts = std::fs::File::options();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts
        .open(path)
        .map_err(|error| format!("无法打开 payload 文件：{error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("无法读取 payload 文件：{error}"))?;
    #[cfg(test)]
    unit_tests::after_payload_metadata();
    if !metadata.is_file() || metadata.len() > MAX_PAYLOAD_BYTES as u64 {
        return Err("请选择不超过 32 MiB 的普通 payload 文件；列表未改变。".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PAYLOAD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 payload 文件：{error}"))?;
    payload_lines(&bytes)
}

fn payload_label(value: &[u8]) -> String {
    let prefix = &value[..value.len().min(640)];
    let text = match std::str::from_utf8(prefix) {
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&prefix[..error.valid_up_to()])
        }
        result => result,
    };
    match text {
        Ok(text) if !text.chars().any(char::is_control) => {
            let mut chars = text.chars();
            let mut label: String = chars.by_ref().take(160).collect();
            if chars.next().is_some() || prefix.len() < value.len() {
                label.push_str(&format!("…（{} 字节）", value.len()));
            }
            if label.is_empty() {
                "（空 payload）".into()
            } else {
                label
            }
        }
        _ => format!(
            "Hex: {}{}",
            editing::hex_encode(&value[..value.len().min(48)]),
            if value.len() > 48 {
                format!("…（{} 字节）", value.len())
            } else {
                String::new()
            }
        ),
    }
}

fn payload_request_count(spec: &IntruderSpec, positions: usize) -> Option<u64> {
    if spec.payload_sets.len() != payload_set_count(spec, positions) || positions == 0 {
        return None;
    }
    match spec.attack_type {
        AttackType::Sniper => (spec.payload_sets[0].len() as u64).checked_mul(positions as u64),
        AttackType::BatteringRam => Some(spec.payload_sets[0].len() as u64),
        AttackType::Pitchfork => spec.payload_sets.iter().map(|set| set.len() as u64).min(),
        AttackType::ClusterBomb => spec
            .payload_sets
            .iter()
            .try_fold(1_u64, |count, set| count.checked_mul(set.len() as u64)),
    }
}

pub(super) fn payload_sets_identical(spec: &IntruderSpec) -> bool {
    let first = spec.payload_sets.first().map_or(&[][..], Vec::as_slice);
    spec.payload_sets.iter().skip(1).all(|set| set == first)
}

fn displayed_payloads<'a>(
    spec: &'a IntruderSpec,
    editors: &IntruderEditors,
) -> Option<&'a [Vec<u8>]> {
    if let Some(position) = editors.payload_position {
        return Some(spec.payload_sets.get(position).map_or(&[], Vec::as_slice));
    }
    editors
        .payloads_identical
        .then(|| spec.payload_sets.first().map_or(&[][..], Vec::as_slice))
}

fn payload_append_capacity(
    spec: &IntruderSpec,
    selected: Option<usize>,
    count: usize,
    bytes: usize,
) -> Result<usize, &'static str> {
    let sets = expected_payload_sets(spec).max(spec.payload_sets.len());
    if sets > MAX_PAYLOAD_SETS {
        return Err("当前攻击类型需要超过 20 个 payload set；未添加或截断任何条目。");
    }
    let copies = match selected {
        Some(index) if index < sets => 1,
        Some(_) => return Err("选中的 payload 位置已不存在；输入仍保留，请重新选择。"),
        None if sets > 0 => sets,
        None => return Err("请先添加 payload 位置；输入仍保留。"),
    };
    let existing_count = spec
        .payload_sets
        .iter()
        .try_fold(0_usize, |total, set| total.checked_add(set.len()));
    let existing_bytes = spec
        .payload_sets
        .iter()
        .flatten()
        .try_fold(0_usize, |total, value| total.checked_add(value.len()));
    if existing_count
        .and_then(|total| count.checked_mul(copies)?.checked_add(total))
        .is_none_or(|total| total > MAX_PAYLOADS)
    {
        return Err("添加后 payload 总数量将超过 100000；未添加或截断任何条目。");
    }
    if existing_bytes
        .and_then(|total| bytes.checked_mul(copies)?.checked_add(total))
        .is_none_or(|total| total > MAX_PAYLOAD_BYTES)
    {
        return Err("添加后 payload 总字节数将超过 32 MiB；未添加或截断任何条目。");
    }
    Ok(sets)
}

impl Workbench {
    pub(super) fn current_intruder_payload_type(
        &self,
        id: TabId,
        editors: &IntruderEditors,
    ) -> IntruderPayloadType {
        let idx = editors.payload_position.unwrap_or(0);
        self.workspace
            .intruder
            .payload_configs
            .get(&id)
            .and_then(|configs| configs.get(idx))
            .map(|cfg| cfg.payload_type)
            .unwrap_or(editors.payload_type)
    }

    pub(super) fn current_intruder_processing_rules(
        &self,
        id: TabId,
        editors: &IntruderEditors,
    ) -> Vec<IntruderPayloadProcessingEntry> {
        let idx = editors.payload_position.unwrap_or(0);
        self.workspace
            .intruder
            .payload_configs
            .get(&id)
            .and_then(|configs| configs.get(idx))
            .map(|cfg| cfg.processing_rules.clone())
            .unwrap_or_default()
    }

    pub(in crate::workbench) fn update_intruder_payload_set_configs(
        &mut self,
        id: TabId,
        mutate: impl FnOnce(&mut Vec<IntruderPayloadSetConfig>, Option<usize>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return false;
        }
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return false;
        };
        let sets_len = expected_payload_sets(&tab.spec)
            .max(tab.spec.payload_sets.len())
            .clamp(1, MAX_PAYLOAD_SETS);
        let selected = self
            .intruder
            .editors
            .get(&id)
            .and_then(|editors| editors.payload_position);
        let mut configs = self
            .workspace
            .intruder
            .payload_configs
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if configs.len() < sets_len {
            configs.resize_with(sets_len, IntruderPayloadSetConfig::default);
        }
        mutate(&mut configs, selected);
        if configs.iter().all(IntruderPayloadSetConfig::is_default) {
            self.workspace.intruder.payload_configs.remove(&id);
        } else {
            self.workspace.intruder.payload_configs.insert(id, configs);
        }
        self.save_workspace(cx);
        cx.notify();
        true
    }

    pub(in crate::workbench) fn select_intruder_payload_type(
        &mut self,
        id: TabId,
        payload_type: IntruderPayloadType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.payload_type = payload_type;
        }
        self.update_intruder_payload_set_configs(
            id,
            |configs, selected| {
                if let Some(idx) = selected {
                    if idx >= configs.len() && idx < MAX_PAYLOAD_SETS {
                        configs.resize_with(idx + 1, IntruderPayloadSetConfig::default);
                    }
                    if let Some(cfg) = configs.get_mut(idx) {
                        cfg.payload_type = payload_type;
                    }
                } else {
                    for cfg in configs.iter_mut() {
                        cfg.payload_type = payload_type;
                    }
                }
            },
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn add_intruder_processing_pipeline_rule(
        &mut self,
        id: TabId,
        rule_template: PayloadProcessingRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.payload_input.clone();
        let uses_input = matches!(
            rule_template,
            PayloadProcessingRule::AddPrefix(_)
                | PayloadProcessingRule::AddSuffix(_)
                | PayloadProcessingRule::Substring { .. }
                | PayloadProcessingRule::MatchReplace { .. }
                | PayloadProcessingRule::MatchReplaceRegex { .. }
                | PayloadProcessingRule::AddIfNotMatchRegex { .. }
                | PayloadProcessingRule::SkipIfMatchesRegex { .. }
        );
        let (rule, clear_input) = if uses_input {
            let text = match editing::input_snapshot(&input, window, cx) {
                Ok(value) => value.to_string(),
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            match generators_ui::resolve_processing_rule_with_input(rule_template, &text) {
                Ok(resolved) => resolved,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            }
        } else {
            (rule_template, false)
        };
        if self.update_intruder_payload_set_configs(
            id,
            |configs, selected| {
                let entry = IntruderPayloadProcessingEntry {
                    enabled: true,
                    rule,
                };
                if let Some(idx) = selected {
                    if let Some(cfg) = configs.get_mut(idx) {
                        cfg.processing_rules.push(entry);
                    }
                } else {
                    for cfg in configs.iter_mut() {
                        cfg.processing_rules.push(entry.clone());
                    }
                }
            },
            window,
            cx,
        ) && clear_input
        {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    pub(in crate::workbench) fn run_intruder_processing_pipeline(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let selected = editors.payload_position;
        let entries = self.current_intruder_processing_rules(id, editors);
        let enabled_rules: Vec<PayloadProcessingRule> = entries
            .into_iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.rule)
            .collect();
        if enabled_rules.is_empty() {
            self.fail("当前尚无已启用的 Payload Processing 管线规则。", cx);
            return;
        }
        self.try_change_intruder_spec(
            id,
            |spec| {
                let mut next_sets = spec.payload_sets.clone();
                if let Some(index) = selected {
                    let set = next_sets.get_mut(index).ok_or_else(|| {
                        "选中的 payload 位置尚无列表；未改变其它位置。".to_owned()
                    })?;
                    *set = apply_payload_processing_rules(set, &enabled_rules)
                        .map_err(|error| error.to_string())?;
                } else {
                    for set in &mut next_sets {
                        *set = apply_payload_processing_rules(set, &enabled_rules)
                            .map_err(|error| error.to_string())?;
                    }
                }
                spec.payload_sets = next_sets;
                Ok(())
            },
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn append_intruder_payloads_from_plugin(
        &mut self,
        id: TabId,
        values: &[Vec<u8>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.append_intruder_payloads(id, values, window, cx)
    }

    pub(in crate::workbench) fn apply_processed_intruder_payloads(
        &mut self,
        id: TabId,
        position: Option<usize>,
        payloads: Vec<Vec<u8>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.try_change_intruder_spec(
            id,
            |spec| {
                let mut next_sets = spec.payload_sets.clone();
                if let Some(index) = position {
                    let set = next_sets
                        .get_mut(index)
                        .ok_or_else(|| "选中的 payload 位置已不存在；列表未改变。".to_owned())?;
                    *set = payloads;
                } else {
                    for set in &mut next_sets {
                        *set = payloads.clone();
                    }
                }
                let total_count = next_sets
                    .iter()
                    .try_fold(0usize, |sum, set| sum.checked_add(set.len()))
                    .ok_or_else(|| "Payload 总数量溢出；列表未改变。".to_owned())?;
                if total_count > MAX_PAYLOADS {
                    return Err("处理后 payload 总数量将超过 100000；列表未改变。".into());
                }
                let total_bytes = next_sets
                    .iter()
                    .flatten()
                    .try_fold(0usize, |sum, value| sum.checked_add(value.len()))
                    .ok_or_else(|| "Payload 总字节数溢出；列表未改变。".to_owned())?;
                if total_bytes > MAX_PAYLOAD_BYTES {
                    return Err("处理后 payload 总字节数将超过 32 MiB；列表未改变。".into());
                }
                spec.payload_sets = next_sets;
                Ok(())
            },
            window,
            cx,
        )
    }

    pub(in crate::workbench) fn generate_plugin_intruder_payloads(
        &mut self,
        id: TabId,
        instance_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let config = match editing::input_snapshot(&editors.payload_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.request_plugin_payload_generation(instance_id, id, config, cx);
    }

    pub(in crate::workbench) fn process_plugin_intruder_payloads(
        &mut self,
        id: TabId,
        instance_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let selected = editors.payload_position;
        let config = match editing::input_snapshot(&editors.payload_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let position = match selected {
            Some(index) => Some(index),
            None if tab.spec.payload_sets.len() == 1 => Some(0),
            None if payload_sets_identical(&tab.spec) => None,
            None => {
                self.fail("各位置的 payload 列表不同；请先选择要处理的具体位置。", cx);
                return;
            }
        };
        let source_index = position.unwrap_or(0);
        let Some(inputs) = tab.spec.payload_sets.get(source_index).cloned() else {
            self.fail("当前 payload 位置尚无列表；未执行插件处理。", cx);
            return;
        };
        if inputs.is_empty() {
            self.fail("当前 payload 列表为空；请先添加 payload。", cx);
            return;
        }
        self.request_plugin_payload_processing(instance_id, id, position, config, inputs, cx);
    }

    pub(in crate::workbench) fn append_intruder_payloads(
        &mut self,
        id: TabId,
        values: &[Vec<u8>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if values.is_empty() {
            return true;
        }
        let Some(editors) = self.intruder.editors.get(&id) else {
            return false;
        };
        let selected = editors.payload_position;
        let Some(bytes) = values
            .iter()
            .try_fold(0_usize, |sum, value| sum.checked_add(value.len()))
        else {
            self.fail("Payload 总字节数溢出；列表未改变。", cx);
            return false;
        };
        self.try_change_intruder_spec(
            id,
            |spec| {
                let count = payload_append_capacity(spec, selected, values.len(), bytes)
                    .map_err(str::to_owned)?;
                // Check aggregate limits before duplicating one file into multiple sets.
                spec.payload_sets.resize_with(count, Vec::new);
                if let Some(index) = selected {
                    spec.payload_sets[index].extend_from_slice(values);
                } else {
                    for set in &mut spec.payload_sets {
                        set.extend_from_slice(values);
                    }
                }
                Ok(())
            },
            window,
            cx,
        )
    }

    pub(in crate::workbench) fn edit_intruder_payloads(
        &mut self,
        id: TabId,
        edit: impl Fn(&mut Vec<Vec<u8>>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return false;
        };
        let selected = editors.payload_position;
        if let Some(index) = selected
            && !self
                .workspace
                .intruder
                .tabs
                .iter()
                .any(|tab| tab.id == id && index < tab.spec.payload_sets.len())
        {
            self.fail("选中的 payload 位置尚无列表；未改变其它位置。", cx);
            return false;
        }
        self.change_intruder_spec(
            id,
            |spec| {
                if let Some(index) = selected {
                    if let Some(set) = spec.payload_sets.get_mut(index) {
                        edit(set);
                    }
                } else {
                    for set in &mut spec.payload_sets {
                        edit(set);
                    }
                }
            },
            window,
            cx,
        )
    }

    pub(in crate::workbench) fn add_intruder_payload(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.payload_input.clone();
        let value = match editing::input_snapshot(&input, window, cx) {
            Ok(value) if value.len() <= MAX_PAYLOAD_BYTES => value.as_bytes().to_vec(),
            Ok(_) => {
                self.fail("Payload 超过 32 MiB；输入仍保留，列表未改变。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if self.append_intruder_payloads(id, &[value], window, cx) {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }
}
