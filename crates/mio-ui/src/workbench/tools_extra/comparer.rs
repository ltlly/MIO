use super::*;

mod diff;
mod render;

const MAX_COMPARER_ITEMS: usize = 64;
const MAX_COMPARER_ITEM_BYTES: usize = 4 * 1024 * 1024;

impl Workbench {
    pub(in crate::workbench) fn send_to_comparer(
        &mut self,
        response: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        match self.tool_message_bytes(response, window, cx) {
            Ok(bytes) => {
                self.add_comparer_item(if response { "响应" } else { "请求" }, bytes, window, cx);
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Comparer;
                self.focus.focus(window, cx);
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    fn add_comparer_item(
        &mut self,
        label: &str,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if bytes.len() > MAX_COMPARER_ITEM_BYTES {
            self.fail(
                format!(
                    "Comparer 单项超过 {} KiB 上限，未添加。",
                    MAX_COMPARER_ITEM_BYTES / 1024
                ),
                cx,
            );
            return;
        }
        if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
            self.fail(
                format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                cx,
            );
            return;
        }
        let id = self
            .extra_tools
            .comparer_items
            .last()
            .map_or(1, |item| item.id + 1);
        self.extra_tools.comparer_items.push(ComparerItem {
            id,
            label: format!("{id}: {label} ({} bytes)", bytes.len()),
            bytes,
        });
        let count = self.extra_tools.comparer_items.len();
        self.extra_tools.comparer_selected = count - 1;
        if count >= 2 {
            self.extra_tools.left_index = count - 2;
            self.extra_tools.right_index = count - 1;
        }
        self.refresh_comparer(window, cx);
    }

    pub(in crate::workbench) fn remove_selected_comparer_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.extra_tools.comparer_items.is_empty() {
            return;
        }
        let idx = self
            .extra_tools
            .comparer_selected
            .min(self.extra_tools.comparer_items.len() - 1);
        self.extra_tools.comparer_items.remove(idx);
        let len = self.extra_tools.comparer_items.len();
        if len == 0 {
            self.extra_tools.left_index = 0;
            self.extra_tools.right_index = 0;
            self.extra_tools.comparer_selected = 0;
        } else {
            if idx < self.extra_tools.left_index {
                self.extra_tools.left_index -= 1;
            }
            if idx < self.extra_tools.right_index {
                self.extra_tools.right_index -= 1;
            }
            self.extra_tools.left_index = self.extra_tools.left_index.min(len - 1);
            self.extra_tools.right_index = self.extra_tools.right_index.min(len - 1);
            self.extra_tools.comparer_selected = idx.min(len - 1);
            if len >= 2 && self.extra_tools.left_index == self.extra_tools.right_index {
                if self.extra_tools.right_index > 0 {
                    self.extra_tools.left_index = self.extra_tools.right_index - 1;
                } else {
                    self.extra_tools.right_index = 1;
                }
            }
        }
        self.refresh_comparer(window, cx);
    }

    pub(in crate::workbench) fn refresh_comparer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = &self.extra_tools;
        let (unified, unified_blocks, left_text, right_text, side_blocks) = match (
            pane.comparer_items.get(pane.left_index),
            pane.comparer_items.get(pane.right_index),
        ) {
            (Some(left), Some(right)) if pane.comparer_items.len() >= 2 => {
                let (unified, unified_blocks) = diff::compare_items(&left.bytes, &right.bytes, pane.byte_mode);
                let (l_side, r_side, side_blocks) = diff::compare_side_by_side(
                    &left.label,
                    &left.bytes,
                    &right.label,
                    &right.bytes,
                    pane.byte_mode,
                );
                (unified, unified_blocks, l_side, r_side, side_blocks)
            }
            _ => {
                let empty = "请选择两个消息进行比较。\nModified: 0  Added: 0  Deleted: 0";
                (empty.into(), Vec::new(), empty.into(), empty.into(), Vec::new())
            }
        };
        self.extra_tools.comparer_diff_cursor = 0;
        self.extra_tools.comparer_unified_blocks = unified_blocks;
        self.extra_tools.comparer_side_blocks = side_blocks;
        self.extra_tools
            .comparer_view
            .update(cx, |state, cx| state.set_value(unified, window, cx));
        self.extra_tools
            .comparer_left_view
            .update(cx, |state, cx| state.set_value(left_text, window, cx));
        self.extra_tools
            .comparer_right_view
            .update(cx, |state, cx| state.set_value(right_text, window, cx));
        cx.notify();
    }

