use gpui_kit::accesskit::Role;
use gpui_kit::{Bounds, WeakEntity, size};
use mio_core::{
    IntruderAutoThrottle, IntruderPoolSelection, IntruderResourcePool, IntruderResourcePools, Uuid,
};

use super::*;
use crate::CloseSettings;
use crate::workbench::intruder::resource_pools::{
    CreatePoolForm, PoolFlag, PoolFormAction, PoolFormChange, render_pool_form,
};

impl Workbench {
    fn submit_pool_edit(
        &mut self,
        project: ProjectKey,
        operation: Uuid,
        before: &IntruderResourcePools,
        pool: IntruderResourcePool,
        new: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.project != Some(project)
            || self.intruder.resource_pools.catalog.as_ref() != Some(before)
        {
            return Err("资源池目录已改变；输入保留，请关闭并重新读取后编辑。".into());
        }
        pool.validate().map_err(|error| error.to_string())?;
        let mut pools = before.pools.clone();
        if new {
            if before.find(pool.id).is_some() {
                return Err("新资源池身份已存在；未覆盖。".into());
            }
            pools.push(pool);
        } else {
            let Some(current) = pools.iter_mut().find(|current| current.id == pool.id) else {
                return Err("待编辑资源池已不存在。".into());
            };
            *current = pool;
        }
        let mut proposed = before.clone();
        proposed.pools = pools.clone();
        proposed.validate().map_err(|error| error.to_string())?;
        self.start_pool_mutation(
            operation,
            MutationKind::Update {
                catalog: before.clone(),
                pools: pools.clone(),
            },
            cx,
        )?;
        let cancel = self
            .intruder
            .resource_pools
            .management
            .pending
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        if !self.enqueue(
            Command::SetIntruderResourcePools {
                project,
                operation_id: operation,
                expected_revision: before.revision,
                pools,
                cancel,
            },
            cx,
        ) {
            self.intruder.resource_pools.management.pending = None;
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "资源池修改未入队。".into()));
        }
        cx.notify();
        Ok(())
    }
}

impl Settings {
    pub(in crate::workbench) fn open_pool_editor(
        &mut self,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.pools.ready() || self.pools.confirming || self.pools.opening {
            return;
        }
        if let Some(dialog) = self.pools.dialog
            && crate::chrome::activate_popup_window(dialog, cx)
        {
            return;
        }
        let (Some(project), Some(catalog)) = (self.state.project, self.state.pools.catalog.clone())
        else {
            return;
        };
        let pool = if new {
            IntruderResourcePool {
                retained_inputs: None,
                id: Uuid::new_v4(),
                name: String::new(),
                max_concurrent_requests: Some(10),
                delay: None,
                automatic_throttling: IntruderAutoThrottle {
                    enabled: true,
                    ..Default::default()
                },
            }
        } else {
            let Some(pool) = self.pools.selected.and_then(|id| catalog.find(id)).cloned() else {
                return;
            };
            pool
        };
        let owner = self.owner.clone();
        let settings = cx.weak_entity();
        let scale = theme::ui_scale();
        let content_w = if scale <= 1.0 {
            px(478.)
        } else {
            theme::scaled_px(478.)
        };
        let content_h = if scale <= 1.0 {
            px(425.)
        } else {
            theme::scaled_px(425.)
        };
        let raw_h = if cfg!(test) {
            content_h
        } else {
            let bar_h = if scale <= 1.0 {
                px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            } else {
                theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                    .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
            };
            content_h + bar_h
        };
        let desired = if scale <= 1.0 {
            size(content_w, raw_h)
        } else if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds().size;
            size(content_w.min(visible.width), raw_h.min(visible.height))
        } else {
            size(content_w, raw_h)
        };
        let options = crate::chrome::mio_window_options(
            Bounds::centered(window.display(cx).map(|display| display.id()), desired, cx),
            false,
            false,
        );
        self.pools.opening = true;
        cx.defer(move |cx| {
            if settings
                .upgrade()
                .is_none_or(|settings| settings.read(cx).state.project != Some(project))
            {
                return;
            }
            let opened = crate::chrome::open_popup_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    PoolEditor::new(
                        (owner, settings.clone()),
                        project,
                        catalog,
                        pool,
                        new,
                        window,
                        cx,
                    )
                })
            });
            let _ = settings.update(cx, |settings, cx| {
                settings.pools.opening = false;
                match opened {
                    Ok((window, _)) => settings.pools.dialog = Some(window),
                    Err(error) => {
                        settings.state.pools.error =
                            Some(format!("无法打开资源池编辑窗口：{error}"))
                    }
                }
                cx.notify();
            });
        });
    }
}

