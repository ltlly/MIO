use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;

fn group(
    id: &'static str,
    label: &'static str,
    width: f32,
    height: f32,
) -> gpui_kit::base::ObservedElement<gpui_kit::Stateful<gpui_kit::Div>> {
    div().id(id)
        .test_support().role(gpui_kit::accesskit::Role::Group).aria_label(label)
        .w(theme::scaled_px(width)).h(theme::scaled_px(height)).flex_shrink_0().relative()
        .pt(theme::scaled_px(19.)).px(theme::scaled_px(5.)).flex().flex_col().gap(theme::scaled_px(5.))
        .child(div().absolute().top(theme::scaled_px(8.)).bottom(theme::scaled_px(3.)).left_0().right_0()
            .border_1().border_color(theme::border()))
        .child(div().absolute().top_0().left(theme::scaled_px(5.))
            .px(theme::scaled_px(3.)).h(theme::scaled_px(17.))
            .line_height(theme::scaled_px(17.)).bg(theme::background()).child(label))
}

fn dialog_button(id: &'static str, label: &'static str, width: f32) -> Button {
    button(id, label).w(theme::scaled_px(width)).h(theme::scaled_px(23.))
        .flex_shrink_0().px_0().font_weight(gpui_kit::FontWeight::BOLD)
        .bg(rgb(if theme::is_dark() { 0x33363a } else { 0xeeeeee }))
}

fn filter_input(state: &Entity<InputState>, enabled: bool) -> Input {
    let mut input = Input::new(state)
        .small()
        .px(theme::scaled_px(4.))
        .py_0()
        .text_size(theme::ui_font_size())
        .bg(theme::toolbar_surface())
        .disabled(!enabled);
    input.style().size.height = Some(theme::scaled_px(23.).into());
    input
}

fn mode_button(id: &'static str, label: &'static str, width: f32, active: bool, enabled: bool) -> Button {
    Button::new(id)
        .ghost()
        .rounded(ButtonRounded::None)
        .p_0()
        .w(theme::scaled_px(width))
        .h(theme::scaled_px(21.))
        .flex_shrink_0()
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .label(label)
        .selected(active)
        .disabled(!enabled)
        .bg(rgb(if active { 0x26649d } else if theme::is_dark() { 0x35383d } else { 0xdfdfdf }))
        .text_color(if active { rgb(0xffffff).into() } else { theme::foreground() })
}

pub(super) fn filter_summary(filter: &HttpHistoryFilter) -> String {
    if !filter.filter_script.is_empty() {
        return "筛选设置：Script mode (脚本已启用)".into();
    }
    let mut parts = Vec::new();
    if filter.in_scope_only { parts.push("仅显示范围内".into()); }
    if filter.hide_without_response { parts.push("隐藏无响应项".to_owned()); }
    if filter.hide_connect { parts.push("隐藏 CONNECT".to_owned()); }
    if filter.parameterized_only { parts.push("仅显示含参数请求".to_owned()); }
    if filter.status_classes != [true; 4] { parts.push("按状态码筛选".into()); }
    if matches!(
        filter.mime_types,
        [true, true, true, false, true, false, true, true]
    ) {
        parts.push("隐藏 CSS 与图像内容".into());
    } else if filter.mime_types != [true; 8] {
        parts.push("按 MIME 类型筛选".into());
    }
    match &filter.extensions {
        HttpExtensionFilter::Any => {}
        HttpExtensionFilter::Only(_) => parts.push("仅显示指定扩展名".into()),
        HttpExtensionFilter::Hide(_) => parts.push("隐藏指定扩展名".into()),
    }
    if !filter.search_term.is_empty() {
        parts.push(if filter.negative_search { format!("排除表达式 {}", filter.search_term) }
            else { format!("匹配表达式 {}", filter.search_term) });
    }
    if filter.only_with_notes { parts.push("仅显示有备注项".into()); }
    if filter.only_highlighted { parts.push("仅显示高亮项".into()); }
    if let Some(port) = filter.listener_port { parts.push(format!("Listener 端口 {port}")); }
    if parts.is_empty() { "筛选设置：显示全部".into() }
    else { format!("筛选设置：{}", parts.join("；")) }
}

