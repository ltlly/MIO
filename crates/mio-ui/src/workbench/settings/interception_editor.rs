use gpui_kit::accesskit::Role;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::{AnyWindowHandle, size};
use mio_core::{
    ProxyInterceptOperator, ProxyInterceptRule, ProxyInterceptType, ProxyInterceptionSettings, Uuid,
};

use super::*;
use crate::CloseSettings;

pub(in crate::workbench) struct InterceptionEditor {
    pub(super) owner: WeakEntity<Workbench>,
    pub(super) owner_window: AnyWindowHandle,
    pub(super) project: ProjectKey,
    pub(super) revision: u64,
    pub(super) base: ProxyInterceptionSettings,
    pub(super) rule: ProxyInterceptRule,
    pub(super) response: bool,
    pub(super) new: bool,
    operator: Entity<SelectState<Vec<&'static str>>>,
    match_type: Entity<SelectState<Vec<&'static str>>>,
    relationship: Entity<SelectState<Vec<InterceptionRelationChoice>>>,
    pub(in crate::workbench) condition: Entity<InputState>,
    pub(super) pending: Option<u64>,
    pub(super) checking: Option<Uuid>,
    pub(super) validation: Option<Task<()>>,
    pub(super) submitted: Option<ProxyInterceptionSettings>,
    pub(super) prior_save: Option<(u64, ProxyInterceptionSettings)>,
    pub(super) acknowledged: bool,
    pub(super) error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct InterceptionRelationChoice(&'static str);

impl SelectItem for InterceptionRelationChoice {
    type Value = &'static str;
    fn title(&self) -> SharedString {
        self.0.into()
    }
    fn value(&self) -> &&'static str {
        &self.0
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(SharedString::from(format!(
                "proxy-rule-relation-choice-{}",
                self.0
            )))
            .test_support()
            .role(Role::Label)
            .aria_label(self.0)
            .child(self.0)
    }
}

impl InterceptionEditor {
    pub(super) fn new(
        (owner, settings): (WeakEntity<Workbench>, WeakEntity<Settings>),
        (project, revision, base, rule, response, new): (
            ProjectKey,
            u64,
            ProxyInterceptionSettings,
            ProxyInterceptRule,
            bool,
            bool,
        ),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale();
        let mut target_w = if scale <= 1.0 {
            px(522.)
        } else {
            theme::scaled_px(522.)
        };
        let mut target_h = theme::scaled_px(276.).max(px(276.));
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
        let owner_window = owner
            .upgrade()
            .expect("settings owner exists while opening")
            .read(cx)
            .owner_window;
        let operator = cx.new(|cx| {
            SelectState::new(
                vec!["And", "Or"],
                Some(gpui_kit::component::IndexPath::new(usize::from(
                    rule.operator == ProxyInterceptOperator::Or,
                ))),
                window,
                cx,
            )
        });
        let types: Vec<_> = intercept_match_types(response)
            .into_iter()
            .map(intercept_type_label)
            .collect();
        let type_index = types
            .iter()
            .position(|label| *label == intercept_type_label(rule.match_type))
            .unwrap_or(0);
        let match_type = cx.new(|cx| {
            SelectState::new(
                types,
                Some(gpui_kit::component::IndexPath::new(type_index)),
                window,
                cx,
            )
        });
        let relations = intercept_relations(rule.match_type, response);
        let relation_index = relations
            .iter()
            .position(|relation| *relation == rule.relationship)
            .unwrap_or(0);
        let relationship = cx.new(|cx| {
            SelectState::new(
                relations
                    .iter()
                    .map(|relation| {
                        InterceptionRelationChoice(intercept_relation_label(
                            *relation,
                            rule.match_type,
                        ))
                    })
                    .collect::<Vec<_>>(),
                Some(gpui_kit::component::IndexPath::new(relation_index)),
                window,
                cx,
            )
        });
        let condition =
            cx.new(|cx| InputState::new(window, cx).default_value(rule.condition.clone()));
        let operator_change = cx.subscribe_in(&operator, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(value) = event;
            if let Some(value) = value {
                this.rule.operator = if *value == "Or" {
                    ProxyInterceptOperator::Or
                } else {
                    ProxyInterceptOperator::And
                };
            }
            cx.notify();
        });
        let type_change = cx.subscribe_in(&match_type, window, |this, _, event, window, cx| {
            let SelectEvent::Confirm(value) = event;
            let Some(value) = value else { return };
            let match_type = match *value {
                "Domain name" => ProxyInterceptType::DomainName,
                "IP address" => ProxyInterceptType::IpAddress,
                "Protocol" => ProxyInterceptType::Protocol,
                "HTTP method" => ProxyInterceptType::HttpMethod,
                "File extension" => ProxyInterceptType::FileExtension,
                "URL" => ProxyInterceptType::Url,
                "Any header" => ProxyInterceptType::Header,
                "Request" => ProxyInterceptType::Request,
                "Cookie name" => ProxyInterceptType::CookieName,
                "Cookie value" => ProxyInterceptType::CookieValue,
                "Body" => ProxyInterceptType::Body,
                "Param name" => ProxyInterceptType::ParamName,
                "Param value" => ProxyInterceptType::ParamValue,
                "Content type header" => ProxyInterceptType::ContentTypeHeader,
                "Mime type" => ProxyInterceptType::MimeType,
                "Page title" => ProxyInterceptType::PageTitle,
                "Status code" => ProxyInterceptType::StatusCode,
                "Listener port" => ProxyInterceptType::ListenerPort,
                _ => {
                    this.error = Some("未知拦截匹配类型；规则输入保留，未应用。".into());
                    cx.notify();
                    return;
                }
            };
            this.rule.match_type = match_type;
            let relations = intercept_relations(match_type, this.response);
            let index = relations
                .iter()
                .position(|relationship| *relationship == this.rule.relationship)
                .unwrap_or(0);
            this.rule.relationship = relations[index];
            if !this.rule.relationship.is_regex() {
                this.rule.condition.clear();
            }
            this.relationship.update(cx, |state, cx| {
                state.set_items(
                    relations
                        .iter()
                        .map(|relation| {
                            InterceptionRelationChoice(intercept_relation_label(
                                *relation, match_type,
                            ))
                        })
                        .collect(),
                    window,
                    cx,
                );
                state.set_selected_index(
                    Some(gpui_kit::component::IndexPath::new(index)),
                    window,
                    cx,
                );
            });
            cx.notify();
        });
        let relation_change = cx.subscribe_in(&relationship, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(value) = event;
            if let Some(value) = value
                && let Some(relationship) = intercept_relations(this.rule.match_type, this.response)
                    .into_iter()
                    .find(|relationship| {
                        intercept_relation_label(*relationship, this.rule.match_type) == *value
                    })
            {
                this.rule.relationship = relationship;
                if !relationship.is_regex() {
                    this.rule.condition.clear();
                }
            }
            cx.notify();
        });
        let watched_owner = owner
            .upgrade()
            .expect("settings owner exists while opening");
        let observer = cx.observe_in(&watched_owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            if view.project != Some(this.project) {
                this.error = Some("项目已切换；当前规则输入保留，未应用到新项目。".into());
                this.pending = None;
                this.checking = None;
                this.validation = None;
            } else if let Some(revision) = this.pending
                && view.pending_workspace != Some(revision)
            {
                let candidate_is_current =
                    this.submitted.as_ref() == Some(&view.workspace.proxy_interception);
                if candidate_is_current
                    && view.pending_workspace.is_some_and(|next| next > revision)
                {
                    this.pending = view.pending_workspace;
                    this.prior_save = None;
                } else if candidate_is_current && this.acknowledged && !view.workspace_dirty {
                    window.remove_window();
                    return;
                } else {
                    this.pending = None;
                    this.prior_save = None;
                    if candidate_is_current {
                        this.base = view.workspace.proxy_interception.clone();
                        this.revision = view.workspace.revision;
                        this.new = false;
                    }
                    this.error = view
                        .error
                        .clone()
                        .or_else(|| Some("规则保存未确认；输入仍保留。".into()));
                }
            }
            cx.notify();
        });
        let dialog = window.window_handle();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = settings.update(cx, |settings, cx| {
                    if settings.interception_dialog == Some(dialog) {
                        settings.interception_dialog = None;
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        window.set_window_title(match (response, new) {
            (false, true) => "添加请求拦截规则",
            (false, false) => "编辑请求拦截规则",
            (true, true) => "添加响应拦截规则",
            (true, false) => "编辑响应拦截规则",
        });
        window.on_window_should_close(cx, |_, _| true);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            owner,
            owner_window,
            project,
            revision,
            base,
            rule,
            response,
            new,
            operator,
            match_type,
            relationship,
            condition,
            pending: None,
            checking: None,
            validation: None,
            submitted: None,
            prior_save: None,
            acknowledged: false,
            error: None,
            focus,
            _subscriptions: vec![operator_change, type_change, relation_change, observer],
        }
    }
}

