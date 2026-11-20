use std::collections::BTreeMap;

use anyhow::Result;

use super::{
    MAX_SITE_MAP_DEPTH, MAX_SITE_MAP_NODES, MAX_TREE_BUILD_NODES, SiteMapFilter, SiteMapNode,
    SiteMapNodeKind,
};

pub(super) struct ParsedSiteMapUrl {
    origin_key: String,
    origin_label: String,
    display_url: String,
    pub(super) full_key: String,
    segments: Vec<String>,
    directory: bool,
}

pub(super) fn parse_site_map_url(raw: &str) -> Option<ParsedSiteMapUrl> {
    let parsed = url::Url::parse(raw).ok()?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let port = parsed.port_or_known_default()?;
    let origin_key = format!("{scheme}://{host}:{port}");
    let default_port = (scheme == "http" && port == 80) || (scheme == "https" && port == 443);
    let origin_label = if default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    };
    let segments: Vec<String> = parsed
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let path = if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    };
    let mut display_url = format!("{origin_label}{path}");
    if let Some(query) = parsed.query() {
        display_url.push('?');
        display_url.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        display_url.push('#');
        display_url.push_str(fragment);
    }
    let full_key = format!("{origin_key}{path}");
    let directory = parsed.path().ends_with('/') || segments.len() > MAX_SITE_MAP_DEPTH as usize;
    Some(ParsedSiteMapUrl {
        origin_key,
        origin_label,
        display_url,
        full_key,
        segments,
        directory,
    })
}

fn key_in_subtree(entry_key: &str, selected_key: &str) -> bool {
    entry_key == selected_key
        || (entry_key.len() > selected_key.len()
            && entry_key.starts_with(selected_key)
            && entry_key.as_bytes()[selected_key.len()] == b'/')
}

pub(super) struct EntryFilters<'a> {
    scope: &'a crate::TargetScope,
    needle: String,
    in_scope_only: bool,
    hide_not_found: bool,
    status_classes: [bool; 4],
}

impl<'a> EntryFilters<'a> {
    pub(super) fn new(scope: &'a crate::TargetScope, filter: &'a SiteMapFilter) -> Self {
        Self {
            scope,
            needle: filter.search_term.to_lowercase(),
            in_scope_only: filter.in_scope_only,
            hide_not_found: filter.hide_not_found,
            status_classes: filter.status_classes,
        }
    }

    fn status_matches(&self, status: Option<u16>) -> bool {
        if self.status_classes == [true; 4] || self.status_classes == [false; 4] {
            return true;
        }
        let Some(status) = status else {
            return true;
        };
        match status {
            200..=299 => self.status_classes[0],
            300..=399 => self.status_classes[1],
            400..=499 => self.status_classes[2],
            500..=599 => self.status_classes[3],
            _ => false,
        }
    }

    fn matches(
        &self,
        raw_url: &str,
        parsed: &ParsedSiteMapUrl,
        method: &str,
        status: Option<u16>,
    ) -> bool {
        if self.in_scope_only && !self.scope.is_url_in_scope(raw_url) {
            return false;
        }
        if self.hide_not_found && status == Some(404) {
            return false;
        }
        if !self.status_matches(status) {
            return false;
        }
        if self.needle.is_empty() {
            return true;
        }
        if raw_url.to_lowercase().contains(&self.needle)
            || parsed.display_url.to_lowercase().contains(&self.needle)
            || method.to_lowercase().contains(&self.needle)
        {
            return true;
        }
        url::Url::parse(raw_url).is_ok_and(|candidate| {
            candidate
                .host_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&self.needle)
                || candidate.path().to_lowercase().contains(&self.needle)
        })
    }
}

struct OriginTree {
    key: String,
    url_prefix: String,
    direct_count: usize,
    subtree_count: usize,
    children: BTreeMap<String, PathNode>,
}

struct PathNode {
    parent_key: String,
    is_folder: bool,
    direct_count: usize,
    subtree_count: usize,
    children: BTreeMap<String, PathNode>,
}

#[derive(Default)]
pub(super) struct SiteMapTree {
    origins: BTreeMap<(String, String), OriginTree>,
    node_budget: usize,
    truncated: bool,
}

struct TreeEmitter<'a> {
    scope: &'a crate::TargetScope,
    nodes: Vec<SiteMapNode>,
    truncated: bool,
}

impl<'a> TreeEmitter<'a> {
    pub(super) fn new(scope: &'a crate::TargetScope, truncated: bool) -> Self {
        Self {
            scope,
            nodes: Vec::new(),
            truncated,
        }
    }

    fn push_origin(&mut self, label: String, origin: OriginTree) {
        let key = origin.key.clone();
        self.nodes.push(SiteMapNode {
            key,
            parent_key: None,
            label,
            url_prefix: origin.url_prefix.clone(),
            kind: SiteMapNodeKind::Origin,
            depth: 0,
            in_scope: self.scope.is_url_in_scope(&origin.url_prefix),
            direct_count: origin.direct_count,
            subtree_count: origin.subtree_count,
        });
        self.emit_children(origin.children, "", &origin.key, &origin.url_prefix, 0);
    }

