use super::*;

#[cfg(test)]
mod tests;

impl Workbench {
    pub(crate) fn can_copy_proxy_row_har(&self, row_entry: Option<&HistoryRow>) -> bool {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return false;
        }
        let Some(HistoryRow::Live(flow)) = row_entry else {
            return false;
        };
        self.selected_history == Some(HistoryEntryId::Live(flow.id))
            && self.detail_error.is_none()
            && self
                .detail
                .as_ref()
                .is_some_and(|detail| detail.summary.id == flow.id)
    }

    fn selected_flow_for_har(&self) -> Result<&FlowRecord, String> {
        match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                let selected = self
                    .selected_history
                    .ok_or_else(|| "没有选中的 HTTP 历史记录。".to_owned())?;
                match selected {
                    HistoryEntryId::Live(flow_id) => {
                        if let Some(error) = &self.detail_error {
                            return Err(format!("无法读取所选历史记录：{error}"));
                        }
                        self.detail
                            .as_deref()
                            .filter(|record| record.summary.id == flow_id)
                            .ok_or_else(|| {
                                "正在读取所选历史记录详情，请稍后重试复制为 HAR。".into()
                            })
                    }
                    HistoryEntryId::External { .. } => {
                        Err("仅支持将实时流量记录复制为 HAR (JSON)。".into())
                    }
                }
            }
            Tool::Repeater => {
                let tab_id = self
                    .workspace
                    .active
                    .ok_or_else(|| "Repeater 当前没有可导出的 HTTP 标签。".to_owned())?;
                let tab = self
                    .workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .ok_or_else(|| "Repeater 当前活动标签不可用。".to_owned())?;
                let flow_id = tab
                    .last_flow
                    .ok_or_else(|| "当前 Repeater 标签尚无已完成的实时流量记录。".to_owned())?;
                let editors = self
                    .tab_editors
                    .get(&tab_id)
                    .ok_or_else(|| "当前 Repeater 标签编辑器尚未就绪。".to_owned())?;
                editors
                    .loaded_flow
                    .as_deref()
                    .filter(|record| record.summary.id == flow_id)
                    .ok_or_else(|| "正在读取 Repeater 流量记录详情，请稍后重试复制为 HAR。".into())
            }
            Tool::Logger => {
                let flow_id = self
                    .extra_tools
                    .logger_selected
                    .ok_or_else(|| "没有选中的 Logger 流量记录。".to_owned())?;
                self.extra_tools
                    .logger_loaded_detail
                    .as_ref()
                    .map(|(record, _)| record)
                    .filter(|record| record.summary.id == flow_id)
                    .ok_or_else(|| "正在读取所选 Logger 记录详情，请稍后重试复制为 HAR。".into())
            }
            _ => Err("请在 Proxy HTTP 历史记录、Repeater 或 Logger 中复制流量为 HAR (JSON)。".into()),
        }
    }

    pub(in crate::workbench) fn copy_selected_flow_har(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let exported = self
            .selected_flow_for_har()
            .and_then(|record| {
                mio_core::export_flows_to_har(std::slice::from_ref(record))
                    .map_err(|error| error.to_string())
            })
            .and_then(|bytes| {
                String::from_utf8(bytes).map_err(|error| format!("HAR JSON 编码异常：{error}"))
            });
        match exported {
            Ok(har_json) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(har_json));
                self.error = None;
                self.status = "已复制 HAR (JSON) 到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }
}
