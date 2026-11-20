use super::*;
use crate::workbench::wb_render_chrome::sidebar_rail_tab_with_dock;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::{Bounds, WeakEntity, size};
use markdown::{ParseOptions, mdast::Node, to_mdast};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum NoteMarkdownAction {
    Bold,
    Italic,
    Strikethrough,
    Code,
    List,
    NumberedList,
    Heading,
    HeadingLevel(u8),
    Link,
}

fn heading_label(source: &str, cursor: usize) -> &'static str {
    fn depth_at(node: &Node, cursor: usize) -> Option<u8> {
        if let Node::Heading(heading) = node
            && let Some(position) = &heading.position
            && position.start.offset <= cursor
            && cursor <= position.end.offset
        {
            return Some(heading.depth);
        }
        node.children()?
            .iter()
            .find_map(|child| depth_at(child, cursor))
    }
    let parsed = to_mdast(source, &ParseOptions::gfm()).expect("Markdown 备注能够解析");
    match depth_at(&parsed, cursor) {
        Some(1) => "H1",
        Some(2) => "H2",
        Some(3) => "H3",
        Some(4) => "H4",
        Some(5) => "H5",
        Some(6) => "H6",
        _ => "Aa",
    }
}

fn heading_snippet(source: &str, level: u8) -> (String, usize, usize) {
    assert!(level <= 6, "Markdown 标题级别须在 0 至 6 范围内");
    let parsed = to_mdast(source, &ParseOptions::gfm()).expect("Markdown 备注能够解析");
    let mut text = source.to_owned();
    if let Some(children) = parsed.children() {
        for node in children.iter().rev() {
            if let Node::Heading(heading) = node {
                let position = heading.position.as_ref().expect("标题包含源码位置");
                let content = match (heading.children.first(), heading.children.last()) {
                    (Some(first), Some(last)) => {
                        let first = first.position().expect("标题内容包含源码位置");
                        let last = last.position().expect("标题内容包含源码位置");
                        &source[first.start.offset..last.end.offset]
                    }
                    _ => "",
                };
                text.replace_range(position.start.offset..position.end.offset, content);
            }
        }
    }
    if level == 0 {
        let len = text.len();
        return (text, 0, len);
    }
    let prefix = format!("{} ", "#".repeat(usize::from(level)));
    if !text.contains('\n') {
        let len = text.len();
        return (format!("{prefix}{text}"), prefix.len(), len);
    }
    let transformed = text
        .split_inclusive('\n')
        .map(|line| format!("{prefix}{line}"))
        .collect::<String>();
    let len = transformed.len();
    (transformed, 0, len)
}