impl FilterWindow {
    fn render_settings(&self, enabled: bool, cx: &Context<Self>) -> AnyElement {
        let only_selected = matches!(self.draft.extensions, HttpExtensionFilter::Only(_));
        let hide_selected = matches!(self.draft.extensions, HttpExtensionFilter::Hide(_));
        let request = group("http-filter-request-group", "按请求类型筛选", 328., 123.)
            .child(self.checkbox("http-filter-in-scope-only", "仅显示范围内条目", self.draft.in_scope_only,
                enabled, |this, value| this.draft.in_scope_only = value, cx))
            .child(self.checkbox("http-filter-no-response", "隐藏没有响应的项", self.draft.hide_without_response,
                enabled, |this, value| this.draft.hide_without_response = value, cx))
            .child(self.checkbox("http-filter-hide-connect", "隐藏 CONNECT 请求", self.draft.hide_connect,
                enabled, |this, value| this.draft.hide_connect = value, cx))
            .child(self.checkbox("http-filter-parameterized-only", "仅显示含参数的请求", self.draft.parameterized_only,
                enabled, |this, value| this.draft.parameterized_only = value, cx));
        let mime = group("http-filter-mime-group", "按 MIME 类型筛选", 299., 123.)
            .children([
                [(0, "HTML", "http-filter-mime-html"), (4, "其他文本", "http-filter-mime-other-text")],
                [(1, "Script", "http-filter-mime-script"), (5, "图像", "http-filter-mime-images")],
                [(2, "XML", "http-filter-mime-xml"), (6, "Flash", "http-filter-mime-flash")],
                [(3, "CSS", "http-filter-mime-css"), (7, "其他二进制", "http-filter-mime-other-binary")],
            ].into_iter().map(|row| {
                div().h(theme::scaled_px(21.)).flex_shrink_0().flex().items_center().children(row.into_iter().map(|(index, label, id)| {
                    div().w(theme::scaled_px(144.)).child(self.checkbox(id, label, self.draft.mime_types[index],
                        enabled, move |this, value| this.draft.mime_types[index] = value, cx))
                }))
            }));
        let status = group("http-filter-status-group", "按状态码筛选", 235., 123.)
            .children([(0, "http-filter-2xx", "2xx  [成功]"), (1, "http-filter-3xx", "3xx  [重定向]"),
                (2, "http-filter-4xx", "4xx  [请求错误]"), (3, "http-filter-5xx", "5xx  [服务器错误]")]
                .into_iter().map(|(index, id, label)| self.checkbox(id, label, self.draft.status_classes[index],
                    enabled, move |this, value| this.draft.status_classes[index] = value, cx)));
        let search = group("http-filter-search-group", "按搜索词筛选", 253., 99.)
            .child(filter_input(&self.search_term, enabled).id("http-filter-search-term"))
            .child(self.checkbox("http-filter-regex", "正则表达式", self.draft.regex, enabled,
                |this, value| this.draft.regex = value, cx))
            .child(div().flex().gap(theme::scaled_px(5.))
                .child(div().w(theme::scaled_px(115.)).child(self.checkbox("http-filter-case-sensitive",
                    "区分大小写", self.draft.case_sensitive, enabled, |this, value| this.draft.case_sensitive = value, cx)))
                .child(self.checkbox("http-filter-negative", "反向搜索", self.draft.negative_search, enabled,
                    |this, value| this.draft.negative_search = value, cx)));
        let extension = group("http-filter-extension-group", "按文件扩展名筛选", 287., 99.)
            .pt(theme::scaled_px(28.)).gap(theme::scaled_px(10.))
            .child(div().flex().items_center().gap(theme::scaled_px(5.))
                .child(div().w(theme::scaled_px(93.)).child(self.checkbox("http-filter-only-extensions", "仅显示：",
                    only_selected, enabled, |this, value| this.draft.extensions = if value {
                        HttpExtensionFilter::Only(String::new())
                    } else { HttpExtensionFilter::Any }, cx)))
                .child(div().w(theme::scaled_px(179.)).child(filter_input(&self.only, enabled))))
            .child(div().flex().items_center().gap(theme::scaled_px(5.))
                .child(div().w(theme::scaled_px(93.)).child(div().w(theme::scaled_px(60.)).child(self.checkbox("http-filter-hide-extensions", "隐藏：",
                    hide_selected, enabled, |this, value| this.draft.extensions = if value {
                        HttpExtensionFilter::Hide(String::new())
                    } else { HttpExtensionFilter::Any }, cx))))
                .child(div().w(theme::scaled_px(179.)).child(filter_input(&self.hide, enabled))));
        let annotation = group("http-filter-annotation-group", "按注解筛选", 202., 99.)
            .pt(theme::scaled_px(30.)).gap(theme::scaled_px(10.))
            .child(self.checkbox("http-filter-notes", "仅显示有备注的项", self.draft.only_with_notes, enabled,
                |this, value| this.draft.only_with_notes = value, cx))
            .child(self.checkbox("http-filter-highlight", "仅显示高亮项", self.draft.only_highlighted, enabled,
                |this, value| this.draft.only_highlighted = value, cx));
        let listener = group("http-filter-listener-group", "按监听器筛选", 112., 99.)
            .pt(theme::scaled_px(45.))
            .child(div().flex().items_center().gap(theme::scaled_px(7.))
                .child(div().w(theme::scaled_px(26.)).child("端口"))
                .child(div().w(theme::scaled_px(69.)).child(filter_input(&self.listener_port, enabled)
                    .id("http-filter-listener-port"))));
        div().id("http-filter-settings-body").test_support().w(theme::scaled_px(884.))
            .flex_shrink_0().flex().flex_col().gap(theme::scaled_px(10.))
            .child(div().id("http-filter-settings-first-row").test_support().flex()
                .pl(theme::scaled_px(1.)).gap(theme::scaled_px(10.)).child(request).child(mime).child(status))
            .child(div().id("http-filter-settings-second-row").test_support().flex()
                .gap(theme::scaled_px(10.)).child(search).child(extension).child(annotation).child(listener))
            .into_any_element()
    }

