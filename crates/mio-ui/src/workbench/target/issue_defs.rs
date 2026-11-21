use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};

use super::*;

struct RichIssueArticle {
    description_paragraphs: Vec<String>,
    remediation_paragraphs: Vec<String>,
    remediation_bullets: Vec<String>,
}

fn rich_issue_article(def: &MioIssueDefinition) -> RichIssueArticle {
    match def.id {
        "0x00100100" => RichIssueArticle {
            description_paragraphs: vec![
                "当应用程序将用户可控的数据拼接到交由操作系统命令行解释器（shell）执行的命令中时，就会产生操作系统命令注入漏洞。如果未对用户输入实施严格校验，攻击者即可利用 shell 元字符（如 ;、|、&&、$() 或反引号）篡改原本执行的命令结构，并在服务器上注入任意额外的系统命令。".into(),
                "操作系统命令注入属于极高风险漏洞，往往会导致托管该应用的服务器被完全接管，或导致应用自身的数据与核心业务功能遭到破坏。攻击者还可能以受控服务器作为跳板，继续攻击内网中的其他系统。具体的利用危害取决于命令执行所处的安全上下文，以及该进程账户对服务器上敏感资源的访问权限。".into(),
            ],
            remediation_paragraphs: vec![
                "如果条件允许，应用程序应完全避免将用户可控数据直接带入操作系统命令中。在绝大多数业务场景下，都存在更安全的替代方案（例如语言内置库或受控系统 API）来完成服务器级任务，从根本上避免被操纵执行非预期命令。".into(),
                "若经评估确实无法避免将用户提供的数据纳入操作系统命令，则应采用以下两层纵深防御措施来防范注入攻击：".into(),
            ],
            remediation_bullets: vec![
                "对用户输入实施严格的白名单校验。理想情况下，应仅允许预先定义好的受控取值列表；否则也应限制为极短的纯字母数字字符串，拒绝包含任何 shell 元字符、空白符或控制字符的输入。".into(),
                "调用操作系统进程时，应使用按可执行文件名与参数数组启动特定进程的 API，避免将整条命令字符串交由支持命令串联与重定向的 shell 解释器执行（例如使用 Rust std::process::Command::arg、Java ProcessBuilder 或安全的无 shell 调用接口），从而在输入校验被绕过时显著降低危害。".into(),
                "以最小权限原则运行应用服务进程，剥离非必要的系统工具与文件系统写权限，并结合容器隔离或沙箱策略限制命令执行后的横向影响范围。".into(),
            ],
        },
        "0x00100200" => RichIssueArticle {
            description_paragraphs: vec![
                "当应用程序将不可信的用户输入直接拼接进动态构造的 SQL 语句时，就会产生 SQL 注入漏洞。攻击者可以通过构造包含 SQL 语法字符的输入来改变原定查询的逻辑结构，从而绕过身份认证、读取任意表中的敏感数据，甚至篡改或删除数据库记录。".into(),
                "SQL 注入通常具有极高危害性。根据数据库类型、驱动配置以及数据库账号权限的不同，攻击者还可能读取服务器本地文件、写入恶意脚本，或借助扩展存储过程与外部函数直接在数据库主机上执行操作系统命令。".into(),
            ],
            remediation_paragraphs: vec![
                "防止 SQL 注入最有效且通用的方式是在所有数据库访问层中统一使用参数化查询（Prepared Statements），确保用户输入始终作为纯数据值绑定，而绝不会被数据库引擎当作 SQL 代码解析。".into(),
                "在实际工程实践中，建议同步落实以下加固措施：".into(),
            ],
            remediation_bullets: vec![
                "全面使用参数化查询或安全 ORM 绑定接口，彻底消除字符串拼接构造 SQL 的写法。".into(),
                "当表名、列名或排序方向确实需要动态指定时，必须通过严格的静态白名单映射校验，切勿直接拼接原始请求参数。".into(),
                "遵循最小权限原则配置应用数据库账号，禁止业务账号拥有 DBA、文件读写或系统命令执行等高危权限。".into(),
            ],
        },
        "0x00100300" => RichIssueArticle {
            description_paragraphs: vec![
                "当应用程序直接使用用户可控的输入作为文件系统路径（或将其拼接至文件路径中），且未对路径边界进行严格限制时，就会产生路径遍历（目录穿越）漏洞。攻击者可利用 ../ 序列、绝对路径、URL 编码或平台路径规范化差异突破预期目录。".into(),
                "成功利用路径遍历漏洞可能使攻击者读取服务器上的敏感配置文件、应用源代码、凭据密钥或操作系统文件；若应用同时支持文件写入，则可能进一步导致任意文件覆盖乃至远程代码执行。".into(),
            ],
            remediation_paragraphs: vec![
                "在设计文件访问功能时，最理想的做法是彻底避免将用户可控输入直接传递给文件系统 API，改为使用服务端维护的不可预测标识符或索引表映射真实文件。".into(),
                "若业务上必须基于用户提供的文件名访问文件，应落实以下两层防护：".into(),
            ],
            remediation_bullets: vec![
                "对用户输入实施严格白名单校验，仅允许安全的纯文件名字符，拒绝包含路径分隔符（/、\\）、父目录引用（..）或空字节的输入。".into(),
                "拼接基础目录后先调用文件系统规范化接口解析真实绝对路径，并验证规范化后的路径严格位于允许的基准根目录前缀之内。".into(),
            ],
        },
        "0x00100400" => RichIssueArticle {
            description_paragraphs: vec![
                "当应用程序使用配置不当的 XML 解析器处理用户提交的 XML 文档，并允许解析外部实体（External Entities）或文档类型定义（DTD）时，就会产生 XML 外部实体注入（XXE）漏洞。".into(),
                "攻击者可通过声明指向 file://、http:// 等协议的外部实体读取服务器本地文件、探测内网服务（SSRF）、利用带外通道（OOB）外带敏感数据，或通过实体扩展炸弹引发拒绝服务。".into(),
            ],
            remediation_paragraphs: vec![
                "防范 XXE 漏洞最根本的方法是在所有 XML 解析库中显式禁用 DTD 声明与外部实体解析功能；对于纯数据交换接口，优先采用不涉及 DTD 的 JSON 等简化格式。".into(),
            ],
            remediation_bullets: vec![
                "在解析器配置中禁用 DOCTYPE/DTD、外部通用实体、外部参数实体以及 XInclude 处理。".into(),
                "限制 XML 文档大小、嵌套深度与实体展开上限，并对解析服务的出站网络访问实施严格隔离。".into(),
            ],
        },
        "0x00100a00" => RichIssueArticle {
            description_paragraphs: vec![
                "当服务端根据客户端可控的 URL、主机名或地址参数发起对外网络请求，且未对目标地址实施严格限制时，就会产生服务端请求伪造（SSRF）漏洞。".into(),
                "攻击者可诱使服务端访问 localhost、内网管理接口、云平台元数据服务（如 169.254.169.254）或其他受防火墙保护的内部资产，造成内网拓扑探测、云凭据泄露乃至内部未授权操作。".into(),
            ],
            remediation_paragraphs: vec![
                "应将服务端远程拉取逻辑与用户输入解耦，仅允许应用向预定义且经过白名单校验的业务目标发起请求。".into(),
            ],
            remediation_bullets: vec![
                "对目标协议仅放行 http/https，并对域名解析后的实际 IP 地址（包括所有 HTTP 重定向跳转目标）实施校验，阻断回环、链路本地、私有网段与云元数据地址。".into(),
                "在网络层通过独立出口代理或安全组策略隔离应用服务器与内网敏感服务之间的访问路径。".into(),
            ],
        },
        "0x00200100" => RichIssueArticle {
            description_paragraphs: vec![
                "当应用程序将来自不可信来源的数据持久化存储（如数据库、评论区或个人资料），并在后续向其他用户渲染页面时未做适当的上下文输出编码，就会产生存储型跨站脚本（Stored XSS）漏洞。".into(),
                "攻击者注入的恶意 JavaScript 会在受害者浏览受影响页面时自动在受害者浏览器上下文中执行，从而窃取会话凭据、冒充用户执行敏感操作、篡改页面内容或诱导钓鱼。".into(),
            ],
            remediation_paragraphs: vec![
                "应在输出渲染阶段严格按照所处上下文（HTML 正文、HTML 属性、JavaScript、CSS、URL）执行对应的安全编码，并在架构层结合纵深防护策略：".into(),
            ],
            remediation_bullets: vec![
                "优先采用支持自动上下文转义的现代模板引擎与前端框架，避免使用 innerHTML、document.write 等危险 DOM 注入接口。".into(),
                "部署严格的内容安全策略（Content-Security-Policy）并配合 HttpOnly 会话 Cookie，降低脚本注入发生后的实际危害。".into(),
            ],
        },
        _ => {
            let desc_parts: Vec<String> = def
                .description
                .split('。')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| format!("{s}。"))
                .collect();
            let description_paragraphs = if desc_parts.len() > 2 {
                vec![desc_parts[..2].concat(), desc_parts[2..].concat()]
            } else {
                vec![def.description.to_owned()]
            };
            let rem_parts: Vec<String> = def
                .remediation
                .split(['；', ';'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if s.ends_with('。') {
                        s.to_owned()
                    } else {
                        format!("{s}。")
                    }
                })
                .collect();
            let (remediation_paragraphs, remediation_bullets) = if rem_parts.len() > 1 {
                (
                    vec!["建议采取以下针对性安全修复与纵深防御措施：".into()],
                    rem_parts,
                )
            } else {
                (vec![def.remediation.to_owned()], Vec::new())
            };
            RichIssueArticle {
                description_paragraphs,
                remediation_paragraphs,
                remediation_bullets,
            }
        }
    }
}

