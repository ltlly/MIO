use std::collections::BTreeMap;

use super::engagement::*;
use super::*;

impl Workbench {

    fn origin_path_map(&self, origin_key: &str, cx: &App) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        let row_metadata = self.history.read(cx).delegate().row_metadata.clone();
        for row in self.collect_branch_rows(origin_key, cx) {
            let url = row_url(&row);
            if url.is_empty() {
                continue;
            }
            let summary =
                target_row_summary_with_metadata(&row, None, row_metadata.get(&row.id()));
            let path = if summary.url.is_empty() {
                http_history_path_and_query(&url)
            } else {
                summary.url
            };
            let method = if summary.method.is_empty() {
                "GET".into()
            } else {
                summary.method
            };
            let status = if summary.status.is_empty() {
                "-".into()
            } else {
                summary.status
            };
            let len = if summary.length.is_empty() {
                "0".into()
            } else {
                summary.length
            };
            map.insert(path, format!("{method} · status={status} · len={len}"));
        }
        if map.is_empty() {
            for node in &self.target_pane.tree_state.nodes {
                if !self.target_pane.tree_state.is_hidden(node)
                    && self
                        .target_pane
                        .tree_state
                        .is_descendant_or_self(node, origin_key)
                {
                    let path = http_history_path_and_query(&node.url_prefix);
                    map.entry(path)
                        .or_insert_with(|| format!("node · count={}", node.direct_count));
                }
            }
        }
        map
    }

    pub(in crate::workbench) fn open_compare_site_maps(
        &mut self,
        preferred_origin: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origins: Vec<String> = self
            .target_pane
            .tree_state
            .nodes
            .iter()
            .filter(|n| {
                n.kind == SiteMapNodeKind::Origin && !self.target_pane.tree_state.is_hidden(n)
            })
            .map(|n| n.key.clone())
            .collect();
        let left_origin = preferred_origin
            .and_then(|p| {
                let canon = canonical_site_map_url_key(&p).unwrap_or(p.clone());
                origins
                    .iter()
                    .find(|o| **o == canon || url_matches_node_prefix(&p, o))
                    .cloned()
                    .or(Some(p))
            })
            .or_else(|| origins.first().cloned())
            .unwrap_or_default();
        let right_origin = origins.iter().find(|o| **o != left_origin).cloned();
        let left_map = if left_origin.is_empty() {
            BTreeMap::new()
        } else {
            self.origin_path_map(&left_origin, cx)
        };
        let right_map = right_origin
            .as_deref()
            .map(|r| self.origin_path_map(r, cx))
            .unwrap_or_default();
        let (rows, only_left, only_right, different, same) =
            compute_site_map_diff(&left_map, &right_map);
        self.target_pane.compare_modal = Some(TargetCompareState {
            available_origins: origins,
            left_origin,
            right_origin,
            external_label: None,
            external_map: BTreeMap::new(),
            rows,
            only_left,
            only_right,
            different,
            same,
        });
        self.error = None;
        self.status = "已打开站点地图对比视图。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn select_compare_site_map_right_origin(
        &mut self,
        origin: String,
        cx: &mut Context<Self>,
    ) {
        let Some(left_origin) = self
            .target_pane
            .compare_modal
            .as_ref()
            .map(|s| s.left_origin.clone())
        else {
            return;
        };
        let left_map = self.origin_path_map(&left_origin, cx);
        let right_map = self.origin_path_map(&origin, cx);
        let (rows, only_left, only_right, different, same) =
            compute_site_map_diff(&left_map, &right_map);
        if let Some(state) = self.target_pane.compare_modal.as_mut() {
            state.right_origin = Some(origin);
            state.external_label = None;
            state.external_map.clear();
            state.rows = rows;
            state.only_left = only_left;
            state.only_right = only_right;
            state.different = different;
            state.same = same;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn load_compare_site_map_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("加载待对比的站点地图文件 (XML / JSON / URL 列表)".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match paths {
                    Ok(Ok(Some(paths))) => {
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        match mio_core::ui_files::read_file_bytes(&path) {
                            Ok(bytes) if bytes.len() > MAX_COMPARE_FILE_BYTES => {
                                view.fail("站点地图对比文件超过 4 MB 上限。", cx);
                            }
                            Ok(bytes) => match String::from_utf8(bytes) {
                                Ok(text) => match parse_external_site_map_file(&text) {
                                    Ok(ext_map) => {
                                        let left_origin = view
                                            .target_pane
                                            .compare_modal
                                            .as_ref()
                                            .map(|s| s.left_origin.clone())
                                            .unwrap_or_default();
                                        let left_map = view.origin_path_map(&left_origin, cx);
                                        let (rows, only_left, only_right, different, same) =
                                            compute_site_map_diff(&left_map, &ext_map);
                                        let name = path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("外部站点地图")
                                            .to_owned();
                                        if let Some(state) =
                                            view.target_pane.compare_modal.as_mut()
                                        {
                                            state.right_origin = None;
                                            state.external_label = Some(name.clone());
                                            state.external_map = ext_map;
                                            state.rows = rows;
                                            state.only_left = only_left;
                                            state.only_right = only_right;
                                            state.different = different;
                                            state.same = same;
                                        }
                                        view.error = None;
                                        view.status = format!("已加载对比快照：{name}");
                                        cx.notify();
                                    }
                                    Err(err) => view.fail(err, cx),
                                },
                                Err(_) => view.fail("所选站点地图文件不是有效的 UTF-8 文本。", cx),
                            },
                            Err(err) => view.fail(format!("读取对比文件失败：{err}"), cx),
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => view.fail(format!("选择对比文件失败：{err}"), cx),
                    Err(_) => view.fail("文件选择器已关闭。", cx),
                }
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn send_compare_site_maps_to_comparer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.target_pane.compare_modal.clone() else {
            return;
        };
        let left_map = self.origin_path_map(&state.left_origin, cx);
        let (right_label, right_map) = if let Some(ext) = state.external_label.clone() {
            (ext, state.external_map)
        } else if let Some(ref right_origin) = state.right_origin {
            (right_origin.clone(), self.origin_path_map(right_origin, cx))
        } else {
            ("空站点地图".to_owned(), BTreeMap::new())
        };
        let format_manifest = |m: &BTreeMap<String, String>| -> Vec<u8> {
            m.iter()
                .map(|(path, meta)| format!("{path}  [{meta}]"))
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes()
        };
        let left_label = if state.left_origin.is_empty() {
            "Site map A".to_owned()
        } else {
            format!("Site map: {}", state.left_origin)
        };
        let right_title = format!("Site map: {right_label}");
        let ok_l = self.push_text_to_comparer(&left_label, format_manifest(&left_map), window, cx);
        let ok_r = if ok_l {
            self.push_text_to_comparer(&right_title, format_manifest(&right_map), window, cx)
        } else {
            false
        };
        if ok_l && ok_r {
            self.target_pane.compare_modal = None;
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.focus.focus(window, cx);
            self.error = None;
            self.status = "已将两个站点地图清单发送到 Comparer 进行逐行对比。".into();
            cx.notify();
        }
    }

    // --- Detached Site Map window ---

    pub(in crate::workbench) fn open_detached_site_map_window(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let owner = cx.entity();
        let bounds = gpui_kit::Bounds::centered(None, gpui_kit::size(px(1100.), px(720.)), cx);
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            if let Some(existing) = owner.read(cx).target_pane.detached_site_map_window
                && cx.windows().contains(&existing)
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        super::engagement_overlays::DetachedSiteMapWindow::new(
                            owner.clone(),
                            window,
                            cx,
                        )
                    })
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((handle, _)) => {
                    view.target_pane.detached_site_map_window = Some(handle);
                    view.error = None;
                    view.status = "已在新窗口中打开站点地图。".into();
                    cx.notify();
                }
                Err(error) => view.fail(format!("无法打开新站点地图窗口：{error}"), cx),
            });
        });
    }

}
