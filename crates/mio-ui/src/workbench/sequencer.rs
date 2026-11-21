use std::{cell::Cell, rc::Rc};

use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    button::ButtonVariants,
    input::{Editor, EditorState, Input, InputEvent, InputState},
};
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, Render, Subscription, WeakEntity, Window,
};
use mio_core::{
    Command, DEFAULT_SEQUENCER_CONCURRENCY, DEFAULT_SEQUENCER_TARGET_SAMPLES, Draft, HttpVersion,
    MAX_SEQUENCER_CONCURRENCY, SequencerCaptureConfig, SequencerCommand, SequencerEvent, Uuid,
    sequencer::{
        FipsStatus, FipsTest, MAX_SAMPLE_COUNT, MAX_TOKEN_BYTES, MAX_TOTAL_SAMPLE_BYTES,
        MIN_FIPS_TEST_BITS, MIN_SAMPLE_COUNT, MIN_TOKEN_BYTES, QualityRating, SampleEncoding,
        SequencerAnalysis, SequencerTokenLocation,
    },
};
use tokio_util::sync::CancellationToken;

use super::*;

mod controller;
mod render_analysis;
mod render_live;
mod render_manual;
#[cfg(test)]
mod tests;
mod widgets;

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
    pub(in crate::workbench) target_url: Entity<InputState>,
    pub(in crate::workbench) request_version: HttpVersion,
    pub(in crate::workbench) request: MessageEditor,
    pub(in crate::workbench) location_kind: LocationKind,
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
    pub(super) view_inputs: Vec<Entity<InputState>>,
    _selection_subs: Vec<Subscription>,
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
        let preview_samples = editing::new_editor(window, cx);
        let manual_editor = editing::new_editor(window, cx);
        let sample_selection_seqs: [Rc<Cell<u64>>; 2] =
            std::array::from_fn(|_| Rc::new(Cell::new(0)));

        let selection_subs = vec![
            editing::observe_selection_state(&preview_samples, sample_selection_seqs[0].clone(), cx),
            editing::observe_selection_state(&manual_editor, sample_selection_seqs[1].clone(), cx),
        ];

        Self {
            page: SequencerPage::Live,
            live_requests: Vec::new(),
            next_request_seq: 1,
            selected_request_seq: None,
            target_url,
            request_version: HttpVersion::Http11,
            request,
            location_kind: LocationKind::BodyDelimiter,
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
            view_inputs: Vec::new(),
            _selection_subs: selection_subs,
        }
    }

    pub(in crate::workbench) fn attach_view_inputs(&mut self, inputs: Vec<Entity<InputState>>) {
        self.view_inputs = inputs;
    }

    pub(in crate::workbench) fn has_non_request_focus(&self, window: &Window, cx: &App) -> bool {
        self.target_url.focus_handle(cx).is_focused(window)
            || self
                .view_inputs
                .iter()
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
        editing::input_snapshot(&self.target_url, window, cx)?;
        editing::input_snapshot(&self.request.search, window, cx)?;
        for input in &self.view_inputs {
            editing::input_snapshot(input, window, cx)?;
        }
        for editor in [&self.request.raw, &self.request.hex, &self.manual_editor] {
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

pub(in crate::workbench) struct SequencerView {
    pub(super) workbench: WeakEntity<Workbench>,
    pub(super) request_drawer_open: bool,
    pub(super) settings_open: bool,
    pub(super) custom_config_open: bool,
    pub(super) capture_panel_open: bool,
    pub(super) cookie_name: Entity<InputState>,
    pub(super) form_field_name: Entity<InputState>,
    pub(super) header_name: Entity<InputState>,
    pub(super) delim_start: Entity<InputState>,
    pub(super) delim_end: Entity<InputState>,
    pub(super) regex_pattern: Entity<InputState>,
    pub(super) target_samples: Entity<InputState>,
    pub(super) concurrency: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SequencerView {
    pub(in crate::workbench) fn new(
        workbench: WeakEntity<Workbench>,
        pane: &SequencerPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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

        let mut subscriptions = Vec::new();
        for editor in [&pane.request.raw, &pane.request.hex, &pane.manual_editor] {
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
            &pane.target_url,
            &pane.request.search,
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
            workbench,
            request_drawer_open: false,
            settings_open: false,
            custom_config_open: false,
            capture_panel_open: false,
            cookie_name,
            form_field_name,
            header_name,
            delim_start,
            delim_end,
            regex_pattern,
            target_samples,
            concurrency,
            _subscriptions: subscriptions,
        }
    }

    pub(in crate::workbench) fn non_editor_inputs(&self) -> Vec<Entity<InputState>> {
        vec![
            self.cookie_name.clone(),
            self.form_field_name.clone(),
            self.header_name.clone(),
            self.delim_start.clone(),
            self.delim_end.clone(),
            self.regex_pattern.clone(),
            self.target_samples.clone(),
            self.concurrency.clone(),
        ]
    }

    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_drawer_open = false;
        self.settings_open = false;
        self.custom_config_open = false;
        self.capture_panel_open = false;
        for (input, value) in [
            (&self.cookie_name, String::new()),
            (&self.form_field_name, String::new()),
            (&self.header_name, String::new()),
            (&self.delim_start, String::new()),
            (&self.delim_end, String::new()),
            (&self.regex_pattern, String::new()),
            (
                &self.target_samples,
                DEFAULT_SEQUENCER_TARGET_SAMPLES.to_string(),
            ),
            (&self.concurrency, DEFAULT_SEQUENCER_CONCURRENCY.to_string()),
        ] {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        cx.notify();
    }

    pub(super) fn snapshot_location(
        &self,
        kind: LocationKind,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SequencerTokenLocation, String> {
        match kind {
            LocationKind::Cookie => {
                let name = editing::input_snapshot(&self.cookie_name, window, cx)
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
                let name = editing::input_snapshot(&self.form_field_name, window, cx)
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
                let name = editing::input_snapshot(&self.header_name, window, cx)
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
                let start = editing::input_snapshot(&self.delim_start, window, cx)
                    .map_err(str::to_owned)?
                    .to_string();
                let end = editing::input_snapshot(&self.delim_end, window, cx)
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
                let pattern = editing::input_snapshot(&self.regex_pattern, window, cx)
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

    pub(super) fn start_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wb_entity) = self.workbench.upgrade() else {
            return;
        };
        let (can_start, location_kind) = {
            let wb = wb_entity.read(cx);
            (
                wb.sequencer_can_operate() && wb.sequencer_pane.running.is_none(),
                wb.sequencer_pane.location_kind,
            )
        };
        if !can_start {
            return;
        }
        let location = match self.snapshot_location(location_kind, window, cx) {
            Ok(loc) => loc,
            Err(err) => {
                wb_entity.update(cx, |wb, cx| wb.fail(err, cx));
                cx.notify();
                return;
            }
        };
        let samples_text = match editing::input_snapshot(&self.target_samples, window, cx) {
            Ok(text) => text.to_string(),
            Err(err) => {
                wb_entity.update(cx, |wb, cx| wb.fail(err, cx));
                cx.notify();
                return;
            }
        };
        let concurrency_text = match editing::input_snapshot(&self.concurrency, window, cx) {
            Ok(text) => text.to_string(),
            Err(err) => {
                wb_entity.update(cx, |wb, cx| wb.fail(err, cx));
                cx.notify();
                return;
            }
        };
        let started = wb_entity.update(cx, |wb, cx| {
            wb.start_sequencer_capture_with(
                location,
                &samples_text,
                &concurrency_text,
                window,
                cx,
            )
        });
        if started {
            self.capture_panel_open = true;
        }
        cx.notify();
    }

    pub(super) fn fill_delimiters_from_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wb_entity) = self.workbench.upgrade() else {
            return;
        };
        let delims = wb_entity.update(cx, |wb, cx| {
            wb.extract_sequencer_selection_delimiters(window, cx)
        });
        if let Some((start, end)) = delims {
            self.custom_config_open = true;
            self.delim_start
                .update(cx, |s, cx| s.set_value(start, window, cx));
            self.delim_end
                .update(cx, |s, cx| s.set_value(end, window, cx));
        }
        cx.notify();
    }

    pub(super) fn remove_selected_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self
            .workbench
            .update(cx, |wb, cx| wb.remove_selected_sequencer_request(window, cx))
            .unwrap_or(false);
        if empty {
            self.request_drawer_open = false;
        }
        cx.notify();
    }

    pub(super) fn clear_requests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cleared = self
            .workbench
            .update(cx, |wb, cx| wb.clear_sequencer_requests(window, cx))
            .unwrap_or(false);
        if cleared {
            self.request_drawer_open = false;
        }
        cx.notify();
    }
}

impl Render for SequencerView {
    #[inline(never)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(wb_entity) = self.workbench.upgrade() else {
            return div().id("sequencer-root").size_full().into_any_element();
        };
        let (page, base64_predecode, show_analysis_tab, can_edit_config, status_text) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.sequencer_pane;
            (
                pane.page,
                pane.base64_predecode,
                pane.analysis.is_some() || pane.page == SequencerPage::Analysis,
                wb.sequencer_can_operate() && pane.running.is_none(),
                pane.status_text.clone(),
            )
        };
        let settings_open = self.settings_open;
        let dark = theme::is_dark();

        let live_tab = wb_render_chrome::secondary_tab(
            "sequencer-tab-live",
            "实时采样",
            page == SequencerPage::Live,
            window,
            cx,
        )
        .px(theme::scaled_px(16.))
        .font_weight(if page == SequencerPage::Live {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("Live capture — 切换至实时并发采样视图")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.sequencer_pane.page = SequencerPage::Live;
                cx.notify();
            });
            cx.notify();
        }));

        let manual_tab = wb_render_chrome::secondary_tab(
            "sequencer-tab-manual",
            "手动加载",
            page == SequencerPage::Manual,
            window,
            cx,
        )
        .px(theme::scaled_px(16.))
        .font_weight(if page == SequencerPage::Manual {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("Manual load — 切换至手动样本粘贴与加载视图")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.sequencer_pane.page = SequencerPage::Manual;
                cx.notify();
            });
            cx.notify();
        }));

        let analysis_tab = show_analysis_tab.then(|| {
            wb_render_chrome::secondary_tab(
                "sequencer-tab-analysis",
                "统计分析",
                page == SequencerPage::Analysis,
                window,
                cx,
            )
            .px(theme::scaled_px(16.))
            .font_weight(if page == SequencerPage::Analysis {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "Analysis — 查看 FIPS 140-2 与信息熵随机性统计分析报告",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                let _ = this.workbench.update(cx, |wb, cx| {
                    wb.sequencer_pane.page = SequencerPage::Analysis;
                    cx.notify();
                });
                cx.notify();
            }))
        });

        let settings_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x2f3237).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let settings_active_bg: gpui_kit::Hsla = if dark {
            rgb(0x35383d).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let settings_btn = flat_button("sequencer-settings", true, window, cx)
            .accessibility_label("Sequencer settings")
            .h(theme::secondary_tab_height() - px(1.))
            .self_start()
            .flex_shrink_0()
            .px(theme::scaled_px(10.))
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .font_weight(if settings_open {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .line_height(theme::scaled_px(17.))
            .text_color(if settings_open && !dark {
                rgb(0x000000).into()
            } else {
                theme::foreground()
            })
            .flex()
            .items_center()
            .gap(theme::scaled_px(6.))
            .bg(if settings_open {
                settings_active_bg
            } else {
                theme::toolbar_surface()
            })
            .hover(move |s| s.bg(settings_hover_bg))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "展开或收起 Sequencer 设置（采样样本数、并发数与 Base64 预解码选项）",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.settings_open = !this.settings_open;
                cx.notify();
            }))
            .child(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                    .size(px(16.))
                    .text_color(if base64_predecode {
                        theme::accent()
                    } else if dark {
                        theme::foreground()
                    } else {
                        rgb(0x1f1f1f).into()
                    }),
            )
            .child("Sequencer 设置")
            .when(base64_predecode, |btn| {
                btn.child(div().size(px(6.)).rounded(px(3.)).bg(theme::accent()))
            });

        let settings_panel = settings_open.then(|| {
            self.render_settings_panel(
                can_edit_config,
                base64_predecode,
                settings_hover_bg,
                window,
                cx,
            )
        });

        let content = match page {
            SequencerPage::Live => self.render_live(&wb_entity, window, cx),
            SequencerPage::Manual => self.render_manual(&wb_entity, window, cx),
            SequencerPage::Analysis => self.render_analysis(&wb_entity, window, cx),
        };

        div()
            .id("sequencer-root")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(
                wb_render_chrome::secondary_tab_bar()
                    .child(live_tab)
                    .child(manual_tab)
                    .children(analysis_tab)
                    .child(
                        div()
                            .w(px(20.))
                            .h(theme::secondary_tab_height() - px(1.))
                            .self_start()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().w(px(1.)).h(px(22.)).bg(theme::border())),
                    )
                    .child(settings_btn)
                    .child(div().flex_1())
                    .when_some(status_text, |bar, status| {
                        bar.child(
                            div()
                                .id("sequencer-status-banner")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(status.clone())
                                .max_w(px(460.))
                                .pr(px(12.))
                                .truncate()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child(status),
                        )
                    }),
            )
            .children(settings_panel)
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}

