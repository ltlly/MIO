use super::*;

pub(in crate::workbench) const ADVANCED_INCLUDE_FIELDS: [(&str, &str); 3] = [
    ("target-scope-advanced-host", "Host 正则"),
    ("target-scope-advanced-port", "Port 正则"),
    ("target-scope-advanced-file", "File 正则"),
];

pub(in crate::workbench) const ADVANCED_EXCLUDE_FIELDS: [(&str, &str); 3] = [
    ("target-scope-advanced-exclude-host", "Host 正则"),
    ("target-scope-advanced-exclude-port", "Port 正则"),
    ("target-scope-advanced-exclude-file", "File 正则"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum TargetPage {
    SiteMap,
    Scope,
    Issues,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum TargetRead {
    SiteMap { row_id: HistoryEntryId, entry_id: Uuid, revision: u64 },
    Live(FlowId),
    ExtRequest {
        id: HistoryEntryId,
        message_id: Uuid,
    },
    ExtResponse {
        id: HistoryEntryId,
        message_id: Uuid,
    },
}

#[derive(Default)]
pub(in crate::workbench) struct TargetTreeState {
    pub(in crate::workbench) nodes: Vec<SiteMapNode>,
    pub(in crate::workbench) expanded: HashSet<String>,
    pub(in crate::workbench) selected_key: Option<String>,
    pub(in crate::workbench) hidden_prefixes: Vec<String>,
}

impl TargetTreeState {
    pub(in crate::workbench) fn is_hidden(&self, node: &SiteMapNode) -> bool {
        is_url_hidden(&node.key, &self.hidden_prefixes)
            || is_url_hidden(&node.url_prefix, &self.hidden_prefixes)
    }

    #[allow(dead_code)]
    pub(in crate::workbench) fn ensure_origins(&mut self) {
        for node in &self.nodes {
            if node.kind == SiteMapNodeKind::Origin {
                self.expanded.insert(node.key.clone());
            }
        }
    }

    pub(in crate::workbench) fn toggle_node(&mut self, key: String) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
    }

    pub(in crate::workbench) fn visible_nodes(&self) -> Vec<&SiteMapNode> {
        let len = self.nodes.len();
        if len == 0 {
            return Vec::new();
        }
        let mut index_by_key: HashMap<&str, usize> = HashMap::with_capacity(len);
        for (idx, node) in self.nodes.iter().enumerate() {
            // Preserve first-match semantics of `self.nodes.iter().find(|n| n.key == current)`.
            index_by_key.entry(node.key.as_str()).or_insert(idx);
        }
        // 0 = unvisited, 1 = visiting (cycle guard), 2 = visible, 3 = hidden/collapsed
        let mut state = vec![0u8; len];
        let mut chain: Vec<usize> = Vec::with_capacity(16);
        let mut result = Vec::with_capacity(len);
        for idx in 0..len {
            if state[idx] == 0 {
                chain.clear();
                let mut cur_idx = idx;
                let resolved_visible = loop {
                    if state[cur_idx] == 2 {
                        break true;
                    }
                    if state[cur_idx] == 3 || state[cur_idx] == 1 {
                        break false;
                    }
                    state[cur_idx] = 1;
                    chain.push(cur_idx);
                    let cur = &self.nodes[cur_idx];
                    if cur.key.is_empty() || self.is_hidden(cur) {
                        break false;
                    }
                    let canonical_idx = *index_by_key.get(cur.key.as_str()).unwrap_or(&cur_idx);
                    if canonical_idx != cur_idx {
                        cur_idx = canonical_idx;
                        continue;
                    }
                    let Some(parent) = cur.parent_key.as_deref() else {
                        break true;
                    };
                    if !self.expanded.contains(parent) {
                        break false;
                    }
                    let Some(&parent_idx) = index_by_key.get(parent) else {
                        break false;
                    };
                    cur_idx = parent_idx;
                };
                let final_state = if resolved_visible { 2 } else { 3 };
                for &visited in &chain {
                    state[visited] = final_state;
                }
            }
            if state[idx] == 2 {
                result.push(&self.nodes[idx]);
            }
        }
        result
    }

    pub(in crate::workbench) fn has_children(&self, node: &SiteMapNode) -> bool {
        node.subtree_count > node.direct_count
    }

    pub(in crate::workbench) fn is_descendant_or_self(
        &self,
        node: &SiteMapNode,
        ancestor_key: &str,
    ) -> bool {
        let mut current = node.key.as_str();
        let mut hops = 0usize;
        while hops <= self.nodes.len() {
            if current == ancestor_key {
                return true;
            }
            let Some(found) = self.nodes.iter().find(|n| n.key == current) else {
                break;
            };
            let Some(parent) = found.parent_key.as_deref() else {
                break;
            };
            current = parent;
            hops += 1;
        }
        false
    }

    pub(in crate::workbench) fn expand_branch(&mut self, key: &str) -> usize {
        let keys: Vec<String> = self
            .nodes
            .iter()
            .filter(|node| self.is_descendant_or_self(node, key))
            .map(|node| node.key.clone())
            .collect();
        let mut count = 0usize;
        for node_key in keys {
            if self.expanded.insert(node_key) {
                count += 1;
            }
        }
        count
    }

    pub(in crate::workbench) fn collapse_branch(&mut self, key: &str) -> usize {
        let keys: Vec<String> = self
            .nodes
            .iter()
            .filter(|node| self.is_descendant_or_self(node, key))
            .map(|node| node.key.clone())
            .collect();
        let mut count = 0usize;
        for node_key in keys {
            if self.expanded.remove(&node_key) {
                count += 1;
            }
        }
        count
    }

    pub(in crate::workbench) fn expand_ancestors_within(
        &mut self,
        node_key: &str,
        stop_key: &str,
    ) -> usize {
        let mut chain = Vec::new();
        let mut current = node_key;
        let mut hops = 0usize;
        let mut reached_stop = false;
        while hops <= self.nodes.len() {
            let Some(found) = self.nodes.iter().find(|n| n.key == current) else {
                break;
            };
            if current == stop_key {
                chain.push(found.key.clone());
                reached_stop = true;
                break;
            }
            if let Some(parent) = found.parent_key.as_deref() {
                chain.push(parent.to_owned());
                current = parent;
            } else {
                break;
            }
            hops += 1;
        }
        if !reached_stop {
            return 0;
        }
        let mut count = 0usize;
        for ancestor in chain {
            if self.expanded.insert(ancestor) {
                count += 1;
            }
        }
        count
    }
}

pub(in crate::workbench) struct AdvancedScopeInputs {
    pub(in crate::workbench) include: [Entity<InputState>; 3],
    pub(in crate::workbench) exclude: [Entity<InputState>; 3],
}

pub(in crate::workbench) struct SimpleScopeInputs {
    pub(in crate::workbench) include: Entity<InputState>,
    pub(in crate::workbench) exclude: Entity<InputState>,
}

pub(in crate::workbench) struct TargetItemsCacheKey {
    pub(in crate::workbench) items_ptr: usize,
    pub(in crate::workbench) items_len: usize,
    pub(in crate::workbench) first_item: Option<(HistoryEntryId, Option<u16>, u64)>,
    pub(in crate::workbench) last_item: Option<(HistoryEntryId, Option<u16>, u64)>,
    pub(in crate::workbench) query_revision: u64,
    pub(in crate::workbench) history_query_revision: u64,
    pub(in crate::workbench) row_metadata_len: usize,
    pub(in crate::workbench) selected_id: Option<HistoryEntryId>,
    pub(in crate::workbench) selected_response_len: usize,
    pub(in crate::workbench) response_rev: crate::workbench::inspector::MessageEditorRevKey,
    pub(in crate::workbench) hidden_prefixes: Vec<String>,
    pub(in crate::workbench) hidden_item_ids_len: usize,
    pub(in crate::workbench) hidden_item_ids_sig: u64,
    pub(in crate::workbench) sort: Option<SiteMapSort>,
    pub(in crate::workbench) hide_not_found: bool,
    pub(in crate::workbench) status_classes: [bool; 4],
    pub(in crate::workbench) hide_css_image_binary: bool,
}

pub(in crate::workbench) struct CachedTargetItems {
    pub(in crate::workbench) key: TargetItemsCacheKey,
    pub(in crate::workbench) rows: std::rc::Rc<Vec<(usize, super::mime::TargetRowSummary)>>,
}

pub(in crate::workbench) struct TargetPane {
    pub(in crate::workbench) page: TargetPage,
    pub(in crate::workbench) site_map_subview: SiteMapSubView,
    pub(in crate::workbench) item_sort: Option<SiteMapSort>,
    pub(in crate::workbench) tree_state: TargetTreeState,
    pub(in crate::workbench) items: Vec<HistoryRow>,
    pub(in crate::workbench) items_cache: std::cell::RefCell<Option<CachedTargetItems>>,
    pub(in crate::workbench) items_offset: usize,
    pub(in crate::workbench) total_items: usize,
    pub(in crate::workbench) node_count: usize,
    pub(in crate::workbench) truncated_nodes: bool,
    pub(in crate::workbench) site_map_loading: bool,
    pub(in crate::workbench) site_map_error: Option<String>,
    pub(in crate::workbench) in_scope_only: bool,
    pub(in crate::workbench) hide_not_found: bool,
    pub(in crate::workbench) hide_css_image_binary: bool,
    pub(in crate::workbench) hide_empty_folders: bool,
    pub(in crate::workbench) status_classes: [bool; 4],
    pub(in crate::workbench) site_map_inspector_initialized: bool,
    pub(in crate::workbench) search: Entity<InputState>,
    pub(in crate::workbench) query_revision: u64,
    pub(in crate::workbench) query_cancel: Option<CancellationToken>,
    pub(in crate::workbench) pending_reads: HashMap<u64, TargetRead>,
    pub(in crate::workbench) explicit_entries: HashMap<HistoryEntryId, (Uuid, u64)>,
    pub(in crate::workbench) pending_adds: HashMap<Uuid, (u64, CancellationToken)>,
    pub(in crate::workbench) selected_explicit_detail: Option<SiteMapEntryDetail>,
    pub(in crate::workbench) selected_id: Option<HistoryEntryId>,
    pub(in crate::workbench) selected_ids: HashSet<HistoryEntryId>,
    pub(in crate::workbench) selection_anchor: Option<HistoryEntryId>,
    pub(in crate::workbench) selected_row: Option<HistoryRow>,
    pub(in crate::workbench) item_col_widths: [f32; SITEMAP_COL_COUNT],
    pub(in crate::workbench) item_col_visible: [bool; SITEMAP_COL_COUNT],
    pub(in crate::workbench) selected_response_bytes: Vec<u8>,
    pub(in crate::workbench) selected_live_record: Option<Box<FlowRecord>>,
    pub(in crate::workbench) selected_site_map_finding_id: Option<Uuid>,
    pub(in crate::workbench) hidden_item_ids: HashSet<HistoryEntryId>,
    pub(in crate::workbench) finding_originals:
        HashMap<Uuid, (MioIssueSeverity, mio_core::audit_engine::IssueConfidence)>,
    pub(in crate::workbench) verified_findings: HashSet<Uuid>,
    pub(in crate::workbench) engagement_report: Option<TargetEngagementReport>,
    pub(in crate::workbench) compare_modal: Option<TargetCompareState>,
    pub(in crate::workbench) detached_site_map_window: Option<gpui_kit::AnyWindowHandle>,
    pub(in crate::workbench) pending_branch_scan: Option<HistoryEntryId>,
    pub(in crate::workbench) pending_branch_scan_prefix: Option<String>,
    pub(in crate::workbench) pending_branch_action: BranchItemAction,
    pub(in crate::workbench) request: MessageEditor,
    pub(in crate::workbench) response: MessageEditor,
    pub(in crate::workbench) scope_protocol: ScopeProtocol,
    pub(in crate::workbench) advanced_scope_inputs: AdvancedScopeInputs,
    pub(in crate::workbench) simple_scope_inputs: SimpleScopeInputs,
    pub(in crate::workbench) simple_scope_include_subdomains: [bool; 2],
    pub(in crate::workbench) editing_simple_scope: [Option<Uuid>; 2],
    pub(in crate::workbench) editing_advanced_scope: [Option<Uuid>; 2],
    pub(in crate::workbench) selected_simple_scope: [Option<Uuid>; 2],
    pub(in crate::workbench) selected_advanced_scope: [Option<Uuid>; 2],
    pub(in crate::workbench) scope_editor_open: [bool; 2],
    pub(in crate::workbench) settings_editing_scope: [Option<Uuid>; 2],
    pub(in crate::workbench) issue_search: Entity<InputState>,
    pub(in crate::workbench) issue_search_term: String,
    pub(in crate::workbench) issue_severity: Option<MioIssueSeverity>,
    pub(in crate::workbench) issue_selected: Option<&'static MioIssueDefinition>,
    pub(in crate::workbench) issue_sort: (IssueSortColumn, bool),
    pub(in crate::workbench) issue_filter_open: bool,
    pub(in crate::workbench) advanced_mode_input: bool,
    pub(in crate::workbench) include_protocol: ScopeProtocol,
    pub(in crate::workbench) exclude_protocol: ScopeProtocol,
}

impl TargetPane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let advanced_inputs = ADVANCED_INCLUDE_FIELDS.map(|(_, placeholder)| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        });
        let advanced_exclude_inputs = ADVANCED_EXCLUDE_FIELDS.map(|(_, placeholder)| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        });
        let simple_include = cx.new(|cx| InputState::new(window, cx).placeholder("包含 URL 前缀"));
        let simple_exclude = cx.new(|cx| InputState::new(window, cx).placeholder("排除 URL 前缀"));
        let issue_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索漏洞定义"));
        let (include_protocol, exclude_protocol) = (ScopeProtocol::Any, ScopeProtocol::Any);
        Self {
            page: TargetPage::SiteMap,
            site_map_subview: SiteMapSubView::Contents,
            item_sort: None,
            tree_state: TargetTreeState::default(),
            items: Vec::new(),
            items_cache: std::cell::RefCell::new(None),
            items_offset: 0,
            total_items: 0,
            node_count: 0,
            truncated_nodes: false,
            site_map_loading: false,
            site_map_error: None,
            in_scope_only: false,
            hide_not_found: true,
            hide_css_image_binary: true,
            hide_empty_folders: true,
            status_classes: [true, true, false, true],
            site_map_inspector_initialized: false,
            search,
            query_revision: 0,
            query_cancel: None,
            pending_reads: HashMap::new(),
            explicit_entries: HashMap::new(),
            pending_adds: HashMap::new(),
            selected_explicit_detail: None,
            selected_id: None,
            selected_ids: HashSet::new(),
            selection_anchor: None,
            selected_row: None,
            item_col_widths: SITEMAP_DEFAULT_COL_WIDTHS,
            item_col_visible: [true; SITEMAP_COL_COUNT],
            selected_response_bytes: Vec::new(),
            selected_live_record: None,
            selected_site_map_finding_id: None,
            hidden_item_ids: HashSet::new(),
            finding_originals: HashMap::new(),
            verified_findings: HashSet::new(),
            engagement_report: None,
            compare_modal: None,
            detached_site_map_window: None,
            pending_branch_scan: None,
            pending_branch_scan_prefix: None,
            pending_branch_action: BranchItemAction::ActiveScan,
            request: MessageEditor::new(window, cx),
            response: MessageEditor::new(window, cx),
            scope_protocol: ScopeProtocol::Any,
            advanced_scope_inputs: AdvancedScopeInputs {
                include: advanced_inputs,
                exclude: advanced_exclude_inputs,
            },
            simple_scope_inputs: SimpleScopeInputs {
                include: simple_include,
                exclude: simple_exclude,
            },
            simple_scope_include_subdomains: [false, false],
            editing_simple_scope: [None, None],
            editing_advanced_scope: [None, None],
            selected_simple_scope: [None, None],
            selected_advanced_scope: [None, None],
            scope_editor_open: [false, false],
            settings_editing_scope: [None, None],
            issue_search,
            issue_search_term: String::new(),
            issue_severity: None,
            issue_selected: default_first_issue_definition(),
            issue_sort: (IssueSortColumn::TypeIndex, true),
            issue_filter_open: false,
            advanced_mode_input: false,
            include_protocol,
            exclude_protocol,
        }
    }

    pub(in crate::workbench) fn simple_inputs(&self) -> [&Entity<InputState>; 2] {
        [
            &self.simple_scope_inputs.include,
            &self.simple_scope_inputs.exclude,
        ]
    }
}

