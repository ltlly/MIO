use super::*;

impl Workbench {
    pub(in crate::workbench) fn send_selected_to_organiser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.archive_to_organiser(window, cx);
    }

    pub(in crate::workbench) fn archive_to_organiser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let number = match next_organiser_number(&self.workspace.organiser_items) {
            Ok(number) => number,
            Err(err) => {
                self.fail(err.to_string(), cx);
                return;
            }
        };
        let extracted = match self.extract_organiser_item(number, window, cx) {
            Ok(item) => item,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let item_id = extracted.id;
        let added_number = extracted.number;
        self.workspace.organiser_items.push(extracted);
        if let Err(err) = validate_organiser_items(&self.workspace.organiser_items)
            .and_then(|()| self.workspace.validate_structure())
        {
            self.workspace.organiser_items.pop();
            self.fail(format!("未添加到 Organiser：{err}"), cx);
            return;
        }
        if self.organiser.selected_id.is_none() {
            self.organiser.selected_id = Some(item_id);
            self.sync_organiser_table(window, cx);
            self.reload_selected_organiser_detail(window, cx);
        } else {
            self.sync_organiser_table(window, cx);
        }
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("已添加到 Organiser (#{added_number})。");
        cx.notify();
    }

    fn extract_organiser_item(
        &self,
        number: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<OrganiserItem, String> {
        match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                self.extract_intercept_organiser_item(number, window, cx)
            }
            Tool::Proxy => self.extract_proxy_history_organiser_item(number, window, cx),
            Tool::Repeater => self.extract_repeater_organiser_item(number, window, cx),
            Tool::Intruder => {
                let id = self
                    .workspace
                    .intruder
                    .active
                    .ok_or_else(|| "没有选中的 Intruder 请求。".to_owned())?;
                let spec = self.snapshot_intruder_spec(id, window, cx)?;
                OrganiserItem::from_draft(number, "Intruder", &spec.draft, None)
                    .map_err(|err| err.to_string())
            }
            Tool::Target => self.extract_target_organiser_item(number, window, cx),
            Tool::Logger => {
                let flow_id = self
                    .extra_tools
                    .logger_selected
                    .ok_or_else(|| "没有选中的 Logger 流量记录。".to_owned())?;
                let (record, _) = self
                    .extra_tools
                    .logger_loaded_detail
                    .as_ref()
                    .filter(|(record, _)| record.summary.id == flow_id)
                    .ok_or_else(|| "Logger 流量详情尚未读取完成。".to_owned())?;
                OrganiserItem::from_flow_record(number, "Logger", record)
                    .map_err(|err| err.to_string())
            }
            _ => Err("当前工具上下文没有可归档到 Organiser 的 HTTP 报文。".into()),
        }
    }

    fn extract_intercept_organiser_item(
        &self,
        number: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<OrganiserItem, String> {
        let pending = self
            .intercepts
            .iter()
            .find(|pending| Some(pending.id()) == self.selected_intercept)
            .ok_or_else(|| "没有选中的拦截消息。".to_owned())?;
        match &pending.target {
            InterceptTarget::Http { stage, record, .. } => {
                let (req_raw, resp_raw, status) = match stage {
                    InterceptStage::Request => (
                        pending.editor.bytes(window, cx)?,
                        record.response.as_ref().map(response_to_raw),
                        record.response.as_ref().map(|resp| resp.status),
                    ),
                    InterceptStage::Response => (
                        request_to_raw(&record.request),
                        Some(pending.editor.bytes(window, cx)?),
                        None,
                    ),
                };
                OrganiserItem::from_raw_parts(
                    number,
                    "Proxy",
                    &record.request.url,
                    req_raw,
                    resp_raw,
                    status,
                    record.request.version,
                )
                .map_err(|err| err.to_string())
            }
            InterceptTarget::WebSocket { .. } => {
                Err("当前选中的是 WebSocket 消息，暂不支持添加到 Organiser。".into())
            }
        }
    }

    fn extract_proxy_history_organiser_item(
        &self,
        number: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<OrganiserItem, String> {
        let selected = self
            .selected_history
            .ok_or_else(|| "请先选中一条 HTTP 历史记录。".to_owned())?;
        if let Some(error) = &self.detail_error {
            return Err(format!("无法读取所选历史记录：{error}"));
        }
        match selected {
            HistoryEntryId::Live(flow_id) => {
                let detail = self
                    .detail
                    .as_ref()
                    .filter(|record| record.summary.id == flow_id)
                    .ok_or_else(|| "历史记录正文尚未读取完成。".to_owned())?;
                let chosen = if self.history_original_view {
                    FlowRecord {
                        summary: detail.summary.clone(),
                        request: self
                            .detail_original
                            .request
                            .clone()
                            .unwrap_or_else(|| detail.request.clone()),
                        response: self
                            .detail_original
                            .response
                            .clone()
                            .or_else(|| detail.response.clone()),
                    }
                } else {
                    (**detail).clone()
                };
                OrganiserItem::from_flow_record(number, "Proxy", &chosen)
                    .map_err(|err| err.to_string())
            }
            HistoryEntryId::External { source_id, item_id } => {
                if !self.imported_history_request_ready() {
                    return Err("归档请求正文尚未读取完成。".into());
                }
                let (origin, row_req_ver, status, highlight) = {
                    let row = self
                        .history
                        .read(cx)
                        .delegate()
                        .row_by_id(HistoryEntryId::External { source_id, item_id })
                        .or(self.imported_history.row.as_ref())
                        .and_then(|row| match row {
                            HistoryRow::ExtProxy(row) => Some(row.as_ref()),
                            HistoryRow::Live(_) => None,
                        })
                        .ok_or_else(|| "所选归档历史记录摘要不可用。".to_owned())?;
                    let req_ver = row
                        .request
                        .as_ref()
                        .or(row.edited_request.as_ref())
                        .and_then(|meta| meta.declared_version);
                    let status = row
                        .response
                        .as_ref()
                        .or(row.edited_response.as_ref())
                        .and_then(|meta| meta.status);
                    (
                        row.origin.clone().unwrap_or_default(),
                        req_ver,
                        status,
                        row.highlight
                            .as_ref()
                            .map(|h| h.color)
                            .filter(|color| (1..=9).contains(color)),
                    )
                };
                let req_raw = self.history_request.bytes(window, cx)?;
                let resp_raw = if self.imported_history_response_ready() {
                    let bytes = self.history_response.bytes(window, cx)?;
                    (!bytes.is_empty()).then_some(bytes)
                } else {
                    None
                };
                let version = self
                    .imported_history_request()
                    .and_then(|meta| meta.declared_version)
                    .or(row_req_ver)
                    .unwrap_or(HttpVersion::Http11);
                let mut item = OrganiserItem::from_raw_parts(
                    number, "Proxy", &origin, req_raw, resp_raw, status, version,
                )
                .map_err(|err| err.to_string())?;
                item.highlight = highlight;
                Ok(item)
            }
        }
    }

    fn extract_repeater_organiser_item(
        &self,
        number: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<OrganiserItem, String> {
        let tab_id = self.workspace.active.ok_or_else(|| {
            if self.workspace.active_websocket_tab.is_some() {
                "当前选中的是 WebSocket Repeater 标签，暂不支持添加到 Organiser。".to_owned()
            } else {
                "没有活动的 Repeater 请求。".to_owned()
            }
        })?;
        let tab = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| "没有活动的 Repeater 请求。".to_owned())?;
        if let Some(parsed_resp) = self.repeater_response_for_tab(tab, window, cx) {
            return OrganiserItem::from_draft(number, "Repeater", &tab.draft, Some(&parsed_resp))
                .map_err(|err| err.to_string());
        }
        let resp_raw = self
            .tab_editors
            .get(&tab.id)
            .map(|editors| editors.response.bytes(window, cx))
            .transpose()?
            .filter(|bytes| !bytes.is_empty());
        if let Some(resp_bytes) = resp_raw {
            OrganiserItem::from_raw_parts(
                number,
                "Repeater",
                &tab.draft.target,
                tab.draft.raw.clone(),
                Some(resp_bytes),
                None,
                tab.draft.version,
            )
            .map_err(|err| err.to_string())
        } else {
            OrganiserItem::from_draft(number, "Repeater", &tab.draft, None)
                .map_err(|err| err.to_string())
        }
    }

    fn extract_target_organiser_item(
        &self,
        number: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<OrganiserItem, String> {
        let row = self
            .target_pane
            .selected_row
            .as_ref()
            .ok_or_else(|| "没有选中的站点地图请求。".to_owned())?;
        let req_raw = self.target_pane.request.bytes(window, cx)?;
        if req_raw.is_empty() {
            return Err("站点地图请求报文尚未加载完成。".into());
        }
        let resp_bytes = self.target_pane.response.bytes(window, cx)?;
        let resp_raw = (!resp_bytes.is_empty()).then_some(resp_bytes);
        let (origin_or_url, status) = match row {
            HistoryRow::Live(summary) => (summary.url.clone(), summary.status),
            HistoryRow::ExtProxy(archive) => (
                archive.origin.clone().unwrap_or_default(),
                archive
                    .response
                    .as_ref()
                    .or(archive.edited_response.as_ref())
                    .and_then(|meta| meta.status),
            ),
        };
        OrganiserItem::from_raw_parts(
            number,
            "Target",
            &origin_or_url,
            req_raw,
            resp_raw,
            status,
            HttpVersion::Http11,
        )
        .map_err(|err| err.to_string())
    }
}
