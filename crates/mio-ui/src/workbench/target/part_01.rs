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
    pub(in crate::workbench) site_map_view_mode: SiteMapViewMode,
    pub(in crate::workbench) site_map_layout: SiteMapLayoutMode,
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
    pub(in crate::workbench) tree_scroll: ScrollHandle,
    pub(in crate::workbench) items_v_scroll: UniformListScrollHandle,
    pub(in crate::workbench) items_h_scroll: ScrollHandle,
    pub(in crate::workbench) issues_list_scroll: ScrollHandle,
    pub(in crate::workbench) issues_detail_scroll: ScrollHandle,
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
    pub(in crate::workbench) item_col_resize_origin: Option<(usize, f32, f32)>,
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
    pub(in crate::workbench) _subscriptions: Vec<Subscription>,
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
        let search_subscription = cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.refresh_target_site_map(0, window, cx);
            }
        });
        let issue_subscription =
            cx.subscribe_in(&issue_search, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.apply_issue_search(window, cx);
                }
            });
        let simple_include_subscription =
            cx.subscribe_in(&simple_include, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_target_simple_scope(true, window, cx);
                }
            });
        let simple_exclude_subscription =
            cx.subscribe_in(&simple_exclude, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_target_simple_scope(false, window, cx);
                }
            });
        let (include_protocol, exclude_protocol) = (ScopeProtocol::Any, ScopeProtocol::Any);
        Self {
            page: TargetPage::SiteMap,
            site_map_subview: SiteMapSubView::Contents,
            site_map_view_mode: SiteMapViewMode::UrlView,
            site_map_layout: SiteMapLayoutMode::TreeLeftStackedRight,
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
            tree_scroll: ScrollHandle::new(),
            items_v_scroll: UniformListScrollHandle::new(),
            items_h_scroll: ScrollHandle::new(),
            issues_list_scroll: ScrollHandle::new(),
            issues_detail_scroll: ScrollHandle::new(),
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
            item_col_resize_origin: None,
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
            _subscriptions: vec![
                search_subscription,
                issue_subscription,
                simple_include_subscription,
                simple_exclude_subscription,
            ],
        }
    }

    pub(in crate::workbench) fn simple_inputs(&self) -> [&Entity<InputState>; 2] {
        [
            &self.simple_scope_inputs.include,
            &self.simple_scope_inputs.exclude,
        ]
    }
}