    pub(super) fn load_comparer_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() || self.file_picker_pending() {
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
                prompt: Some("加载对比文件".into()),
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
                        Ok(Err(error)) => {
                            view.fail(format!("选择对比文件失败：{error}"), cx);
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
            let label = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("文件")
                .to_owned();
            let read_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::read_regular_file_bounded(
                        &path,
                        MAX_COMPARER_ITEM_BYTES as u64,
                        true,
                        "对比文件",
                    )
                })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                if view.project != Some(project) {
                    return;
                }
                match read_result {
                    Ok(bytes) => view.add_comparer_item(&label, bytes, window, cx),
                    Err(error) => view.fail(error, cx),
                }
            });
        })
        .detach();
    }

    fn comparer_diff_count(&self) -> usize {
        if self.extra_tools.comparer_side_by_side {
            self.extra_tools.comparer_side_blocks.len()
        } else {
            self.extra_tools.comparer_unified_blocks.len()
        }
    }

    fn step_comparer_diff(&mut self, next: bool, cx: &mut Context<Self>) {
        let total = self.comparer_diff_count();
        if total == 0 {
            return;
        }
        let cursor = if self.extra_tools.comparer_diff_cursor == 0 {
            if next { 0 } else { total - 1 }
        } else {
            let idx = (self.extra_tools.comparer_diff_cursor - 1).min(total - 1);
            if next { (idx + 1) % total } else { (idx + total - 1) % total }
        };
        self.extra_tools.comparer_diff_cursor = cursor + 1;
        if self.extra_tools.comparer_side_by_side {
            if let Some((left, right)) = self.extra_tools.comparer_side_blocks.get(cursor).cloned() {
                if let Some(range) = left {
                    self.extra_tools.comparer_left_view.update(cx, |state, cx| state.set_selected_range(range, cx));
                }
                if let Some(range) = right {
                    self.extra_tools.comparer_right_view.update(cx, |state, cx| state.set_selected_range(range, cx));
                }
            }
        } else if let Some(range) = self.extra_tools.comparer_unified_blocks.get(cursor).cloned() {
            self.extra_tools.comparer_view.update(cx, |state, cx| state.set_selected_range(range, cx));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;

    use super::*;
    use crate::workbench::tests::{drain, fixture, tab};

    #[gpui_kit::test]
    fn comparer_ext_two_table_selection_layout_and_inline_diff_work(cx: &mut TestAppContext) {
        let mut f = fixture(cx, vec![tab("Comparer")], 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-comparer", cx);
            window.render_frame(cx);

            assert!(window.find("comparer-heading").visible());
            assert!(window.find("comparer-help").visible());
            assert!(window.find("comparer-paste-button").visible());
            assert!(window.find("comparer-load-file").visible());
            assert!(window.find("comparer-remove-selected").visible());
            assert!(window.find("comparer-clear-button").visible());
            assert!(window.find("comparer-mode-words").visible());
            assert!(window.find("comparer-mode-bytes").visible());
            assert!(window.try_find("inspector-sidebar").is_none());
            assert!(window.try_find("inspector-toggle").is_none());
            assert!(window.try_find("comparer-view-unified").is_none());

            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("alpha one".into()));
            window.click("comparer-paste-button", cx);
            window.render_frame(cx);
            assert!(window.find(("comparer-item-row", 0_usize)).visible());
            assert!(window.find(("comparer-item2-row", 0_usize)).visible());
            assert!(window.try_find("comparer-view-unified").is_none());

            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("beta two".into()));
            window.click("comparer-paste-button", cx);
            window.render_frame(cx);
            assert!(window.find("comparer-view-unified").visible());
            assert!(window.find("comparer-view-side-by-side").visible());
            assert!(window.try_find("inspector-sidebar").is_none());

            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("alpha three".into()));
            window.click("comparer-paste-button", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).extra_tools.comparer_items.len(), 3);

            window.click(("comparer-item-row", 0_usize), cx);
            window.render_frame(cx);
            window.click(("comparer-item2-row", 2_usize), cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).extra_tools.left_index, 0);
            assert_eq!(f.view.read(cx).extra_tools.right_index, 2);
            let diff = f.view.read(cx).extra_tools.comparer_view.read(cx).value();
            assert!(
                diff.contains("Modified: 1  Added: 0  Deleted: 0")
                    && diff.contains("- one\n+ three"),
                "unexpected diff: {diff}"
            );

            window.click("comparer-mode-bytes", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).extra_tools.byte_mode);
            assert!(
                f.view
                    .read(cx)
                    .extra_tools
                    .comparer_view
                    .read(cx)
                    .value()
                    .contains("Hex bytes")
            );

            window.click("comparer-clear-button", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).extra_tools.comparer_items.is_empty());
            assert!(window.try_find("comparer-view-unified").is_none());

            window.click("comparer-mode-words", cx);
            window.render_frame(cx);
            assert!(!f.view.read(cx).extra_tools.byte_mode);
        })
        .unwrap();
    }
}