pub(super) struct PoolEditor {
    owner: WeakEntity<Workbench>,
    settings: WeakEntity<Settings>,
    project: ProjectKey,
    catalog: IntruderResourcePools,
    pub(super) form: CreatePoolForm,
    new: bool,
    operation: Option<Uuid>,
    pub(super) pending: bool,
    pub(super) error: Option<String>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl PoolEditor {
    fn new(
        (owner, settings): (WeakEntity<Workbench>, WeakEntity<Settings>),
        project: ProjectKey,
        catalog: IntruderResourcePools,
        pool: IntruderResourcePool,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale();
        let mut target_w = if scale <= 1.0 {
            px(478.)
        } else {
            theme::scaled_px(478.)
        };
        let mut target_h = if scale <= 1.0 {
            px(425.)
        } else {
            theme::scaled_px(425.)
        };
        if scale > 1.0
            && let Some(display) = window.display(cx)
        {
            let visible = display.visible_bounds().size;
            target_w = target_w.min(visible.width);
            target_h = target_h.min(visible.height);
        }
        let width = target_w - (frame.width - viewport.width);
        let height = target_h - (frame.height - viewport.height);
        if width > px(0.) && height > px(0.) {
            window.resize(size(width, height));
        }
        let legacy_delay = pool.delay.is_none() && pool.retained_inputs.is_none();
        let form = CreatePoolForm::new(Some(&IntruderPoolSelection::Create(pool)), window, cx);
        if legacy_delay {
            form.delay
                .update(cx, |input, cx| input.set_value("500", window, cx));
        }
        let owner_entity = owner.upgrade().unwrap();
        let subscription = cx.observe_in(&owner_entity, window, |this, owner, window, cx| {
            let state = PoolPageState::new(owner.read(cx));
            if owner.read(cx).project != Some(this.project) {
                window.remove_window();
                return;
            }
            if let Some(operation) = this.operation
                && let Some((actual, outcome)) = state.outcome
                && actual == operation
            {
                this.pending = false;
                this.operation = None;
                match outcome {
                    Ok(()) => {
                        window.remove_window();
                        return;
                    }
                    Err(error) => this.error = Some(error),
                }
            } else if this.pending
                && let Some(error) = state.error
            {
                this.error = Some(error);
            }
            cx.notify();
        });
        let release_owner = owner.clone();
        cx.on_release(move |this, cx| {
            if let Some(operation) = this.operation {
                let _ = release_owner.update(cx, |view, _| {
                    if view
                        .intruder
                        .resource_pools
                        .management
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.operation == operation)
                    {
                        view.intruder.resource_pools.management.cancel_pending();
                    }
                });
            }
            let settings = this.settings.clone();
            cx.defer(move |cx| {
                let _ = settings.update(cx, |settings, cx| {
                    settings.pools.dialog = None;
                    cx.notify();
                });
            });
        })
        .detach();
        window.set_window_title(if new {
            "新建资源池"
        } else {
            "编辑资源池"
        });
        let focus = cx.focus_handle();
        form.name.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        window.on_window_should_close(cx, |_, _| true);
        Self {
            owner,
            settings,
            project,
            catalog,
            form,
            new,
            operation: None,
            pending: false,
            error: None,
            focus,
            _subscription: subscription,
        }
    }

    fn change(&mut self, action: PoolFormAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        if self.form.has_composition(window, cx) {
            self.error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        match action {
            PoolFormAction::Flag(flag, checked) => match flag {
                PoolFlag::Cap => self.form.cap_enabled = checked,
                PoolFlag::Delay => self.form.delay_enabled = checked,
                PoolFlag::Auto => self.form.automatic = checked,
                PoolFlag::Status429 => self.form.status_429 = checked,
                PoolFlag::Status503 => self.form.status_503 = checked,
                PoolFlag::Other => self.form.other_enabled = checked,
            },
            PoolFormAction::Delay(kind) => self.form.delay_kind = kind,
        }
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let result = self
            .form
            .snapshot_preserving_inputs(window, cx)
            .and_then(|pool| {
                pool.validate().map_err(|error| error.to_string())?;
                Ok(pool)
            });
        let pool = match result {
            Ok(pool) => pool,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(owner) = self.owner.upgrade() else {
            self.error = Some("主工作台已关闭。".into());
            return;
        };
        let refreshed = {
            let owner = owner.read(cx);
            let state = &owner.intruder.resource_pools;
            if state.pending.is_some()
                || state.management.query.is_some()
                || state.management.error.is_some()
                || state.error.is_some()
            {
                self.error =
                    Some("正在核对资源池实际目录，请等待读取完成后再确认；输入保留。".into());
                cx.notify();
                return;
            }
            state.catalog.clone()
        };
        if let Some(current) = refreshed
            && current != self.catalog
        {
            self.catalog = current;
            self.error =
                Some("资源池目录已更新，输入仍保留。请核对后再次确认以提交当前输入。".into());
            cx.notify();
            return;
        }
        let operation = Uuid::new_v4();
        let submitted = self.owner.update(cx, |view, cx| {
            view.submit_pool_edit(self.project, operation, &self.catalog, pool, self.new, cx)
        });
        match submitted {
            Ok(Ok(())) => {
                self.operation = Some(operation);
                self.pending = true;
                self.error = None;
            }
            Ok(Err(error)) => self.error = Some(error),
            Err(_) => self.error = Some("主工作台已关闭；未提交。".into()),
        }
        cx.notify();
    }
}

impl Render for PoolEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-pool-editor",
            if self.new {
                "新建资源池"
            } else {
                "编辑资源池"
            },
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let owner = cx.weak_entity();
        let change: PoolFormChange =
            std::rc::Rc::new(move |action, window: &mut Window, cx: &mut App| {
                let _ = owner.update(cx, |view, cx| view.change(action, window, cx));
            });
        let content = div()
            .id("settings-pool-dialog")
            .test_support()
            .size_full()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .font_family(".SystemUIFont")
            .text_size(theme::ui_font_size())
            .bg(theme::background())
            .text_color(theme::foreground())
            .key_context("MioSettings")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseSettings, window, _| window.remove_window()))
            .child(
                div()
                    .id("settings-pool-dialog-form")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(render_pool_form(&self.form, !self.pending, true, change)),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("settings-pool-dialog-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("settings-pool-ok", "确定")
                            .icon(gpui_kit::assets::IconName::Check)
                            .tooltip("确认并保存资源池配置")
                            .min_w(theme::scaled_px(82.).max(px(82.)))
                            .disabled(self.pending)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    )
                    .child(
                        button("settings-pool-cancel", "取消")
                            .icon(gpui_kit::assets::IconName::Close)
                            .tooltip("取消编辑并关闭对话框")
                            .min_w(theme::scaled_px(82.).max(px(82.)))
                            .on_click(|_, window, _| window.remove_window()),
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
