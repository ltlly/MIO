use std::{cell::Cell, rc::Rc};

use super::*;

pub(in crate::workbench) const DEFAULT_TARGET_URL: &str = "http://127.0.0.1:8080/login";
pub(in crate::workbench) const DEFAULT_RAW_REQUEST: &[u8] =
    b"GET /login HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n";

pub(in crate::workbench) const MAX_TARGET_URL_BYTES: usize = 4096;

pub(in crate::workbench) const MAX_LOCATION_PARAM_BYTES: usize = 512;

pub(in crate::workbench) const MAX_PREVIEW_SAMPLES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum SequencerPage {
    Live,
    Manual,
    Analysis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum LocationKind {
    Cookie,
    FormField,
    Header,
    BodyDelimiter,
    Regex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct LiveCaptureRequestItem {
    pub(in crate::workbench) seq: usize,
    pub(in crate::workbench) target: String,
    pub(in crate::workbench) host: String,
    pub(in crate::workbench) request_summary: String,
    pub(in crate::workbench) raw: Vec<u8>,
    pub(in crate::workbench) version: HttpVersion,
}

#[derive(Clone, Debug, Default)]
pub(in crate::workbench) struct CaptureSnapshot {
    pub(in crate::workbench) target_samples: usize,
    pub(in crate::workbench) requests_sent: usize,
    pub(in crate::workbench) tokens_collected: usize,
    pub(in crate::workbench) extraction_misses: usize,
    pub(in crate::workbench) request_errors: usize,
    pub(in crate::workbench) latest_samples: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) struct ManualStats {
    pub(in crate::workbench) count: usize,
    pub(in crate::workbench) min_len: usize,
    pub(in crate::workbench) max_len: usize,
    pub(in crate::workbench) total_bytes: usize,
}

pub(in crate::workbench) struct SequencerPane {
    pub(in crate::workbench) page: SequencerPage,
    pub(in crate::workbench) live_requests: Vec<LiveCaptureRequestItem>,
    pub(in crate::workbench) next_request_seq: usize,
    pub(in crate::workbench) selected_request_seq: Option<usize>,
    pub(in crate::workbench) request_drawer_open: bool,
    pub(in crate::workbench) settings_open: bool,
    pub(in crate::workbench) custom_config_open: bool,
    pub(in crate::workbench) capture_panel_open: bool,
    pub(in crate::workbench) target_url: Entity<InputState>,
    pub(in crate::workbench) request_version: HttpVersion,
    pub(in crate::workbench) request: MessageEditor,
    pub(in crate::workbench) location_kind: LocationKind,
    pub(in crate::workbench) cookie_name: Entity<InputState>,
    pub(in crate::workbench) form_field_name: Entity<InputState>,
    pub(in crate::workbench) header_name: Entity<InputState>,
    pub(in crate::workbench) delim_start: Entity<InputState>,
    pub(in crate::workbench) delim_end: Entity<InputState>,
    pub(in crate::workbench) regex_pattern: Entity<InputState>,
    pub(in crate::workbench) target_samples: Entity<InputState>,
    pub(in crate::workbench) concurrency: Entity<InputState>,
    pub(in crate::workbench) preview_samples: Entity<EditorState>,
    pub(in crate::workbench) manual_editor: Entity<EditorState>,
    sample_selection_seqs: [Rc<Cell<u64>>; 2],
    pub(in crate::workbench) running: Option<(ProjectKey, Uuid, CancellationToken)>,
    pub(in crate::workbench) pending_analysis: Option<(ProjectKey, Uuid, CancellationToken)>,
    pub(in crate::workbench) capture: CaptureSnapshot,
    pub(in crate::workbench) captured_samples: Vec<String>,
    pub(in crate::workbench) base64_predecode: bool,
    pub(in crate::workbench) status_text: Option<String>,
    pub(in crate::workbench) analysis: Option<Rc<SequencerAnalysis>>,
    pub(in crate::workbench) analysis_error: Option<String>,
    pub(in crate::workbench) _subscriptions: Vec<Subscription>,
}

impl SequencerPane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let target_url = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_TARGET_URL)
                .placeholder("http://127.0.0.1:8080/login")
        });
        let mut request = MessageEditor::new(window, cx);
        request.load(DEFAULT_RAW_REQUEST, window, cx);
        let cookie_name = cx.new(|cx| InputState::new(window, cx));
        let form_field_name = cx.new(|cx| InputState::new(window, cx));
        let header_name = cx.new(|cx| InputState::new(window, cx).placeholder("例如 X-CSRF-Token"));
        let delim_start =
            cx.new(|cx| InputState::new(window, cx).placeholder("起始分隔符，例如 value=\""));
        let delim_end = cx.new(|cx| InputState::new(window, cx).placeholder("结束分隔符，例如 \""));
        let regex_pattern = cx.new(|cx| {
            InputState::new(window, cx).placeholder("正则表达式（提取首个捕获组或完整匹配）")
        });
        let target_samples = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_SEQUENCER_TARGET_SAMPLES.to_string())
                .placeholder("2–20,000")
        });
        let concurrency = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_SEQUENCER_CONCURRENCY.to_string())
                .placeholder("1–16")
        });
        let preview_samples = editing::new_editor(window, cx);
        let manual_editor = editing::new_editor(window, cx);
        let sample_selection_seqs: [Rc<Cell<u64>>; 2] =
            std::array::from_fn(|_| Rc::new(Cell::new(0)));

        let mut subscriptions = Vec::new();
        subscriptions.push(editing::observe_selection_state(
            &preview_samples,
            sample_selection_seqs[0].clone(),
            cx,
        ));
        subscriptions.push(editing::observe_selection_state(
            &manual_editor,
            sample_selection_seqs[1].clone(),
            cx,
        ));
        for editor in [&request.raw, &request.hex, &request.pretty, &manual_editor] {
            let mut composing = false;
            subscriptions.push(cx.observe_in(editor, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(editor, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        for input in [
            &target_url,
            &request.search,
            &cookie_name,
            &form_field_name,
            &header_name,
            &delim_start,
            &delim_end,
            &regex_pattern,
            &target_samples,
            &concurrency,
        ] {
            let mut composing = false;
            subscriptions.push(cx.observe_in(input, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }

        Self {
            page: SequencerPage::Live,
            live_requests: Vec::new(),
            next_request_seq: 1,
            selected_request_seq: None,
            request_drawer_open: false,
            settings_open: false,
            custom_config_open: false,
            capture_panel_open: false,
            target_url,
            request_version: HttpVersion::Http11,
            request,
            location_kind: LocationKind::BodyDelimiter,
            cookie_name,
            form_field_name,
            header_name,
            delim_start,
            delim_end,
            regex_pattern,
            target_samples,
            concurrency,
            preview_samples,
            manual_editor,
            sample_selection_seqs,
            running: None,
            pending_analysis: None,
            capture: CaptureSnapshot {
                target_samples: DEFAULT_SEQUENCER_TARGET_SAMPLES,
                ..CaptureSnapshot::default()
            },
            captured_samples: Vec::new(),
            base64_predecode: false,
            status_text: None,
            analysis: None,
            analysis_error: None,
            _subscriptions: subscriptions,
        }
    }

    pub(in crate::workbench) fn has_non_request_focus(&self, window: &Window, cx: &App) -> bool {
        [
            &self.target_url,
            &self.cookie_name,
            &self.form_field_name,
            &self.header_name,
            &self.delim_start,
            &self.delim_end,
            &self.regex_pattern,
            &self.target_samples,
            &self.concurrency,
        ]
        .into_iter()
        .any(|input| input.focus_handle(cx).is_focused(window))
            || self.manual_editor.focus_handle(cx).is_focused(window)
    }

    pub(in crate::workbench) fn selection_candidate(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<editing::SelectionCandidate> {
        match self.page {
            SequencerPage::Live => editing::pick_selection_candidate(
                [
                    self.request.selection_candidate(false, window, cx),
                    editing::plain_editor_selection_candidate(
                        &self.preview_samples,
                        self.sample_selection_seqs[0].get(),
                        window,
                        cx,
                    ),
                ]
                .into_iter()
                .flatten(),
            ),
            SequencerPage::Manual => editing::plain_editor_selection_candidate(
                &self.manual_editor,
                self.sample_selection_seqs[1].get(),
                window,
                cx,
            ),
            SequencerPage::Analysis => None,
        }
    }

    pub(in crate::workbench) fn is_selecting(&self, cx: &App) -> bool {
        match self.page {
            SequencerPage::Live => {
                self.request.is_selecting(cx) || self.preview_samples.read(cx).is_selecting()
            }
            SequencerPage::Manual => self.manual_editor.read(cx).is_selecting(),
            SequencerPage::Analysis => false,
        }
    }

    pub(in crate::workbench) fn validate_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in [
            &self.target_url,
            &self.request.search,
            &self.cookie_name,
            &self.form_field_name,
            &self.header_name,
            &self.delim_start,
            &self.delim_end,
            &self.regex_pattern,
            &self.target_samples,
            &self.concurrency,
        ] {
            editing::input_snapshot(input, window, cx)?;
        }
        for editor in [
            &self.request.raw,
            &self.request.hex,
            &self.request.pretty,
            &self.manual_editor,
        ] {
            editing::snapshot(editor, window, cx)?;
        }
        Ok(())
    }

    pub(in crate::workbench) fn sync_preview(&mut self, window: &mut Window, cx: &mut App) {
        let preview = format_preview_samples(&self.capture.latest_samples);
        self.preview_samples
            .update(cx, |state, cx| state.set_value(preview, window, cx));
    }
}

pub(in crate::workbench) fn extract_live_request_host(target: &str, raw: &[u8]) -> String {
    let trimmed = target.trim();
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
        if !authority.is_empty() {
            return format!("{scheme}://{authority}");
        }
    }
    let text = String::from_utf8_lossy(raw);
    for line in text.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("host") {
                let host = value.trim();
                if !host.is_empty() {
                    return host.to_owned();
                }
            }
        }
    }
    "(未指定)".to_owned()
}

pub(in crate::workbench) fn extract_live_request_summary(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r').trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    "(空请求)".to_owned()
}

pub(in crate::workbench) fn format_preview_samples(samples: &[String]) -> String {
    if samples.is_empty() {
        return String::new();
    }
    let start = samples.len().saturating_sub(MAX_PREVIEW_SAMPLES);
    samples[start..].join("\n")
}

pub(in crate::workbench) fn parse_manual_lines(text: &str) -> Vec<&str> {
    let trimmed = text.trim_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

pub(in crate::workbench) fn compute_manual_stats(text: &str) -> ManualStats {
    let mut stats = ManualStats::default();
    for line in parse_manual_lines(text) {
        if line.is_empty() {
            continue;
        }
        let len = line.len();
        if stats.count == 0 {
            stats.min_len = len;
            stats.max_len = len;
        } else {
            stats.min_len = stats.min_len.min(len);
            stats.max_len = stats.max_len.max(len);
        }
        stats.count += 1;
        stats.total_bytes = stats.total_bytes.saturating_add(len);
    }
    stats
}

pub(in crate::workbench) fn validate_manual_samples(text: &str) -> Result<Vec<String>, String> {
    let lines = parse_manual_lines(text);
    if !(MIN_SAMPLE_COUNT..=MAX_SAMPLE_COUNT).contains(&lines.len()) {
        return Err(format!(
            "样本数量须在 {MIN_SAMPLE_COUNT}–{MAX_SAMPLE_COUNT} 条之间（当前 {} 条）。",
            lines.len()
        ));
    }
    let mut total_bytes = 0_usize;
    let mut samples = Vec::with_capacity(lines.len());
    for (idx, line) in lines.into_iter().enumerate() {
        let len = line.len();
        if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&len) {
            return Err(format!(
                "第 {} 行样本长度须在 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节之间（当前 {len} 字节）。",
                idx + 1
            ));
        }
        total_bytes = total_bytes.saturating_add(len);
        if total_bytes > MAX_TOTAL_SAMPLE_BYTES {
            return Err(format!(
                "样本总字节数不能超过 {} MiB（{MAX_TOTAL_SAMPLE_BYTES} 字节）。",
                MAX_TOTAL_SAMPLE_BYTES / (1024 * 1024)
            ));
        }
        samples.push(line.to_owned());
    }
    Ok(samples)
}

pub(in crate::workbench) fn quality_badge(quality: QualityRating) -> (&'static str, u32, u32) {
    match quality {
        QualityRating::Excellent => ("Excellent 极佳", 0xe6f4ea, 0x137333),
        QualityRating::Good => ("Good 良好", 0xe8f0fe, 0x1967d2),
        QualityRating::Fair => ("Fair 一般", 0xfef7e0, 0xb06000),
        QualityRating::Weak => ("Weak 薄弱", 0xfce8e6, 0xc5221f),
        QualityRating::InsufficientSample => ("样本不足", 0xeeeeee, 0x555555),
    }
}

pub(in crate::workbench) fn encoding_label(encoding: SampleEncoding) -> &'static str {
    match encoding {
        SampleEncoding::Hex => "Hex",
        SampleEncoding::Base64 => "Base64",
        SampleEncoding::Ascii => "ASCII",
    }
}

