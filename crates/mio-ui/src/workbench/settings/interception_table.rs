use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::table::{Column, TableDelegate};
use mio_core::{
    ProxyInterceptOperator, ProxyInterceptRelation, ProxyInterceptRule, ProxyInterceptRuleSet,
    ProxyInterceptType, ProxyInterceptionSettings, Uuid,
};

use super::*;

pub(super) fn intercept_set(
    settings: &ProxyInterceptionSettings,
    response: bool,
) -> &ProxyInterceptRuleSet {
    if response {
        &settings.response
    } else {
        &settings.request
    }
}

pub(super) fn intercept_set_mut(
    settings: &mut ProxyInterceptionSettings,
    response: bool,
) -> &mut ProxyInterceptRuleSet {
    if response {
        &mut settings.response
    } else {
        &mut settings.request
    }
}

pub(super) fn intercept_type_label(match_type: ProxyInterceptType) -> &'static str {
    match match_type {
        ProxyInterceptType::DomainName => "域名",
        ProxyInterceptType::IpAddress => "IP 地址",
        ProxyInterceptType::Protocol => "协议",
        ProxyInterceptType::HttpMethod => "HTTP 方法",
        ProxyInterceptType::FileExtension => "文件扩展名",
        ProxyInterceptType::Url => "URL",
        ProxyInterceptType::Header => "任意标头",
        ProxyInterceptType::Request => "请求",
        ProxyInterceptType::CookieName => "Cookie 名称",
        ProxyInterceptType::CookieValue => "Cookie 值",
        ProxyInterceptType::Body => "正文",
        ProxyInterceptType::ParamName => "参数名称",
        ProxyInterceptType::ParamValue => "参数值",
        ProxyInterceptType::ContentTypeHeader => "Content-Type 标头",
        ProxyInterceptType::MimeType => "MIME 类型",
        ProxyInterceptType::PageTitle => "页面标题",
        ProxyInterceptType::StatusCode => "状态码",
        ProxyInterceptType::ListenerPort => "监听端口",
    }
}

pub(super) fn intercept_match_types(response: bool) -> Vec<ProxyInterceptType> {
    use ProxyInterceptType::*;
    let mut types = vec![
        DomainName,
        IpAddress,
        Protocol,
        HttpMethod,
        Url,
        FileExtension,
        Request,
        CookieName,
        CookieValue,
        Header,
        Body,
        ParamName,
        ParamValue,
    ];
    if response {
        types.extend([StatusCode, ContentTypeHeader, MimeType, PageTitle]);
    }
    types.push(ListenerPort);
    types
}

pub(super) fn intercept_relation_label(
    relationship: ProxyInterceptRelation,
    match_type: ProxyInterceptType,
) -> &'static str {
    match relationship {
        ProxyInterceptRelation::Matches => "匹配",
        ProxyInterceptRelation::DoesNotMatch => "不匹配",
        ProxyInterceptRelation::RequestMatches => {
            if match_type == ProxyInterceptType::Body {
                "请求中的正文匹配"
            } else {
                "请求中匹配"
            }
        }
        ProxyInterceptRelation::RequestDoesNotMatch => {
            if match_type == ProxyInterceptType::Body {
                "请求中的正文不匹配"
            } else {
                "请求中不匹配"
            }
        }
        ProxyInterceptRelation::ResponseMatches => {
            if match_type == ProxyInterceptType::Body {
                "响应中的正文匹配"
            } else {
                "响应中匹配"
            }
        }
        ProxyInterceptRelation::ResponseDoesNotMatch => {
            if match_type == ProxyInterceptType::Body {
                "响应中的正文不匹配"
            } else {
                "响应中不匹配"
            }
        }
        ProxyInterceptRelation::IsHttp => "为 HTTP",
        ProxyInterceptRelation::IsHttps => "为 HTTPS",
        ProxyInterceptRelation::ContainsParameters => "包含参数",
        ProxyInterceptRelation::DoesNotContainParameters => "不包含参数",
        ProxyInterceptRelation::WasModified => "已被修改",
        ProxyInterceptRelation::WasNotModified => "未被修改",
        ProxyInterceptRelation::WasIntercepted => "已被拦截",
        ProxyInterceptRelation::WasNotIntercepted => "未被拦截",
        ProxyInterceptRelation::WasAnnotated => "已添加批注",
        ProxyInterceptRelation::WasNotAnnotated => "未添加批注",
        ProxyInterceptRelation::InScope => "在目标范围内",
        ProxyInterceptRelation::NotInScope => "不在目标范围内",
    }
}

