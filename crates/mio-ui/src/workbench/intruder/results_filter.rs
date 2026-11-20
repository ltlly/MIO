use gpui_kit::{Bounds, size};
use mio_core::IntruderResultFilter;

use super::*;

mod render;
const FILTER_WINDOW_WIDTH: f32 = 665.;
const FILTER_WINDOW_HEIGHT: f32 = 410.;

impl Workbench {
    pub(in crate::workbench) fn open_intruder_result_filter(
        &mut self,
        view_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return;
        };
        if view.discarded || !self.backend_available() || self.transition.is_some() {
            return;
        }
        let target = results_view::ResultTarget {
            workbench: cx.weak_entity(),
            project,
            view_id,
            run_id: view.run_id,
            owner: view.owner,
            editor_id: view.request.raw.entity_id(),
        };
        let scale = theme::ui_scale();
        let content_w = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_WIDTH)
        } else {
            px(FILTER_WINDOW_WIDTH)
        };
        let content_h = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_HEIGHT)
        } else {
            px(FILTER_WINDOW_HEIGHT)
        };
        let desired_h = if cfg!(test) {
            content_h
        } else {
            let bar_h = if scale > 1.0 {
                theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                    .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
            } else {
                px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            };
            content_h + bar_h
        };
        let bounds = if let Some(display) = window.display(cx).or_else(|| cx.primary_display()) {
            let visible = display.visible_bounds();
            let fitted = size(
                content_w.min(visible.size.width),
                desired_h.min(visible.size.height),
            );
            let mut bounds = Bounds::centered(Some(display.id()), fitted, cx);
            bounds.origin.x = bounds.origin.x.clamp(
                visible.origin.x,
                (visible.right() - fitted.width).max(visible.origin.x),
            );
            bounds.origin.y = bounds.origin.y.clamp(
                visible.origin.y,
                (visible.bottom() - fitted.height).max(visible.origin.y),
            );
            bounds
        } else {
            Bounds::centered(None, size(content_w, desired_h), cx)
        };
        cx.defer(move |cx| {
            let Some(owner) = target.workbench.upgrade() else {
                return;
            };
            if !target.owns(owner.read(cx)) {
                return;
            }
            if let Some(existing) = owner.read(cx).intruder.results.views[&view_id]
                .query
                .filter_window
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let filter = owner.read(cx).intruder.results.views[&view_id]
                .query
                .applied
                .clone();
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| ResultFilterWindow::new(target.clone(), filter, window, cx))
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((handle, _)) if target.owns(view) => {
                    view.intruder
                        .results
                        .views
                        .get_mut(&view_id)
                        .unwrap()
                        .query
                        .filter_window = Some(handle);
                    cx.notify();
                }
                Ok((handle, _)) => {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
                Err(error) => view.fail(format!("无法打开 Intruder 结果筛选：{error}"), cx),
            });
        });
    }
}

pub(super) struct ResultFilterWindow {
    target: results_view::ResultTarget,
    applied: IntruderResultFilter,
    pub(super) draft: IntruderResultFilter,
    pub(super) term: Entity<InputState>,
    pending: Option<u64>,
    submitting: bool,
    error: Option<String>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl ResultFilterWindow {
    fn new(
        target: results_view::ResultTarget,
        filter: IntruderResultFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale();
        let mut target_w = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_WIDTH)
        } else {
            px(FILTER_WINDOW_WIDTH)
        };
        let mut target_h = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_HEIGHT)
        } else {
            px(FILTER_WINDOW_HEIGHT)
        };
        if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds().size;
            target_w = target_w.min(visible.width);
            target_h = target_h.min(visible.height);
        }
        let w = target_w - (frame.width - viewport.width);
        let h = target_h - (frame.height - viewport.height);
        if w > px(0.) && h > px(0.) {
            window.resize(size(w, h));
        }
        let term =
            cx.new(|cx| InputState::new(window, cx).default_value(filter.search_term.clone()));
        let owner = target.workbench.upgrade().unwrap();
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let owner = owner.read(cx);
            if !this.target.owns(owner) {
                window.remove_window();
                return;
            }
            let state = &owner.intruder.results.views[&this.target.view_id].query;
            this.applied = state.applied.clone();
            if let Some(revision) = this.pending {
                if let Some((_, outcome)) = state
                    .outcome
                    .as_ref()
                    .filter(|(actual, _)| *actual == revision)
                {
                    this.pending = None;
                    match outcome {
                        Ok(()) => {
                            window.remove_window();
                            return;
                        }
                        Err(error) => this.error = Some(error.clone()),
                    }
                } else if let Some(pending) = &state.pending
                    && pending.query.query_revision != revision
                    && pending.query.filter == this.draft
                {
                    this.pending = Some(pending.query.query_revision);
                } else if state
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.query.query_revision != revision)
                {
                    this.pending = None;
                    this.error = Some("结果筛选已被较新的查询替代；当前条件保留。".into());
                }
            }
            cx.notify();
        });
        let released = target.clone();
        let handle = window.window_handle();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.workbench.update(cx, |view, cx| {
                    if released.owns(view) {
                        let state = &mut view
                            .intruder
                            .results
                            .views
                            .get_mut(&released.view_id)
                            .unwrap()
                            .query;
                        if state.filter_window == Some(handle) {
                            state.filter_window = None;
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        window.set_window_title("Intruder 结果筛选");
        Self {
            target,
            applied: filter.clone(),
            draft: filter,
            term,
            pending: None,
            submitting: false,
            error: None,
            focus,
            _subscription: subscription,
        }
    }

    pub(in crate::workbench) fn restore(
        &mut self,
        filter: IntruderResultFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = editing::input_snapshot(&self.term, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.draft = filter;
        self.term.update(cx, |input, cx| {
            input.set_value(self.draft.search_term.clone(), window, cx)
        });
        self.error = None;
        cx.notify();
    }

    pub(in crate::workbench) fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        let term = match editing::input_snapshot(&self.term, window, cx) {
            Ok(term) => term,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if term.len() > 4096 {
            self.error = Some("搜索词不能超过 4096 字节。".into());
            cx.notify();
            return;
        }
        self.draft.search_term = term.to_string();
        let target = self.target.clone();
        let filter = self.draft.clone();
        let dialog = cx.weak_entity();
        let handle = window.window_handle();
        self.submitting = true;
        cx.notify();
        cx.defer(move |cx| {
            if !cx.windows().contains(&handle) || dialog.upgrade().is_none() {
                return;
            }
            let mut sent = None;
            let mut error = None;
            let result = target.owner.update(cx, |_, window, cx| {
                target.update(window, cx, |view, window, cx| {
                    let sort = view.intruder.results.views[&target.view_id]
                        .query
                        .requested()
                        .1;
                    sent = view.query_intruder_result_view(
                        target.view_id,
                        0,
                        filter,
                        sort,
                        window,
                        cx,
                    );
                    if sent.is_none() {
                        error = Some(
                            view.error
                                .clone()
                                .unwrap_or_else(|| "结果查询未发送；条件保留。".into()),
                        );
                    }
                });
            });
            let _ = dialog.update(cx, |this, cx| {
                this.submitting = false;
                this.pending = sent;
                this.error = if result.is_err() || sent.is_none() && error.is_none() {
                    Some("结果窗口已关闭；筛选未应用。".into())
                } else {
                    error
                };
                cx.notify();
            });
        });
    }
}
