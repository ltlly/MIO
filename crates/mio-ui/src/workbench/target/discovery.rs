use mio_core::{
    Draft, HttpVersion, WorkTab, audit_engine::discover_same_origin_links_and_forms,
    parse_response, request_to_raw,
};

use super::*;

const MAX_EXTRACTED_REPEATER_TABS: usize = 16;
const MAX_WORKSPACE_TABS: usize = 512;

fn workspace_total_tabs(ws: &Workspace) -> usize {
    ws.tabs.len()
        + ws.closed.len()
        + ws.intruder.tabs.len()
        + ws.intruder.closed.len()
        + ws.websocket_tabs.len()
}

fn selected_row_base_url(row: &HistoryRow) -> Option<String> {
    match row {
        HistoryRow::Live(summary) => (!summary.url.is_empty()).then(|| summary.url.clone()),
        HistoryRow::ExtProxy(archive) => {
            let req = archive
                .request
                .as_ref()
                .or(archive.edited_request.as_ref())?;
            let target = req.request_target.as_deref().unwrap_or_default();
            if target.is_empty() {
                return None;
            }
            if target.starts_with('/') {
                let origin = archive.origin.as_deref().unwrap_or_default();
                (!origin.is_empty()).then(|| format!("{origin}{target}"))
            } else {
                Some(target.to_owned())
            }
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn extract_selected_target_response_links(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let Some(base_url) = selected_row_base_url(row) else {
            self.fail("所选站点地图记录缺少有效的请求 URL。", cx);
            return;
        };
        let raw_response = match self.target_pane.response.bytes(window, cx) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if raw_response.is_empty() {
            self.fail("所选站点地图记录尚无已加载的 HTTP 响应报文。", cx);
            return;
        }
        let parsed_response = match parse_response(&raw_response, HttpVersion::Http11) {
            Ok(resp) => resp,
            Err(error) => {
                self.fail(format!("无法解析选中响应报文：{error}"), cx);
                return;
            }
        };
        let endpoints = discover_same_origin_links_and_forms(
            &base_url,
            &parsed_response,
            &self.workspace.scope,
        );
        if endpoints.is_empty() {
            self.error = None;
            self.status = "选中响应中未发现符合同源与目标范围的链接或表单。".into();
            cx.notify();
            return;
        }

        let available = MAX_WORKSPACE_TABS
            .saturating_sub(workspace_total_tabs(&self.workspace))
            .min(MAX_EXTRACTED_REPEATER_TABS);
        if available == 0 {
            self.fail("工作标签数量已达 512 上限；未载入提取链接。", cx);
            return;
        }

        let prev_len = self.workspace.tabs.len();
        let prev_active = self.workspace.active;
        let prev_ws = self.workspace.active_websocket_tab;
        let mut first_new_id = None;
        let mut created = 0usize;
        for ep in endpoints.iter().take(available) {
            let id = Uuid::new_v4();
            if first_new_id.is_none() {
                first_new_id = Some(id);
            }
            let raw = request_to_raw(&ep.request);
            let title = self.next_repeater_tab_title(Some(&ep.request), Some(&raw));
            self.workspace.tabs.push(WorkTab {
                id,
                title,
                group: None,
                draft: Draft {
                    revision: 1,
                    target: ep.url.clone(),
                    raw,
                    version: ep.request.version,
                },
                last_flow: None,
                archive_response: None,
                history: None,
            });
            created += 1;
        }
        let Some(active_id) = first_new_id else {
            return;
        };
        self.workspace.active = Some(active_id);
        self.workspace.active_websocket_tab = None;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.truncate(prev_len);
            self.workspace.active = prev_active;
            self.workspace.active_websocket_tab = prev_ws;
            self.fail(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        if let Some(editor) = self.tab_editors.get(&active_id) {
            editor
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        self.error = None;
        self.status = format!(
            "已从选中响应提取 {} 个同源端点，载入 {} 个 Repeater 标签。",
            endpoints.len(),
            created
        );
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::test::{TestEventExt, TestWindowExt};
    use gpui_kit::{MouseButton, TestAppContext};
    use mio_core::Header;

    use super::*;
    use crate::workbench::tests::{drain, fixture, record};

    #[gpui_kit::test]
    fn sitemap_extract_links_loads_same_origin_endpoints_into_repeater_without_network(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);
        let mut flow = record("https://portal.example.com/index.html", "");
        flow.summary.status = Some(200);
        flow.response = Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: b"text/html; charset=utf-8".to_vec(),
            }],
            body: br#"<html><body>
                <a href="/account/profile#sec">Profile</a>
                <a href="javascript:void(0)">IgnoreJs</a>
                <a href="https://other.example.org/out">CrossOrigin</a>
                <form action="/account/prefs" method="post">
                    <input name="lang" value="zh-CN">
                </form>
            </body></html>"#
                .to_vec(),
            trailers: Vec::new(),
        });
        let flow_id = flow.summary.id;

        f.view.update(cx, |view, cx| {
            view.tool = Tool::Target;
            view.target_pane.items = vec![HistoryRow::Live(flow.summary.clone())];
            view.target_pane.total_items = 1;
            cx.notify();
        });
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click(("target-sitemap-row", 0usize), cx);
            })
            .unwrap();
        let read_rev = drain(&mut f.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::ReadFlow {
                    selection_revision,
                    flow_id: id,
                    ..
                } if id == flow_id => Some(selection_revision),
                _ => None,
            })
            .expect("clicking sitemap row reads flow detail");

        f.events
            .try_send(Event::FlowDetail {
                project: f.project,
                selection_revision: read_rev,
                record: Box::new(flow),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();

        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-sitemap-options-menu", cx);
                window.render_frame(cx);
                let item = window
                    .within("popup-menu")
                    .find_by_label("提取选中响应链接")
                    .bounds()
                    .center();
                window.pointer_down(item, MouseButton::Left, cx);
                window.pointer_up(item, MouseButton::Left, cx);
            })
            .unwrap();
        cx.run_until_parked();

        f.view.update(cx, |view, _| {
            assert_eq!(view.tool, Tool::Repeater);
            assert_eq!(view.workspace.tabs.len(), 2);
            assert_eq!(
                view.workspace.tabs[0].draft.target,
                "https://portal.example.com/account/profile"
            );
            assert!(
                view.workspace.tabs[0]
                    .draft
                    .raw
                    .starts_with(b"GET /account/profile HTTP/1.1\r\n")
            );
            assert_eq!(
                view.workspace.tabs[1].draft.target,
                "https://portal.example.com/account/prefs"
            );
            let post_raw = String::from_utf8_lossy(&view.workspace.tabs[1].draft.raw);
            assert!(post_raw.starts_with("POST /account/prefs HTTP/1.1\r\n"));
            assert!(post_raw.ends_with("\r\n\r\nlang=zh-CN"));
        });
        let emitted = drain(&mut f.commands);
        assert!(
            !emitted.is_empty()
                && emitted
                    .iter()
                    .all(|cmd| matches!(cmd, Command::SaveWorkspace { .. }))
        );
    }
}
