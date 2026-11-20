use mio_core::intruder::{
    MAX_PAYLOAD_BYTES, PayloadProcessingRule, apply_payload_processing_rule,
    generate_number_payloads,
};

use super::*;

pub(super) fn encode_selected_chars_in_set(
    values: &[Vec<u8>],
    chars_to_encode: &str,
) -> Result<Vec<Vec<u8>>, String> {
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mask = chars_to_encode.as_bytes();
    if mask.is_empty() {
        return Err("请先填写要进行 URL 编码的字符集合；列表未改变。".into());
    }
    let mut table = [false; 256];
    for &byte in mask {
        table[usize::from(byte)] = true;
    }
    let mut total_bytes = 0usize;
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let extra = value
            .iter()
            .filter(|&&byte| table[usize::from(byte)])
            .count();
        let next_len = extra
            .checked_mul(2)
            .and_then(|added| value.len().checked_add(added))
            .ok_or_else(|| "Payload 编码长度溢出；列表未改变。".to_owned())?;
        total_bytes = total_bytes
            .checked_add(next_len)
            .ok_or_else(|| "Payload 总字节数溢出；列表未改变。".to_owned())?;
        if total_bytes > MAX_PAYLOAD_BYTES {
            return Err("编码后 payload 总字节数将超过 32 MiB；列表未改变。".into());
        }
        let mut encoded = Vec::with_capacity(next_len);
        for &byte in value {
            if table[usize::from(byte)] {
                encoded.extend_from_slice(&[
                    b'%',
                    HEX_UPPER[usize::from(byte >> 4)],
                    HEX_UPPER[usize::from(byte & 0x0f)],
                ]);
            } else {
                encoded.push(byte);
            }
        }
        out.push(encoded);
    }
    Ok(out)
}