impl Workbench {
    pub(in crate::workbench) fn apply_http_notes_markdown(
        &mut self,
        action: NoteMarkdownAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.http_notes.active == self.selected_http_notes()
            && self.http_notes.active.is_some();
        if !active
            || self.http_notes.stored.is_none()
            || self.http_notes.preview_mode
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let editor = self.http_notes.editor.clone();
        if editor.read(cx).has_marked_text() {
            self.http_notes.error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let text = editor.read(cx).value().to_string();
        let selected = editor.read(cx).selected_range();
        let range = if matches!(action, NoteMarkdownAction::HeadingLevel(_)) {
            let start = text[..selected.start].rfind('\n').map_or(0, |offset| offset + 1);
            let end_cursor = if !selected.is_empty() && text[..selected.end].ends_with('\n') {
                selected.end - 1
            } else {
                selected.end
            };
            let end = text[end_cursor..]
                .find('\n')
                .map_or(text.len(), |offset| end_cursor + offset);
            start..end
        } else {
            selected
        };
        let reversed = editor.read(cx).is_selection_reversed();
        let (replacement, inner_offset, inner_len) =
            format_markdown_snippet(action, &text, range.clone());
        if text.len() - range.len() + replacement.len() > MAX_NOTE_BYTES {
            self.http_notes.error = Some("备注超过 64 KiB；当前输入仍保留，请缩短后重试。".into());
            cx.notify();
            return;
        }
        let sel_start = range.start + inner_offset;
        let sel_end = sel_start + inner_len;
        editor.update(cx, |state, cx| {
            state.set_selected_range_with_direction(range, reversed, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range_with_direction(sel_start..sel_end, reversed, cx);
        });
        match self.capture_http_notes(window, cx) {
            Ok(()) => self.send_http_notes(false, cx),
            Err(error) => {
                self.http_notes.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(in crate::workbench) fn open_http_notes_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(note_id) = self.http_notes.active.filter(|id| {
            Some(*id) == self.selected_http_notes() && self.http_notes.stored.is_some()
        }) else {
            return;
        };
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        if let Err(error) = self.validate_http_notes_composition(window, cx) {
            self.http_notes.error = Some(error);
            cx.notify();
            return;
        }
        let owner = cx.entity();
        let owner_window = window.window_handle();
        let project = self.project;
        let read_revision = self.http_notes.read_revision;
        let bounds = Bounds::centered(None, size(px(640.), px(520.)), cx);
        cx.defer(move |cx| {
            if owner.read(cx).project != project
                || owner.read(cx).http_notes.active != Some(note_id)
                || owner.read(cx).http_notes.read_revision != read_revision
            {
                return;
            }
            if let Some(handle) = owner.read(cx).http_notes.detached_window
                && cx.windows().contains(&handle)
            {
                assert!(crate::chrome::activate_popup_window(handle, cx), "备注窗口能够激活");
                return;
            }
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        DetachedHttpNotesWindow::new(owner.clone(), window, cx)
                    })
                },
            )
            .expect("独立备注窗口能够打开");
            owner.update(cx, |view, cx| {
                view.http_notes.detached_window = Some(opened.0);
                view.http_notes.detached_owner_window = Some(owner_window);
                cx.notify();
            });
        });
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_http_notes(
        &self,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_note = self.selected_http_notes();
        let has_selected = selected_note.is_some();
        let notes = &self.http_notes;
        let notes_open = has_selected && notes.open;
        let inspector_open = self.inspector.open && !notes_open;
        let has_non_empty_note = selected_note.is_some_and(|id| {
            if let Some(draft) = notes.drafts.get(&id) {
                !draft.text.trim().is_empty()
            } else if notes.active == Some(id)
                && let Some(stored) = &notes.stored
            {
                !stored.text.trim().is_empty()
            } else if id.0.is_nil() {
                self.history
                    .read(cx)
                    .delegate()
                    .live_notes
                    .borrow()
                    .get(&id.1)
                    .is_some_and(|(_, preview)| !preview.trim().is_empty())
            } else {
                let entry_id = HistoryEntryId::External {
                    source_id: id.0,
                    item_id: id.1,
                };
                self.history
                    .read(cx)
                    .delegate()
                    .row_by_id(entry_id)
                    .or_else(|| self.imported_history.row.as_ref().filter(|r| r.id() == entry_id))
                    .is_some_and(|row| match row {
                        HistoryRow::ExtProxy(archive) => !archive.notes_preview.trim().is_empty(),
                        HistoryRow::Live(_) => false,
                    })
            }
        });
        let rail = sidebar_rail_with_dock(self.inspector.dock_left)
                    .child(
                        sidebar_rail_tab_with_dock(
                            "inspector-toggle",
                            "Inspector",
                            "Inspector",
                            IconName::SlidersHorizontal,
                            inspector_open,
                            false,
                            self.inspector.dock_left,
                            if inspector_open {
                                "收起 Inspector 检视面板"
                            } else {
                                "展开 Inspector 检视面板（查看参数、Cookie、协议头与选区编解码）"
                            },
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            let notes_open =
                                this.selected_http_notes().is_some() && this.http_notes.open;
                            if notes_open {
                                if !this.open_inspector(window, cx) {
                                    return;
                                }
                                let notes_focused =
                                    this.http_notes.editor.focus_handle(cx).is_focused(window);
                                this.http_notes.open = false;
                                if notes_focused {
                                    this.focus.focus(window, cx);
                                }
                                cx.notify();
                                return;
                            }
                            if this.inspector.open {
                                if !this.dismiss_inspector_for_current_selection(window, cx) {
                                    return;
                                }
                            } else if !this.open_inspector(window, cx) {
                                return;
                            } else {
                                this.http_notes.open = false;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        sidebar_rail_tab_with_dock(
                            "http-notes-toggle",
                            if has_non_empty_note {
                                "Notes\n•"
                            } else {
                                "Notes"
                            },
                            "Notes",
                            IconName::FileText,
                            notes_open,
                            !has_selected,
                            self.inspector.dock_left,
                            if !has_selected {
                                "请先在列表中选择一条 HTTP 记录以编辑备注"
                            } else if notes_open {
                                "收起 HTTP 消息备注侧栏"
                            } else {
                                "展开 HTTP 消息备注侧栏"
                            },
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.selected_http_notes().is_none()
                                || !this.leave_http_notes(window, cx)
                            {
                                return;
                            }
                            let notes_open =
                                this.selected_http_notes().is_some() && this.http_notes.open;
                            if notes_open {
                                let notes_focused =
                                    this.http_notes.editor.focus_handle(cx).is_focused(window);
                                this.http_notes.open = false;
                                if notes_focused {
                                    this.focus.focus(window, cx);
                                }
                            } else {
                                if !this.dismiss_inspector_for_current_selection(window, cx) {
                                    return;
                                }
                                this.http_notes.open = true;
                                this.select_http_notes(window, cx);
                            }
                            cx.notify();
                        })),
                    );
        if self.inspector.dock_left {
            div()
                .size_full()
                .flex()
                .child(rail)
                .when(inspector_open, |r| {
                    r.child(self.render_inspector_sidebar(cx))
                        .child(div().w(px(1.)).h_full().flex_shrink_0().bg(theme::border()))
                })
                .when(notes_open, |r| {
                    r.child(self.render_http_notes_sidebar(false, cx))
                        .child(div().w(px(1.)).h_full().flex_shrink_0().bg(theme::border()))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(body),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(body),
                )
                .when(inspector_open, |r| {
                    r.child(div().w(px(1.)).h_full().flex_shrink_0().bg(theme::border()))
                        .child(self.render_inspector_sidebar(cx))
                })
                .when(notes_open, |r| {
                    r.child(div().w(px(1.)).h_full().flex_shrink_0().bg(theme::border()))
                        .child(self.render_http_notes_sidebar(false, cx))
                })
                .child(rail)
                .into_any_element()
        }
    }

    #[inline(never)]
    fn render_http_notes_sidebar(&self, detached: bool, cx: &mut Context<Self>) -> AnyElement {
        let notes = &self.http_notes;
        let active = notes.active == self.selected_http_notes() && notes.active.is_some();
        let ready = active && notes.stored.is_some();
        let preview_mode = notes.preview_mode;
        let can_format = ready && !preview_mode && self.backend_available() && self.transition.is_none();
        let heading_source = notes.editor.read(cx).value();
        let heading_cursor = notes.editor.read(cx).cursor();
        let heading_current = heading_label(heading_source.as_ref(), heading_cursor);
        let heading_owner = cx.weak_entity();
        let error = notes
            .active
            .and_then(|id| notes.drafts.get(&id))
            .and_then(|draft| draft.error.clone())
            .or_else(|| notes.error.clone());
        let dock_left = self.inspector.dock_left;
        let inactive_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xdfdfdf }).into();
        div()
                        .id("http-notes-sidebar")
                        .test_support()
                        .w_full()
                        .when(!detached, |sidebar| sidebar.w(px(343.)))
                        .flex_shrink_0()
                        .h_full()
                        .bg(theme::toolbar_surface())
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("http-notes-title-bar")
                                .test_support()
                                .w_full()
                                .h(px(40.))
                                .min_h(px(40.))
                                .max_h(px(40.))
                                .px(px(10.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .bg(theme::toolbar_surface())
                                .border_b_1()
                                .border_color(theme::border())
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .text_color(theme::foreground())
                                        .child("Notes"),
                                )
                                .child(div().flex_1())
                                .when(!detached, |bar| bar.child(
                                    div()
                                        .w(px(50.))
                                        .h(px(19.))
                                        .flex()
                                        .items_center()
                                        .gap_0()
                                        .child(
                                            Button::new("http-notes-dock-left")
                                                .ghost()
                                                .small()
                                                .rounded(ButtonRounded::None)
                                                .w(px(25.))
                                                .h(px(19.))
                                                .p_0()
                                                .bg(if dock_left { rgb(0x26649d).into() } else { inactive_bg })
                                                .text_color(if dock_left { rgb(0xffffff).into() } else { theme::foreground() })
                                                .icon(gpui_kit::component::Icon::new(IconName::PanelLeft).size(px(13.)))
                                                .accessibility_label("Dock on the left")
                                                .tooltip("Dock on the left")
                                                .selected(dock_left)
                                                .toggled(dock_left)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.inspector.dock_left = true;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("http-notes-dock-right")
                                                .ghost()
                                                .small()
                                                .rounded(ButtonRounded::None)
                                                .w(px(25.))
                                                .h(px(19.))
                                                .p_0()
                                                .bg(if !dock_left { rgb(0x26649d).into() } else { inactive_bg })
                                                .text_color(if !dock_left { rgb(0xffffff).into() } else { theme::foreground() })
                                                .icon(gpui_kit::component::Icon::new(IconName::PanelRight).size(px(13.)))
                                                .accessibility_label("Dock on the right")
                                                .tooltip("Dock on the right")
                                                .selected(!dock_left)
                                                .toggled(!dock_left)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.inspector.dock_left = false;
                                                    cx.notify();
                                                })),
                                        ),
                                ))
                                .child(
                                    Button::new("http-notes-close")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .w(px(18.))
                                        .h(px(18.))
                                        .p_0()
                                        .icon(gpui_kit::component::Icon::new(IconName::Close).size(px(14.)))
                                        .accessibility_label("关闭备注")
                                        .tooltip("收起 HTTP 消息备注侧栏并自动保存草稿")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.leave_http_notes(window, cx) {
                                                if detached {
                                                    window.remove_window();
                                                    return;
                                                }
                                                let notes_focused = this
                                                    .http_notes
                                                    .editor
                                                    .focus_handle(cx)
                                                    .is_focused(window);
                                                this.http_notes.open = false;
                                                if notes_focused {
                                                    this.focus.focus(window, cx);
                                                }
                                                cx.notify();
                                            }
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .p(px(5.))
                                .relative()
                                .bg(theme::toolbar_surface())
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .id("http-notes-toolbar-bottom-rule")
                                        .test_support()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(px(42.))
                                        .h(px(1.))
                                        .bg(theme::control_border()),
                                )
                                .child(
                                    div()
                                        .id("http-notes-body-frame")
                                        .test_support()
                                        .w_full()
                                        .flex_1()
                                        .min_h_0()
                                        .bg(theme::toolbar_surface())
                                        .flex()
                                        .flex_col()
                                        .child(
                            div()
                                .id("http-notes-toolbar")
                                .test_support()
                                .w_full()
                                .h(px(38.))
                                .min_h(px(38.))
                                .max_h(px(38.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .flex_nowrap()
                                .gap(px(5.))
                                .px(px(9.))
                                .bg(theme::toolbar_surface())
                                .child(
                                    Button::new("http-notes-preview-toggle")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("T")
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(
                                                    gpui_kit::assets::AllAssets::get("icons/type.svg")
                                                        .expect("文本图标存在")
                                                        .data
                                                        .as_ref(),
                                                )
                                                .size(px(15.)),
                                        )
                                        .selected(preview_mode)
                                        .toggled(preview_mode)
                                        .tooltip(if preview_mode {
                                            "切换回 Markdown 源码编辑模式 (Edit as plain text)"
                                        } else {
                                            "以富文本排版预览当前 Markdown 备注内容"
                                        })
                                        .disabled(!ready)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            if !this.http_notes.preview_mode
                                                && let Err(error) =
                                                    this.validate_http_notes_composition(window, cx)
                                            {
                                                this.http_notes.error = Some(error);
                                                cx.notify();
                                                return;
                                            }
                                            this.http_notes.preview_mode =
                                                !this.http_notes.preview_mode;
                                            cx.notify();
                                        })),
                                )
                                .child(div().w(px(1.)).h(px(20.)).flex_shrink_0().bg(theme::control_border()))
                                .child(
                                    Button::new("http-notes-heading")
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(56.))
                                        .min_w(px(56.))
                                        .h(px(27.))
                                        .p(px(6.))
                                        .rounded(ButtonRounded::Small)
                                        .bg(theme::toolbar_surface())
                                        .border_1()
                                        .border_color(theme::control_border())
                                        .accessibility_label(heading_current)
                                        .tooltip("设置正文 Aa 或 H1 至 H6 标题级别")
                                        .disabled(!can_format)
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .text_color(theme::foreground())
                                                        .child(heading_current),
                                                )
                                                .child(
                                                    gpui_kit::component::Icon::new(IconName::ChevronDown)
                                                        .size(px(12.))
                                                        .text_color(theme::foreground()),
                                                ),
                                        )
                                        .dropdown_menu(move |mut menu, _, _| {
                                            for (level, label) in ["Aa", "H1", "H2", "H3", "H4", "H5", "H6"].into_iter().enumerate() {
                                                let owner = heading_owner.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::element(move |_, _| {
                                                        div()
                                                            .id(SharedString::from(format!("http-notes-heading-{label}")))
                                                            .test_support()
                                                            .role(gpui_kit::accesskit::Role::MenuItem)
                                                            .aria_label(label)
                                                            .w_full()
                                                            .min_h(px(22.))
                                                            .child(label)
                                                    })
                                                    .on_click(move |_, window, cx| {
                                                        owner.update(cx, |view, cx| {
                                                            view.apply_http_notes_markdown(NoteMarkdownAction::HeadingLevel(level as u8), window, cx);
                                                        }).expect("备注标题控件宿主有效");
                                                    }),
                                                );
                                            }
                                            menu
                                        }),
                                )
                                .child(
                                    Button::new("http-notes-bold")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("B")
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(
                                                    gpui_kit::assets::AllAssets::get("icons/bold.svg")
                                                        .expect("加粗图标存在")
                                                        .data
                                                        .as_ref(),
                                                )
                                                .size(px(15.)),
                                        )
                                        .tooltip("为所选备注文本添加 Markdown 加粗格式 (**文本**)")
                                        .disabled(!can_format)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_http_notes_markdown(
                                                NoteMarkdownAction::Bold,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    Button::new("http-notes-italic")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("I")
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(
                                                    gpui_kit::assets::AllAssets::get("icons/italic.svg")
                                                        .expect("斜体图标存在")
                                                        .data
                                                        .as_ref(),
                                                )
                                                .size(px(15.)),
                                        )
                                        .tooltip("为所选备注文本添加 Markdown 斜体格式 (*文本*)")
                                        .disabled(!can_format)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_http_notes_markdown(
                                                NoteMarkdownAction::Italic,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    Button::new("http-notes-strikethrough")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("S")
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(
                                                    gpui_kit::assets::AllAssets::get("icons/strikethrough.svg")
                                                        .expect("删除线图标存在")
                                                        .data
                                                        .as_ref(),
                                                )
                                                .size(px(15.)),
                                        )
                                        .tooltip("为所选备注文本添加删除线格式 (~~文本~~)")
                                        .disabled(!can_format)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_http_notes_markdown(
                                                NoteMarkdownAction::Strikethrough,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(div().w(px(1.)).h(px(20.)).flex_shrink_0().bg(theme::control_border()))
                                .child(
                                    Button::new("http-notes-list")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("List")
                                        .child(
                                            gpui_kit::component::Icon::new(IconName::List).size(px(15.)),
                                        )
                                        .tooltip("将所选备注行格式化为 Markdown 无序列表")
                                        .disabled(!can_format)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_http_notes_markdown(
                                                NoteMarkdownAction::List,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    Button::new("http-notes-numbered-list")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("Numbered list")
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(
                                                    gpui_kit::assets::AllAssets::get("icons/list-ordered.svg")
                                                        .expect("有序列表图标存在")
                                                        .data.as_ref(),
                                                )
                                                .size(px(15.)),
                                        )
                                        .tooltip("将所选备注行格式化为 Markdown 有序列表")
                                        .disabled(!can_format)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_http_notes_markdown(
                                                NoteMarkdownAction::NumberedList,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(div().w(px(1.)).h(px(20.)).flex_shrink_0().bg(theme::control_border()))
                                .child(
                                    Button::new("http-notes-format-link")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(19.))
                                        .min_w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .accessibility_label("Link")
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(
                                                    gpui_kit::assets::AllAssets::get("icons/link.svg")
                                                        .expect("链接图标存在")
                                                        .data.as_ref(),
                                                )
                                                .size(px(15.)),
                                        )
                                        .tooltip("将所选备注文本格式化为超链接")
                                        .disabled(!can_format)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_http_notes_markdown(
                                                NoteMarkdownAction::Link,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(div().w(px(1.)).h(px(20.)).flex_shrink_0().bg(theme::control_border()))
                                .child(
                                    Button::new("http-notes-open-window")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .flex_shrink_0()
                                        .w(px(15.))
                                        .min_w(px(15.))
                                        .h(px(15.))
                                        .p_0()
                                        .accessibility_label("Open in new window")
                                        .child(
                                            gpui_kit::component::Icon::new(IconName::ExternalLink)
                                                .size(px(15.)),
                                        )
                                        .tooltip("Open in new window")
                                        .disabled(!ready || self.transition.is_some())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_http_notes_window(window, cx);
                                        })),
                                ),
                        )
                        .when_some(error, |this, error| {
                            this.child(
                                div()
                                    .id("http-notes-error")
                                    .test_support()
                                    .role(gpui_kit::accesskit::Role::Alert)
                                    .aria_label(error.clone())
                                    .px(px(8.))
                                    .max_h(theme::scaled_px(54.))
                                    .overflow_y_scroll()
                                    .text_color(theme::danger_fg())
                                    .child(error)
                                    .child(
                                        button("http-notes-retry", "重试")
                                            .icon(IconName::RefreshCw)
                                            .tooltip("重新提交并同步当前 HTTP 消息备注")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if this.leave_http_notes(window, cx) {
                                                    this.http_notes.error = None;
                                                    this.send_http_notes(true, cx);
                                                    this.select_http_notes(window, cx);
                                                }
                                            })),
                                    ),
                            )
                        })
                        .child(div().flex_1().min_h_0().bg(theme::toolbar_surface()).child(if ready {
                            if !detached && notes.detached_window.is_some() {
                                div()
                                    .p(px(10.))
                                    .text_size(px(12.))
                                    .child("当前备注正在独立窗口中编辑。")
                                    .into_any_element()
                            } else if preview_mode {
                                render_markdown_preview(notes.editor.read(cx).value().as_ref())
                            } else {
                                Editor::new(&notes.editor)
                                    .readonly(!self.backend_available())
                                    .appearance(true)
                                    .size_full()
                                    .bordered(false)
                                    .rounded(px(0.))
                                    .bg(theme::toolbar_surface())
                                    .aria_label("HTTP 归档 Notes")
                                    .into_any_element()
                            }
                        } else {
                            div()
                                .p(px(10.))
                                .child(if notes.reading {
                                    "正在读取备注…"
                                } else {
                                    "选择一条消息查看备注"
                                })
                                .into_any_element()
                        })),
                        ),
                        )
            .into_any_element()
    }
}