impl SequencerView {
    fn render_settings_panel(
        &self,
        can_edit_config: bool,
        base64_predecode: bool,
        settings_hover_bg: gpui_kit::Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::assets::IconName;
        use gpui_kit::base::{Checkbox, CheckboxIndicator};
        use gpui_kit::component::Icon;

        let capture_panel_open = self.capture_panel_open;
        let focus = window
            .use_keyed_state("sequencer-base64-predecode", cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let base64_checkbox = Checkbox::new("sequencer-base64-predecode")
            .accessibility_label("Base64 预解码")
            .checked(base64_predecode)
            .track_focus(&focus)
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(2.))
            .py(px(2.))
            .rounded(px(3.))
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .when(focus.is_focused(window), |this| this.bg(theme::selected()))
            .child(
                CheckboxIndicator::new()
                    .checked(base64_predecode)
                    .size(px(14.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(if base64_predecode {
                        theme::accent()
                    } else {
                        theme::control_border()
                    })
                    .bg(if base64_predecode {
                        theme::accent()
                    } else {
                        theme::surface()
                    })
                    .rounded(px(3.))
                    .when(base64_predecode, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .size(px(11.))
                                .text_color(rgb(0xffffff)),
                        )
                    }),
            )
            .child("Base64 预解码")
            .on_click(cx.listener(|this, _, _, cx| {
                let _ = this.workbench.update(cx, |wb, _| {
                    wb.sequencer_pane.base64_predecode = !wb.sequencer_pane.base64_predecode;
                });
                cx.notify();
            }));

        let toggle_capture_panel_btn = widgets::sequencer_stack_button(
            "sequencer-toggle-capture-panel",
            if capture_panel_open {
                "收起采样面板"
            } else {
                "采样进度面板"
            },
            true,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "显示或隐藏实时采样进度与样本导入/导出面板",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.capture_panel_open = !this.capture_panel_open;
            cx.notify();
        }));

        div()
            .id("sequencer-settings-panel")
            .test_support()
            .w_full()
            .px(px(16.))
            .py(px(7.))
            .flex_shrink_0()
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .text_size(theme::scaled_px(12.5))
                    .text_color(theme::foreground())
                    .child("目标样本数："),
            )
            .child(
                div().w(px(78.)).flex_shrink_0().child(
                    Input::new(&self.target_samples)
                        .id("sequencer-target-samples")
                        .aria_label("目标样本数")
                        .small()
                        .disabled(!can_edit_config),
                ),
            )
            .child(
                div()
                    .ml(px(4.))
                    .text_size(theme::scaled_px(12.5))
                    .text_color(theme::foreground())
                    .child("并发数："),
            )
            .child(
                div().w(px(56.)).flex_shrink_0().child(
                    Input::new(&self.concurrency)
                        .id("sequencer-concurrency")
                        .aria_label("并发请求数")
                        .small()
                        .disabled(!can_edit_config),
                ),
            )
            .child(div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(theme::border()))
            .child(base64_checkbox)
            .child(div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(theme::border()))
            .child(toggle_capture_panel_btn)
            .child(div().flex_1())
            .child(
                flat_button("sequencer-settings-close", false, window, cx)
                    .accessibility_label("关闭 Sequencer 设置")
                    .size(px(22.))
                    .rounded(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |s| s.bg(settings_hover_bg))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(13.))
                            .text_color(theme::muted_foreground()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        cx.notify();
                    })),
            )
            .into_any_element()
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