    fn render_script(&self, enabled: bool, cx: &Context<Self>) -> AnyElement {
        let owner = cx.weak_entity();
        div().id("http-filter-script-body").test_support().flex_1().min_h_0().flex().flex_col()
            .child(div().px(theme::scaled_px(15.)).h(theme::scaled_px(19.)).flex_shrink_0()
                .font_weight(gpui_kit::FontWeight::BOLD).text_size(theme::scaled_px(15.))
                .child(self.script_title.clone()))
            .child(div().h(theme::scaled_px(37.)).flex_shrink_0().px(theme::scaled_px(15.)).pt(theme::scaled_px(5.))
                .flex().items_start().gap(theme::scaled_px(10.))
                .child(div().id("http-filter-script-signature").test_support().w(theme::scaled_px(630.)).h(theme::scaled_px(19.))
                    .flex_shrink_0().flex().items_center().overflow_hidden().whitespace_nowrap()
                    .font_family(theme::mono_font_family())
                    .child(div().font_weight(gpui_kit::FontWeight::BOLD).border_b_1().border_dashed().border_color(theme::muted_foreground()).child("boolean"))
                    .child(div().child(" matches(ProxyHttpRequestResponse requestResponse, Utilities utilities)")))
                .child(Button::new("http-filter-script-reference").ghost().compact().size(theme::scaled_px(21.))
                    .border_1().border_color(theme::border())
                    .bg(rgb(if theme::is_dark() { 0x33363a } else { 0xeeeeee }))
                    .icon(IconName::ChevronDown).accessibility_label("脚本作用域与函数说明").disabled(!enabled)
                    .selected(self.script_reference_open)
                    .on_click(cx.listener(|this, _, _, cx| { this.script_reference_open = !this.script_reference_open; cx.notify(); })))
                .child(div().flex_1().min_w_0())
                .child(dialog_button("http-filter-script-load", "加载", 88.)
                    .icon(gpui_kit::component::Icon::default().data(
                        gpui_kit::assets::AllAssets::get("icons/download.svg").expect("加载图标存在").data.as_ref(),
                    ).size(theme::scaled_px(13.))).disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.open_library(false, window, cx))))
                .child(dialog_button("http-filter-script-new", "新建", 85.).icon(IconName::Plus).disabled(!enabled)
                    .dropdown_menu(move |menu, _, _| {
                        let fresh = owner.clone();
                        let template = owner.clone();
                        menu.item(PopupMenuItem::new("新建脚本").on_click(move |_, window, cx| {
                            let _ = fresh.update(cx, |this, cx| this.clear_script(window, cx));
                        })).item(PopupMenuItem::element(|_, _| div().id("http-filter-script-template").test_support()
                            .w_full().child("HTTP 筛选模板")).on_click(move |_, window, cx| {
                            let _ = template.update(cx, |this, cx| this.load_template(window, cx));
                        }))
                    })))
            .when(self.script_reference_open, |body| body.child(div().id("http-filter-script-capabilities").test_support()
                .h(theme::scaled_px(126.)).flex_shrink_0().overflow_y_scroll().px(theme::scaled_px(15.))
                .flex().flex_col().gap(theme::scaled_px(4.))
                .child("基于 Rhai 引擎执行，支持 return 语句、requestResponse / utilities 同名方法链及顶层字段，不支持 Java 类型声明与 import。")
                .child("request：method、url、host、path、query、version、headers、body、raw；response：status、length、version、headers、body、raw。")
                .child("作用域：notes、highlighted、listener_port、in_scope、parameterized、has_response、mime_index、search_text。")
                .child("函数：header_value、has_header、query_param、regex_matches、regex_replace、url_encode、url_decode、base64_encode、base64_decode、sha256_hex、md5_hex、crc32_hex、unix_timestamp、unix_timestamp_ms。")
                .child("限制：脚本 64 KiB、50000 次运算、表达式深度 64、函数调用深度 32、Map/Array 4096 项；消息字符串使用 UTF-8 替换无效字节。")))
            .child(div().id("http-filter-script-editor").test_support().flex_1().min_h_0()
                .border_t_1().border_b_1().border_color(theme::border()).bg(theme::surface())
                .child(crate::editing::message_editor(&self.filter_script_editor).size_full().readonly(!enabled).aria_label("HTTP MioScript Script")))
            .into_any_element()
    }

    fn render_library_overlay(&self, cx: &Context<Self>) -> AnyElement {
        let overlay = self.library_overlay.as_ref().expect("脚本库覆盖层须已打开");
        let mut panel = div().id("http-filter-library-overlay").test_support().absolute().inset_0()
            .bg(theme::background()).p(theme::scaled_px(15.)).flex().flex_col().gap(theme::scaled_px(10.));
        match overlay {
            form::LibraryOverlay::Load { library } => {
                panel = panel.child(div().font_weight(gpui_kit::FontWeight::BOLD).child("加载脚本库"))
                    .child(div().id("http-filter-library-list").test_support().flex_1().min_h_0().overflow_y_scroll()
                        .flex().flex_col().gap(theme::scaled_px(5.))
                        .when(library.scripts.is_empty(), |list| list.child("脚本库尚未保存脚本。"))
                        .children(library.scripts.iter().enumerate().map(|(index, script)| {
                            button(("http-filter-library-entry", index), script.name.clone()).justify_start()
                                .on_click(cx.listener(move |this, _, window, cx| this.load_library_script(index, window, cx)))
                        })));
            }
            form::LibraryOverlay::Save { overwrite, .. } => {
                panel = panel.child(div().font_weight(gpui_kit::FontWeight::BOLD).child("保存至脚本库"))
                    .child("脚本名称")
                    .child(Input::new(&self.library_name_input).id("http-filter-library-name").small())
                    .when_some(overwrite.clone(), |panel, name| panel.child(div().id("http-filter-library-overwrite-warning")
                        .test_support().child(format!("脚本库已有“{name}”，确认后替换该脚本。"))))
                    .child(div().flex_1())
                    .child(dialog_button("http-filter-library-save", if overwrite.is_some() { "确认覆盖" } else { "保存" }, 100.)
                        .on_click(cx.listener(|this, _, window, cx| this.save_library_script(window, cx))));
            }
        }
        panel.when_some(self.error.clone(), |panel, error| panel.child(div().id("http-filter-library-error")
            .test_support().role(gpui_kit::accesskit::Role::Alert).aria_label(error.clone())
            .max_h(theme::scaled_px(80.)).overflow_y_scroll().text_color(theme::danger_fg()).child(error)))
            .child(div().flex().justify_end().child(dialog_button("http-filter-library-cancel", "取消", 84.)
                .on_click(cx.listener(|this, _, window, cx| this.close_library(window, cx)))))
            .into_any_element()
    }
}