    fn emit_children(
        &mut self,
        children: BTreeMap<String, PathNode>,
        parent_path: &str,
        key_base: &str,
        url_base: &str,
        parent_depth: u8,
    ) {
        for (label, node) in children {
            if self.nodes.len() >= MAX_SITE_MAP_NODES {
                self.truncated = true;
                return;
            }
            let path = format!("{parent_path}/{label}");
            let url_prefix = format!("{url_base}{path}");
            let key = format!("{key_base}{path}");
            let kind = if node.is_folder || !node.children.is_empty() {
                SiteMapNodeKind::Folder
            } else {
                SiteMapNodeKind::File
            };
            let in_scope = self.scope.is_url_in_scope(&url_prefix);
            self.nodes.push(SiteMapNode {
                parent_key: Some(node.parent_key),
                label,
                url_prefix,
                kind,
                depth: parent_depth + 1,
                in_scope,
                direct_count: node.direct_count,
                subtree_count: node.subtree_count,
                key,
            });
            self.emit_children(node.children, &path, key_base, url_base, parent_depth + 1);
            if self.nodes.len() >= MAX_SITE_MAP_NODES {
                self.truncated = true;
                return;
            }
        }
    }
}

impl SiteMapTree {
    fn insert(&mut self, parsed: &ParsedSiteMapUrl) {
        let tree_key = (parsed.origin_label.clone(), parsed.origin_key.clone());
        if !self.origins.contains_key(&tree_key) {
            if self.node_budget >= MAX_TREE_BUILD_NODES {
                self.truncated = true;
                return;
            }
            self.origins.insert(
                tree_key.clone(),
                OriginTree {
                    key: parsed.origin_key.clone(),
                    url_prefix: parsed.origin_label.clone(),
                    direct_count: 0,
                    subtree_count: 0,
                    children: BTreeMap::new(),
                },
            );
            self.node_budget += 1;
        }
        let remaining_budget = MAX_TREE_BUILD_NODES.saturating_sub(self.node_budget);
        let mut created = 0usize;
        let mut exhausted = false;
        {
            let origin = self.origins.get_mut(&tree_key).expect("origin node exists");
            origin.subtree_count += 1;
            if parsed.segments.is_empty() {
                origin.direct_count += 1;
            } else {
                let mut parent_key = parsed.origin_key.clone();
                let mut children = &mut origin.children;
                let capped_segments = parsed.segments.len().min(MAX_SITE_MAP_DEPTH as usize);
                for (index, segment) in parsed.segments.iter().enumerate().take(capped_segments) {
                    if !children.contains_key(segment.as_str()) {
                        if created >= remaining_budget {
                            exhausted = true;
                            break;
                        }
                        children.insert(
                            segment.clone(),
                            PathNode {
                                parent_key: parent_key.clone(),
                                is_folder: false,
                                direct_count: 0,
                                subtree_count: 0,
                                children: BTreeMap::new(),
                            },
                        );
                        created += 1;
                    }
                    let node = children
                        .get_mut(segment.as_str())
                        .expect("path node was just checked");
                    node.subtree_count += 1;
                    let is_last = index + 1 == capped_segments;
                    if is_last {
                        node.direct_count += 1;
                        if parsed.directory {
                            node.is_folder = true;
                        }
                    } else {
                        node.is_folder = true;
                    }
                    parent_key.push('/');
                    parent_key.push_str(segment);
                    children = &mut node.children;
                }
            }
        }
        self.node_budget += created;
        if exhausted {
            self.truncated = true;
        }
    }

    pub(super) fn into_nodes(self, scope: &crate::TargetScope) -> (Vec<SiteMapNode>, bool) {
        let mut emitter = TreeEmitter::new(scope, self.truncated);
        for ((label, _), origin) in self.origins {
            if emitter.nodes.len() >= MAX_SITE_MAP_NODES {
                emitter.truncated = true;
                break;
            }
            emitter.push_origin(label, origin);
        }
        (emitter.nodes, emitter.truncated)
    }
}

pub(super) struct SnapshotBuilder<'a> {
    pub(super) filters: EntryFilters<'a>,
    pub(super) selected_key: Option<&'a str>,
    pub(super) offset: usize,
    pub(super) page_end: usize,
    pub(super) tree: SiteMapTree,
    pub(super) total_items: usize,
    pub(super) page_rows: Vec<crate::HistoryRow>,
}

impl SnapshotBuilder<'_> {
    pub(super) fn push_live(&mut self, summary: &crate::FlowSummary) {
        self.push_candidate(&summary.url, &summary.method, summary.status, || {
            Ok(crate::HistoryRow::Live(summary.clone()))
        })
        .expect("live summary page row construction cannot fail");
    }

    pub(super) fn push_candidate(
        &mut self,
        url: &str,
        method: &str,
        status: Option<u16>,
        build_row: impl FnOnce() -> Result<crate::HistoryRow>,
    ) -> Result<()> {
        let Some(parsed) = parse_site_map_url(url) else {
            return Ok(());
        };
        if !self.filters.matches(url, &parsed, method, status) {
            return Ok(());
        }
        self.tree.insert(&parsed);
        let selected = self
            .selected_key
            .is_none_or(|selected| key_in_subtree(&parsed.full_key, selected));
        if selected {
            if (self.offset..self.page_end).contains(&self.total_items) {
                self.page_rows.push(build_row()?);
            }
            self.total_items += 1;
        }
        Ok(())
    }
}
