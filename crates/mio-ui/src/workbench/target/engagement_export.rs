use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use gpui_kit::ClipboardItem;

use super::engagement::*;
use super::*;

impl Workbench {
    pub(in crate::workbench) fn delete_selected_target_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut target_ids: HashSet<HistoryEntryId> = self.target_pane.selected_ids.clone();
        if let Some(sel) = self.target_pane.selected_id {
            target_ids.insert(sel);
        }
        if target_ids.is_empty() {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        }
        let count = target_ids.len();
        for id in &target_ids {
            self.target_pane.hidden_item_ids.insert(*id);
        }
        let before = self.target_pane.items.len();
        self.target_pane
            .items
            .retain(|row| !target_ids.contains(&row.id()));
        let removed = before.saturating_sub(self.target_pane.items.len());
        if removed > 0 {
            self.target_pane.total_items = self.target_pane.total_items.saturating_sub(removed);
        }
        self.target_pane.selected_id = None;
        self.target_pane.selected_ids.clear();
        self.target_pane.selection_anchor = None;
        self.target_pane.selected_row = None;
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        self.error = None;
        self.status = if count > 1 {
            format!("已从站点地图删除 {count} 个所选项。")
        } else {
            "已从站点地图删除所选项。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn save_selected_target_item_to_file(
        &mut self,
        as_xml: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let Some(row) = self.target_pane.selected_row.clone() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(&row);
        if url.trim().is_empty() {
            self.fail("所选站点地图记录缺少有效 URL。", cx);
            return;
        }
        let req_bytes = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        let resp_bytes = self
            .selected_target_response_bytes(cx)
            .unwrap_or_default();
        let summary = target_row_summary_with_metadata(&row, Some(&resp_bytes), None);
        let status_code = row_status_code(&row);
        let payload = if as_xml {
            match crate::workbench::wb_render_history::build_ext_items_xml(
                &url,
                if summary.method.is_empty() {
                    "GET"
                } else {
                    &summary.method
                },
                status_code,
                "",
                &req_bytes,
                &resp_bytes,
                "",
            ) {
                Ok(xml) => xml.into_bytes(),
                Err(err) => {
                    self.fail(err, cx);
                    return;
                }
            }
        } else if let Some(record) = self.target_pane.selected_live_record.as_deref() {
            match mio_core::export_flows_to_har(std::slice::from_ref(record)) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(err.to_string(), cx);
                    return;
                }
            }
        } else {
            let item = serde_json::json!({
                "url": url,
                "host": summary.host,
                "method": summary.method,
                "status": status_code,
                "length": resp_bytes.len(),
                "mime": summary.mime,
                "title": summary.title,
                "request_base64": BASE64_STANDARD.encode(&req_bytes),
                "response_base64": BASE64_STANDARD.encode(&resp_bytes),
            });
            match serde_json::to_vec_pretty(&item) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(format!("序列化 JSON 失败：{err}"), cx);
                    return;
                }
            }
        };
        let suggested = if as_xml {
            "site-map-item.xml"
        } else if self.target_pane.selected_live_record.is_some() {
            "site-map-item.har"
        } else {
            "site-map-item.json"
        };
        self.prompt_and_write_target_file(
            suggested,
            if as_xml { "xml" } else { "json" },
            payload,
            "已导出站点地图条目到文件",
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn save_target_branch_items_to_file(
        &mut self,
        key: String,
        as_xml: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let rows = self.collect_branch_rows(&key, cx);
        let selected_id = self.target_pane.selected_id;
        let selected_req = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        let selected_resp = self
            .selected_target_response_bytes(cx)
            .unwrap_or_default();

        let mut export_items = Vec::new();
        for row in &rows {
            let url = row_url(row);
            if url.is_empty() {
                continue;
            }
            let summary = target_row_summary_with_metadata(row, None, None);
            let method = if summary.method.is_empty() {
                "GET".to_owned()
            } else {
                summary.method
            };
            let status = row_status_code(row);
            let (req, resp) = if selected_id == Some(row.id()) {
                (selected_req.clone(), selected_resp.clone())
            } else {
                (
                    super::site_map_entries::synthetic_scan_draft(&method, &url).raw,
                    Vec::new(),
                )
            };
            export_items.push((url, method, status, String::new(), req, resp, String::new()));
        }
        if export_items.is_empty() {
            for url in collect_branch_urls(
                &self.target_pane.items,
                &self.target_pane.tree_state.nodes,
                &self.target_pane.tree_state.hidden_prefixes,
                &key,
            ) {
                let req = super::site_map_entries::synthetic_scan_draft("GET", &url).raw;
                export_items.push((
                    url,
                    "GET".into(),
                    None,
                    String::new(),
                    req,
                    Vec::new(),
                    String::new(),
                ));
            }
        }
        if export_items.is_empty() {
            self.fail("当前分支下没有可保存的站点地图条目。", cx);
            return;
        }
        let count = export_items.len();
        let payload = if as_xml {
            match build_multi_item_ext_xml(&export_items) {
                Ok(xml) => xml.into_bytes(),
                Err(err) => {
                    self.fail(err, cx);
                    return;
                }
            }
        } else {
            let json_items: Vec<serde_json::Value> = export_items
                .iter()
                .map(|(url, method, status, _, req, resp, _)| {
                    serde_json::json!({
                        "url": url,
                        "method": method,
                        "status": status,
                        "length": resp.len(),
                        "request_base64": BASE64_STANDARD.encode(req),
                        "response_base64": BASE64_STANDARD.encode(resp),
                    })
                })
                .collect();
            match serde_json::to_vec_pretty(&serde_json::json!({
                "branch": key,
                "count": count,
                "items": json_items,
            })) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(format!("序列化 JSON 失败：{err}"), cx);
                    return;
                }
            }
        };
        let suggested = if as_xml {
            "site-map-items.xml"
        } else {
            "site-map-items.json"
        };
        self.prompt_and_write_target_file(
            suggested,
            if as_xml { "xml" } else { "json" },
            payload,
            &format!("已保存 {count} 条站点地图项到文件"),
            window,
            cx,
        );
    }

    pub(super) fn prompt_and_write_target_file(
        &mut self,
        suggested_name: &str,
        default_ext: &'static str,
        payload: Vec<u8>,
        success_prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        let success_prefix = success_prefix.to_owned();
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested_name),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_path = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != project {
                            view.fail("项目已切换，导出文件未写入磁盘。", cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(mut path))) => {
                                if path.extension().is_none() {
                                    path.set_extension(default_ext);
                                }
                                Some(path)
                            }
                            Ok(Ok(None)) => None,
                            Ok(Err(err)) => {
                                view.fail(format!("无法选择保存路径：{err}"), cx);
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已意外关闭。", cx);
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(path) = pending_path else {
                return;
            };
            let write_path = path.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_path, &payload)
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| match write_result {
                    Ok(()) => {
                        view.error = None;
                        view.status = format!("{success_prefix}：{}", path.display());
                        cx.notify();
                    }
                    Err(err) => view.fail(format!("写入导出文件失败：{err}"), cx),
                })
            });
        })
        .detach();
    }

    // --- Branch copy links ---

    pub(in crate::workbench) fn copy_target_branch_links(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut seen = HashSet::new();
        let mut links = Vec::new();
        for (url, raw) in self.collect_loaded_responses_for_scope(Some(&key), window, cx) {
            if let Ok(found) =
                crate::workbench::wb_render_history::extract_response_links_list(&url, &raw)
            {
                for link in found {
                    if seen.insert(link.clone()) {
                        links.push(link);
                    }
                }
            }
        }
        // Also include known child/branch URLs from site map items & nodes under this host
        for url in collect_branch_urls(
            &self.target_pane.items,
            &self.target_pane.tree_state.nodes,
            &self.target_pane.tree_state.hidden_prefixes,
            &key,
        ) {
            if seen.insert(url.clone()) {
                links.push(url);
            }
        }
        let history_rows = self.history.read(cx).delegate().rows.clone();
        for row in &history_rows {
            if self.target_pane.hidden_item_ids.contains(&row.id()) {
                continue;
            }
            let url = row_url(row);
            if !url.is_empty()
                && !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                && url_matches_node_prefix(&url, &key)
                && seen.insert(url.clone())
            {
                links.push(url);
            }
        }
        if links.is_empty() {
            self.fail("当前主机分支中未提取到可复制的链接。", cx);
            return;
        }
        let count = links.len();
        cx.write_to_clipboard(ClipboardItem::new_string(links.join("\n")));
        self.error = None;
        self.status = format!("已复制该主机中的 {count} 条链接到剪贴板。");
        cx.notify();
    }

    // --- Engagement tools ---

    pub(in crate::workbench) fn open_global_search_with_term(
        &mut self,
        term: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let term = term.trim().to_owned();
        self.global_search.input.update(cx, |input, cx| {
            input.set_value(term, window, cx);
        });
        self.open_global_search(window, cx);
    }
}