fn humanize_slug(raw: &str) -> String {
    let clean = raw
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(raw)
        .trim_end_matches(".html")
        .trim_end_matches(".htm");
    let words: Vec<String> = clean
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| match w.to_ascii_lowercase().as_str() {
            "os" => "OS".into(),
            "sql" => "SQL".into(),
            "xss" => "XSS".into(),
            "xxe" => "XXE".into(),
            "ssrf" => "SSRF".into(),
            "csrf" => "CSRF".into(),
            "cors" => "CORS".into(),
            "jwt" => "JWT".into(),
            "http" => "HTTP".into(),
            "https" => "HTTPS".into(),
            "tls" => "TLS".into(),
            "xml" => "XML".into(),
            "dom" => "DOM".into(),
            "idor" => "IDOR".into(),
            "graphql" => "GraphQL".into(),
            "websocket" => "WebSocket".into(),
            "hsts" => "HSTS".into(),
            "csp" => "CSP".into(),
            "ssti" => "SSTI".into(),
            _ => {
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                    None => String::new(),
                }
            }
        })
        .collect();
    if words.is_empty() {
        raw.to_owned()
    } else {
        words.join(" ")
    }
}

fn format_issue_reference_title(url: &str, def: &MioIssueDefinition) -> String {
    if let Some(rest) = url.strip_prefix("https://cwe.mitre.org/data/definitions/") {
        let num = rest.trim_end_matches(".html");
        return format!("CWE-{num}: {} (MITRE)", def.cwe);
    }
    if let Some(rest) = url.strip_prefix("https://cheatsheetseries.owasp.org/cheatsheets/") {
        return format!("OWASP Cheat Sheet: {}", humanize_slug(rest));
    }
    if let Some(rest) = url.strip_prefix("https://owasp.org/www-community/attacks/") {
        return format!("OWASP: {}", humanize_slug(rest));
    }
    if let Some(rest) = url.strip_prefix("https://owasp.org/") {
        return format!("OWASP: {}", humanize_slug(rest));
    }
    if let Some(rest) = url.strip_prefix("https://developer.mozilla.org/") {
        return format!("MDN Web Docs: {}", humanize_slug(rest));
    }
    url.to_owned()
}

