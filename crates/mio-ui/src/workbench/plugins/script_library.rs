use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result as AnyResult, ensure};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::ButtonVariants,
    input::{Textarea, TextareaState},
};
use serde::{Deserialize, Serialize};

use super::render_suite::mio_neutral_button;
use super::*;

const MAX_SCRIPT_LIBRARY_SCRIPTS: usize = 128;
const MAX_SCRIPT_LIBRARY_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScriptLibraryItem {
    pub(super) name: String,
    pub(super) code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredScriptPresetLibrary {
    version: u32,
    scripts: Vec<ScriptLibraryItem>,
}

impl Default for StoredScriptPresetLibrary {
    fn default() -> Self {
        Self {
            version: 1,
            scripts: Vec::new(),
        }
    }
}

pub(super) struct ScriptPresetLibraryUi {
    pub(super) loaded: bool,
    pub(super) library_path: Option<PathBuf>,
    pub(super) scripts: Vec<ScriptLibraryItem>,
    pub(super) selected_name: Option<String>,
    pub(super) name_input: Entity<InputState>,
    pub(super) code_editor: Entity<TextareaState>,
    pub(super) feedback: Option<(bool, String)>,
}

impl ScriptPresetLibraryUi {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("脚本名称（如：仅显示 4xx/5xx 响应）"));
        let code_editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(
                    "输入 MioScript / Rhai 筛选脚本，例如：\nresponse.status >= 400 && request.method == \"POST\"",
                )
                .soft_wrap(true)
                .autocorrect(false)
        });
        Self {
            loaded: false,
            library_path: None,
            scripts: Vec::new(),
            selected_name: None,
            name_input,
            code_editor,
            feedback: None,
        }
    }

    pub(super) fn resolved_path(&mut self) -> AnyResult<PathBuf> {
        if let Some(path) = &self.library_path {
            return Ok(path.clone());
        }
        #[cfg(test)]
        let path = std::env::temp_dir().join(format!(
            "mio-ui-test-scripts-{}.json",
            mio_core::Uuid::new_v4()
        ));
        #[cfg(not(test))]
        let path =
            mio_core::default_user_preferences_path()?.with_file_name("http-filter-scripts.json");
        self.library_path = Some(path.clone());
        Ok(path)
    }
}

fn validate_script_name(name: &str) -> AnyResult<String> {
    let trimmed = name.trim();
    ensure!(
        !trimmed.is_empty()
            && trimmed.len() <= 256
            && !trimmed.chars().any(char::is_control),
        "脚本名称须为非空、不含控制字符且不超过 256 字节的文本"
    );
    Ok(trimmed.to_owned())
}

fn validate_stored_library(library: &StoredScriptPresetLibrary) -> AnyResult<()> {
    ensure!(library.version == 1, "脚本库版本无效（须为 version = 1）");
    ensure!(
        library.scripts.len() <= MAX_SCRIPT_LIBRARY_SCRIPTS,
        "脚本库最多保存 {MAX_SCRIPT_LIBRARY_SCRIPTS} 个脚本"
    );
    for (index, script) in library.scripts.iter().enumerate() {
        let valid_name = validate_script_name(&script.name)?;
        ensure!(
            valid_name == script.name,
            "脚本名称首尾不得包含空白字符：{}",
            script.name
        );
        ensure!(
            !library.scripts[..index]
                .iter()
                .any(|prior| prior.name == script.name),
            "脚本库包含重复名称：{}",
            script.name
        );
        mio_core::validate_filter_script(&script.code)?;
    }
    Ok(())
}

fn read_script_library_file(path: &Path) -> AnyResult<StoredScriptPresetLibrary> {
    let Some(bytes) =
        mio_core::ui_files::read_bounded_optional_file(path, MAX_SCRIPT_LIBRARY_BYTES, "脚本库")?
    else {
        return Ok(StoredScriptPresetLibrary::default());
    };
    let library: StoredScriptPresetLibrary = serde_json::from_slice(&bytes)
        .with_context(|| format!("解析脚本库 JSON 失败：{}", path.display()))?;
    validate_stored_library(&library)?;
    Ok(library)
}