impl Workbench {
    pub(in crate::workbench) fn active_target_message_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.target_pane.response.has_focus(window, cx) {
            if let Err(error) = self.target_pane.response.focus_search(window, cx) {
                self.fail(error, cx);
            }
        } else if let Err(error) = self.target_pane.request.focus_search(window, cx) {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn reset_target_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query_cancel = self.target_pane.query_cancel.take();
        if let Some(cancel) = query_cancel {
            cancel.cancel();
        }
        self.target_pane.pending_reads.clear();
        for (_, (_, cancel)) in self.target_pane.pending_adds.drain() { cancel.cancel(); }
        self.target_pane.explicit_entries.clear();
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.page = TargetPage::SiteMap;
        self.target_pane.site_map_subview = SiteMapSubView::Contents;
        self.target_pane.site_map_view_mode = SiteMapViewMode::UrlView;
        self.target_pane.site_map_layout = SiteMapLayoutMode::TreeLeftStackedRight;
        self.target_pane.item_sort = None;
        self.target_pane.tree_state = TargetTreeState::default();
        self.target_pane.items.clear();
        *self.target_pane.items_cache.borrow_mut() = None;
        self.target_pane.items_offset = 0;
        self.target_pane.total_items = 0;
        self.target_pane.node_count = 0;
        self.target_pane.truncated_nodes = false;
        self.target_pane.site_map_loading = false;
        self.target_pane.site_map_error = None;
        self.target_pane.in_scope_only = false;
        self.target_pane.hide_not_found = true;
        self.target_pane.hide_css_image_binary = true;
        self.target_pane.hide_empty_folders = true;
        self.target_pane.status_classes = [true, true, false, true];
        self.target_pane.site_map_inspector_initialized = false;
        self.target_pane.search.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.target_pane.query_revision += 1;
        self.target_pane.selected_id = None;
        self.target_pane.selected_ids.clear();
        self.target_pane.selection_anchor = None;
        self.target_pane.selected_row = None;
        self.target_pane.item_col_resize_origin = None;
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_site_map_finding_id = None;
        self.target_pane.hidden_item_ids.clear();
        self.target_pane.finding_originals.clear();
        self.target_pane.verified_findings.clear();
        self.target_pane.engagement_report = None;
        self.target_pane.compare_modal = None;
        self.target_pane.detached_site_map_window = None;
        self.target_pane.pending_branch_scan = None;
        self.target_pane.pending_branch_scan_prefix = None;
        self.target_pane.pending_branch_action = BranchItemAction::ActiveScan;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        self.target_pane.scope_protocol = ScopeProtocol::Any;
        self.target_pane.simple_scope_include_subdomains = [false, false];
        self.target_pane.editing_simple_scope = [None, None];
        self.target_pane.editing_advanced_scope = [None, None];
        self.target_pane.selected_simple_scope = [None, None];
        self.target_pane.selected_advanced_scope = [None, None];
        self.target_pane.scope_editor_open = [false, false];
        self.target_pane.settings_editing_scope = [None, None];
        for input in self
            .target_pane
            .advanced_scope_inputs
            .include
            .iter()
            .chain(&self.target_pane.advanced_scope_inputs.exclude)
        {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.target_pane.issue_search.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.target_pane.issue_search_term.clear();
        self.target_pane.issue_severity = None;
        self.target_pane.issue_selected = default_first_issue_definition();
        self.target_pane.issue_sort = (IssueSortColumn::TypeIndex, true);
        self.target_pane.issue_filter_open = false;
        self.target_pane.advanced_mode_input = self.workspace.scope.advanced_mode;
        self.target_pane.include_protocol = ScopeProtocol::Any;
        self.target_pane.exclude_protocol = ScopeProtocol::Any;
        for input in self.target_pane.simple_inputs().into_iter() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        if self.tool == Tool::Target {
            self.refresh_target_site_map(0, window, cx);
        }
    }

    pub(in crate::workbench) fn target_backend_disconnected(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.target_pane.query_cancel.take() {
            cancel.cancel();
        }
        self.target_pane.pending_reads.clear();
        for (_, (_, cancel)) in self.target_pane.pending_adds.drain() { cancel.cancel(); }
        self.target_pane.pending_branch_scan = None;
        self.target_pane.pending_branch_scan_prefix = None;
        self.target_pane.site_map_loading = false;
        cx.notify();
    }

    pub(in crate::workbench) fn on_site_map_page(
        &mut self,
        project: ProjectKey,
        query_revision: u64,
        query: SiteMapQuery,
        result: Result<SiteMapSnapshot, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project) || self.target_pane.query_revision != query_revision {
            return;
        }
        self.target_pane.site_map_loading = false;
        self.target_pane.in_scope_only = query.filter.in_scope_only;
        self.target_pane.hide_not_found = query.filter.hide_not_found;
        self.target_pane.status_classes = query.filter.status_classes;
        match result {
            Ok(snapshot) => {
                let previous_explicit = self.target_pane.selected_id.and_then(|id| self.target_pane.explicit_entries.get(&id).copied());
                let appending = snapshot.offset > 0 && snapshot.offset == self.target_pane.items.len();
                if appending {
                    for (id, entry_id, revision) in snapshot.explicit_entries {
                        self.target_pane.explicit_entries.insert(id, (entry_id, revision));
                    }
                } else {
                    self.target_pane.explicit_entries = snapshot
                        .explicit_entries
                        .into_iter()
                        .map(|(id, entry_id, revision)| (id, (entry_id, revision)))
                        .collect();
                }
                self.target_pane.tree_state.nodes = snapshot.nodes.clone();
                if let Some(selected) = self.target_pane.tree_state.selected_key.as_ref() {
                    for node in &self.target_pane.tree_state.nodes {
                        if url_matches_node_prefix(selected, &node.key) {
                            self.target_pane.tree_state.expanded.insert(node.key.clone());
                        }
                    }
                }
                self.target_pane.node_count = snapshot.nodes.len();
                self.target_pane.truncated_nodes = snapshot.truncated_nodes;
                self.target_pane.total_items = snapshot.total_items;
                if appending {
                    let mut existing_ids: HashSet<HistoryEntryId> =
                        self.target_pane.items.iter().map(HistoryRow::id).collect();
                    for row in snapshot.items {
                        if !self.target_pane.hidden_item_ids.contains(&row.id())
                            && existing_ids.insert(row.id())
                        {
                            self.target_pane.items.push(row);
                        }
                    }
                    self.target_pane.items_offset = 0;
                } else {
                    self.target_pane.items_offset = snapshot.offset;
                    self.target_pane.items = snapshot
                        .items
                        .into_iter()
                        .filter(|row| !self.target_pane.hidden_item_ids.contains(&row.id()))
                        .collect();
                }
                let selected = self
                    .target_pane
                    .items
                    .iter()
                    .find(|row| Some(row.id()) == self.target_pane.selected_id)
                    .cloned();
                if let Some(row) = selected {
                    self.target_pane.selected_row = Some(row);
                    if self.target_pane.selected_id.and_then(|id| self.target_pane.explicit_entries.get(&id).copied()) != previous_explicit {
                        self.target_pane.selected_explicit_detail = None;
                        self.target_pane.request.load(&[], window, cx);
                        self.target_pane.response.load(&[], window, cx);
                    }
                } else if !appending {
                    self.target_pane.selected_id = None;
                    self.target_pane.selected_row = None;
                    self.target_pane.selected_response_bytes.clear();
                    self.target_pane.selected_live_record = None;
                    self.target_pane.selected_explicit_detail = None;
                    self.target_pane.request.load(&[], window, cx);
                    self.target_pane.response.load(&[], window, cx);
                }
                self.target_pane.site_map_error = None;
                if self.target_pane.selected_id.is_none() && !self.target_pane.items.is_empty() {
                    let first_visible = {
                        let row_metadata = &self.history.read(cx).delegate().row_metadata;
                        let sorted_indices = sort_site_map_indices_with_metadata(
                            &self.target_pane.items,
                            None,
                            None,
                            Some(row_metadata),
                            &self.target_pane.tree_state.hidden_prefixes,
                            self.target_pane.item_sort,
                        );
                        sorted_indices
                            .iter()
                            .filter_map(|&idx| self.target_pane.items.get(idx))
                            .find(|row| {
                                site_map_row_passes_ui_filters(
                                    row,
                                    None,
                                    row_metadata.get(&row.id()),
                                    self.target_pane.hide_not_found,
                                    self.target_pane.status_classes,
                                    self.target_pane.hide_css_image_binary,
                                )
                            })
                            .cloned()
                    };
                    if let Some(row) = first_visible {
                        self.select_target_row(&row, window, cx);
                    }
                } else {
                    self.load_selected_target_detail(false, window, cx);
                }
                if !self.target_pane.site_map_inspector_initialized
                    && self.target_pane.selected_row.is_some()
                {
                    self.target_pane.site_map_inspector_initialized = true;
                    if !self.inspector.open
                        && !self.http_notes.open
                        && self.inspector.dismissed_selection.is_none()
                    {
                        self.open_inspector(window, cx);
                    }
                }
                if let Some(prefix) = self.target_pane.pending_branch_scan_prefix.take() {
                    let action = self.target_pane.pending_branch_action;
                    self.finish_branch_action_from_items(&prefix, action, window, cx);
                }
            }
            Err(error) => {
                self.target_pane.pending_branch_scan_prefix = None;
                self.target_pane.site_map_error = Some(error);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_target_flow_detail(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        record: &FlowRecord,
        original: &OriginalMessages,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(project)
            || self.target_pane.pending_reads.get(&selection_revision)
                != Some(&TargetRead::Live(record.summary.id))
        {
            return false;
        }
        self.target_pane.pending_reads.remove(&selection_revision);
        if self.target_pane.selected_id != Some(HistoryEntryId::Live(record.summary.id)) { return true; }
        self.target_pane.selected_id = Some(HistoryEntryId::Live(record.summary.id));
        self.target_pane.selected_row = Some(HistoryRow::Live(record.summary.clone()));
        self.target_pane.selected_live_record = Some(Box::new(record.clone()));
        self.target_pane
            .request
            .load(&request_to_raw(&record.request), window, cx);
        let raw_response = record
            .response
            .as_ref()
            .map(response_to_raw)
            .unwrap_or_default();
        self.target_pane.selected_response_bytes = raw_response.clone();
        self.target_pane.response.load(&raw_response, window, cx);
        if self.inspector.open {
            self.sync_inspector_selection(window, cx);
        }
        if self.target_pane.pending_branch_scan == Some(HistoryEntryId::Live(record.summary.id)) {
            self.target_pane.pending_branch_scan = None;
            let draft = Draft {
                revision: 1,
                target: record.request.url.clone(),
                raw: request_to_raw(&record.request),
                version: record.request.version,
            };
            let action = self.target_pane.pending_branch_action;
            self.execute_branch_action_with_draft(draft, action, window, cx);
        }
        let _ = original;
        cx.notify();
        true
    }
}