pub(in crate::workbench) fn format_sequencer_analysis_report(report: &SequencerAnalysis) -> String {
    use std::fmt::Write as _;

    let (rating_text, _, _) = quality_badge(report.quality);
    let length_range = if report.min_length == report.max_length {
        format!("{} B", report.min_length)
    } else {
        format!("{}–{} B", report.min_length, report.max_length)
    };
    let mut out = String::new();
    let _ = writeln!(out, "=== Sequencer 随机性统计分析报告 ===");
    let _ = writeln!(out, "质量评级：{rating_text}");
    let _ = writeln!(out, "有效熵：{:.2} bits", report.effective_entropy_bits);
    let _ = writeln!(out, "样本总数：{}", report.sample_count);
    let _ = writeln!(out, "长度范围：{length_range}");
    let _ = writeln!(out, "字符集大小：{}", report.charset_size);
    let _ = writeln!(out, "识别编码：{}", encoding_label(report.encoding));
    let _ = writeln!(out, "诊断结论：{}", report.diagnosis);
    let _ = writeln!(out);
    let _ = writeln!(out, "--- FIPS 140-2 四项随机性检验 ---");
    match &report.bit_analysis.fips {
        FipsStatus::Pass => {
            let _ = writeln!(
                out,
                "状态：Pass (4/4 全部通过，已检验前 {MIN_FIPS_TEST_BITS} bits)"
            );
        }
        FipsStatus::InsufficientBits { available_bits } => {
            let _ = writeln!(
                out,
                "状态：样本比特不足（当前 {available_bits} bits < {MIN_FIPS_TEST_BITS} bits）"
            );
        }
        FipsStatus::Failed { failures } => {
            let _ = writeln!(out, "状态：Failed（{} / 4 项未通过）", failures.len());
            for failure in failures {
                let _ = writeln!(
                    out,
                    "  - {} | 实测：{} | 标准区间：{}",
                    fips_test_label(failure.test),
                    failure.observed,
                    failure.requirement
                );
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "--- 比特级统计 ---");
    let _ = writeln!(
        out,
        "可用比特：{} | 已分析比特：{} | 1-bit 计数：{} ({:.2}%)",
        report.bit_analysis.available_bits,
        report.bit_analysis.analyzed_bits,
        report.bit_analysis.ones_count,
        report.bit_analysis.ones_ratio * 100.0
    );
    let _ = writeln!(
        out,
        "比特香农熵：{:.4} bits | 相邻比特自相关：{:.4}",
        report.bit_analysis.shannon_entropy_bits, report.bit_analysis.adjacent_autocorrelation
    );
    if !report.character_analysis.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "--- 字符级逐位置熵表 ---");
        let _ = writeln!(out, "位置\t样本数\t字符种类\tShannon(b)\tχ²\tp-value\t1%有效熵");
        for item in &report.character_analysis {
            let _ = writeln!(
                out,
                "#{}\t{}\t{}\t{:.3}\t{:.2}\t{:.4}\t{:.3}",
                item.position,
                item.samples_with_position,
                item.observed_characters,
                item.shannon_entropy_bits,
                item.chi_square,
                item.p_value,
                item.effective_entropy_bits
            );
        }
    }
    out
}