pub(super) fn intercept_relations(
    match_type: ProxyInterceptType,
    response: bool,
) -> Vec<ProxyInterceptRelation> {
    use ProxyInterceptRelation::*;
    match match_type {
        ProxyInterceptType::Protocol => vec![IsHttp, IsHttps],
        ProxyInterceptType::CookieName
        | ProxyInterceptType::CookieValue
        | ProxyInterceptType::Body
        | ProxyInterceptType::Header
            if response =>
        {
            vec![
                RequestMatches,
                RequestDoesNotMatch,
                ResponseMatches,
                ResponseDoesNotMatch,
            ]
        }
        ProxyInterceptType::Request if response => vec![
            ContainsParameters,
            DoesNotContainParameters,
            WasIntercepted,
            WasNotIntercepted,
            WasModified,
            WasNotModified,
            WasAnnotated,
            WasNotAnnotated,
        ],
        ProxyInterceptType::Request => vec![ContainsParameters, DoesNotContainParameters],
        ProxyInterceptType::Url => vec![Matches, DoesNotMatch, InScope, NotInScope],
        _ => vec![Matches, DoesNotMatch],
    }
}

pub(super) struct InterceptionTable {
    pub(super) owner: WeakEntity<Settings>,
    pub(super) response: bool,
}

const INTERCEPT_COLUMNS: [(&str, f32); 5] = [
    ("启用", 72.),
    ("逻辑", 74.),
    ("匹配类型", 144.),
    ("匹配关系", 166.),
    ("匹配条件", 360.),
];

impl InterceptionTable {
    fn row(&self, row: usize, cx: &App) -> Option<ProxyInterceptRule> {
        self.owner.upgrade().and_then(|owner| {
            intercept_set(&owner.read(cx).state.proxy_interception, self.response)
                .rules
                .get(row)
                .cloned()
        })
    }

    fn cell(&self, row: usize, column: usize, cx: &App) -> String {
        self.row(row, cx)
            .map_or_else(String::new, |rule| match column {
                1 if row > 0 => if rule.operator == ProxyInterceptOperator::And {
                    "且"
                } else {
                    "或"
                }
                .into(),
                2 => intercept_type_label(rule.match_type).into(),
                3 => intercept_relation_label(rule.relationship, rule.match_type).into(),
                4 => rule.condition,
                _ => String::new(),
            })
    }
}

impl TableDelegate for InterceptionTable {
    fn columns_count(&self, _: &App) -> usize {
        INTERCEPT_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), INTERCEPT_COLUMNS[index].0)
            .width((INTERCEPT_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.owner.upgrade().map_or(0, |owner| {
            intercept_set(&owner.read(cx).state.proxy_interception, self.response)
                .rules
                .len()
        })
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(INTERCEPT_COLUMNS[col_ix].0, INTERCEPT_COLUMNS[col_ix].1)
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(SharedString::from(format!(
                "proxy-{}-rule-row-{}",
                if self.response { "response" } else { "request" },
                self.row(row, cx).map_or(Uuid::nil(), |rule| rule.id)
            )))
            .border_b_0()
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if column != 0 {
            return div()
                .px(px(6.))
                .truncate()
                .child(self.cell(row, column, cx))
                .into_any_element();
        }
        let Some(rule) = self.row(row, cx) else {
            return div().into_any_element();
        };
        let owner = self.owner.clone();
        let response = self.response;
        let base = owner.upgrade().map(|owner| {
            let state = &owner.read(cx).state;
            (state.project, state.proxy_interception.clone())
        });
        let available = owner
            .upgrade()
            .is_some_and(|owner| owner.read(cx).interception_available());
        let blue = settings_blue();
        settings_table_checkbox_cell(
            Checkbox::new(SharedString::from(format!(
                "proxy-{}-rule-enabled-{}",
                if response { "response" } else { "request" },
                rule.id
            )))
            .accessibility_label(format!(
                "启用{}拦截规则",
                if response { "响应" } else { "请求" }
            ))
            .checked(rule.enabled)
            .disabled(!available)
            .size(px(14.))
            .child(
                CheckboxIndicator::new()
                    .checked(rule.enabled)
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(settings_checkbox_border(rule.enabled))
                    .bg(if rule.enabled { blue } else { theme::surface() })
                    .when(rule.enabled, |this| {
                        this.child(
                            gpui_kit::component::Icon::new(gpui_kit::component::IconName::Check)
                                .size(px(11.5))
                                .text_color(rgb(0xffffff)),
                        )
                    }),
            )
            .on_change(move |checked, _, _, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if let Some((project, base)) = &base {
                        this.change_interception(
                            *project,
                            response,
                            base,
                            |set| {
                                if let Some(current) =
                                    set.rules.iter_mut().find(|current| current.id == rule.id)
                                {
                                    current.enabled = checked == CheckboxState::Checked;
                                }
                            },
                            cx,
                        );
                    }
                });
            }),
        )
        .into_any_element()
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        Some(self.cell(row, column, cx).into())
    }
}