impl Render for InterceptionEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-interception-editor",
            match (self.response, self.new) {
                (false, true) => "添加请求拦截规则",
                (false, false) => "编辑请求拦截规则",
                (true, true) => "添加响应拦截规则",
                (true, false) => "编辑响应拦截规则",
            },
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let condition_enabled = self.rule.relationship.is_regex();
        let busy = self.pending.is_some() || self.checking.is_some();
        let label_w = if theme::ui_scale() <= 1.0 {
            px(128.)
        } else {
            theme::scaled_px(148.)
        };
        let field_w = if theme::ui_scale() <= 1.0 {
            px(354.)
        } else {
            (window.viewport_size().width - px(32.) - label_w).max(px(140.))
        };
        let btn_min_w = theme::scaled_px(82.).max(px(82.));
        let content = div()
            .id("proxy-interception-rule-editor")
            .test_support()
            .key_context("MioSettings")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .px(px(12.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .text_size(theme::ui_font_size())
            .bg(theme::surface())
            .on_action(cx.listener(|_, _: &CloseSettings, window, _| window.remove_window()))
            .child("设置拦截规则的匹配条件。")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(label_w).flex_shrink_0().child("Boolean operator:"))
                    .child(
                        Select::new(&self.operator)
                            .id("proxy-rule-editor-operator")
                            .small()
                            .w(field_w)
                            .disabled(busy),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(label_w).flex_shrink_0().child("Match type:"))
                    .child(
                        Select::new(&self.match_type)
                            .id("proxy-rule-editor-type")
                            .small()
                            .w(field_w)
                            .disabled(busy),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(label_w)
                            .flex_shrink_0()
                            .child("Match relationship:"),
                    )
                    .child(
                        Select::new(&self.relationship)
                            .id("proxy-rule-editor-relationship")
                            .small()
                            .w(field_w)
                            .disabled(busy),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(label_w).flex_shrink_0().child("Match condition:"))
                    .child(
                        Input::new(&self.condition)
                            .id("proxy-rule-editor-condition")
                            .aria_label("Match condition")
                            .small()
                            .w(field_w)
                            .disabled(busy || !condition_enabled),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("proxy-rule-editor-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .absolute()
                        .bottom(theme::scaled_px(35.))
                        .left(px(12.))
                        .right(px(12.))
                        .text_size(theme::scaled_px(11.))
                        .h(theme::scaled_px(16.).max(px(16.)))
                        .flex_shrink_0()
                        .truncate()
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
            .child(div().flex_1().min_h_0())
            .child(
                div()
                    .absolute()
                    .bottom(theme::scaled_px(8.))
                    .right(px(12.))
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("proxy-rule-editor-ok", "OK")
                            .icon(gpui_kit::assets::IconName::Check)
                            .tooltip("确认并保存此拦截规则")
                            .primary()
                            .disabled(busy)
                            .min_w(btn_min_w)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    )
                    .child(
                        button("proxy-rule-editor-cancel", "取消")
                            .icon(gpui_kit::assets::IconName::Close)
                            .tooltip("取消编辑并关闭对话框")
                            .min_w(btn_min_w)
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
