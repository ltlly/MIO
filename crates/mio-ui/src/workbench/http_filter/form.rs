use super::*;
use anyhow::{Result as AnyResult, ensure};
use gpui_kit::HighlightStyle;
use gpui_kit::base::input::{
    FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, InputHighlighterFactory, Rope,
};
use serde::{Deserialize, Serialize};
use std::{ops::Range, path::Path, rc::Rc};

const MAX_LIBRARY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HIGHLIGHT_SCRIPT_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct MioScriptSyntaxHighlighter {
    keyword_ranges: Vec<Range<usize>>,
    text_len: usize,
}

fn script_highlighter_factory() -> InputHighlighterFactory {
    Rc::new(|_| Some(Box::new(MioScriptSyntaxHighlighter::default()) as Box<dyn InputHighlighter>))
}

impl InputHighlighter for MioScriptSyntaxHighlighter {
    fn language(&self) -> SharedString {
        "mio_script".into()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.text_len = text.len();
        let mut cap = text.len().min(MAX_HIGHLIGHT_SCRIPT_BYTES);
        let full = text.to_string();
        while cap > 0 && !full.is_char_boundary(cap) {
            cap -= 1;
        }
        self.keyword_ranges = mio_core::script_keyword_ranges(&full[..cap]);
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let start = range.start.min(self.text_len);
        let end = range.end.min(self.text_len).max(start);
        if start >= end {
            return Vec::new();
        }
        let kw_style = HighlightStyle {
            color: Some(rgb(if theme::is_dark() { 0x6cb6ff } else { 0x000d71 }).into()),
            ..Default::default()
        };
        let first = self.keyword_ranges.partition_point(|r| r.end <= start);
        let mut runs = Vec::new();
        let mut cursor = start;
        for tok in &self.keyword_ranges[first..] {
            if tok.start >= end {
                break;
            }
            let s = tok.start.max(start);
            let e = tok.end.min(end);
            if s >= e || e <= cursor {
                continue;
            }
            if cursor < s {
                runs.push((cursor..s, HighlightStyle::default()));
            }
            runs.push((s..e, kw_style));
            cursor = e;
        }
        if cursor < end {
            runs.push((cursor..end, HighlightStyle::default()));
        }
        runs
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::workbench) struct LibraryScript {
    pub(super) name: String,
    pub(super) code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::workbench) struct ScriptLibrary {
    version: u32,
    pub(super) scripts: Vec<LibraryScript>,
}

impl Default for ScriptLibrary {
    fn default() -> Self {
        Self { version: 1, scripts: Vec::new() }
    }
}

pub(in crate::workbench) enum LibraryOverlay {
    Load { library: ScriptLibrary },
    Save { library: ScriptLibrary, code: String, overwrite: Option<String> },
}

fn validate_library(library: &ScriptLibrary) -> AnyResult<()> {
    ensure!(library.version == 1, "脚本库版本无效");
    ensure!(library.scripts.len() <= 128, "脚本库最多保存 128 个脚本");
    for (index, script) in library.scripts.iter().enumerate() {
        ensure!(!script.name.trim().is_empty() && script.name == script.name.trim()
            && script.name.len() <= 256 && !script.name.chars().any(char::is_control),
            "脚本名称须为非空且不超过 256 字节的文本");
        ensure!(!library.scripts[..index].iter().any(|prior| prior.name == script.name),
            "脚本库包含重复名称：{}", script.name);
        mio_core::validate_filter_script(&script.code)?;
    }
    Ok(())
}

pub(super) fn read_library(path: &Path) -> AnyResult<ScriptLibrary> {
    let Some(bytes) =
        mio_core::ui_files::read_owned_file_bounded(path, MAX_LIBRARY_BYTES, "脚本库")?
    else {
        return Ok(ScriptLibrary::default());
    };
    let library: ScriptLibrary = serde_json::from_slice(&bytes)?;
    validate_library(&library)?;
    Ok(library)
}

pub(super) fn write_library(
    path: &Path,
    expected: &ScriptLibrary,
    script: LibraryScript,
) -> AnyResult<ScriptLibrary> {
    let mut saved = None;
    mio_core::ui_files::locked_cas_update_owned_file(
        path,
        "http-filter-scripts.lock",
        MAX_LIBRARY_BYTES,
        "脚本库",
        |before| {
            let current = match before {
                Some(bytes) => {
                    let lib: ScriptLibrary = serde_json::from_slice(&bytes)?;
                    validate_library(&lib)?;
                    lib
                }
                None => ScriptLibrary::default(),
            };
            ensure!(&current == expected, "脚本库已发生变化，请重新加载脚本库");
            let mut next = current;
            if let Some(existing) = next.scripts.iter_mut().find(|entry| entry.name == script.name) {
                *existing = script;
            } else {
                next.scripts.push(script);
            }
            validate_library(&next)?;
            let bytes = serde_json::to_vec(&next)?;
            ensure!(bytes.len() as u64 <= MAX_LIBRARY_BYTES, "脚本库超过 4 MiB");
            saved = Some(next);
            Ok(bytes)
        },
    )?;
    Ok(saved.expect("CAS transform must produce updated library"))
}

impl FilterWindow {
    pub(super) fn new(
        owner: Entity<Workbench>,
        owner_window: AnyWindowHandle,
        project: ProjectKey,
        applied: HttpHistoryFilter,
        mut inputs: ExtensionInputs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        match &applied.extensions {
            HttpExtensionFilter::Only(text) => inputs.only = text.clone(),
            HttpExtensionFilter::Hide(text) => inputs.hide = text.clone(),
            HttpExtensionFilter::Any => {}
        }
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale().max(1.0);
        let mut target_w = px((WINDOW_WIDTH * scale).round());
        let mut target_h = px((if cfg!(test) { CONTENT_HEIGHT } else { WINDOW_HEIGHT } * scale).round());
        if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds();
            target_w = target_w.min(visible.size.width);
            target_h = target_h.min(visible.size.height);
        }
        window.resize(size(
            (target_w - (frame.width - viewport.width)).max(px(480.)),
            (target_h - (frame.height - viewport.height)).max(px(240.)),
        ));
        let only = cx.new(|cx| InputState::new(window, cx).default_value(inputs.only.clone()));
        let hide = cx.new(|cx| InputState::new(window, cx).default_value(inputs.hide.clone()));
        let search_term = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(applied.search_term.clone())
        });
        let listener_port = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(
                    applied
                        .listener_port
                        .map(|port| port.to_string())
                        .unwrap_or_default(),
                )
        });
        let script_mode = !applied.filter_script.is_empty();
        let initial_script = if applied.filter_script.is_empty() { "return true;".into() } else { applied.filter_script.clone() };
        let script_title = owner.read(cx).http_filter.script_title.clone();
        let library_name_input = cx.new(|cx| InputState::new(window, cx).default_value(script_title.clone()));
        let filter_script_editor = crate::editing::new_plain_editor(window, cx);
        filter_script_editor.update(cx, |state, cx| {
            state.set_soft_wrap(false, window, cx);
            state.set_highlighter_factory(script_highlighter_factory(), cx);
            if !initial_script.is_empty() {
                state.set_value(initial_script, window, cx);
            }
        });
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            if view.project != Some(this.project) {
                window.remove_window();
                return;
            }
            this.applied = view.http_filter.applied.clone();
            this.applied_inputs = view.http_filter.inputs.clone();
            if let Some((revision, close)) = this.pending {
                if let Some((_, outcome)) = view
                    .http_filter
                    .outcome
                    .as_ref()
                    .filter(|(actual, _)| *actual == revision)
                {
                    this.pending = None;
                    this.error = outcome.clone().err();
                    if this.error.is_none() {
                        this.applied_script_title = this.script_title.clone();
                        let owner = owner.clone();
                        let title = this.script_title.clone();
                        cx.defer(move |cx| owner.update(cx, |view, _| view.http_filter.script_title = title));
                    }
                    if close && this.error.is_none() {
                        window.remove_window();
                        return;
                    }
                } else if revision != view.query_revision
                    && view
                        .http_filter
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.wait_revision != revision)
                {
                    this.pending = None;
                    this.error = Some("筛选查询已被较新的操作替代；当前草稿仍保留。".into());
                }
            }
            cx.notify();
        });
        let released = owner.downgrade();
        let handle = window.window_handle();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.update(cx, |view, cx| {
                    if view.http_filter.window == Some(handle) {
                        view.http_filter.window = None;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        window.set_window_title("HTTP 历史筛选");
        Self {
            owner: owner.downgrade(),
            owner_window,
            project,
            draft: applied.clone(),
            script_mode,
            applied_script_title: script_title.clone(),
            script_title,
            script_reference_open: false,
            library_overlay: None,
            library_name_input,
            library_path: None,
            applied,
            applied_inputs: inputs,
            only,
            hide,
            search_term,
            listener_port,
            filter_script_editor,
            pending: None,
            submitting: false,
            error: None,
            focus,
            _subscription: subscription,
        }
    }

    fn preset(&mut self, hide_all: bool, cx: &mut Context<Self>) {
        let (search_term, regex, case_sensitive, negative_search) = if hide_all {
            (
                self.search_term.read(cx).value().to_string(),
                self.draft.regex,
                self.draft.case_sensitive,
                self.draft.negative_search,
            )
        } else {
            (String::new(), false, false, false)
        };
        self.draft = HttpHistoryFilter {
            hide_without_response: hide_all,
            hide_connect: hide_all,
            parameterized_only: hide_all,
            status_classes: [!hide_all; 4],
            mime_types: [!hide_all; 8],
            search_term,
            regex,
            case_sensitive,
            negative_search,
            ..Default::default()
        };
        self.error = None;
        cx.notify();
    }

    pub(super) fn show_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.validate_inputs_ime(window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.preset(false, cx);
        self.search_term
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
        self.listener_port
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
    }

    pub(super) fn hide_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.validate_inputs_ime(window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.preset(true, cx);
        self.listener_port
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
    }

    pub(super) fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.validate_inputs_ime(window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.draft = self.applied.clone();
        self.script_mode = !self.applied.filter_script.is_empty();
        self.script_title = self.applied_script_title.clone();
        self.only.update(cx, |input, cx| {
            input.set_value(self.applied_inputs.only.clone(), window, cx)
        });
        self.hide.update(cx, |input, cx| {
            input.set_value(self.applied_inputs.hide.clone(), window, cx)
        });
        self.search_term.update(cx, |input, cx| {
            input.set_value(self.applied.search_term.clone(), window, cx)
        });
        self.listener_port.update(cx, |input, cx| {
            input.set_value(
                self.applied
                    .listener_port
                    .map(|port| port.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.filter_script_editor.update(cx, |editor, cx| {
            let script = if self.applied.filter_script.is_empty() {
                "return true;".into()
            } else {
                self.applied.filter_script.clone()
            };
            editor.set_value(script, window, cx)
        });
        self.error = None;
        cx.notify();
    }

    pub(super) fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.validate_inputs_ime(window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        window.remove_window();
    }

    pub(super) fn switch_mode(&mut self, script_mode: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting { return; }
        if let Err(error) = self.validate_inputs_ime(window, cx) {
            self.error = Some(error.into());
        } else {
            self.script_mode = script_mode;
            self.error = None;
        }
        cx.notify();
    }

    fn script_library_path(&mut self) -> AnyResult<PathBuf> {
        if let Some(path) = &self.library_path { return Ok(path.clone()); }
        #[cfg(test)]
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(format!("mio-ui-test-filter-scripts-{}", mio_core::Uuid::new_v4()))
            .join("http-filter-scripts.json");
        #[cfg(not(test))]
        let path = mio_core::default_user_preferences_path()?.with_file_name("http-filter-scripts.json");
        self.library_path = Some(path.clone());
        Ok(path)
    }

    pub(super) fn open_library(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        let code = match editing::snapshot(&self.filter_script_editor, window, cx) {
            Ok(code) => code.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if save && let Err(error) = mio_core::validate_filter_script(&code) {
            self.error = Some(format!("{error:#}"));
            cx.notify();
            return;
        }
        let path = match self.script_library_path() {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        self.submitting = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_library(&path) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.submitting = false;
                match result {
                    Ok(library) => {
                        this.error = None;
                        this.library_overlay = Some(if save {
                            this.library_name_input.update(cx, |input, cx| {
                                input.set_value(this.script_title.clone(), window, cx)
                            });
                            LibraryOverlay::Save {
                                library,
                                code,
                                overwrite: None,
                            }
                        } else {
                            LibraryOverlay::Load { library }
                        });
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn close_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = editing::input_snapshot(&self.library_name_input, window, cx) {
            self.error = Some(error.into());
        } else {
            self.library_overlay = None;
            self.error = None;
        }
        cx.notify();
    }

    pub(super) fn load_library_script(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = editing::snapshot(&self.filter_script_editor, window, cx) {
            self.error = Some(error.into()); cx.notify(); return;
        }
        let Some(LibraryOverlay::Load { library }) = &self.library_overlay else { return; };
        let script = library.scripts.get(index).expect("脚本库所选索引须存在").clone();
        self.script_title = script.name;
        self.draft.filter_script = script.code.clone();
        self.filter_script_editor.update(cx, |editor, cx| editor.set_value(script.code, window, cx));
        self.library_overlay = None;
        self.error = None;
        cx.notify();
    }

    pub(super) fn save_library_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        let name = match editing::input_snapshot(&self.library_name_input, window, cx) {
            Ok(name) => name.trim().to_owned(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let Some(LibraryOverlay::Save {
            library,
            code,
            overwrite,
        }) = &mut self.library_overlay
        else {
            return;
        };
        if library.scripts.iter().any(|script| script.name == name)
            && overwrite.as_deref() != Some(&name)
        {
            *overwrite = Some(name);
            cx.notify();
            return;
        }
        let expected = library.clone();
        let code = code.clone();
        let path = match self.script_library_path() {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        self.submitting = true;
        cx.notify();
        let save_name = name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    write_library(
                        &path,
                        &expected,
                        LibraryScript {
                            name: save_name,
                            code,
                        },
                    )
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.submitting = false;
                match result {
                    Ok(_) => {
                        this.script_title = name;
                        this.library_overlay = None;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn validate_inputs_ime(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        editing::input_snapshot(&self.only, window, cx)
            .and_then(|_| editing::input_snapshot(&self.hide, window, cx))
            .and_then(|_| editing::input_snapshot(&self.search_term, window, cx))
            .and_then(|_| editing::input_snapshot(&self.listener_port, window, cx))
            .and_then(|_| editing::snapshot(&self.filter_script_editor, window, cx))
            .map(|_| ())
    }

    pub(super) fn convert_to_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        let only = match editing::input_snapshot(&self.only, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let hide = match editing::input_snapshot(&self.hide, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let search_term = match editing::input_snapshot(&self.search_term, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let listener_port = match editing::input_snapshot(&self.listener_port, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if editing::snapshot(&self.filter_script_editor, window, cx).is_err() {
            self.error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        if only.len() > 4096 || hide.len() > 4096 {
            self.error = Some("扩展名条件超过 4096 字节；当前输入保留。".into());
            cx.notify();
            return;
        }
        if search_term.len() > 4096 {
            self.error = Some("搜索词不能超过 4096 字节；当前输入保留。".into());
            cx.notify();
            return;
        }
        self.draft.extensions = match &self.draft.extensions {
            HttpExtensionFilter::Any => HttpExtensionFilter::Any,
            HttpExtensionFilter::Only(_) => HttpExtensionFilter::Only(only),
            HttpExtensionFilter::Hide(_) => HttpExtensionFilter::Hide(hide),
        };
        self.draft.search_term = search_term;
        self.draft.listener_port = if listener_port.is_empty() {
            None
        } else {
            match listener_port.parse() {
                Ok(port @ 1..=65535) => Some(port),
                _ => {
                    self.error = Some("Listener 端口必须是 1–65535 或留空；当前输入保留。".into());
                    cx.notify();
                    return;
                }
            }
        };
        let script = mio_core::convert_http_filter_to_script(&self.draft);
        self.draft.filter_script = script.clone();
        self.script_title = "Untitled".into();
        self.script_mode = true;
        self.filter_script_editor
            .update(cx, |editor, cx| editor.set_value(script, window, cx));
        self.error = None;
        cx.notify();
    }

    pub(super) fn load_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        if let Err(error) = editing::snapshot(&self.filter_script_editor, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        let template = mio_core::DEFAULT_HTTP_FILTER_SCRIPT_TEMPLATE.to_owned();
        self.script_title = "Untitled".into();
        self.draft.filter_script = template.clone();
        self.filter_script_editor
            .update(cx, |editor, cx| editor.set_value(template, window, cx));
        self.error = None;
        cx.notify();
    }

    pub(super) fn clear_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        if let Err(error) = editing::snapshot(&self.filter_script_editor, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.script_title = "Untitled".into();
        self.draft.filter_script = "return true;".into();
        self.filter_script_editor
            .update(cx, |editor, cx| editor.set_value("return true;", window, cx));
        self.error = None;
        cx.notify();
    }

    pub(super) fn apply(&mut self, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        let only = match editing::input_snapshot(&self.only, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let hide = match editing::input_snapshot(&self.hide, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let search_term = match editing::input_snapshot(&self.search_term, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let listener_port = match editing::input_snapshot(&self.listener_port, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let filter_script = match editing::snapshot(&self.filter_script_editor, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if self.script_mode {
            let trimmed = filter_script.trim();
            if let Err(error) = mio_core::validate_filter_script(trimmed) {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
            self.draft.filter_script = trimmed.to_owned();
        } else {
            if only.len() > 4096 || hide.len() > 4096 {
                self.error = Some("扩展名条件超过 4096 字节；当前输入保留。".into());
                cx.notify();
                return;
            }
            if search_term.len() > 4096 {
                self.error = Some("搜索词不能超过 4096 字节；当前输入保留。".into());
                cx.notify();
                return;
            }
            self.draft.extensions = match &self.draft.extensions {
                HttpExtensionFilter::Any => HttpExtensionFilter::Any,
                HttpExtensionFilter::Only(_) => HttpExtensionFilter::Only(only.clone()),
                HttpExtensionFilter::Hide(_) => HttpExtensionFilter::Hide(hide.clone()),
            };
            self.draft.search_term = search_term;
            self.draft.listener_port = if listener_port.is_empty() {
                None
            } else {
                match listener_port.parse() {
                    Ok(port @ 1..=65535) => Some(port),
                    _ => {
                        self.error =
                            Some("Listener 端口必须是 1–65535 或留空；当前输入保留。".into());
                        cx.notify();
                        return;
                    }
                }
            };
            self.draft.filter_script.clear();
        }
        let inputs = ExtensionInputs { only, hide };
        let draft = if self.script_mode {
            HttpHistoryFilter { filter_script: self.draft.filter_script.clone(), ..Default::default() }
        } else { self.draft.clone() };
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let project = self.project;
        let dialog = cx.weak_entity();
        let handle = window.window_handle();
        self.submitting = true;
        cx.notify();
        cx.defer(move |cx| {
            if !cx.windows().contains(&handle) || dialog.upgrade().is_none() {
                return;
            }
            let result = owner_window.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if view.project != Some(project) {
                        return Err("项目已改变；旧筛选未应用。".to_owned());
                    }
                    let (_, sort, enabled) = view.http_filter.requested();
                    let revision = view
                        .query_http_history_filtered(draft, sort, enabled, 0, window, cx)
                        .ok_or_else(|| {
                            view.error
                                .clone()
                                .unwrap_or_else(|| "筛选查询未发送。".into())
                        })?;
                    if let Some(pending) = view
                        .http_filter
                        .pending
                        .as_mut()
                        .filter(|pending| pending.query.query_revision == revision)
                    {
                        pending.inputs = inputs;
                        pending.wait_revision = revision;
                    }
                    Ok(revision)
                })
            });
            let _ = dialog.update(cx, |this, cx| {
                this.submitting = false;
                match result {
                    Ok(Ok(Ok(revision))) => {
                        this.pending = Some((revision, close));
                        this.error = None;
                    }
                    Ok(Ok(Err(error))) => this.error = Some(error),
                    _ => this.error = Some("项目窗口已关闭；筛选未应用。".into()),
                }
                cx.notify();
            });
        });
    }

    pub(super) fn checkbox(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        enabled: bool,
        update: impl Fn(&mut Self, bool) + 'static,
        cx: &Context<Self>,
    ) -> gpui_kit::base::Checkbox {
        let owner = cx.weak_entity();
        intruder::resource_pools::pool_checkbox(id, label, checked, enabled)
            .h(theme::scaled_px(21.))
            .min_h(theme::scaled_px(21.))
            .py_0()
            .gap(theme::scaled_px(5.))
            .on_change(
            move |value, _, _, cx| {
                let _ = owner.update(cx, |this, cx| {
                    update(this, value == gpui_kit::base::CheckboxState::Checked);
                    cx.notify();
                });
            },
        )
    }
}
