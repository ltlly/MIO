use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

use super::*;

pub(super) struct TaskNameWindow {
    owner: WeakEntity<Workbench>,
    id: Uuid,
    pub(super) input: Entity<InputState>,
    pub(in crate::workbench::intruder) status: String,
    pub(in crate::workbench::intruder) pending: bool,
    disconnected: bool,
    _subscriptions: Vec<Subscription>,
}

impl TaskNameWindow {
    pub(super) fn new(
        owner: Entity<Workbench>,
        id: Uuid,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let observation = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            let Some(rename) = view
                .intruder
                .task_name
                .as_ref()
                .filter(|rename| rename.id == this.id)
            else {
                window.remove_window();
                return;
            };
            this.status = rename.status.clone();
            this.pending = rename.pending.is_some();
            this.disconnected = rename.disconnected || !view.backend_available();
            if this.disconnected && !rename.disconnected {
                owner.update(cx, |view, cx| view.intruder_task_name_disconnected(cx));
            }
            cx.notify();
        });
        let entered = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.confirm(window, cx);
            }
        });
        let weak = owner.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = weak.update(cx, |view, cx| view.cancel_task_name(id, cx));
            false
        });
        window.set_window_title("重命名任务");
        input.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        Self {
            owner: owner.downgrade(),
            id,
            input,
            status: String::new(),
            pending: false,
            disconnected: false,
            _subscriptions: vec![observation, entered],
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.disconnected {
            return;
        }
        let title = match editing::input_snapshot(&self.input, window, cx) {
            Ok(title) => title.to_string(),
            Err(error) => {
                self.status = error.into();
                cx.notify();
                return;
            }
        };
        let input_revision = self.input.read(cx).document_revision();
        let _ = self.owner.update(cx, |view, cx| {
            view.confirm_task_name(self.id, title, self.input.clone(), input_revision, cx)
        });
    }
}

impl Render for TaskNameWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.clone();
        let rename_id = self.id;
        let title_bar = crate::chrome::render_title_bar(
            "mio-task-name",
            "重命名任务",
            crate::chrome::TitleBarControls::DIALOG,
            move |_, cx| {
                let _ = owner.update(cx, |view, cx| view.cancel_task_name(rename_id, cx));
            },
            window,
            cx,
        );
        let content = div()
            .id("intruder-task-name-dialog")
            .test_support()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .text_size(theme::ui_font_size())
            .bg(theme::background())
            .text_color(theme::foreground())
            .child("输入任务的新名称")
            .child(Input::new(&self.input).small().aria_label("任务名称"))
            .child(
                div()
                    .id("intruder-task-name-status")
                    .test_support()
                    .flex_1()
                    .text_size(theme::scaled_px(11.))
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("intruder-task-name-cancel", "取消")
                            .tooltip("取消重命名并关闭对话框")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this
                                    .owner
                                    .update(cx, |view, cx| view.cancel_task_name(this.id, cx));
                            })),
                    )
                    .child(
                        button("intruder-task-name-confirm", "确认")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("保存新的 Intruder 攻击任务名称")
                            .disabled(self.pending || self.disconnected)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::foreground())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(content))
    }
}