impl Render for FilterWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close_owner = cx.weak_entity();
        let title_bar = crate::chrome::render_title_bar("mio-http-filter", "HTTP 历史筛选",
            crate::chrome::TitleBarControls::DIALOG,
            move |window, cx| { let _ = close_owner.update(cx, |this, cx| this.cancel(window, cx)); }, window, cx);
        let enabled = self.pending.is_none() && !self.submitting && self.library_overlay.is_none();
        let script_mode = self.script_mode;
        let mode_bar = div().id("http-filter-mode-bar").test_support().h(theme::scaled_px(37.)).flex_shrink_0()
            .pt(theme::scaled_px(15.)).px(theme::scaled_px(15.)).flex().justify_between()
            .child(div().flex()
                .child(mode_button("http-filter-mode-settings", "设置模式", 98., !script_mode, enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.switch_mode(false, window, cx))))
                .child(mode_button("http-filter-mode-script", "脚本模式", 85., script_mode, enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.switch_mode(true, window, cx)))))
            .child(div().flex().gap(theme::scaled_px(5.))
                .when(script_mode, |row| {
                    row.child(
                        Button::new("http-filter-github-scripts")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(22.))
                            .child(gpui_kit::component::Icon::new(IconName::Github).size(theme::scaled_px(18.)))
                            .accessibility_label("Browse scripts on GitHub")
                            .tooltip("Browse scripts on GitHub")
                            .on_click(|_, _, cx| cx.open_url("https://github.com/ltlly/MIO/blob/main/docs/scripting-api.md")),
                    )
                })
                .child(Popover::new("http-filter-help-popover").trigger(Button::new("http-filter-help").ghost().compact().p_0()
                    .size(theme::scaled_px(22.)).child(gpui_kit::component::Icon::new(IconName::CircleQuestionMark).size(theme::scaled_px(18.))).accessibility_label("筛选帮助"))
                    .content(|_, _, _| div().w(theme::scaled_px(340.)).flex().flex_col().gap(theme::scaled_px(6.))
                        .child("筛选条件在应用成功后生效。设置模式支持请求类型、MIME、状态码、搜索词、扩展名、注解及监听端口。")
                        .child("脚本模式执行 Rhai 表达式并返回 bool，加载与保存操作使用本地脚本库。")))
                .child(Button::new("http-filter-user-settings").ghost().compact().p_0().size(theme::scaled_px(22.))
                    .child(gpui_kit::component::Icon::new(IconName::Settings).size(theme::scaled_px(18.))).accessibility_label("消息编辑器设置")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let owner = this.owner.clone();
                        let handle = this.owner_window;
                        cx.defer(move |cx| { let _ = handle.update(cx, |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| view.open_settings_filtered(
                                Some(settings::SettingsPage::UserMessageEditor), Some(settings::SettingsScopeFilter::User), false, window, cx));
                        }); });
                    }))));
        let body = div().id("http-filter-body").test_support().flex_1().min_h_0()
            .py(theme::scaled_px(15.)).flex().flex_col()
            .when(script_mode, |body| body.child(self.render_script(enabled, cx)))
            .when(!script_mode, |body| body.child(div().id("http-filter-settings-scroll").flex_1().min_h_0()
                .px(theme::scaled_px(13.)).overflow_y_scroll().overflow_x_scroll().child(self.render_settings(enabled, cx))));
        let footer_left = if script_mode {
            div().flex_1().min_w_0().child(dialog_button("http-filter-script-save-library", "保存至脚本库…", 160.)
                .h(theme::scaled_px(24.))
                .icon(gpui_kit::component::Icon::default().data(
                    gpui_kit::assets::AllAssets::get("icons/save.svg").expect("保存图标存在").data.as_ref(),
                ).size(theme::scaled_px(13.)))
                .disabled(!enabled).on_click(cx.listener(|this, _, window, cx| this.open_library(true, window, cx))))
        } else {
            div().flex_1().min_w_0().flex().items_center()
                .child(dialog_button("http-filter-all", "显示全部", 94.).disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.show_all(window, cx))))
                .child(div().w(theme::scaled_px(12.)).flex_shrink_0())
                .child(dialog_button("http-filter-hide-all", "隐藏全部", 87.).disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.hide_all(window, cx))))
                .child(div().w(theme::scaled_px(12.)).flex_shrink_0())
                .child(dialog_button("http-filter-revert", "还原更改", 138.).disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.revert(window, cx))))
                .child(div().w(theme::scaled_px(22.)).flex_shrink_0())
                .child(div().w(px(1.)).h(theme::scaled_px(24.)).bg(theme::border()).flex_shrink_0())
                .child(div().w(theme::scaled_px(22.)).flex_shrink_0())
                .child(dialog_button("http-filter-convert-to-script", "转换为脚本", 158.)
                    .icon(gpui_kit::component::Icon::default().data(
                        gpui_kit::assets::AllAssets::get("icons/arrow-left-right.svg").expect("转换图标存在").data.as_ref(),
                    ).size(theme::scaled_px(14.))).disabled(!enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.convert_to_script(window, cx))))
        };
        let footer = div().id("http-filter-footer").test_support().flex_shrink_0().flex().flex_col()
            .when_some(self.error.clone().filter(|_| self.library_overlay.is_none()), |footer, error| footer.child(
                div().id("http-filter-error").test_support().role(gpui_kit::accesskit::Role::Alert)
                    .aria_label(error.clone()).max_h(theme::scaled_px(34.)).min_h(theme::scaled_px(17.))
                    .overflow_y_scroll().px(theme::scaled_px(15.)).text_color(theme::danger_fg()).child(error)))
            .child(div().h(theme::scaled_px(39.)).pb(theme::scaled_px(15.)).px(theme::scaled_px(15.)).flex()
                .gap(theme::scaled_px(22.)).child(footer_left)
                .child(div().w(theme::scaled_px(312.)).flex_shrink_0().pl(theme::scaled_px(5.))
                    .flex().gap(theme::scaled_px(10.))
                    .child(dialog_button("http-filter-cancel", "取消", 84.).disabled(self.library_overlay.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))))
                    .child(dialog_button("http-filter-apply", "应用", 77.).disabled(!enabled)
                        .on_click(cx.listener(|this, _, window, cx| this.apply(false, window, cx))))
                    .child(dialog_button("http-filter-apply-close", "应用并关闭", 126.).primary().bg(rgb(0xe05326)).text_color(rgb(0xffffff)).disabled(!enabled)
                        .on_click(cx.listener(|this, _, window, cx| this.apply(true, window, cx))))));
        let content = div().id("http-filter-dialog").test_support().track_focus(&self.focus)
            .size_full().relative().flex().flex_col().child(mode_bar).child(body).child(footer)
            .when(self.library_overlay.is_some(), |content| content.child(self.render_library_overlay(cx)));
        div().size_full().flex().flex_col().font_family(theme::ui_font_family()).text_size(theme::ui_font_size())
            .line_height(theme::scaled_px(17.)).bg(theme::background()).text_color(theme::foreground())
            .child(title_bar).child(div().flex_1().min_h_0().child(content))
    }
}
