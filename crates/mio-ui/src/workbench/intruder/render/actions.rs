use super::*;

impl Workbench {
    pub(in crate::workbench) fn restore_intruder_attack_results(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let revision = tab.spec.revision;
        let Some(editor) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editor.baseline_restore.is_some() || self.transition.is_some() {
            return;
        }
        let operation = Uuid::new_v4();
        editor.baseline_restore = Some(operation);
        let response = window.prompt(
            PromptLevel::Warning,
            "恢复默认设置",
            Some("是否恢复 Attack results 组的默认设置？"),
            &["否", "是"],
            cx,
        );
        let owner = self.owner_window;
        cx.spawn(async move |this, cx| {
            let accepted = response.await.ok() == Some(1);
            let _ = owner.update(cx, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.project != Some(project)
                        || !view
                            .intruder
                            .editors
                            .get(&id)
                            .is_some_and(|editor| editor.baseline_restore == Some(operation))
                    {
                        return;
                    }
                    view.intruder.editors.get_mut(&id).unwrap().baseline_restore = None;
                    if accepted
                        && view
                            .workspace
                            .intruder
                            .tabs
                            .iter()
                            .any(|tab| tab.id == id && tab.spec.revision == revision)
                    {
                        view.workspace.intruder.grep_configs.remove(&id);
                        view.refresh_intruder_grep_summaries(cx);
                        view.change_intruder_spec(
                            id,
                            |spec| spec.baseline_request = true,
                            window,
                            cx,
                        );
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn add_intruder_grep_match(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.grep_match_input.clone();
        let raw = match editing::input_snapshot(&input, window, cx) {
            Ok(raw) => raw.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if raw.is_empty() || raw.len() > 1024 {
            self.fail("Grep Match 表达式不能为空且不能超过 1024 字节。", cx);
            return;
        }
        let previous = self.workspace.intruder.grep_configs.get(&id).cloned();
        let config = self.workspace.intruder.grep_configs.entry(id).or_default();
        if config.match_expressions.len() >= 64 {
            self.fail("Grep Match 规则不能超过 64 条。", cx);
            return;
        }
        config.match_expressions.push(raw);
        if let Err(error) = self.workspace.validate_structure() {
            match previous {
                Some(old) => {
                    self.workspace.intruder.grep_configs.insert(id, old);
                }
                None => {
                    self.workspace.intruder.grep_configs.remove(&id);
                }
            }
            self.fail(format!("未添加 Grep Match 规则：{error}"), cx);
            return;
        }
        input.update(cx, |state, cx| state.set_value("", window, cx));
        self.refresh_intruder_grep_summaries(cx);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn add_intruder_grep_extract(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let start_input = editors.grep_extract_start.clone();
        let end_input = editors.grep_extract_end.clone();
        let snapshots = (|| {
            Ok::<_, &'static str>((
                editing::input_snapshot(&start_input, window, cx)?.to_string(),
                editing::input_snapshot(&end_input, window, cx)?.to_string(),
            ))
        })();
        let (start_delimiter, end_delimiter) = match snapshots {
            Ok(values) => values,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let is_regex = start_delimiter
            .strip_prefix("re:")
            .is_some_and(|pat| !pat.is_empty());
        if let Some(pattern) = start_delimiter.strip_prefix("re:") {
            if pattern.is_empty() || pattern.len() > 1024 {
                self.fail("Grep Extract 正则表达式不能为空且不能超过 1024 字节。", cx);
                return;
            }
            if let Err(err) = regex::bytes::RegexBuilder::new(pattern)
                .size_limit(1 << 20)
                .dfa_size_limit(1 << 20)
                .build()
            {
                self.fail(format!("Grep Extract 正则表达式无效：{err}"), cx);
                return;
            }
        }
        if start_delimiter.is_empty()
            || (!is_regex && end_delimiter.is_empty())
            || start_delimiter.len() > 1024
            || end_delimiter.len() > 1024
        {
            self.fail(
                "Grep Extract 起始和结束分隔符均不能为空且不能超过 1024 字节。",
                cx,
            );
            return;
        }
        let previous = self.workspace.intruder.grep_configs.get(&id).cloned();
        let config = self.workspace.intruder.grep_configs.entry(id).or_default();
        if config.extract_rules.len() >= 32 {
            self.fail("Grep Extract 规则不能超过 32 条。", cx);
            return;
        }
        let rule = if is_regex && end_delimiter.is_empty() {
            mio_core::IntruderGrepExtractRule::regex(start_delimiter)
        } else {
            mio_core::IntruderGrepExtractRule {
                start_delimiter,
                end_delimiter,
            }
        };
        config.extract_rules.push(rule);
        if let Err(error) = self.workspace.validate_structure() {
            match previous {
                Some(old) => {
                    self.workspace.intruder.grep_configs.insert(id, old);
                }
                None => {
                    self.workspace.intruder.grep_configs.remove(&id);
                }
            }
            self.fail(format!("未添加 Grep Extract 规则：{error}"), cx);
            return;
        }
        start_input.update(cx, |state, cx| state.set_value("", window, cx));
        end_input.update(cx, |state, cx| state.set_value("", window, cx));
        self.refresh_intruder_grep_summaries(cx);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn pick_intruder_grep_extract_delimiters(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let start_input = editors.grep_extract_start.clone();
        let end_input = editors.grep_extract_end.clone();
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let extract_surrounding =
            |full_text: &str, range: std::ops::Range<usize>| -> Option<(String, String)> {
                if range.is_empty()
                    || range.end > full_text.len()
                    || !full_text.is_char_boundary(range.start)
                    || !full_text.is_char_boundary(range.end)
                {
                    return None;
                }
                let before = &full_text[..range.start];
                let after = &full_text[range.end..];
                let line_before = before.rsplit(['\r', '\n']).next().unwrap_or(before);
                let line_after = after.split(['\r', '\n']).next().unwrap_or(after);
                let start_chars: Vec<char> = line_before.chars().collect();
                let start_skip = start_chars.len().saturating_sub(16);
                let picked_start: String = start_chars[start_skip..].iter().collect();
                let picked_end: String = line_after.chars().take(16).collect();
                if picked_start.is_empty() || picked_end.is_empty() {
                    return None;
                }
                Some((picked_start, picked_end))
            };
        let mut resolved: Option<(String, String)> = None;
        for view in self.intruder.results.views.values() {
            if view.summary.as_ref().is_some_and(|run| run.tab_id == id) {
                for msg_editor in [&view.response, &view.request] {
                    let state = msg_editor.state().read(cx);
                    let sel = state.selected_range();
                    if !sel.is_empty() {
                        let text = state.text().to_string();
                        if let Some(pair) = extract_surrounding(&text, sel) {
                            resolved = Some(pair);
                            break;
                        }
                    }
                }
                if resolved.is_some() {
                    break;
                }
            }
        }
        if resolved.is_none() {
            let req_state = editors.request.state().read(cx);
            let sel = req_state.selected_range();
            if !sel.is_empty() {
                let content = req_state.content();
                let mut plain = String::new();
                let mut unmarked_start = sel.start;
                let mut unmarked_end = sel.end;
                let mut cursor = 0usize;
                for span in content.tokens() {
                    if span.token().text().as_ref() == "§" {
                        plain.push_str(&content.text()[cursor..span.range().start]);
                        if span.range().end <= sel.start {
                            unmarked_start = unmarked_start.saturating_sub(span.range().len());
                        }
                        if span.range().end <= sel.end {
                            unmarked_end = unmarked_end.saturating_sub(span.range().len());
                        }
                        cursor = span.range().end;
                    }
                }
                plain.push_str(&content.text()[cursor..]);
                resolved = extract_surrounding(&plain, unmarked_start..unmarked_end);
            }
        }
        let Some((picked_start, picked_end)) = resolved else {
            self.fail(
                "请先在请求或响应报文中选中目标字段（且同一行选区前后均存在非空起止分隔符）。",
                cx,
            );
            return;
        };
        start_input.update(cx, |state, cx| state.set_value(picked_start, window, cx));
        end_input.update(cx, |state, cx| state.set_value(picked_end, window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn apply_intruder_retry_inputs(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .workspace
            .intruder
            .attack_settings
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let retries_input = super::settings::ensure_attack_retries_input(
            id,
            current.network_failure_retries,
            window,
            cx,
        );
        let pause_input = super::settings::ensure_attack_retry_pause_input(
            id,
            current.retry_pause_ms,
            window,
            cx,
        );
        let snapshots = (|| {
            Ok::<_, &'static str>((
                editing::input_snapshot(&retries_input, window, cx)?.to_string(),
                editing::input_snapshot(&pause_input, window, cx)?.to_string(),
            ))
        })();
        let (retries_raw, pause_raw) = match snapshots {
            Ok(pair) => pair,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let Ok(retries) = retries_raw.trim().parse::<u8>() else {
            self.fail("网络失败重试次数必须为 0–10 之间的整数。", cx);
            return;
        };
        if retries > 10 {
            self.fail("网络失败重试次数不能超过 10 次。", cx);
            return;
        }
        let Ok(pause_ms) = pause_raw.trim().parse::<u64>() else {
            self.fail("重试暂停时间必须为非负整数毫秒值。", cx);
            return;
        };
        if pause_ms > 60_000 {
            self.fail("重试暂停时间不能超过 60000 ms。", cx);
            return;
        }
        self.update_intruder_attack_settings(
            id,
            |s| {
                s.network_failure_retries = retries;
                s.retry_pause_ms = pause_ms;
            },
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn update_intruder_attack_settings(
        &mut self,
        id: TabId,
        mutate: impl FnOnce(&mut mio_core::IntruderAttackSettings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let mut next = self
            .workspace
            .intruder
            .attack_settings
            .get(&id)
            .cloned()
            .unwrap_or_default();
        mutate(&mut next);
        let next_retries = next.network_failure_retries;
        let next_pause = next.retry_pause_ms;
        if next.is_default() {
            self.workspace.intruder.attack_settings.remove(&id);
        } else {
            self.workspace.intruder.attack_settings.insert(id, next);
        }
        if let Some(inputs) = cx.try_global::<super::settings::IntruderAttackSettingsInputs>() {
            let r_in = inputs.retries.get(&id).cloned();
            let p_in = inputs.retry_pause.get(&id).cloned();
            if let Some(r_in) = r_in {
                r_in.update(cx, |state, cx| {
                    state.set_value(next_retries.to_string(), window, cx)
                });
            }
            if let Some(p_in) = p_in {
                p_in.update(cx, |state, cx| {
                    state.set_value(next_pause.to_string(), window, cx)
                });
            }
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn search_intruder(
        &mut self,
        id: TabId,
        next: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        let Ok(query) = editing::input_snapshot(&editors.search, window, cx) else {
            return;
        };
        let case_insensitive = !editors.request.search_case_sensitive;
        let regex = editors.request.search_regex;
        let mut search_err = None;
        editors.request.state().update(cx, |state, cx| {
            match state.set_search_regex(query.to_string(), case_insensitive, regex, cx) {
                Ok(()) => match next {
                    Some(true) => {
                        state.next_search_match(cx);
                    }
                    Some(false) => {
                        state.previous_search_match(cx);
                    }
                    None => {}
                },
                Err(err) => {
                    search_err = Some(err);
                }
            }
        });
        editors.request.search_error = search_err;
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_intruder_search_case(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.request.search_case_sensitive = !editors.request.search_case_sensitive;
            self.search_intruder(id, None, window, cx);
        }
    }

    pub(in crate::workbench) fn toggle_intruder_search_regex(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.request.search_regex = !editors.request.search_regex;
            self.search_intruder(id, None, window, cx);
        }
    }

    pub(in crate::workbench) fn toggle_intruder_soft_wrap(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.request.toggle_soft_wrap(window, cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_intruder_protocol(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || self.intruder.positions_request.is_some()
            || self.intruder.starts.contains_key(&id)
        {
            return;
        }
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let snapshot = match editors.request.snapshot(window, cx) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let Some(current_version) = self
            .workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.spec.draft.version)
        else {
            return;
        };
        let next_version = match current_version {
            HttpVersion::Http2 => HttpVersion::Http11,
            HttpVersion::Http10 | HttpVersion::Http11 => HttpVersion::Http2,
        };
        let (raw, mut positions) = snapshot;
        let first_len = raw
            .split_inclusive(|byte| *byte == b'\n')
            .next()
            .map_or(0, <[u8]>::len);
        let next_raw = mio_core::rewrite_request_line_version(&raw, next_version);
        let next_first_len = next_raw
            .split_inclusive(|byte| *byte == b'\n')
            .next()
            .map_or(0, <[u8]>::len);
        if next_raw != raw {
            let line_end_no_eol = raw[..first_len]
                .strip_suffix(b"\r\n")
                .or_else(|| raw[..first_len].strip_suffix(b"\n"))
                .map_or(first_len, <[u8]>::len);
            let ver_start = raw[..line_end_no_eol]
                .iter()
                .rposition(|&b| b == b' ')
                .map_or(line_end_no_eol, |idx| idx + 1);
            if positions
                .iter()
                .any(|pos| pos.start < line_end_no_eol && pos.end > ver_start)
            {
                self.fail(
                    "请求行协议版本处包含 § 载荷标记；请先清除该位置标记后再切换协议版本。",
                    cx,
                );
                return;
            }
            let delta = next_first_len as isize - first_len as isize;
            for pos in &mut positions {
                if pos.start >= line_end_no_eol {
                    pos.start = pos.start.saturating_add_signed(delta);
                    pos.end = pos.end.saturating_add_signed(delta);
                }
            }
        }
        let prev_view = editors.request.view;
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.request.load(&next_raw, &positions, window, cx);
            if prev_view != editors.request.view {
                let _ = editors.request.switch(prev_view, window, cx);
            }
            editors.hex_length = editors.request.byte_length(cx);
            editors.dirty = true;
        }
        self.change_intruder_spec(
            id,
            |spec| {
                spec.draft.version = next_version;
            },
            window,
            cx,
        );
    }
}