impl Workbench {
    pub(in crate::workbench) fn ensure_default_issue_selected(&mut self) {
        if self.target_pane.issue_selected.is_none() {
            let (col, asc) = self.target_pane.issue_sort;
            let matches = sort_issue_definitions(
                filter_issue_definitions(
                    &self.target_pane.issue_search_term,
                    self.target_pane.issue_severity,
                ),
                col,
                asc,
            );
            self.target_pane.issue_selected = matches.first().copied();
        }
    }

    pub(in crate::workbench) fn toggle_issue_sort(
        &mut self,
        column: IssueSortColumn,
        cx: &mut Context<Self>,
    ) {
        let (cur_col, cur_asc) = self.target_pane.issue_sort;
        self.target_pane.issue_sort = if cur_col == column {
            (column, !cur_asc)
        } else {
            (column, true)
        };
        cx.notify();
    }

    pub(in crate::workbench) fn apply_issue_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = match editing::input_snapshot(&self.target_pane.issue_search, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.target_pane.issue_search_term = query;
        let (col, asc) = self.target_pane.issue_sort;
        let matches = sort_issue_definitions(
            filter_issue_definitions(
                &self.target_pane.issue_search_term,
                self.target_pane.issue_severity,
            ),
            col,
            asc,
        );
        self.target_pane.issue_selected =
            matches.first().copied().or(self.target_pane.issue_selected);
        cx.notify();
    }