pub(in crate::workbench) fn fips_test_label(test: FipsTest) -> &'static str {
    match test {
        FipsTest::Monobit => "Monobit（单比特频数检验）",
        FipsTest::Poker => "Poker（扑克检验）",
        FipsTest::Runs => "Runs（游程检验）",
        FipsTest::LongRun => "Long run (长游程)",
    }
}

pub(in crate::workbench) fn max_theoretical_char_bits(
    encoding: SampleEncoding,
    charset_size: usize,
) -> f64 {
    match encoding {
        SampleEncoding::Hex => 4.0,
        SampleEncoding::Base64 => 6.0,
        SampleEncoding::Ascii => (charset_size.max(2) as f64).log2().max(1.0),
    }
}

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
        pane.request_drawer_open = false;
        pane.settings_open = false;
        pane.custom_config_open = false;
        pane.capture_panel_open = false;
        pane.request_version = HttpVersion::Http11;
        pane.location_kind = LocationKind::BodyDelimiter;
        pane.request.load(DEFAULT_RAW_REQUEST, window, cx);
        for (input, value) in [
            (&pane.target_url, DEFAULT_TARGET_URL.to_owned()),
            (&pane.cookie_name, String::new()),
            (&pane.form_field_name, String::new()),
            (&pane.header_name, String::new()),
            (&pane.delim_start, String::new()),
            (&pane.delim_end, String::new()),
            (&pane.regex_pattern, String::new()),
            (
                &pane.target_samples,
                DEFAULT_SEQUENCER_TARGET_SAMPLES.to_string(),
            ),
            (&pane.concurrency, DEFAULT_SEQUENCER_CONCURRENCY.to_string()),
        ] {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
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
}
