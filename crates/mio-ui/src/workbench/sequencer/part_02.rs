use super::*;

fn escape_sequencer_regex_literal(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

impl Workbench {
    pub(in crate::workbench) fn sync_selected_sequencer_request(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(seq) = self.sequencer_pane.selected_request_seq else {
            return;
        };
        let target = self
            .sequencer_pane
            .target_url
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let Ok(raw) = self.sequencer_pane.request.bytes(window, cx) else {
            return;
        };
        let version = self.sequencer_pane.request_version;
        let host = extract_live_request_host(&target, &raw);
        let request_summary = extract_live_request_summary(&raw);
        if let Some(item) = self
            .sequencer_pane
            .live_requests
            .iter_mut()
            .find(|item| item.seq == seq)
        {
            item.target = target;
            item.host = host;
            item.request_summary = request_summary;
            item.raw = raw;
            item.version = version;
        }
    }

    pub(in crate::workbench) fn select_sequencer_live_request(
        &mut self,
        seq: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sequencer_pane.running.is_some()
            || self.sequencer_pane.selected_request_seq == Some(seq)
        {
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        self.sync_selected_sequencer_request(window, cx);
        let Some((target, version, raw)) = self
            .sequencer_pane
            .live_requests
            .iter()
            .find(|item| item.seq == seq)
            .map(|item| (item.target.clone(), item.version, item.raw.clone()))
        else {
            return;
        };
        self.sequencer_pane.selected_request_seq = Some(seq);
        self.sequencer_pane.request_version = version;
        self.sequencer_pane
            .target_url
            .update(cx, |state, cx| state.set_value(target, window, cx));
        self.sequencer_pane.request.load(&raw, window, cx);
        cx.notify();
    }

    fn sequencer_live_request_draft(
        &self,
        seq: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Draft, String> {
        let Some(item) = self
            .sequencer_pane
            .live_requests
            .iter()
            .find(|item| item.seq == seq)
        else {
            return Err("所选 Sequencer 候选请求不存在。".into());
        };
        if self.sequencer_pane.selected_request_seq == Some(seq) {
            let target = editing::input_snapshot(&self.sequencer_pane.target_url, window, cx)
                .map_err(str::to_owned)?
                .trim()
                .to_owned();
            let raw = self.sequencer_pane.request.bytes(window, cx)?;
            Ok(Draft {
                revision: 1,
                target: if target.is_empty() {
                    item.target.clone()
                } else {
                    target
                },
                raw,
                version: self.sequencer_pane.request_version,
            })
        } else {
            Ok(Draft {
                revision: 1,
                target: item.target.clone(),
                raw: item.raw.clone(),
                version: item.version,
            })
        }
    }

    pub(in crate::workbench) fn set_sequencer_target_request(
        &mut self,
        seq: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sequencer_pane.running.is_some() {
            return;
        }
        if self.sequencer_pane.selected_request_seq != Some(seq) {
            self.select_sequencer_live_request(seq, window, cx);
        }
        if self.sequencer_pane.selected_request_seq == Some(seq) {
            self.sequencer_pane.status_text =
                Some(format!("已将候选请求 #{seq} 设为实时采样目标请求。"));
            cx.notify();
        }
    }

    pub(in crate::workbench) fn send_sequencer_live_request_to_repeater(
        &mut self,
        seq: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() {
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        let draft = match self.sequencer_live_request_draft(seq, window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        let id = Uuid::new_v4();
        let previous = self.workspace.active;
        let title = self.next_repeater_tab_title(None, Some(&draft.raw));
        self.workspace.tabs.push(WorkTab {
            id,
            title,
            group: None,
            draft,
            last_flow: None,
            archive_response: None,
            history: None,
        });
        self.workspace.active = Some(id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.pop();
            self.workspace.active = previous;
            self.fail(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        self.tab_editors[&id]
            .request
            .state()
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn send_sequencer_live_request_to_intruder(
        &mut self,
        seq: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() {
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        let draft = match self.sequencer_live_request_draft(seq, window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.open_intruder_draft(draft, window, cx);
    }

    pub(in crate::workbench) fn copy_sequencer_live_request_url(
        &mut self,
        seq: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = match self.sequencer_live_request_draft(seq, window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let url = mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
            .unwrap_or_else(|_| draft.target.clone());
        if url.trim().is_empty() {
            self.fail("所选候选请求缺少有效 URL。", cx);
            return;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url));
        self.error = None;
        self.status = "已复制 URL 到剪贴板。".into();
        self.sequencer_pane.status_text = Some("已复制请求 URL 到剪贴板。".into());
        cx.notify();
    }

    pub(in crate::workbench) fn remove_selected_sequencer_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        let Some(seq) = self.sequencer_pane.selected_request_seq else {
            return;
        };
        let Some(idx) = self
            .sequencer_pane
            .live_requests
            .iter()
            .position(|item| item.seq == seq)
        else {
            self.sequencer_pane.selected_request_seq = None;
            cx.notify();
            return;
        };
        self.sequencer_pane.live_requests.remove(idx);
        if self.sequencer_pane.live_requests.is_empty() {
            self.sequencer_pane.selected_request_seq = None;
            self.sequencer_pane.request_drawer_open = false;
        } else {
            let next_idx = idx.min(self.sequencer_pane.live_requests.len() - 1);
            let (next_seq, target, version, raw) = {
                let item = &self.sequencer_pane.live_requests[next_idx];
                (item.seq, item.target.clone(), item.version, item.raw.clone())
            };
            self.sequencer_pane.selected_request_seq = Some(next_seq);
            self.sequencer_pane.request_version = version;
            self.sequencer_pane
                .target_url
                .update(cx, |state, cx| state.set_value(target, window, cx));
            self.sequencer_pane.request.load(&raw, window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn clear_sequencer_requests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        self.sequencer_pane.live_requests.clear();
        self.sequencer_pane.selected_request_seq = None;
        self.sequencer_pane.request_drawer_open = false;
        cx.notify();
    }

    pub(in crate::workbench) fn add_current_sequencer_request_to_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        let target = match editing::input_snapshot(&self.sequencer_pane.target_url, window, cx) {
            Ok(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    DEFAULT_TARGET_URL.to_owned()
                } else {
                    trimmed.to_owned()
                }
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let raw = match self.sequencer_pane.request.bytes(window, cx) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let version = self.sequencer_pane.request_version;
        let seq = self.sequencer_pane.next_request_seq;
        self.sequencer_pane.next_request_seq = seq.saturating_add(1);
        let host = extract_live_request_host(&target, &raw);
        let request_summary = extract_live_request_summary(&raw);
        self.sequencer_pane
            .live_requests
            .push(LiveCaptureRequestItem {
                seq,
                target,
                host,
                request_summary,
                raw,
                version,
            });
        self.sequencer_pane.selected_request_seq = Some(seq);
        cx.notify();
    }

    pub(in crate::workbench) fn send_draft_to_sequencer(
        &mut self,
        draft: Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            self.fail("正在切换或保存项目；未转送请求到 Sequencer。", cx);
            return;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return;
        }
        self.sync_selected_sequencer_request(window, cx);
        let version = match draft.version {
            HttpVersion::Http2 => HttpVersion::Http2,
            HttpVersion::Http10 | HttpVersion::Http11 => HttpVersion::Http11,
        };
        let target = if draft.target.trim().is_empty() {
            DEFAULT_TARGET_URL.to_owned()
        } else {
            draft.target
        };
        let seq = self.sequencer_pane.next_request_seq;
        self.sequencer_pane.next_request_seq = seq.saturating_add(1);
        let host = extract_live_request_host(&target, &draft.raw);
        let request_summary = extract_live_request_summary(&draft.raw);
        self.sequencer_pane
            .live_requests
            .push(LiveCaptureRequestItem {
                seq,
                target: target.clone(),
                host,
                request_summary,
                raw: draft.raw.clone(),
                version,
            });
        self.sequencer_pane.selected_request_seq = Some(seq);
        self.sequencer_pane.request_drawer_open = true;
        self.sequencer_pane.request_version = version;
        self.sequencer_pane
            .target_url
            .update(cx, |state, cx| state.set_value(target, window, cx));
        self.sequencer_pane.request.load(&draft.raw, window, cx);
        self.sequencer_pane.page = SequencerPage::Live;
        self.sequencer_pane.status_text =
            Some("已将请求草稿载入 Sequencer 实时采样源，尚未发送请求。".into());
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Sequencer;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn on_sequencer_event(
        &mut self,
        event: SequencerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(event.project()) {
            return;
        }
        match event {
            SequencerEvent::CaptureStarted {
                project,
                run_id,
                target_samples,
            } => {
                if self
                    .sequencer_pane
                    .running
                    .as_ref()
                    .is_some_and(|(p, r, _)| *p == project && *r == run_id)
                {
                    self.sequencer_pane.capture = CaptureSnapshot {
                        target_samples: target_samples.max(MIN_SAMPLE_COUNT),
                        ..CaptureSnapshot::default()
                    };
                    self.sequencer_pane.captured_samples.clear();
                    self.sequencer_pane.sync_preview(window, cx);
                    self.sequencer_pane.status_text =
                        Some(format!("实时采样进行中（目标 {target_samples} 条）…"));
                }
            }
            SequencerEvent::CaptureProgress {
                project,
                run_id,
                requests_sent,
                tokens_collected,
                extraction_misses,
                request_errors,
                latest_samples,
            } => {
                if self
                    .sequencer_pane
                    .running
                    .as_ref()
                    .is_some_and(|(p, r, _)| *p == project && *r == run_id)
                {
                    self.sequencer_pane.capture.requests_sent = requests_sent;
                    self.sequencer_pane.capture.tokens_collected = tokens_collected;
                    self.sequencer_pane.capture.extraction_misses = extraction_misses;
                    self.sequencer_pane.capture.request_errors = request_errors;
                    self.sequencer_pane.capture.latest_samples = latest_samples;
                    self.sequencer_pane.sync_preview(window, cx);
                }
            }
            SequencerEvent::CaptureFinished {
                project,
                run_id,
                requests_sent,
                tokens_collected,
                extraction_misses,
                request_errors,
                samples,
                stopped_early,
                error,
            } => {
                let matches_run = self
                    .sequencer_pane
                    .running
                    .as_ref()
                    .is_some_and(|(p, r, _)| *p == project && *r == run_id);
                if !matches_run {
                    return;
                }
                self.sequencer_pane.running = None;
                let preview_tail: Vec<String> = samples
                    .iter()
                    .skip(samples.len().saturating_sub(MAX_PREVIEW_SAMPLES))
                    .cloned()
                    .collect();
                self.sequencer_pane.capture.requests_sent = requests_sent;
                self.sequencer_pane.capture.tokens_collected = tokens_collected;
                self.sequencer_pane.capture.extraction_misses = extraction_misses;
                self.sequencer_pane.capture.request_errors = request_errors;
                self.sequencer_pane.capture.latest_samples = preview_tail;
                self.sequencer_pane.captured_samples = samples;
                self.sequencer_pane.sync_preview(window, cx);
                match error {
                    Some(err) => {
                        self.sequencer_pane.status_text = Some(format!(
                            "采样结束：{err}（已提取 {} 条样本）",
                            self.sequencer_pane.captured_samples.len()
                        ));
                        self.fail(err, cx);
                    }
                    None if stopped_early => {
                        self.sequencer_pane.status_text = Some(format!(
                            "实时采样已停止，共提取 {} 条样本。",
                            self.sequencer_pane.captured_samples.len()
                        ));
                    }
                    None => {
                        self.sequencer_pane.status_text = Some(format!(
                            "实时采样完成，共提取 {} 条样本。",
                            self.sequencer_pane.captured_samples.len()
                        ));
                    }
                }
            }
            SequencerEvent::AnalysisReady {
                project,
                operation_id,
                result,
            } => {
                let matches_op = self
                    .sequencer_pane
                    .pending_analysis
                    .as_ref()
                    .is_some_and(|(p, op, _)| *p == project && *op == operation_id);
                if !matches_op {
                    return;
                }
                self.sequencer_pane.pending_analysis = None;
                match result {
                    Ok(analysis) => {
                        self.sequencer_pane.status_text = Some(format!(
                            "统计分析已完成（{} 条样本，有效熵 {:.2} bits）。",
                            analysis.sample_count, analysis.effective_entropy_bits
                        ));
                        self.sequencer_pane.analysis = Some(std::rc::Rc::from(analysis));
                        self.sequencer_pane.analysis_error = None;
                        self.sequencer_pane.page = SequencerPage::Analysis;
                    }
                    Err(err) => {
                        self.sequencer_pane.analysis_error = Some(err.clone());
                        self.fail(err, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn sequencer_can_operate(&self) -> bool {
        self.project.is_some() && self.backend_available() && self.transition.is_none()
    }

    pub(in crate::workbench) fn snapshot_sequencer_location(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SequencerTokenLocation, String> {
        let pane = &self.sequencer_pane;
        match pane.location_kind {
            LocationKind::Cookie => {
                let name = editing::input_snapshot(&pane.cookie_name, window, cx)
                    .map_err(str::to_owned)?
                    .trim()
                    .to_owned();
                if name.is_empty() {
                    return Err("请输入要提取的 Cookie 名称。".into());
                }
                if name.len() > MAX_LOCATION_PARAM_BYTES {
                    return Err("Cookie 名称不能超过 512 字节；输入已保留。".into());
                }
                Ok(SequencerTokenLocation::Cookie { name })
            }
            LocationKind::FormField => {
                let name = editing::input_snapshot(&pane.form_field_name, window, cx)
                    .map_err(str::to_owned)?
                    .trim()
                    .to_owned();
                if name.is_empty() {
                    return Err("请输入要提取的表单字段名称。".into());
                }
                if name.len() > MAX_LOCATION_PARAM_BYTES {
                    return Err("表单字段名称不能超过 512 字节；输入已保留。".into());
                }
                let escaped = escape_sequencer_regex_literal(&name);
                let pattern = format!(
                    r#"(?is)<input\b[^>]*\bname\s*=\s*["']?{escaped}["']?[^>]*\bvalue\s*=\s*["']([^"'<>]+)["']"#
                );
                Ok(SequencerTokenLocation::Regex { pattern })
            }
            LocationKind::Header => {
                let name = editing::input_snapshot(&pane.header_name, window, cx)
                    .map_err(str::to_owned)?
                    .trim()
                    .to_owned();
                if name.is_empty() {
                    return Err("请输入要提取的响应 Header 名称。".into());
                }
                if name.len() > MAX_LOCATION_PARAM_BYTES {
                    return Err("Header 名称不能超过 512 字节；输入已保留。".into());
                }
                Ok(SequencerTokenLocation::Header { name })
            }
            LocationKind::BodyDelimiter => {
                let start = editing::input_snapshot(&pane.delim_start, window, cx)
                    .map_err(str::to_owned)?
                    .to_string();
                let end = editing::input_snapshot(&pane.delim_end, window, cx)
                    .map_err(str::to_owned)?
                    .to_string();
                if start.is_empty() || end.is_empty() {
                    return Err("请输入非空的响应体起始分隔符与结束分隔符。".into());
                }
                if start.len() > MAX_LOCATION_PARAM_BYTES || end.len() > MAX_LOCATION_PARAM_BYTES {
                    return Err("响应体起止分隔符长度不能超过 512 字节；输入已保留。".into());
                }
                Ok(SequencerTokenLocation::BodyDelimiter { start, end })
            }
            LocationKind::Regex => {
                let pattern = editing::input_snapshot(&pane.regex_pattern, window, cx)
                    .map_err(str::to_owned)?
                    .trim()
                    .to_owned();
                if pattern.is_empty() {
                    return Err("请输入用于提取 Token 的正则表达式。".into());
                }
                if pattern.len() > MAX_LOCATION_PARAM_BYTES {
                    return Err("正则表达式长度不能超过 512 字节；输入已保留。".into());
                }
                Ok(SequencerTokenLocation::Regex { pattern })
            }
        }
    }

    pub(in crate::workbench) fn snapshot_sequencer_capture_config(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SequencerCaptureConfig, String> {
        let pane = &self.sequencer_pane;
        let raw = pane.request.bytes(window, cx)?;
        let target = editing::input_snapshot(&pane.target_url, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let location = self.snapshot_sequencer_location(window, cx)?;
        let samples_text =
            editing::input_snapshot(&pane.target_samples, window, cx).map_err(str::to_owned)?;
        let concurrency_text =
            editing::input_snapshot(&pane.concurrency, window, cx).map_err(str::to_owned)?;
        let _ = editing::input_snapshot(&pane.request.search, window, cx).map_err(str::to_owned)?;

        if target.is_empty() {
            return Err("请输入采样源目标 URL（http:// 或 https://）。".into());
        }
        if target.len() > MAX_TARGET_URL_BYTES {
            return Err("采样源目标 URL 不能超过 4096 字节；输入已保留。".into());
        }
        let target_samples: usize = samples_text.trim().parse().map_err(|_| {
            format!("目标样本数须为 {MIN_SAMPLE_COUNT}–{MAX_SAMPLE_COUNT} 的整数。")
        })?;
        if !(MIN_SAMPLE_COUNT..=MAX_SAMPLE_COUNT).contains(&target_samples) {
            return Err(format!(
                "目标样本数须在 {MIN_SAMPLE_COUNT}–{MAX_SAMPLE_COUNT} 之间。"
            ));
        }
        let concurrency: usize = concurrency_text
            .trim()
            .parse()
            .map_err(|_| format!("并发请求数须为 1–{MAX_SEQUENCER_CONCURRENCY} 的整数。"))?;
        if !(1..=MAX_SEQUENCER_CONCURRENCY).contains(&concurrency) {
            return Err(format!(
                "并发请求数须在 1–{MAX_SEQUENCER_CONCURRENCY} 之间。"
            ));
        }
        parse_repeater_request(
            &raw,
            &target,
            pane.request_version,
            self.workspace.repeater_settings.update_content_length,
        )
        .map_err(|err| format!("采样源 HTTP 请求无效：{err}"))?;

        Ok(SequencerCaptureConfig {
            draft: Draft {
                revision: 1,
                target,
                raw,
                version: pane.request_version,
            },
            location,
            target_samples,
            concurrency,
        })
    }

    pub(in crate::workbench) fn start_sequencer_capture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let config = match self.snapshot_sequencer_capture_config(window, cx) {
            Ok(config) => config,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let target_samples = config.target_samples;
        let captured_target = config.draft.target.clone();
        let captured_raw = config.draft.raw.clone();
        let captured_version = config.draft.version;
        let run_id = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        if self.enqueue(
            Command::Sequencer(SequencerCommand::StartLiveCapture {
                project,
                run_id,
                config: Box::new(config),
                cancel: cancel.clone(),
            }),
            cx,
        ) {
            let host = extract_live_request_host(&captured_target, &captured_raw);
            let request_summary = extract_live_request_summary(&captured_raw);
            if let Some(seq) = self.sequencer_pane.selected_request_seq {
                if let Some(item) = self
                    .sequencer_pane
                    .live_requests
                    .iter_mut()
                    .find(|item| item.seq == seq)
                {
                    item.target = captured_target;
                    item.host = host;
                    item.request_summary = request_summary;
                    item.raw = captured_raw;
                    item.version = captured_version;
                }
            } else {
                let seq = self.sequencer_pane.next_request_seq;
                self.sequencer_pane.next_request_seq = seq.saturating_add(1);
                self.sequencer_pane
                    .live_requests
                    .push(LiveCaptureRequestItem {
                        seq,
                        target: captured_target,
                        host,
                        request_summary,
                        raw: captured_raw,
                        version: captured_version,
                    });
                self.sequencer_pane.selected_request_seq = Some(seq);
            }
            self.sequencer_pane.running = Some((project, run_id, cancel));
            self.sequencer_pane.capture_panel_open = true;
            self.sequencer_pane.capture = CaptureSnapshot {
                target_samples,
                ..CaptureSnapshot::default()
            };
            self.sequencer_pane.captured_samples.clear();
            self.sequencer_pane.sync_preview(window, cx);
            self.sequencer_pane.status_text =
                Some(format!("已提交实时采样请求（目标 {target_samples} 条）…"));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn stop_sequencer_capture(&mut self, cx: &mut Context<Self>) {
        let Some((project, run_id, cancel)) = &self.sequencer_pane.running else {
            return;
        };
        let project = *project;
        let run_id = *run_id;
        cancel.cancel();
        if self.backend_available() && self.project == Some(project) {
            let _ = self.enqueue(
                Command::Sequencer(SequencerCommand::StopLiveCapture { project, run_id }),
                cx,
            );
        }
        self.sequencer_pane.status_text = Some("正在停止实时采样，等待后台回收在途请求…".into());
        cx.notify();
    }

    pub(in crate::workbench) fn enqueue_sequencer_analysis(
        &mut self,
        samples: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.sequencer_pane.pending_analysis.is_some() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let samples =
            match prepare_samples_for_analysis(samples, self.sequencer_pane.base64_predecode) {
                Ok(prepared) => prepared,
                Err(err) => {
                    self.sequencer_pane.analysis_error = Some(err.clone());
                    self.fail(err, cx);
                    return;
                }
            };
        let count = samples.len();
        let operation_id = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        if self.enqueue(
            Command::Sequencer(SequencerCommand::AnalyzeSamples {
                project,
                operation_id,
                samples,
                cancel: cancel.clone(),
            }),
            cx,
        ) {
            self.sequencer_pane.pending_analysis = Some((project, operation_id, cancel));
            self.sequencer_pane.analysis_error = None;
            self.sequencer_pane.status_text = Some(format!("正在对 {count} 条样本执行统计分析…"));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn analyze_captured_sequencer_samples(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sequencer_pane.running.is_some()
            || self.sequencer_pane.captured_samples.len() < MIN_SAMPLE_COUNT
        {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let samples = self.sequencer_pane.captured_samples.clone();
        self.enqueue_sequencer_analysis(samples, cx);
    }

    pub(in crate::workbench) fn paste_sequencer_manual_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = match editing::snapshot(&self.sequencer_pane.manual_editor, window, cx) {
            Ok(text) => text.to_string(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.fail("剪贴板中没有可粘贴的文本样本。", cx);
            return;
        };
        if pasted.trim_matches(['\r', '\n']).is_empty() {
            self.fail("剪贴板文本为空，未修改当前样本列表。", cx);
            return;
        }
        let combined = if existing.trim_matches(['\r', '\n']).is_empty() {
            pasted
        } else if existing.ends_with('\n') {
            format!("{existing}{pasted}")
        } else {
            format!("{existing}\n{pasted}")
        };
        if combined.len() > MAX_TOTAL_SAMPLE_BYTES + MAX_SAMPLE_COUNT * 2 {
            self.fail("粘贴后文本超过 8 MiB 样本上限，未修改当前内容。", cx);
            return;
        }
        self.sequencer_pane
            .manual_editor
            .update(cx, |state, cx| state.set_value(combined, window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn clear_sequencer_manual_samples(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = editing::snapshot(&self.sequencer_pane.manual_editor, window, cx) {
            self.fail(err, cx);
            return;
        }
        self.sequencer_pane
            .manual_editor
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }
}