struct DetachedHttpNotesWindow {
    owner: WeakEntity<Workbench>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl DetachedHttpNotesWindow {
    fn new(owner: Entity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = owner.read(cx).project;
        let note_id = owner.read(cx).http_notes.active;
        let read_revision = owner.read(cx).http_notes.read_revision;
        let handle = window.window_handle();
        let subscription = cx.observe_in(&owner, window, move |_, owner, window, cx| {
            let view = owner.read(cx);
            if view.project != project
                || view.http_notes.active != note_id
                || view.selected_http_notes() != note_id
                || view.http_notes.read_revision != read_revision
            {
                window.remove_window();
                return;
            }
            cx.notify();
        });
        let closing_owner = owner.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            closing_owner.update(cx, |view, cx| {
                if let Err(error) = view.capture_http_notes_app(cx) {
                    view.http_notes.error = Some(error);
                    cx.notify();
                    return false;
                }
                view.send_http_notes(false, cx);
                true
            }).expect("独立备注窗口宿主有效")
        });
        let released_owner = owner.downgrade();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released_owner.update(cx, |view, cx| {
                    if view.http_notes.detached_window == Some(handle) {
                        view.http_notes.detached_window = None;
                        view.http_notes.detached_owner_window = None;
                        cx.notify();
                    }
                });
            });
        }).detach();
        window.set_window_title("Notes");
        Self {
            owner: owner.downgrade(),
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }
}