pub(in crate::workbench) fn prepare_samples_for_analysis(
    samples: Vec<String>,
    base64_predecode: bool,
) -> Result<Vec<String>, String> {
    if !base64_predecode {
        return Ok(samples);
    }
    let mut decoded_raw = Vec::with_capacity(samples.len());
    for (idx, token) in samples.iter().enumerate() {
        let bytes = crate::editing::base64_decode_bytes(token.trim()).map_err(|_| {
            format!(
                "第 {} 条样本不是有效的 Base64 编码，未执行预解码分析。",
                idx + 1
            )
        })?;
        if bytes.is_empty() {
            return Err(format!("第 {} 条样本 Base64 解码后为空。", idx + 1));
        }
        decoded_raw.push(bytes);
    }
    let all_ascii = decoded_raw.iter().all(|b| {
        (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&b.len()) && b.iter().all(u8::is_ascii_graphic)
    });
    let mut total_bytes = 0usize;
    let mut out = Vec::with_capacity(decoded_raw.len());
    for (idx, bytes) in decoded_raw.into_iter().enumerate() {
        let transformed = if all_ascii {
            String::from_utf8(bytes).map_err(|_| format!("第 {} 条样本解码失败。", idx + 1))?
        } else {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
            }
            hex
        };
        if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&transformed.len()) {
            return Err(format!(
                "第 {} 条样本解码后长度超出 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节上限。",
                idx + 1
            ));
        }
        total_bytes = total_bytes.saturating_add(transformed.len());
        if total_bytes > MAX_TOTAL_SAMPLE_BYTES {
            return Err("Base64 预解码后样本总字节数超过 8 MiB 上限。".into());
        }
        out.push(transformed);
    }
    Ok(out)
}
