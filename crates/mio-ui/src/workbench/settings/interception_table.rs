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
        ProxyInterceptType::DomainName => "Domain name",
        ProxyInterceptType::IpAddress => "IP address",
        ProxyInterceptType::Protocol => "Protocol",
        ProxyInterceptType::HttpMethod => "HTTP method",
        ProxyInterceptType::FileExtension => "File extension",
        ProxyInterceptType::Url => "URL",
        ProxyInterceptType::Header => "Any header",
        ProxyInterceptType::Request => "Request",
        ProxyInterceptType::CookieName => "Cookie name",
        ProxyInterceptType::CookieValue => "Cookie value",
        ProxyInterceptType::Body => "Body",
        ProxyInterceptType::ParamName => "Param name",
        ProxyInterceptType::ParamValue => "Param value",
        ProxyInterceptType::ContentTypeHeader => "Content type header",
        ProxyInterceptType::MimeType => "Mime type",
        ProxyInterceptType::PageTitle => "Page title",
        ProxyInterceptType::StatusCode => "Status code",
        ProxyInterceptType::ListenerPort => "Listener port",
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
        ProxyInterceptRelation::Matches => "Matches",
        ProxyInterceptRelation::DoesNotMatch => "Does not match",
        ProxyInterceptRelation::RequestMatches => {
            if match_type == ProxyInterceptType::Body {
                "Of request matches"
            } else {
                "In request matches"
            }
        }
        ProxyInterceptRelation::RequestDoesNotMatch => {
            if match_type == ProxyInterceptType::Body {
                "Of request does not match"
            } else {
                "In request does not match"
            }
        }
        ProxyInterceptRelation::ResponseMatches => {
            if match_type == ProxyInterceptType::Body {
                "Of response matches"
            } else {
                "In response matches"
            }
        }
        ProxyInterceptRelation::ResponseDoesNotMatch => {
            if match_type == ProxyInterceptType::Body {
                "Of response does not match"
            } else {
                "In response does not match"
            }
        }
        ProxyInterceptRelation::IsHttp => "Is HTTP",
        ProxyInterceptRelation::IsHttps => "Is HTTPS",
        ProxyInterceptRelation::ContainsParameters => "Contains parameters",
        ProxyInterceptRelation::DoesNotContainParameters => "Does not contain parameters",
        ProxyInterceptRelation::WasModified => "Was modified",
        ProxyInterceptRelation::WasNotModified => "Was not modified",
        ProxyInterceptRelation::WasIntercepted => "Was intercepted",
        ProxyInterceptRelation::WasNotIntercepted => "Was not intercepted",
        ProxyInterceptRelation::WasAnnotated => "Was annotated",
        ProxyInterceptRelation::WasNotAnnotated => "Was not annotated",
        ProxyInterceptRelation::InScope => "Is in target scope",
        ProxyInterceptRelation::NotInScope => "Is not in target scope",
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
    ("Enabled", 72.),
    ("Operator", 74.),
    ("Match type", 144.),
    ("Relationship", 166.),
    ("Condition", 360.),
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
                    "And"
                } else {
                    "Or"
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
                "{} rule enabled",
                if response { "Response" } else { "Request" }
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