impl Render for DetachedHttpNotesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.upgrade().expect("独立备注窗口宿主有效");
        let close_owner = owner.downgrade();
        let title_bar = crate::chrome::render_title_bar(
            "http-notes-detached",
            "Notes",
            crate::chrome::TitleBarControls::DIALOG,
            move |window, cx| {
                let can_close = close_owner.update(cx, |view, cx| {
                    if let Err(error) = view.capture_http_notes_app(cx) {
                        view.http_notes.error = Some(error);
                        cx.notify();
                        return false;
                    }
                    view.send_http_notes(false, cx);
                    true
                }).expect("独立备注窗口宿主有效");
                if can_close {
                    window.remove_window();
                }
            },
            window,
            cx,
        );
        let body = owner.update(cx, |view, cx| view.render_http_notes_sidebar(true, cx));
        div()
            .id("http-notes-detached-window")
            .test_support()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
    }
}

pub(in crate::workbench) fn format_markdown_snippet(
    action: NoteMarkdownAction,
    text: &str,
    range: std::ops::Range<usize>,
) -> (String, usize, usize) {
    let selected = &text[range.clone()];
    match action {
        NoteMarkdownAction::Bold => {
            let inner = if selected.is_empty() {
                "文本"
            } else {
                selected
            };
            (format!("**{inner}**"), 2, inner.len())
        }
        NoteMarkdownAction::Italic => {
            let inner = if selected.is_empty() {
                "文本"
            } else {
                selected
            };
            (format!("*{inner}*"), 1, inner.len())
        }
        NoteMarkdownAction::Strikethrough => {
            let inner = if selected.is_empty() {
                "文本"
            } else {
                selected
            };
            (format!("~~{inner}~~"), 2, inner.len())
        }
        NoteMarkdownAction::Code => {
            let inner = if selected.is_empty() {
                "code"
            } else {
                selected
            };
            if inner.contains('\n') {
                (format!("```\n{inner}\n```"), 4, inner.len())
            } else {
                (format!("`{inner}`"), 1, inner.len())
            }
        }
        NoteMarkdownAction::List => {
            if selected.is_empty() {
                let prefix = if range.start > 0 && !text[..range.start].ends_with('\n') {
                    "\n- "
                } else {
                    "- "
                };
                (format!("{prefix}列表项"), prefix.len(), "列表项".len())
            } else {
                let mut out = String::new();
                for (idx, line) in selected.split('\n').enumerate() {
                    if idx > 0 {
                        out.push('\n');
                    }
                    if line.starts_with("- ") {
                        out.push_str(line);
                    } else {
                        out.push_str("- ");
                        out.push_str(line);
                    }
                }
                let len = out.len();
                (out, 0, len)
            }
        }
        NoteMarkdownAction::NumberedList => {
            if selected.is_empty() {
                let prefix = if range.start > 0 && !text[..range.start].ends_with('\n') {
                    "\n1. "
                } else {
                    "1. "
                };
                (format!("{prefix}列表项"), prefix.len(), "列表项".len())
            } else {
                let mut out = String::new();
                for (idx, line) in selected.split('\n').enumerate() {
                    if idx > 0 {
                        out.push('\n');
                    }
                    let num_prefix = format!("{}. ", idx + 1);
                    if line.starts_with(&num_prefix) {
                        out.push_str(line);
                    } else {
                        out.push_str(&num_prefix);
                        out.push_str(line);
                    }
                }
                let len = out.len();
                (out, 0, len)
            }
        }
        NoteMarkdownAction::Heading => {
            if selected.is_empty() {
                let prefix = if range.start > 0 && !text[..range.start].ends_with('\n') {
                    "\n# "
                } else {
                    "# "
                };
                (format!("{prefix}标题"), prefix.len(), "标题".len())
            } else {
                let mut out = String::new();
                for (idx, line) in selected.split('\n').enumerate() {
                    if idx > 0 {
                        out.push('\n');
                    }
                    if line.starts_with("# ") {
                        out.push_str(line);
                    } else {
                        out.push_str("# ");
                        out.push_str(line);
                    }
                }
                let len = out.len();
                (out, 0, len)
            }
        }
        NoteMarkdownAction::HeadingLevel(level) => heading_snippet(selected, level),
        NoteMarkdownAction::Link => {
            let inner = if selected.is_empty() {
                "链接"
            } else {
                selected
            };
            (format!("[{inner}](https://)"), 1, inner.len())
        }
    }
}

pub(in crate::workbench) fn render_markdown_preview(source: &str) -> AnyElement {
    div()
        .id("http-notes-markdown-preview")
        .test_support()
        .size_full()
        .p(px(10.))
        .overflow_y_scroll()
        .child(
            gpui_kit::base::TextView::markdown(
                "http-notes-markdown-body",
                source.to_owned(),
            )
            .selectable(true),
        )
        .into_any_element()
}