    pub(in crate::workbench) fn set_issue_severity(
        &mut self,
        severity: Option<MioIssueSeverity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.issue_severity = severity;
        self.apply_issue_search(window, cx);
    }

    fn render_issue_page_header(
        &self,
        active: Option<&'static MioIssueDefinition>,
        severity: Option<MioIssueSeverity>,
        filter_open: bool,
        show_clear: bool,
        title_fg: gpui_kit::Hsla,
        btn_fg: gpui_kit::Hsla,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sev_owner = cx.weak_entity();
        let open_url = active
            .and_then(|def| def.references.first().copied())
            .unwrap_or("https://owasp.org/www-community/vulnerabilities/");
        let severity_btn_label = match severity {
            None => "严重度：全部",
            Some(MioIssueSeverity::High) => "严重度：高危",
            Some(MioIssueSeverity::Medium) => "严重度：中危",
            Some(MioIssueSeverity::Low) => "严重度：低危",
            Some(MioIssueSeverity::Information) => "严重度：信息",
        };

        let open_browser_btn = Button::new("target-issues-open-browser")
            .ghost()
            .rounded(ButtonRounded::Size(px(5.)))
            .h(theme::scaled_px(26.).max(px(24.)))
            .px(theme::scaled_px(10.))
            .border_1()
            .border_color(rgb(if dark { 0x4e535b } else { 0xd0d0d0 }))
            .bg(rgb(if dark { 0x35383d } else { 0xececec }))
            .text_color(btn_fg)
            .accessibility_label("在浏览器中打开")
            .tooltip("在外部浏览器中打开当前选中漏洞定义的参考知识库页面")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(Icon::new(IconName::Globe).size(px(15.)).text_color(btn_fg))
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(btn_fg)
                            .child("在浏览器中打开"),
                    ),
            )
            .on_click(move |_, _, cx| {
                cx.open_url(open_url);
            });

        div()
            .w_full()
            .flex_shrink_0()
            .px(px(14.))
            .pt(px(12.))
            .pb(px(10.))
            .bg(theme::surface())
            .flex()
            .items_start()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(title_fg)
                            .child("漏洞定义"),
                    )
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("此列表包含内置扫描器可检测的全部漏洞定义。"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .flex_shrink_0()
                    .when(filter_open, |row| {
                        row.child(
                            div().w(theme::scaled_px(180.)).child(
                                Input::new(&self.target_pane.issue_search)
                                    .id("target-issues-search")
                                    .aria_label("漏洞定义搜索")
                                    .small()
                                    .h(theme::scaled_px(25.))
                                    .focus_bordered(false)
                                    .suffix(
                                        Button::new("target-issues-apply")
                                            .ghost()
                                            .compact()
                                            .h(theme::scaled_px(20.))
                                            .icon(if show_clear {
                                                IconName::Close
                                            } else {
                                                IconName::Search
                                            })
                                            .accessibility_label(if show_clear {
                                                "清除搜索"
                                            } else {
                                                "搜索漏洞定义"
                                            })
                                            .tooltip(if show_clear {
                                                "清除漏洞定义搜索关键词"
                                            } else {
                                                "按输入的关键词检索漏洞名称、编号、CWE 与描述"
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if show_clear {
                                                    this.target_pane.issue_search.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_value("", window, cx);
                                                        },
                                                    );
                                                }
                                                this.apply_issue_search(window, cx);
                                            })),
                                    ),
                            ),
                        )
                        .child(
                            Button::new("target-issues-severity-filter")
                                .ghost()
                                .small()
                                .compact()
                                .h(theme::scaled_px(26.))
                                .px(px(8.))
                                .rounded(ButtonRounded::Small)
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::surface())
                                .text_color(theme::foreground())
                                .icon(Icon::new(IconName::ListFilter).size(px(12.)))
                                .label(severity_btn_label)
                                .tooltip("按典型严重度筛选漏洞定义列表")
                                .dropdown_menu_with_anchor(
                                    gpui_kit::Anchor::TopRight,
                                    move |menu, _, _| {
                                        [
                                            (None, "全部"),
                                            (Some(MioIssueSeverity::High), "高危 High"),
                                            (Some(MioIssueSeverity::Medium), "中危 Medium"),
                                            (Some(MioIssueSeverity::Low), "低危 Low"),
                                            (
                                                Some(MioIssueSeverity::Information),
                                                "信息 Information",
                                            ),
                                        ]
                                        .into_iter()
                                        .fold(
                                            menu.compact().min_w(px(150.)),
                                            |menu, (sev, lbl)| {
                                                let o = sev_owner.clone();
                                                menu.item(
                                                    PopupMenuItem::new(lbl)
                                                        .checked(severity == sev)
                                                        .on_click(move |_, window, cx| {
                                                            let _ = o.update(cx, |v, cx| {
                                                                v.set_issue_severity(
                                                                    sev, window, cx,
                                                                );
                                                            });
                                                        }),
                                                )
                                            },
                                        )
                                    },
                                ),
                        )
                    })
                    .child(open_browser_btn),
            )
            .into_any_element()
    }

    fn render_issue_defs_table(
        &self,
        matches: Vec<&'static MioIssueDefinition>,
        active: Option<&'static MioIssueDefinition>,
        sort_col: IssueSortColumn,
        sort_asc: bool,
        list_scroll: &ScrollHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header_bg = table_header_bg();
        let header_fg = table_header_fg();
        let stripe_bg = if theme::table_zebra_stripe() {
            table_stripe_bg()
        } else {
            theme::surface()
        };
        let sel_border = selected_row_border();
        let issue_sort_header = |id: &'static str,
                                 label: &'static str,
                                 col: IssueSortColumn,
                                 cx: &mut Context<Self>| {
            let is_active = sort_col == col;
            Button::new(id)
                .ghost()
                .small()
                .compact()
                .rounded(ButtonRounded::None)
                .size_full()
                .px(px(8.))
                .py_0()
                .bg(header_bg)
                .text_color(header_fg)
                .accessibility_label(label)
                .tooltip("点击按此列排序")
                .selected(is_active)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_issue_sort(col, cx);
                }))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(theme::ui_font_size())
                                .font_weight(gpui_kit::FontWeight::NORMAL)
                                .text_color(header_fg)
                                .child(label),
                        )
                        .when(is_active, |row| {
                            row.child(
                                Icon::new(if sort_asc {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(px(11.))
                                .text_color(theme::muted_foreground()),
                            )
                        }),
                )
        };

        let table_header = div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(header_bg)
            .text_color(header_fg)
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .min_w(px(140.))
                    .h_full()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(issue_sort_header(
                        "target-issues-sort-name",
                        "名称",
                        IssueSortColumn::Name,
                        cx,
                    )),
            )
            .child(
                div()
                    .w(theme::scaled_px(108.))
                    .h_full()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(issue_sort_header(
                        "target-issues-sort-severity",
                        "典型严重度",
                        IssueSortColumn::Severity,
                        cx,
                    )),
            )
            .child(
                div()
                    .w(theme::scaled_px(112.))
                    .h_full()
                    .flex_shrink_0()
                    .child(issue_sort_header(
                        "target-issues-sort-index",
                        "类型编号",
                        IssueSortColumn::TypeIndex,
                        cx,
                    )),
            )
            .child(div().w(px(12.)).h_full().flex_shrink_0().bg(header_bg));

        div()
            .size_full()
            .pr(px(4.))
            .child(
                div()
                    .size_full()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(table_header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .child(
                                div()
                                    .id("target-issues-list")
                                    .test_support()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .overflow_y_scroll()
                                    .track_scroll(list_scroll)
                                    .flex()
                                    .flex_col()
                                    .bg(theme::surface())
                                    .children(matches.into_iter().enumerate().map(
                                        |(index, definition)| {
                                            let id = definition.id;
                                            let selected = active.map(|item| item.id) == Some(id);
                                            let row_bg = if selected {
                                                theme::selected()
                                            } else if index % 2 == 1 {
                                                stripe_bg
                                            } else {
                                                theme::surface()
                                            };
                                            div()
                                                .id(("target-issue-row", index))
                                                .test_support()
                                                .aria_label(format!("{} {id}", definition.name))
                                                .aria_selected(selected)
                                                .w_full()
                                                .h(theme::scaled_px(21.))
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .gap_0()
                                                .cursor_pointer()
                                                .bg(row_bg)
                                                .when(selected, |el| {
                                                    el.border_1().border_color(sel_border)
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.target_pane.issue_selected =
                                                        Some(definition);
                                                    cx.notify();
                                                }))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w(px(140.))
                                                        .px(px(8.))
                                                        .truncate()
                                                        .text_size(theme::ui_font_size())
                                                        .child(definition.name),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(108.))
                                                        .flex_shrink_0()
                                                        .px(px(8.))
                                                        .truncate()
                                                        .text_size(theme::ui_font_size())
                                                        .child(
                                                            definition.typical_severity.en_label(),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(112.))
                                                        .flex_shrink_0()
                                                        .px(px(8.))
                                                        .truncate()
                                                        .text_size(theme::ui_font_size())
                                                        .child(id),
                                                )
                                        },
                                    )),
                            )
                            .child(mio_vertical_scrollbar_track(
                                "target-issues-list-vscroll",
                                list_scroll,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_issue_def_detail(
        active: Option<&'static MioIssueDefinition>,
        detail_scroll: &ScrollHandle,
        title_fg: gpui_kit::Hsla,
        link_fg: gpui_kit::Hsla,
    ) -> AnyElement {
        div()
            .size_full()
            .pl(px(4.))
            .child(
                div()
                    .size_full()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .flex()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("target-issue-detail")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(detail_scroll)
                            .p(px(18.))
                            .when_some(active, |this, definition| {
                                let article = rich_issue_article(definition);
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(14.))
                                        .child(
                                            div()
                                                .id("target-issue-title")
                                                .test_support()
                                                .aria_label(definition.name)
                                                .text_size(theme::scaled_px(16.))
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .text_color(title_fg)
                                                .child(definition.name),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .text_size(theme::scaled_px(14.))
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .text_color(title_fg)
                                                        .child("描述"),
                                                )
                                                .children(
                                                    article.description_paragraphs.into_iter().map(
                                                        |para| {
                                                            div()
                                                                .text_size(theme::ui_font_size())
                                                                .line_height(theme::scaled_px(20.))
                                                                .text_color(theme::foreground())
                                                                .child(para)
                                                        },
                                                    ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .text_size(theme::scaled_px(14.))
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .text_color(title_fg)
                                                        .child("修复建议"),
                                                )
                                                .children(
                                                    article.remediation_paragraphs.into_iter().map(
                                                        |para| {
                                                            div()
                                                                .text_size(theme::ui_font_size())
                                                                .line_height(theme::scaled_px(20.))
                                                                .text_color(theme::foreground())
                                                                .child(para)
                                                        },
                                                    ),
                                                )
                                                .when(
                                                    !article.remediation_bullets.is_empty(),
                                                    |col| {
                                                        col.child(
                                                            div()
                                                                .pl(px(16.))
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(6.))
                                                                .children(
                                                                    article
                                                                        .remediation_bullets
                                                                        .into_iter()
                                                                        .map(|item| {
                                                                            div()
                                                                                .flex()
                                                                                .items_start()
                                                                                .gap(px(6.))
                                                                                .text_size(
                                                                                    theme::ui_font_size(),
                                                                                )
                                                                                .line_height(
                                                                                    theme::scaled_px(
                                                                                        20.,
                                                                                    ),
                                                                                )
                                                                                .text_color(
                                                                                    theme::foreground(),
                                                                                )
                                                                                .child("•")
                                                                                .child(
                                                                                    div()
                                                                                        .flex_1()
                                                                                        .min_w_0()
                                                                                        .child(item),
                                                                                )
                                                                        }),
                                                                ),
                                                        )
                                                    },
                                                ),
                                        )
                                        .when(!definition.references.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(6.))
                                                    .child(
                                                        div()
                                                            .text_size(theme::scaled_px(14.))
                                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                                            .text_color(title_fg)
                                                            .child("参考资料"),
                                                    )
                                                    .children(
                                                        definition
                                                            .references
                                                            .iter()
                                                            .enumerate()
                                                            .map(|(ref_idx, reference)| {
                                                                let url = *reference;
                                                                let link_title =
                                                                    format_issue_reference_title(
                                                                        url, definition,
                                                                    );
                                                                div()
                                                                    .id((
                                                                        "target-issue-ref",
                                                                        ref_idx,
                                                                    ))
                                                                    .pl(px(12.))
                                                                    .cursor_pointer()
                                                                    .text_size(
                                                                        theme::ui_font_size(),
                                                                    )
                                                                    .text_color(link_fg)
                                                                    .hover(|s| s.underline())
                                                                    .on_click(move |_, _, cx| {
                                                                        cx.open_url(url);
                                                                    })
                                                                    .child(format!(
                                                                        "• {link_title}"
                                                                    ))
                                                            }),
                                                    ),
                                            )
                                        })
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.))
                                                .child(
                                                    div()
                                                        .text_size(theme::scaled_px(14.))
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .text_color(title_fg)
                                                        .child("漏洞分类"),
                                                )
                                                .child(
                                                    div()
                                                        .pl(px(12.))
                                                        .text_size(theme::ui_font_size())
                                                        .text_color(theme::foreground())
                                                        .child(format!(
                                                            "• {} (Common Weakness Enumeration)",
                                                            definition.cwe
                                                        )),
                                                ),
                                        ),
                                )
                            })
                            .when_none(&active, |this| {
                                this.child(
                                    div()
                                        .text_color(theme::muted_foreground())
                                        .child("没有匹配的漏洞定义。"),
                                )
                            }),
                    )
                    .child(mio_vertical_scrollbar_track(
                        "target-issues-detail-vscroll",
                        detail_scroll,
                    )),
            )
            .into_any_element()
    }

    pub(super) fn render_issue_page(
        &mut self,
        view_ctx: &TargetViewContext<'_>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let severity = self.target_pane.issue_severity;
        let search_term = self.target_pane.issue_search_term.clone();
        let filter_open = self.target_pane.issue_filter_open;
        let (sort_col, sort_asc) = self.target_pane.issue_sort;
        let matches = sort_issue_definitions(
            filter_issue_definitions(&search_term, severity),
            sort_col,
            sort_asc,
        );
        let active = self
            .target_pane
            .issue_selected
            .filter(|sel| matches.iter().any(|m| m.id == sel.id))
            .or_else(|| matches.first().copied());
        let current_input = self.target_pane.issue_search.read(cx).value();
        let show_clear = !search_term.is_empty() && current_input.trim() == search_term.as_str();
        let dark = theme::is_dark();
        let title_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };
        let btn_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x1f1f1f).into()
        };
        let link_fg: gpui_kit::Hsla = rgb(if dark { 0x6ba4e8 } else { 0x1a5fb4 }).into();

        let page_header = self.render_issue_page_header(
            active, severity, filter_open, show_clear, title_fg, btn_fg, dark, cx,
        );
        let left_table = self.render_issue_defs_table(
            matches,
            active,
            sort_col,
            sort_asc,
            view_ctx.issues_list_scroll,
            cx,
        );
        let right_detail = Self::render_issue_def_detail(
            active,
            view_ctx.issues_detail_scroll,
            title_fg,
            link_fg,
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                let is_find =
                    event.keystroke.key == "f" && event.keystroke.modifiers.secondary(cx);
                if is_find {
                    this.target_pane.issue_filter_open = !this.target_pane.issue_filter_open;
                    if this.target_pane.issue_filter_open {
                        this.target_pane
                            .issue_search
                            .update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                } else if event.keystroke.key == "escape" && this.target_pane.issue_filter_open {
                    this.target_pane.issue_filter_open = false;
                    cx.notify();
                }
            }))
            .child(page_header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(12.))
                    .pb(px(10.))
                    .child(
                        h_resizable("target-issues-hsplit")
                            .child(
                                resizable_panel()
                                    .size(px(620.))
                                    .size_range(px(320.)..px(920.))
                                    .child(left_table),
                            )
                            .child(
                                resizable_panel()
                                    .size_range(px(280.)..px(10000.))
                                    .child(right_detail),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[allow(dead_code)]
pub(in crate::workbench) fn severity_color(severity: MioIssueSeverity) -> gpui_kit::Hsla {
    match severity {
        MioIssueSeverity::High => rgb(0xd93025).into(),
        MioIssueSeverity::Medium => rgb(0xe37400).into(),
        MioIssueSeverity::Low => rgb(0x1a73e8).into(),
        MioIssueSeverity::Information => rgb(0x5f6368).into(),
    }
}