fn write_script_library_file(path: &Path, scripts: &[ScriptLibraryItem]) -> AnyResult<()> {
    let library = StoredScriptPresetLibrary {
        version: 1,
        scripts: scripts.to_vec(),
    };
    validate_stored_library(&library)?;
    let bytes = serde_json::to_vec_pretty(&library)?;
    mio_core::ui_files::atomic_write_bounded_file(
        path,
        &bytes,
        MAX_SCRIPT_LIBRARY_BYTES,
        "脚本库",
    )?;
    Ok(())
}

fn parse_imported_script_json(bytes: &[u8]) -> AnyResult<Vec<ScriptLibraryItem>> {
    ensure!(
        bytes.len() as u64 <= MAX_SCRIPT_LIBRARY_BYTES,
        "导入文件超过 4 MiB 上限"
    );
    if let Ok(lib) = serde_json::from_slice::<StoredScriptPresetLibrary>(bytes) {
        validate_stored_library(&lib)?;
        return Ok(lib.scripts);
    }
    let items: Vec<ScriptLibraryItem> =
        serde_json::from_slice(bytes).context("JSON 格式无效：应为 ScriptLibrary 对象或脚本数组")?;
    let wrapped = StoredScriptPresetLibrary {
        version: 1,
        scripts: items,
    };
    validate_stored_library(&wrapped)?;
    Ok(wrapped.scripts)
}

pub(super) const MIO_SCRIPT_TEMPLATES: [(&str, &str, &str); 8] = [
    (
        "仅显示 4xx/5xx 响应",
        "4xx/5xx 响应",
        "response.status >= 400",
    ),
    (
        "POST JSON 接口",
        "POST JSON",
        "request.method == \"POST\" && (response.mime_type == \"JSON\" || response.mime_type.contains(\"json\"))",
    ),
    (
        "携带 Set-Cookie 响应",
        "Set-Cookie",
        "response.hasHeader(\"Set-Cookie\")",
    ),
    (
        "仅显示含参数请求",
        "含参数请求",
        "request.hasParameters()",
    ),
    (
        "响应包含敏感关键字",
        "敏感关键字",
        "response.body.contains(\"password\") || response.body.contains(\"secret\") || response.body.contains(\"token\")",
    ),
    (
        "大响应体 (>100KB)",
        "大响应 (>100KB)",
        "response.body.len() > 102400",
    ),
    (
        "携带 Authorization 认证头",
        "Bearer/Auth",
        "request.hasHeader(\"Authorization\")",
    ),
    (
        "3xx 重定向响应",
        "3xx 重定向",
        "response.status >= 300 && response.status < 400 && response.hasHeader(\"Location\")",
    ),
];

impl Workbench {
    fn ensure_script_library_loaded(&mut self) {
        if self.extensions.script_library.loaded {
            return;
        }
        self.extensions.script_library.loaded = true;
        match self
            .extensions
            .script_library
            .resolved_path()
            .and_then(|path| read_script_library_file(&path))
        {
            Ok(lib) => {
                self.extensions.script_library.scripts = lib.scripts;
                self.extensions.script_library.feedback = None;
            }
            Err(err) => {
                self.extensions.script_library.feedback =
                    Some((false, format!("加载本地脚本库失败：{err:#}")));
            }
        }
    }

    pub(in crate::workbench) fn reload_script_library(&mut self, cx: &mut Context<Self>) {
        self.extensions.script_library.loaded = false;
        self.ensure_script_library_loaded();
        if self
            .extensions
            .script_library
            .feedback
            .as_ref()
            .is_none_or(|(ok, _)| *ok)
        {
            let count = self.extensions.script_library.scripts.len();
            self.extensions.script_library.feedback =
                Some((true, format!("已从本地脚本库重新加载 {count} 个 脚本。")));
            self.status = format!("已刷新 脚本库（{count} 个脚本）。");
        }
        cx.notify();
    }

    pub(in crate::workbench) fn new_script_library_script(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extensions.script_library.selected_name = None;
        self.extensions.script_library.feedback = None;
        self.extensions
            .script_library
            .name_input
            .update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        self.extensions
            .script_library
            .code_editor
            .update(cx, |editor, cx| {
                editor.set_value("response.status >= 400", window, cx);
            });
        cx.notify();
    }

