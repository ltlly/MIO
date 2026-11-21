use gpui_kit::{DragMoveEvent, MouseButton};

use super::*;

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_tabs_workspace(
        &self,
        body: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vertical = self.workspace.intruder.orientation == Orientation::Vertical;
        let show_onboarding =
            self.workspace.intruder.tabs.is_empty() && !self.intruder_onboarding_dismissed();
        div()
            .id("intruder-workspace")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .when_some(self.intruder.tabs_focus.as_ref(), |this, focus| {
                this.track_focus(focus)
            })
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::Escape, window, cx| {
                    if this.intruder.rename.is_some() {
                        this.cancel_intruder_rename(window, cx);
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        if cx.has_active_drag() {
                            cx.stop_active_drag(window);
                            this.drop_target = None;
                            this.drop_after = false;
                            if let Some(width) = this.intruder.resize_original.take() {
                                this.workspace.intruder.tab_sidebar_width = width;
                                this.save_workspace(cx);
                            }
                            cx.stop_propagation();
                            cx.notify();
                        } else if this.intruder.rename.is_some() {
                            this.cancel_intruder_rename(window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }
                }),
            )
            .on_drag_move::<ResizeIntruderTabBar>(cx.listener(
                |this, event: &DragMoveEvent<ResizeIntruderTabBar>, window, cx| {
                    if this.transition.is_some() || !this.backend_available() {
                        return;
                    }
                    if this.intruder.resize_original.is_none() {
                        if !this.flush_workspace(window, cx) {
                            cx.stop_active_drag(window);
                            return;
                        }
                        if let Some(focus) = &this.intruder.tabs_focus {
                            focus.focus(window, cx);
                        }
                    }
                    this.intruder
                        .resize_original
                        .get_or_insert(this.workspace.intruder.tab_sidebar_width);
                    this.workspace.intruder.tab_sidebar_width =
                        (event.event.position.x - event.bounds.origin.x)
                            .as_f32()
                            .clamp(120., 640.) as u16;
                    cx.notify();
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.intruder.resize_original.take().is_some() {
                        this.workspace_dirty = true;
                        this.flush_workspace(window, cx);
                    }
                    this.drop_target = None;
                    this.drop_after = false;
                    cx.notify();
                }),
            )
            .child(if show_onboarding {
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(body)
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!vertical, |this| this.flex_col())
                    .child(self.render_intruder_tab_bar(window, cx))
                    .child(div().flex_1().min_w_0().min_h_0().child(body))
                    .into_any_element()
            })
            .into_any_element()
    }
}
