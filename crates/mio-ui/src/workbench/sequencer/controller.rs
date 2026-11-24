use gpui_kit::ClipboardItem;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn reset_sequencer_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = &mut self.sequencer_pane;
        if let Some((_, _, cancel)) = pane.running.take() {
            cancel.cancel();
        }
        if let Some((_, _, cancel)) = pane.pending_analysis.take() {
            cancel.cancel();
        }
        pane.page = SequencerPage::Live;
        pane.live_requests.clear();
        pane.next_request_seq = 1;
        pane.selected_request_seq = None;
        pane.request_version = HttpVersion::Http11;
        pane.location_kind = LocationKind::BodyDelimiter;
        pane.request.load(DEFAULT_RAW_REQUEST, window, cx);
        pane.target_url
            .update(cx, |state, cx| state.set_value(DEFAULT_TARGET_URL, window, cx));
        for editor in [&pane.preview_samples, &pane.manual_editor] {
            editor.update(cx, |state, cx| state.set_value("", window, cx));
        }
        pane.capture = CaptureSnapshot {
            target_samples: DEFAULT_SEQUENCER_TARGET_SAMPLES,
            ..CaptureSnapshot::default()
        };
        pane.captured_samples.clear();
        pane.base64_predecode = false;
        pane.status_text = None;
        pane.analysis = None;
        pane.analysis_error = None;
        self.sequencer_view
            .update(cx, |view, cx| view.reset(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn sequencer_backend_disconnected(&mut self, cx: &mut Context<Self>) {
        let pane = &mut self.sequencer_pane;
        let had_running = pane
            .running
            .take()
            .map(|(_, _, cancel)| cancel.cancel())
            .is_some();
        let had_analysis = pane
            .pending_analysis
            .take()
            .map(|(_, _, cancel)| cancel.cancel())
            .is_some();
        if had_running || had_analysis {
            pane.status_text = Some("后台服务已断开；采样或统计分析结果未确认。".into());
        }
        self.sequencer_view.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub(in crate::workbench) fn validate_sequencer_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        self.sequencer_pane.validate_composition(window, cx)
    }

    pub(in crate::workbench) fn active_sequencer_message_editor(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<&mut MessageEditor> {
        if self.sequencer_pane.page != SequencerPage::Live
            || self.sequencer_pane.has_non_request_focus(window, cx)
        {
            return None;
        }
        Some(&mut self.sequencer_pane.request)
    }

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
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.error = None;
        self.status = "已复制 URL 到剪贴板。".into();
        self.sequencer_pane.status_text = Some("已复制请求 URL 到剪贴板。".into());
        cx.notify();
    }

    pub(super) fn remove_selected_sequencer_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return false;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return false;
        }
        let Some(seq) = self.sequencer_pane.selected_request_seq else {
            return false;
        };
        let Some(idx) = self
            .sequencer_pane
            .live_requests
            .iter()
            .position(|item| item.seq == seq)
        else {
            self.sequencer_pane.selected_request_seq = None;
            cx.notify();
            return false;
        };
        self.sequencer_pane.live_requests.remove(idx);
        let now_empty = self.sequencer_pane.live_requests.is_empty();
        if now_empty {
            self.sequencer_pane.selected_request_seq = None;
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
        now_empty
    }

    pub(super) fn clear_sequencer_requests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return false;
        }
        if let Err(error) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(error, cx);
            return false;
        }
        self.sequencer_pane.live_requests.clear();
        self.sequencer_pane.selected_request_seq = None;
        cx.notify();
        true
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
        self.sequencer_pane.request_version = version;
        self.sequencer_pane
            .target_url
            .update(cx, |state, cx| state.set_value(target, window, cx));
        self.sequencer_pane.request.load(&draft.raw, window, cx);
        self.sequencer_pane.page = SequencerPage::Live;
        self.sequencer_pane.status_text =
            Some("已将请求草稿载入 Sequencer 实时采样源，尚未发送请求。".into());
        self.sequencer_view.update(cx, |view, cx| {
            view.request_drawer_open = true;
            cx.notify();
        });
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
                        self.sequencer_pane.analysis = Some(Rc::from(analysis));
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
        self.sequencer_view.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub(in crate::workbench) fn sequencer_can_operate(&self) -> bool {
        self.project.is_some() && self.backend_available() && self.transition.is_none()
    }

    pub(super) fn start_sequencer_capture_with(
        &mut self,
        location: SequencerTokenLocation,
        samples_text: &str,
        concurrency_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sequencer_can_operate() || self.sequencer_pane.running.is_some() {
            return false;
        }
        let Some(project) = self.project else {
            return false;
        };
        let config = match self.build_sequencer_capture_config(
            location,
            samples_text,
            concurrency_text,
            window,
            cx,
        ) {
            Ok(config) => config,
            Err(err) => {
                self.fail(err, cx);
                return false;
            }
        };
        let target_samples = config.target_samples;
        let captured_target = config.draft.target.clone();
        let captured_raw = config.draft.raw.clone();
        let captured_version = config.draft.version;
        let run_id = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        let started = self.enqueue(
            Command::Sequencer(SequencerCommand::StartLiveCapture {
                project,
                run_id,
                config: Box::new(config),
                cancel: cancel.clone(),
            }),
            cx,
        );
        if started {
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
        started
    }

    fn build_sequencer_capture_config(
        &self,
        location: SequencerTokenLocation,
        samples_text: &str,
        concurrency_text: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SequencerCaptureConfig, String> {
        let pane = &self.sequencer_pane;
        let raw = pane.request.bytes(window, cx)?;
        let target = editing::input_snapshot(&pane.target_url, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
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

    pub(super) fn extract_sequencer_selection_delimiters(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, String)> {
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return None;
        }
        let editor = self.sequencer_pane.request.state();
        let range = editor.read(cx).selected_range();
        let full = editor.read(cx).value();
        if range.is_empty() || range.end > full.len() {
            self.fail("请先在采样源请求编辑器中选中目标位置两侧上下文中间的文本。", cx);
            return None;
        }
        let before = full[..range.start].rsplit(['\r', '\n']).next().unwrap_or("");
        let after = full[range.end..].split(['\r', '\n']).next().unwrap_or("");
        let start: String = before
            .chars()
            .rev()
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let end: String = after.chars().take(24).collect();
        if start.is_empty() || end.is_empty() {
            self.fail("当前选区前后缺少非空同行的起止分隔上下文。", cx);
            return None;
        }
        self.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
        self.error = None;
        self.sequencer_pane.status_text = Some("已根据请求选区自动生成起止分隔符。".into());
        Some((start, end))
    }

    pub(in crate::workbench) fn import_sequencer_samples_from_file(
        &mut self,
        to_manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate()
            || self.sequencer_pane.running.is_some()
            || self.sequencer_pane.pending_analysis.is_some()
            || self.file_picker_pending()
        {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("导入 Sequencer 样本文件".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let pending_path = this
                .update_in(cx, |view, _, cx| {
                    if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                        return None;
                    }
                    match paths {
                        Ok(Ok(Some(paths))) => paths.into_iter().next(),
                        Ok(Ok(None)) => None,
                        Ok(Err(err)) => {
                            view.fail(format!("选择样本文件失败：{err}"), cx);
                            None
                        }
                        Err(_) => {
                            view.fail("文件选择器已关闭。", cx);
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(path) = pending_path else {
                return;
            };
            let read_path = path.clone();
            let read_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::read_regular_file_bounded(
                        &read_path,
                        4 * 1024 * 1024,
                        false,
                        "样本文件",
                    )
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map_err(|err| format!("读取样本文件失败：{err}"))
                    })
                })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                if view.project != Some(project) {
                    return;
                }
                let content = match read_result {
                    Ok(text) => text,
                    Err(err) => {
                        view.fail(err, cx);
                        return;
                    }
                };
                let lines: Vec<String> = parse_manual_lines(&content)
                    .into_iter()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                if lines.is_empty() {
                    view.fail("所选样本文件不包含有效样本行。", cx);
                    return;
                }
                for (idx, line) in lines.iter().enumerate() {
                    if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&line.len()) {
                        view.fail(
                            format!(
                                "第 {} 行长度超出 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节范围。",
                                idx + 1
                            ),
                            cx,
                        );
                        return;
                    }
                }
                if to_manual {
                    let existing = view
                        .sequencer_pane
                        .manual_editor
                        .read(cx)
                        .value()
                        .to_string();
                    let joined = lines.join("\n");
                    let combined = if existing.trim_matches(['\r', '\n']).is_empty() {
                        joined
                    } else if existing.ends_with('\n') {
                        format!("{existing}{joined}")
                    } else {
                        format!("{existing}\n{joined}")
                    };
                    if combined.len() > MAX_TOTAL_SAMPLE_BYTES + MAX_SAMPLE_COUNT * 2 {
                        view.fail("导入后样本总大小超过 8 MiB 上限。", cx);
                        return;
                    }
                    view.sequencer_pane
                        .manual_editor
                        .update(cx, |s, cx| s.set_value(combined, window, cx));
                    view.sequencer_pane.status_text = Some(format!(
                        "已从 {} 导入 {} 条样本。",
                        path.display(),
                        lines.len()
                    ));
                } else {
                    if view.sequencer_pane.captured_samples.len() + lines.len() > MAX_SAMPLE_COUNT {
                        view.fail(format!("导入后样本总数超过 {MAX_SAMPLE_COUNT} 条上限。"), cx);
                        return;
                    }
                    let added = lines.len();
                    view.sequencer_pane.captured_samples.extend(lines);
                    let total = view.sequencer_pane.captured_samples.len();
                    view.sequencer_pane.capture.tokens_collected = total;
                    view.sequencer_pane.capture.latest_samples = view
                        .sequencer_pane
                        .captured_samples
                        .iter()
                        .skip(total.saturating_sub(MAX_PREVIEW_SAMPLES))
                        .cloned()
                        .collect();
                    view.sequencer_pane.sync_preview(window, cx);
                    view.sequencer_pane.status_text = Some(format!(
                        "已从文件追加 {added} 条样本（共 {total} 条）。"
                    ));
                }
                view.error = None;
                view.sequencer_view.update(cx, |_, cx| cx.notify());
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn export_sequencer_samples_to_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.file_picker_pending() {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let tokens: Vec<String> = if self.sequencer_pane.page == SequencerPage::Manual {
            let raw = self.sequencer_pane.manual_editor.read(cx).value();
            parse_manual_lines(raw.as_ref())
                .into_iter()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        } else if !self.sequencer_pane.captured_samples.is_empty() {
            self.sequencer_pane.captured_samples.clone()
        } else {
            self.sequencer_pane.capture.latest_samples.clone()
        };
        if tokens.is_empty() {
            self.fail("当前没有可导出的 Token 样本。", cx);
            return;
        }
        let payload = format!("{}\n", tokens.join("\n"));
        let count = tokens.len();
        self.save_sequencer_file(
            "sequencer-samples.txt",
            payload.into_bytes(),
            format!("已导出 {count} 条样本"),
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn copy_sequencer_analysis_report(&mut self, cx: &mut Context<Self>) {
        let Some(report) = self.sequencer_pane.analysis.as_deref() else {
            self.fail("当前尚无统计分析报告可复制。", cx);
            return;
        };
        let text = format_sequencer_analysis_report(report);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.error = None;
        let msg = "已复制完整 Sequencer 统计分析报告到剪贴板。".to_owned();
        self.sequencer_pane.status_text = Some(msg.clone());
        self.status = msg;
        cx.notify();
    }

    pub(in crate::workbench) fn export_sequencer_analysis_to_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.file_picker_pending() {
            return;
        }
        let Some(report) = self.sequencer_pane.analysis.as_deref() else {
            self.fail("当前尚无统计分析报告可导出。", cx);
            return;
        };
        let json = match serde_json::to_vec_pretty(report) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.fail(format!("序列化分析报告失败：{err}"), cx);
                return;
            }
        };
        self.save_sequencer_file(
            "sequencer-analysis.json",
            json,
            "已导出 Sequencer 分析报告".into(),
            window,
            cx,
        );
    }

    fn save_sequencer_file(
        &mut self,
        default_name: &'static str,
        bytes: Vec<u8>,
        done_prefix: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let directory = self.default_directory();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt =
            crate::native_terminate::prompt_for_new_path(window, cx, &directory, Some(default_name));
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_path = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != Some(project) {
                            view.fail("项目已切换，本次文件导出未写入磁盘。", cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(path))) => Some(path),
                            Ok(Ok(None)) => None,
                            Ok(Err(err)) => {
                                view.fail(format!("选择导出路径失败：{err}"), cx);
                                view.sequencer_view.update(cx, |_, cx| cx.notify());
                                cx.notify();
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已关闭。", cx);
                                view.sequencer_view.update(cx, |_, cx| cx.notify());
                                cx.notify();
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(path) = pending_path else {
                return;
            };
            let write_path = path.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_path, &bytes)
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if view.project != Some(project) {
                        return;
                    }
                    match write_result {
                        Ok(()) => {
                            view.error = None;
                            let msg = format!("{done_prefix}到 {}。", path.display());
                            view.sequencer_pane.status_text = Some(msg.clone());
                            view.status = msg;
                        }
                        Err(err) => view.fail(format!("写入文件失败：{err}"), cx),
                    }
                    view.sequencer_view.update(cx, |_, cx| cx.notify());
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn copy_all_sequencer_samples(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let tokens: Vec<String> = if self.sequencer_pane.page == SequencerPage::Manual {
            let raw = self.sequencer_pane.manual_editor.read(cx).value();
            parse_manual_lines(raw.as_ref())
                .into_iter()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        } else if !self.sequencer_pane.captured_samples.is_empty() {
            self.sequencer_pane.captured_samples.clone()
        } else {
            self.sequencer_pane.capture.latest_samples.clone()
        };
        if tokens.is_empty() {
            self.fail("当前没有可复制的 Token 样本。", cx);
            return;
        }
        let count = tokens.len();
        cx.write_to_clipboard(ClipboardItem::new_string(tokens.join("\n")));
        self.error = None;
        self.sequencer_pane.status_text = Some(format!("已复制全部 {count} 条样本到剪贴板。"));
        cx.notify();
    }

    pub(in crate::workbench) fn append_sequencer_captured_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sequencer_pane.running.is_some() {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.fail("剪贴板中没有可追加的文本样本。", cx);
            return;
        };
        let incoming: Vec<String> = parse_manual_lines(&pasted)
            .into_iter()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if incoming.is_empty() {
            self.fail("剪贴板文本为空，未追加样本。", cx);
            return;
        }
        if self.sequencer_pane.captured_samples.len() + incoming.len() > MAX_SAMPLE_COUNT {
            self.fail(
                format!("追加后样本数量超过 {MAX_SAMPLE_COUNT} 条上限，未修改当前样本。"),
                cx,
            );
            return;
        }
        for (idx, line) in incoming.iter().enumerate() {
            if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&line.len()) {
                self.fail(
                    format!(
                        "剪贴板第 {} 行长度须在 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节之间。",
                        idx + 1
                    ),
                    cx,
                );
                return;
            }
        }
        let added = incoming.len();
        self.sequencer_pane.captured_samples.extend(incoming);
        let total = self.sequencer_pane.captured_samples.len();
        self.sequencer_pane.capture.tokens_collected = total;
        self.sequencer_pane.capture.latest_samples = self
            .sequencer_pane
            .captured_samples
            .iter()
            .skip(total.saturating_sub(MAX_PREVIEW_SAMPLES))
            .cloned()
            .collect();
        self.sequencer_pane.sync_preview(window, cx);
        self.error = None;
        self.sequencer_pane.status_text = Some(format!(
            "已从剪贴板追加 {added} 条样本（当前共 {total} 条）。"
        ));
        cx.notify();
    }

    pub(in crate::workbench) fn analyze_manual_sequencer_samples(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match editing::snapshot(&self.sequencer_pane.manual_editor, window, cx) {
            Ok(text) => text,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let samples = match validate_manual_samples(text.as_ref()) {
            Ok(samples) => samples,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        self.enqueue_sequencer_analysis(samples, cx);
    }

    pub(in crate::workbench) fn switch_sequencer_request_pretty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.request.switch_pretty(window, cx) {
            self.fail(err, cx);
        }
    }

    pub(in crate::workbench) fn switch_sequencer_request_view(
        &mut self,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.request.switch(view, window, cx) {
            self.fail(err, cx);
        }
    }

    pub(in crate::workbench) fn handle_sequencer_search_action(
        &mut self,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let readonly = self.project.is_none()
            || self.transition.is_some()
            || !self.backend_available()
            || self.sequencer_pane.running.is_some()
            || self.sequencer_pane.request.is_readonly_preview();
        if let Err(err) = self
            .sequencer_pane
            .request
            .handle_search_bar_action_editable(action, readonly, window, cx)
        {
            self.fail(err, cx);
        }
        self.sequencer_view.update(cx, |_, cx| cx.notify());
    }
}