pub(super) struct TargetViewContext<'a> {
    pub(super) view: WeakEntity<TargetView>,
    pub(super) view_entity_id: gpui_kit::EntityId,
    pub(super) site_map_view_mode: SiteMapViewMode,
    pub(super) site_map_layout: SiteMapLayoutMode,
    pub(super) tree_scroll: &'a ScrollHandle,
    pub(super) items_v_scroll: &'a UniformListScrollHandle,
    pub(super) items_h_scroll: &'a ScrollHandle,
    pub(super) issues_list_scroll: &'a ScrollHandle,
    pub(super) issues_detail_scroll: &'a ScrollHandle,
}

pub(in crate::workbench) struct TargetView {
    workbench: WeakEntity<Workbench>,
    pub(super) site_map_view_mode: SiteMapViewMode,
    pub(super) site_map_layout: SiteMapLayoutMode,
    tree_scroll: ScrollHandle,
    items_v_scroll: UniformListScrollHandle,
    items_h_scroll: ScrollHandle,
    issues_list_scroll: ScrollHandle,
    issues_detail_scroll: ScrollHandle,
    item_col_resize_origin: Option<(usize, f32, f32)>,
    _subscriptions: Vec<Subscription>,
}

impl TargetView {
    pub(in crate::workbench) fn new(
        workbench: WeakEntity<Workbench>,
        pane: &TargetPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let wb_search = workbench.clone();
        let search_subscription =
            cx.subscribe_in(&pane.search, window, move |_, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let _ = wb_search.update(cx, |this, cx| {
                        this.refresh_target_site_map(0, window, cx);
                    });
                }
            });
        let wb_issue = workbench.clone();
        let issue_subscription =
            cx.subscribe_in(&pane.issue_search, window, move |_, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let _ = wb_issue.update(cx, |this, cx| {
                        this.apply_issue_search(window, cx);
                    });
                }
            });
        let wb_inc = workbench.clone();
        let simple_include_subscription = cx.subscribe_in(
            &pane.simple_scope_inputs.include,
            window,
            move |_, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let _ = wb_inc.update(cx, |this, cx| {
                        this.submit_target_simple_scope(true, window, cx);
                    });
                }
            },
        );
        let wb_exc = workbench.clone();
        let simple_exclude_subscription = cx.subscribe_in(
            &pane.simple_scope_inputs.exclude,
            window,
            move |_, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let _ = wb_exc.update(cx, |this, cx| {
                        this.submit_target_simple_scope(false, window, cx);
                    });
                }
            },
        );

        Self {
            workbench,
            site_map_view_mode: SiteMapViewMode::UrlView,
            site_map_layout: SiteMapLayoutMode::TreeLeftStackedRight,
            tree_scroll: ScrollHandle::new(),
            items_v_scroll: UniformListScrollHandle::new(),
            items_h_scroll: ScrollHandle::new(),
            issues_list_scroll: ScrollHandle::new(),
            issues_detail_scroll: ScrollHandle::new(),
            item_col_resize_origin: None,
            _subscriptions: vec![
                search_subscription,
                issue_subscription,
                simple_include_subscription,
                simple_exclude_subscription,
            ],
        }
    }

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.site_map_view_mode = SiteMapViewMode::UrlView;
        self.site_map_layout = SiteMapLayoutMode::TreeLeftStackedRight;
        self.item_col_resize_origin = None;
        cx.notify();
    }

    pub(super) fn clear_column_resize(&mut self, cx: &mut Context<Self>) {
        self.item_col_resize_origin = None;
        cx.notify();
    }

    pub(super) fn set_view_mode(&mut self, mode: SiteMapViewMode, cx: &mut Context<Self>) {
        if self.site_map_view_mode != mode {
            self.site_map_view_mode = mode;
            cx.notify();
        }
    }

    pub(super) fn set_layout_mode(&mut self, mode: SiteMapLayoutMode, cx: &mut Context<Self>) {
        if self.site_map_layout != mode {
            self.site_map_layout = mode;
            cx.notify();
        }
    }

    pub(super) fn begin_column_resize(&mut self, col_ix: usize, start_x: f32, start_w: f32) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        self.item_col_resize_origin = Some((col_ix, start_x, start_w));
    }

    pub(super) fn update_column_resize(
        &mut self,
        col_ix: usize,
        current_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let Some((drag_col, start_x, start_w)) = self.item_col_resize_origin else {
            return;
        };
        if drag_col != col_ix {
            return;
        }
        let scale = theme::ui_scale().max(0.75);
        let delta_unscaled = (current_x - start_x) / scale;
        let next_w = (start_w + delta_unscaled).round().clamp(44.0, 560.0);
        let Some(wb_entity) = self.workbench.upgrade() else {
            return;
        };
        let changed = wb_entity.update(cx, |wb, _| {
            if (wb.target_pane.item_col_widths[col_ix] - next_w).abs() >= 0.5 {
                wb.target_pane.item_col_widths[col_ix] = next_w;
                true
            } else {
                false
            }
        });
        if changed {
            cx.notify();
        }
    }
}

impl gpui_kit::Render for TargetView {
    #[inline(never)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(wb_entity) = self.workbench.upgrade() else {
            return div().size_full().into_any_element();
        };
        // Register dependency on Workbench so gpui-fast splices re-render
        // this view whenever Workbench state changes.
        let _ = wb_entity.read(cx);
        let view_ctx = TargetViewContext {
            view: cx.weak_entity(),
            view_entity_id: cx.entity_id(),
            site_map_view_mode: self.site_map_view_mode,
            site_map_layout: self.site_map_layout,
            tree_scroll: &self.tree_scroll,
            items_v_scroll: &self.items_v_scroll,
            items_h_scroll: &self.items_h_scroll,
            issues_list_scroll: &self.issues_list_scroll,
            issues_detail_scroll: &self.issues_detail_scroll,
        };
        wb_entity.update(cx, |wb, cx| wb.render_target(&view_ctx, window, cx))
    }
}
