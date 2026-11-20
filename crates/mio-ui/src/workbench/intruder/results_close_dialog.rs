use std::rc::Rc;

use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, WeakEntity, WindowBounds, WindowKind, WindowOptions, size,
};

use super::*;

#[cfg(test)]
#[path = "results_close_dialog_tests.rs"]
mod tests;

type AnswerHandler = dyn Fn(
    &mut Workbench,
    Uuid,
    Option<(usize, bool)>,
    &mut Context<Workbench>,
) -> Result<bool, String>;

/// Call after leaving the Workbench update; the caller owns the close-operation guard.
/// `Ok(true)` accepts and closes, `Ok(false)` retains the pending dialog, and `Err` permits retry.
pub(super) fn open(
    owner: WeakEntity<Workbench>,
    operation: Uuid,
    terminal: bool,
    can_save: bool,
    parent: &mut Window,
    cx: &mut App,
    on_answer: impl Fn(
        &mut Workbench,
        Uuid,
        Option<(usize, bool)>,
        &mut Context<Workbench>,
    ) -> Result<bool, String>
    + 'static,
) -> anyhow::Result<(AnyWindowHandle, Entity<CloseDialog>)> {
    anyhow::ensure!(!operation.is_nil(), "关闭确认缺少有效 operation ID");
    let owner_entity = owner
        .upgrade()
        .ok_or_else(|| anyhow::anyhow!("关闭确认的工作台已释放"))?;
    let owner_window = owner_entity.read(cx).owner_window;
    let parent_window = parent.window_handle();
    let display = parent
        .display(cx)
        .ok_or_else(|| anyhow::anyhow!("无法确定结果窗口所在显示器"))?;
    let visible = display.visible_bounds();
    let scale = theme::ui_scale();
    let raw_dimensions = if terminal {
        let base_w = if can_save { 614. } else { 452. };
        if scale > 1.0 {
            size(theme::scaled_px(base_w), theme::scaled_px(184.))
        } else {
            size(px(base_w), px(166.))
        }
    } else if scale > 1.0 {
        size(theme::scaled_px(538.), theme::scaled_px(194.))
    } else {
        size(px(538.), px(176.))
    };
    let dimensions = if scale > 1.0 {
        size(
            raw_dimensions.width.min(visible.size.width),
            raw_dimensions.height.min(visible.size.height),
        )
    } else {
        raw_dimensions
    };
    anyhow::ensure!(
        visible.size.width >= dimensions.width && visible.size.height >= dimensions.height,
        "当前显示器可用区域不足以显示关闭确认"
    );
    let mut bounds = Bounds::centered_at(parent.window_bounds().get_bounds().center(), dimensions);
    bounds.origin.x = bounds
        .origin
        .x
        .clamp(visible.origin.x, visible.right() - dimensions.width);
    bounds.origin.y = bounds
        .origin
        .y
        .clamp(visible.origin.y, visible.bottom() - dimensions.height);
    // macOS attaches Dialog to NSApplication.mainWindow; activate the intended result first.
    parent.activate_window();
    gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            display_id: Some(display.id()),
            titlebar: None,
            kind: WindowKind::Dialog,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        cx,
        |window, cx| {
            cx.new(|cx: &mut Context<CloseDialog>| {
                let on_answer: Rc<AnswerHandler> = Rc::new(on_answer);
                let dialog_window = window.window_handle();
                let released = cx.observe_release(&owner_entity, move |_, _, cx| {
                    cx.defer(move |cx| {
                        let _ = dialog_window.update(cx, |_, window, _| window.remove_window());
                    });
                });
                let close_owner = owner_entity.downgrade();
                let close_callback = on_answer.clone();
                let mut parent_closed = false;
                let closed = cx.on_window_closed(move |cx, closed| {
                    if !parent_closed
                        && (closed == owner_window.window_id()
                            || closed == parent_window.window_id())
                    {
                        parent_closed = true;
                        let owner = close_owner.clone();
                        let callback = close_callback.clone();
                        cx.defer(move |cx| {
                            if cx.windows().contains(&owner_window) {
                                let _ = owner_window.update(cx, |_, _, cx| {
                                    let _ = owner.update(cx, |owner, cx| {
                                        callback(owner, operation, None, cx)
                                    });
                                });
                            } else {
                                let _ = owner
                                    .update(cx, |owner, cx| callback(owner, operation, None, cx));
                            }
                            let _ = dialog_window.update(cx, |_, window, _| window.remove_window());
                        });
                    }
                });
                let weak = cx.weak_entity();
                window.on_window_should_close(cx, move |window, cx| {
                    let _ = weak.update(cx, |view, cx| view.answer(None, window, cx));
                    false
                });
                let remember_focus = cx.focus_handle();
                remember_focus.focus(window, cx);
                window.set_window_title(if terminal {
                    "保存攻击？"
                } else {
                    "继续攻击？"
                });
                CloseDialog {
                    owner: owner_entity.downgrade(),
                    owner_window,
                    operation,
                    terminal,
                    can_save,
                    remember: false,
                    pending: false,
                    error: None,
                    answer_revision: 0,
                    focus: cx.focus_handle(),
                    remember_focus,
                    button_focus: (0..if terminal { 3 } else { 2 })
                        .map(|_| cx.focus_handle())
                        .collect(),
                    on_answer,
                    _subscriptions: vec![released, closed],
                }
            })
        },
    )
}

use dialog_window::CloseDialog;
mod dialog_window;