    pub(in crate::workbench) fn select_script_library_script(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .extensions
            .script_library
            .scripts
            .iter()
            .find(|item| item.name == name)
            .cloned()
        else {
            return;
        };
        self.extensions.script_library.selected_name = Some(entry.name.clone());
        self.extensions.script_library.feedback = None;
        self.extensions
            .script_library
            .name_input
            .update(cx, |input, cx| input.set_value(entry.name, window, cx));
        self.extensions
            .script_library
            .code_editor
            .update(cx, |editor, cx| editor.set_value(entry.code, window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn validate_script_library_editor(&mut self, cx: &mut Context<Self>) {
        let code = self
            .extensions
            .script_library
            .code_editor
            .read(cx)
            .content()
            .text()
            .to_string();
        match mio_core::validate_filter_script(&code) {
            Ok(()) => {
                self.extensions.script_library.feedback =
                    Some((true, "编译通过：脚本语法有效。".to_owned()));
                self.error = None;
                self.status = "脚本编译校验通过。".into();
            }
            Err(err) => {
                self.extensions.script_library.feedback =
                    Some((false, format!("编译失败：{err}")));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn save_script_library_script(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_script_library_loaded();
        let raw_name = match editing::input_snapshot(
            &self.extensions.script_library.name_input,
            window,
            cx,
        ) {
            Ok(v) => v.to_string(),
            Err(err) => {
                self.extensions.script_library.feedback = Some((false, err.to_owned()));
                cx.notify();
                return;
            }
        };
        let name = match validate_script_name(&raw_name) {
            Ok(v) => v,
            Err(err) => {
                self.extensions.script_library.feedback = Some((false, err.to_string()));
                cx.notify();
                return;
            }
        };
        let code = self
            .extensions
            .script_library
            .code_editor
            .read(cx)
            .content()
            .text()
            .to_string();
        if let Err(err) = mio_core::validate_filter_script(&code) {
            self.extensions.script_library.feedback = Some((false, format!("未保存，编译失败：{err}")));
            cx.notify();
            return;
        }
        let mut next = self.extensions.script_library.scripts.clone();
        let prev_selected = self.extensions.script_library.selected_name.clone();
        if let Some(prev) = &prev_selected
            && prev != &name
        {
            if next.iter().any(|s| s.name == name) {
                self.extensions.script_library.feedback =
                    Some((false, format!("已存在名为「{name}」的脚本。")));
                cx.notify();
                return;
            }
            if let Some(existing) = next.iter_mut().find(|s| &s.name == prev) {
                existing.name = name.clone();
                existing.code = code.clone();
            } else {
                next.push(ScriptLibraryItem {
                    name: name.clone(),
                    code: code.clone(),
                });
            }
        } else if let Some(existing) = next.iter_mut().find(|s| s.name == name) {
            existing.code = code.clone();
        } else {
            next.push(ScriptLibraryItem {
                name: name.clone(),
                code: code.clone(),
            });
        }
        let result = self
            .extensions
            .script_library
            .resolved_path()
            .and_then(|path| write_script_library_file(&path, &next));
        match result {
            Ok(()) => {
                self.extensions.script_library.scripts = next;
                self.extensions.script_library.selected_name = Some(name.clone());
                self.extensions.script_library.feedback =
                    Some((true, format!("已保存 脚本「{name}」到本地脚本库。")));
                self.error = None;
                self.status = format!("已保存 脚本「{name}」。");
            }
            Err(err) => {
                self.extensions.script_library.feedback =
                    Some((false, format!("写入脚本库失败：{err:#}")));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn delete_script_library_script(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_script_library_loaded();
        let mut next = self.extensions.script_library.scripts.clone();
        let before = next.len();
        next.retain(|item| item.name != name);
        if next.len() == before {
            return;
        }
        let result = self
            .extensions
            .script_library
            .resolved_path()
            .and_then(|path| write_script_library_file(&path, &next));
        match result {
            Ok(()) => {
                self.extensions.script_library.scripts = next;
                if self.extensions.script_library.selected_name.as_deref() == Some(name.as_str()) {
                    self.extensions.script_library.selected_name = None;
                    self.extensions
                        .script_library
                        .name_input
                        .update(cx, |inp, cx| inp.set_value("", window, cx));
                    self.extensions
                        .script_library
                        .code_editor
                        .update(cx, |ed, cx| ed.set_value("", window, cx));
                }
                self.extensions.script_library.feedback =
                    Some((true, format!("已删除 脚本「{name}」。")));
                self.error = None;
                self.status = format!("已删除 脚本「{name}」。");
            }
            Err(err) => {
                self.extensions.script_library.feedback =
                    Some((false, format!("删除脚本失败：{err:#}")));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn import_script_library_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("导入 脚本库 JSON".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if !this.finish_file_picker(picker, cx) {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let imported = match mio_core::ui_files::read_file_bytes(&path)
                            .map_err(anyhow::Error::from)
                            .and_then(|bytes| parse_imported_script_json(&bytes))
                        {
                            Ok(items) => items,
                            Err(err) => {
                                this.extensions.script_library.feedback =
                                    Some((false, format!("导入 JSON 失败：{err:#}")));
                                cx.notify();
                                return;
                            }
                        };
                        this.ensure_script_library_loaded();
                        let mut merged = this.extensions.script_library.scripts.clone();
                        let count = imported.len();
                        for script in imported {
                            if let Some(existing) =
                                merged.iter_mut().find(|s| s.name == script.name)
                            {
                                existing.code = script.code;
                            } else {
                                merged.push(script);
                            }
                        }
                        let save_res = this
                            .extensions
                            .script_library
                            .resolved_path()
                            .and_then(|p| write_script_library_file(&p, &merged));
                        match save_res {
                            Ok(()) => {
                                this.extensions.script_library.scripts = merged;
                                this.extensions.script_library.feedback = Some((
                                    true,
                                    format!("已成功导入 {count} 个 脚本并写入本地脚本库。"),
                                ));
                                this.error = None;
                                this.status = format!("已导入 {count} 个 脚本。");
                            }
                            Err(err) => {
                                this.extensions.script_library.feedback =
                                    Some((false, format!("保存导入的脚本库失败：{err:#}")));
                            }
                        }
                        cx.notify();
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        this.extensions.script_library.feedback =
                            Some((false, format!("无法打开导入文件：{err}")));
                        cx.notify();
                    }
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn export_script_library_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        self.ensure_script_library_loaded();
        let lib = StoredScriptPresetLibrary {
            version: 1,
            scripts: self.extensions.script_library.scripts.clone(),
        };
        let payload = match serde_json::to_vec_pretty(&lib) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.extensions.script_library.feedback =
                    Some((false, format!("序列化脚本库失败：{err}")));
                cx.notify();
                return;
            }
        };
        let count = lib.scripts.len();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some("script-library.json"),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension("json");
                            }
                            match mio_core::ui_files::write_export_file(&path, &payload) {
                                Ok(()) => {
                                    view.extensions.script_library.feedback = Some((
                                        true,
                                        format!(
                                            "已导出 {count} 个 脚本到：{}",
                                            path.display()
                                        ),
                                    ));
                                    view.error = None;
                                    view.status =
                                        format!("已导出 脚本库到：{}", path.display());
                                }
                                Err(err) => {
                                    view.extensions.script_library.feedback =
                                        Some((false, format!("写入导出文件失败：{err}")));
                                }
                            }
                            cx.notify();
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => {
                            view.extensions.script_library.feedback =
                                Some((false, format!("无法选择导出路径：{err}")));
                            cx.notify();
                        }
                        Err(_) => {}
                    }
                })
            });
        })
        .detach();
    }

    pub(super) fn render_extensions_script_library(
        &mut self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_script_library_loaded();
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();
        let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        let sel_border: gpui_kit::Hsla = rgb(if dark { 0x5b8ad6 } else { 0x4876b5 }).into();

        let selected_name = self.extensions.script_library.selected_name.clone();
        let scripts = self.extensions.script_library.scripts.clone();
        let feedback = self.extensions.script_library.feedback.clone();

        let mut table = div()
            .id("extensions-script-table")
            .test_support()
            .w_full()
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(table_border)
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .text_color(header_fg)
                    .child(
                        div()
                            .w(px(220.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("脚本名称 (Name)"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("脚本表达式预览 (Script)"),
                    )
                    .child(
                        div()
                            .w(px(120.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child("操作"),
                    ),
            );

        if scripts.is_empty() {
            table = table.child(
                div()
                    .id("extensions-script-empty")
                    .test_support()
                    .min_h(px(72.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(12.))
                    .text_size(px(12.))
                    .text_color(theme::muted_foreground())
                    .child("本地脚本库暂无自定义 脚本。点击「新建脚本」或下方快捷模板创建并复用至 Proxy 历史筛选。"),
            );
        } else {
            for (index, item) in scripts.iter().enumerate() {
                let is_selected = selected_name.as_deref() == Some(item.name.as_str());
                let name_for_click = item.name.clone();
                let name_for_edit = item.name.clone();
                let name_for_del = item.name.clone();
                let preview = item
                    .code
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                let row_bg = if is_selected {
                    theme::selected()
                } else if index % 2 == 1 {
                    zebra_bg
                } else {
                    theme::surface()
                };
                let row_hover = if is_selected {
                    theme::selected()
                } else {
                    hover_bg
                };

                table = table.child(
                    div()
                        .id(("extensions-script-row", index))
                        .test_support()
                        .min_h(px(26.))
                        .w_full()
                        .bg(row_bg)
                        .when(is_selected, |row| {
                            row.border_t_1().border_b_1().border_color(sel_border)
                        })
                        .hover(move |s| s.bg(row_hover))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_size(theme::ui_font_size())
                        .text_color(theme::foreground())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_script_library_script(name_for_click.clone(), window, cx);
                        }))
                        .child(
                            div()
                                .w(px(220.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .py(px(3.))
                                .truncate()
                                .child(item.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.))
                                .py(px(3.))
                                .font_family(theme::mono_font_family())
                                .text_size(px(12.))
                                .truncate()
                                .child(preview),
                        )
                        .child(
                            div()
                                .w(px(120.))
                                .flex_shrink_0()
                                .px(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(
                                    button(("extensions-script-edit", index), "编辑")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.select_script_library_script(
                                                name_for_edit.clone(),
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    button(("extensions-script-row-del", index), "删除")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.delete_script_library_script(
                                                name_for_del.clone(),
                                                window,
                                                cx,
                                            );
                                        })),
                                ),
                        ),
                );
            }
        }

        let editor_card = div()
            .id("extensions-script-editor-card")
            .test_support()
            .w_full()
            .p(px(12.))
            .border_1()
            .border_color(table_border)
            .rounded(px(3.))
            .bg(theme::toolbar_surface())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child(match &selected_name {
                                Some(name) => format!("编辑脚本：{name}"),
                                None => "新建 脚本".to_owned(),
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(4.))
                            .children(MIO_SCRIPT_TEMPLATES.into_iter().enumerate().map(
                                |(idx, (full_name, chip_label, sample_code))| {
                                    button(
                                        SharedString::from(format!("extensions-script-tpl-{idx}")),
                                        format!("模板：{chip_label}"),
                                    )
                                    .ghost()
                                    .small()
                                    .tooltip(format!("填入示例脚本：{full_name}"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.extensions.script_library.selected_name = None;
                                        this.extensions.script_library.name_input.update(
                                            cx,
                                            |inp, cx| inp.set_value(full_name, window, cx),
                                        );
                                        this.extensions.script_library.code_editor.update(
                                            cx,
                                            |ed, cx| ed.set_value(sample_code, window, cx),
                                        );
                                        this.extensions.script_library.feedback = None;
                                        cx.notify();
                                    }))
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(72.))
                            .flex_shrink_0()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("脚本名称"),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.extensions.script_library.name_input)
                                .id("extensions-script-name-input")
                                .aria_label("脚本名称")
                                .small()
                                .h(px(26.)),
                        ),
                    ),
            )
            .child(
                div()
                    .id("extensions-script-code-editor")
                    .test_support()
                    .w_full()
                    .h(px(120.))
                    .font_family(theme::mono_font_family())
                    .text_size(theme::editor_font_size())
                    .child(
                        Textarea::new(&self.extensions.script_library.code_editor)
                            .size_full()
                            .bordered(true)
                            .aria_label("脚本代码编辑器"),
                    ),
            )
            .when_some(feedback, |card, (ok, msg)| {
                card.child(
                    div()
                        .id("extensions-script-feedback")
                        .test_support()
                        .text_size(px(12.))
                        .text_color(if ok {
                            rgb(0x2e8b57).into()
                        } else {
                            theme::danger_fg()
                        })
                        .child(msg),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button("extensions-script-validate", "测试编译")
                            .icon(IconName::ShieldCheck)
                            .tooltip("使用 MIO MioScript/Rhai 编译器验证当前脚本语法")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.validate_script_library_editor(cx);
                            })),
                    )
                    .child(
                        button("extensions-script-save", "保存脚本")
                            .primary()
                            .icon(IconName::Save)
                            .tooltip("编译验证并保存到共享本地 ScriptLibrary（与 HTTP 历史筛选互通）")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_script_library_script(window, cx);
                            })),
                    )
                    .when_some(selected_name, |row, sel| {
                        row.child(
                            button("extensions-script-delete", "删除当前")
                                .ghost()
                                .icon(IconName::Trash)
                                .tooltip("从本地脚本库删除当前选中的 脚本")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.delete_script_library_script(sel.clone(), window, cx);
                                })),
                        )
                    }),
            );

        div()
            .id("extensions-script-library-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child("脚本库 (MioScript library)"),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("管理本地 脚本库（http-filter-scripts.json），新建、编辑、编译校验与导入导出后可在 Proxy HTTP 历史筛选与 Match and replace 中直接复用。"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(px(108.)).child(
                            mio_neutral_button(
                                "extensions-script-new",
                                "新建脚本",
                                "清空编辑器以创建新的 脚本",
                                true,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_script_library_script(window, cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(116.)).child(
                            mio_neutral_button(
                                "extensions-script-import",
                                "导入 JSON…",
                                "从 JSON 文件导入并校验 脚本",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_script_library_json(window, cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(116.)).child(
                            mio_neutral_button(
                                "extensions-script-export",
                                "导出 JSON…",
                                "将当前脚本库导出为 JSON 文件",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export_script_library_json(window, cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(84.)).child(
                            mio_neutral_button(
                                "extensions-script-reload",
                                "刷新",
                                "从磁盘重新加载本地 ScriptLibrary",
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reload_script_library(cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(156.)).child(
                            mio_neutral_button(
                                "extensions-script-open-history",
                                "打开 HTTP 历史筛选",
                                "跳转到 Proxy → HTTP 历史记录使用 筛选脚本",
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tool = Tool::Proxy;
                                this.proxy_page = ProxyPage::History;
                                cx.notify();
                            })),
                        ),
                    )
                    .child(
                        div().w(px(174.)).child(
                            mio_neutral_button(
                                "extensions-script-open-mr",
                                "打开 Match and replace",
                                "跳转到 Proxy → Match and replace 配置 脚本改写规则",
                                true,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.tool = Tool::Proxy;
                                this.switch_proxy_page(ProxyPage::MatchAndReplace, window, cx);
                            })),
                        ),
                    ),
            )
            .child(table)
            .child(editor_card)
            .into_any_element()
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn all_script_library_templates_validate_and_evaluate() {
        let raw_req = b"POST /api?id=1 HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer tok\r\n\r\n{\"a\":1}";
        let raw_resp = b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nSet-Cookie: sid=1\r\nLocation: https://example.com/next\r\n\r\n{\"error\":\"secret\"}";
        for (idx, (title, _, script)) in MIO_SCRIPT_TEMPLATES.into_iter().enumerate() {
            mio_core::validate_filter_script(script)
                .unwrap_or_else(|e| panic!("mio_script template {title} syntax error: {e}"));
            let matched = mio_core::eval_http_filter_script(
                script,
                "POST",
                "https://example.com/api?id=1",
                Some(500),
                raw_resp.len() as u64,
                Some(raw_req),
                Some(raw_resp),
                "",
                false,
                Some(8080),
            )
            .unwrap_or_else(|e| panic!("mio_script template {title} eval error: {e}"));
            if matches!(idx, 0 | 1 | 2 | 3 | 4 | 6) {
                assert!(
                    matched,
                    "mio_script template [{idx}] {title} should match sample traffic"
                );
            } else {
                assert!(
                    !matched,
                    "mio_script template [{idx}] {title} should not match sample traffic"
                );
            }
        }
    }
}