impl Workbench {
    pub(in crate::workbench) fn encode_intruder_payload_selected_chars(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self
            .intruder
            .editors
            .get(&id)
            .map(|editors| editors.payload_position)
        else {
            return;
        };
        let input = ensure_payload_encode_input(id, window, cx);
        let chars_to_encode = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.try_change_intruder_spec(
            id,
            |spec| {
                let mut next_sets = spec.payload_sets.clone();
                if let Some(index) = selected {
                    let set = next_sets.get_mut(index).ok_or_else(|| {
                        "选中的 payload 位置尚无列表；未改变其它位置。".to_owned()
                    })?;
                    *set = encode_selected_chars_in_set(set, &chars_to_encode)?;
                } else {
                    for set in &mut next_sets {
                        *set = encode_selected_chars_in_set(set, &chars_to_encode)?;
                    }
                }
                let total_bytes = next_sets
                    .iter()
                    .flatten()
                    .try_fold(0usize, |sum, value| sum.checked_add(value.len()))
                    .ok_or_else(|| "Payload 总字节数溢出；列表未改变。".to_owned())?;
                if total_bytes > MAX_PAYLOAD_BYTES {
                    return Err("编码后 payload 总字节数将超过 32 MiB；列表未改变。".into());
                }
                spec.payload_sets = next_sets;
                Ok(())
            },
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn generate_intruder_numbers(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let hex = editors.numbers_hex;
        let snapshots = (|| {
            Ok::<_, &'static str>((
                editing::input_snapshot(&editors.numbers_from, window, cx)?.to_string(),
                editing::input_snapshot(&editors.numbers_to, window, cx)?.to_string(),
                editing::input_snapshot(&editors.numbers_step, window, cx)?.to_string(),
                editing::input_snapshot(&editors.numbers_min_digits, window, cx)?.to_string(),
            ))
        })();
        let (from_raw, to_raw, step_raw, min_digits_raw) = match snapshots {
            Ok(values) => values,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let Ok(from) = from_raw.trim().parse::<i64>() else {
            self.fail("Numbers 起始值必须为有效整数；列表未改变。", cx);
            return;
        };
        let Ok(to) = to_raw.trim().parse::<i64>() else {
            self.fail("Numbers 结束值必须为有效整数；列表未改变。", cx);
            return;
        };
        let step = if step_raw.trim().is_empty() {
            1
        } else {
            match step_raw.trim().parse::<i64>() {
                Ok(value) => value,
                Err(_) => {
                    self.fail("Numbers 步长必须为有效正整数；列表未改变。", cx);
                    return;
                }
            }
        };
        let min_digits = if min_digits_raw.trim().is_empty() {
            1
        } else {
            match min_digits_raw.trim().parse::<usize>() {
                Ok(value) => value,
                Err(_) => {
                    self.fail("Numbers 位数必须为有效正整数；列表未改变。", cx);
                    return;
                }
            }
        };
        match generate_number_payloads(from, to, step, min_digits, hex) {
            Ok(values) => {
                self.append_intruder_payloads(id, &values, window, cx);
            }
            Err(error) => self.fail(error.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn process_intruder_payloads(
        &mut self,
        id: TabId,
        rule: PayloadProcessingRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let selected = editors.payload_position;
        let input = editors.payload_input.clone();
        let uses_input = matches!(
            rule,
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
            match super::generators_ui::resolve_processing_rule_with_input(rule, &text) {
                Ok(resolved) => resolved,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            }
        } else {
            (rule, false)
        };
        if self.try_change_intruder_spec(
            id,
            |spec| {
                let mut next_sets = spec.payload_sets.clone();
                if let Some(index) = selected {
                    let set = next_sets.get_mut(index).ok_or_else(|| {
                        "选中的 payload 位置尚无列表；未改变其它位置。".to_owned()
                    })?;
                    *set = apply_payload_processing_rule(set, &rule)
                        .map_err(|error| error.to_string())?;
                } else {
                    for set in &mut next_sets {
                        *set = apply_payload_processing_rule(set, &rule)
                            .map_err(|error| error.to_string())?;
                    }
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
        ) && clear_input
        {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(in crate::workbench) fn paste_intruder_payloads(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.fail("剪贴板没有可粘贴的文本 payload；列表未改变。", cx);
            return;
        };
        match payload_lines(text.as_bytes()) {
            Ok(values) => {
                self.append_intruder_payloads(id, &values, window, cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn load_intruder_payloads(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.file_picker_pending()
            || !self.backend_available()
            || self
                .intruder
                .editors
                .get(&id)
                .is_none_or(|editors| editors.loading_payloads)
        {
            return;
        }
        let Some(project) = self.project else { return };
        let spec = match self.snapshot_intruder_spec(id, window, cx) {
            Ok(spec) => spec,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let editors = self.intruder.editors.get_mut(&id).expect("checked editor");
        let position = editors.payload_position;
        let editor_id = editors.payload_input.entity_id();
        editors.loading_payloads = true;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("加载 payload 列表".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let current = this
                .update_in(cx, |view, _, cx| {
                    if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                        return false;
                    }
                    let available = view.backend_available();
                    let Some(editors) = view.intruder.editors.get_mut(&id) else {
                        return false;
                    };
                    if editors.payload_input.entity_id() != editor_id {
                        return false;
                    }
                    if !available {
                        editors.loading_payloads = false;
                    }
                    available
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let result = match paths {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => Some(
                        cx.background_spawn(async move { read_payload_file(&path) })
                            .await,
                    ),
                    None => None,
                },
                Ok(Ok(None)) => None,
                Ok(Err(error)) => Some(Err(format!("无法选择 payload 文件：{error}"))),
                Err(_) => Some(Err("Payload 文件选择器已关闭；列表未改变。".into())),
            };
            let _ = this.update_in(cx, |view, window, cx| {
                if view.project != Some(project) {
                    return;
                }
                let available = view.backend_available();
                let Some(editors) = view.intruder.editors.get_mut(&id) else {
                    return;
                };
                if editors.payload_input.entity_id() != editor_id {
                    return;
                }
                editors.loading_payloads = false;
                let position_unchanged = editors.payload_position == position;
                if !available {
                    cx.notify();
                    return;
                }
                let Some(result) = result else {
                    cx.notify();
                    return;
                };
                if !position_unchanged
                    || view.snapshot_intruder_spec(id, window, cx).as_ref() != Ok(&spec)
                {
                    view.fail(
                        "读取期间 Intruder 配置或 payload 位置已改变；文件未应用，请明确重试。",
                        cx,
                    );
                    return;
                }
                match result {
                    Ok(values) => {
                        view.append_intruder_payloads(id, &values, window, cx);
                    }
                    Err(error) => view.fail(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn remove_intruder_payloads(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        if displayed_payloads(&tab.spec, editors).is_none() {
            self.fail("各位置的列表不同；请选择具体 payload 位置后移除。", cx);
            return;
        }
        let selection = editors.payload_selection.clone();
        if selection.is_empty() {
            return;
        }
        if self.edit_intruder_payloads(
            id,
            |set| {
                let mut index = 0;
                set.retain(|_| {
                    let keep = !selection.contains(&index);
                    index += 1;
                    keep
                });
            },
            window,
            cx,
        ) && let Some(editors) = self.intruder.editors.get_mut(&id)
        {
            editors.payload_selection.clear();
        }
    }

    pub(in crate::workbench) fn clear_intruder_payloads(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .intruder
            .editors
            .get(&id)
            .and_then(|editors| editors.payload_position);
        if self.change_intruder_spec(
            id,
            |spec| {
                if let Some(index) = selected {
                    if let Some(set) = spec.payload_sets.get_mut(index) {
                        set.clear();
                    }
                } else {
                    spec.payload_sets = vec![Vec::new(); expected_payload_sets(spec)];
                }
            },
            window,
            cx,
        ) && let Some(editors) = self.intruder.editors.get_mut(&id)
        {
            editors.payload_selection.clear();
        }
    }

    pub(in crate::workbench) fn deduplicate_intruder_payloads(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_intruder_payloads(
            id,
            |set| {
                let mut seen = HashSet::new();
                set.retain(|value| seen.insert(value.clone()));
            },
            window,
            cx,
        ) && let Some(editors) = self.intruder.editors.get_mut(&id)
        {
            editors.payload_selection.clear();
        }
    }

    pub(in crate::workbench) fn copy_intruder_payload(
        &mut self,
        id: TabId,
        row_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let Some(values) = displayed_payloads(&tab.spec, editors) else {
            self.fail("各位置的列表不同；请选择具体 payload 位置后复制。", cx);
            return;
        };
        let mut indices: Vec<usize> = if editors.payload_selection.contains(&row_index) {
            editors.payload_selection.iter().copied().collect()
        } else {
            vec![row_index]
        };
        indices.sort_unstable();
        let lines: Vec<String> = indices
            .into_iter()
            .filter_map(|idx| values.get(idx))
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect();
        if lines.is_empty() {
            return;
        }
        let count = lines.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!("已复制 {count} 条载荷到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn remove_intruder_payload_at(
        &mut self,
        id: TabId,
        row_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let in_selection = self
            .intruder
            .editors
            .get(&id)
            .is_some_and(|editors| editors.payload_selection.contains(&row_index));
        if in_selection {
            self.remove_intruder_payloads(id, window, cx);
            return;
        }
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        if displayed_payloads(&tab.spec, editors).is_none() {
            self.fail("各位置的列表不同；请选择具体 payload 位置后删除。", cx);
            return;
        }
        if self.edit_intruder_payloads(
            id,
            |set| {
                if row_index < set.len() {
                    set.remove(row_index);
                }
            },
            window,
            cx,
        ) && let Some(editors) = self.intruder.editors.get_mut(&id)
        {
            editors.payload_selection.clear();
        }
    }

    pub(in crate::workbench) fn move_intruder_payload(
        &mut self,
        id: TabId,
        row_index: usize,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let Some(values) = displayed_payloads(&tab.spec, editors) else {
            self.fail("各位置的列表不同；请选择具体 payload 位置后调整顺序。", cx);
            return;
        };
        let Some(target) = row_index
            .checked_add_signed(delta)
            .filter(|&idx| idx < values.len())
        else {
            return;
        };
        if self.edit_intruder_payloads(
            id,
            |set| {
                if row_index < set.len() && target < set.len() {
                    set.swap(row_index, target);
                }
            },
            window,
            cx,
        ) && let Some(editors) = self.intruder.editors.get_mut(&id)
        {
            editors.payload_selection.clear();
            editors.payload_selection.insert(target);
        }
    }
}
