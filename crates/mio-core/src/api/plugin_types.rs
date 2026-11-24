use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use super::*;

pub type PluginInstanceId = Uuid;

const MAX_HOOK_PATTERNS: usize = 64;
const MAX_HOOK_PATTERN_BYTES: usize = 256;

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHookFailurePolicy {
    #[default]
    FailClosed,
    FailOpen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginHookFilter {
    pub scope_only: bool,
    #[serde(default = "default_true")]
    pub request_stage: bool,
    #[serde(default = "default_true")]
    pub response_stage: bool,
    pub skip_static_resources: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_denylist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_denylist: Vec<String>,
    pub failure_policy: PluginHookFailurePolicy,
}

impl Default for PluginHookFilter {
    fn default() -> Self {
        Self {
            scope_only: false,
            request_stage: true,
            response_stage: true,
            skip_static_resources: false,
            host_allowlist: Vec::new(),
            host_denylist: Vec::new(),
            path_allowlist: Vec::new(),
            path_denylist: Vec::new(),
            failure_policy: PluginHookFailurePolicy::FailClosed,
        }
    }
}

impl PluginHookFilter {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn parse_pattern_list(raw: &str) -> Vec<String> {
        raw.split([',', ';', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn format_pattern_list(items: &[String]) -> String {
        items.join(", ")
    }

    pub fn normalized(&self) -> Self {
        let dedup = |items: &[String], lowercase: bool| -> Vec<String> {
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            for item in items {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let key = if lowercase {
                    trimmed.to_ascii_lowercase()
                } else {
                    trimmed.to_owned()
                };
                if seen.insert(key.clone()) {
                    out.push(if lowercase { key } else { trimmed.to_owned() });
                }
            }
            out
        };
        Self {
            scope_only: self.scope_only,
            request_stage: self.request_stage,
            response_stage: self.response_stage,
            skip_static_resources: self.skip_static_resources,
            host_allowlist: dedup(&self.host_allowlist, true),
            host_denylist: dedup(&self.host_denylist, true),
            path_allowlist: dedup(&self.path_allowlist, false),
            path_denylist: dedup(&self.path_denylist, false),
            failure_policy: self.failure_policy,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for (label, list) in [
            ("Host 白名单", &self.host_allowlist),
            ("Host 黑名单", &self.host_denylist),
            ("Path 白名单", &self.path_allowlist),
            ("Path 黑名单", &self.path_denylist),
        ] {
            anyhow::ensure!(
                list.len() <= MAX_HOOK_PATTERNS,
                "{label}规则数量不能超过 {MAX_HOOK_PATTERNS} 条"
            );
            for item in list {
                let trimmed = item.trim();
                anyhow::ensure!(
                    !trimmed.is_empty()
                        && trimmed.len() <= MAX_HOOK_PATTERN_BYTES
                        && !trimmed.bytes().any(|b| b == 0 || b.is_ascii_control()),
                    "{label}包含空值、控制字符或超长匹配项（上限 {MAX_HOOK_PATTERN_BYTES} 字节）"
                );
            }
        }
        Ok(())
    }

    pub fn should_invoke_http(
        &self,
        url: &str,
        response_stage: bool,
        response_content_type: Option<&str>,
        scope: Option<&TargetScope>,
    ) -> bool {
        if (!response_stage && !self.request_stage) || (response_stage && !self.response_stage) {
            return false;
        }
        if !self.matches_target(url, scope) {
            return false;
        }
        if self.skip_static_resources {
            let (_, path) = split_url_host_path(url);
            if is_static_path(path)
                || (response_stage && response_content_type.is_some_and(is_static_content_type))
            {
                return false;
            }
        }
        true
    }

    pub fn should_invoke_ws(&self, url: &str, scope: Option<&TargetScope>) -> bool {
        self.matches_target(url, scope)
    }

    fn matches_target(&self, url: &str, scope: Option<&TargetScope>) -> bool {
        if self.scope_only && scope.is_some_and(|s| !s.is_url_in_scope(url)) {
            return false;
        }
        let (host, path) = split_url_host_path(url);
        if !self.host_allowlist.is_empty()
            && !self
                .host_allowlist
                .iter()
                .any(|pattern| host_pattern_matches(host, pattern))
        {
            return false;
        }
        if self
            .host_denylist
            .iter()
            .any(|pattern| host_pattern_matches(host, pattern))
        {
            return false;
        }
        if !self.path_allowlist.is_empty()
            && !self
                .path_allowlist
                .iter()
                .any(|pattern| path_pattern_matches(path, pattern))
        {
            return false;
        }
        if self
            .path_denylist
            .iter()
            .any(|pattern| path_pattern_matches(path, pattern))
        {
            return false;
        }
        true
    }
}

fn split_url_host_path(raw: &str) -> (&str, &str) {
    let without_scheme = raw
        .split_once("://")
        .map_or(raw, |(_, remainder)| remainder);
    let authority_end = without_scheme
        .find(['/', '?', '#'])
        .unwrap_or(without_scheme.len());
    let authority = &without_scheme[..authority_end];
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split_once(']').map_or(host_port, |(inner, _)| inner)
    } else {
        host_port.split_once(':').map_or(host_port, |(h, _)| h)
    };
    let rest = &without_scheme[authority_end..];
    let path = rest.split(['?', '#']).next().unwrap_or("/");
    let path = if path.is_empty() { "/" } else { path };
    (host.trim_end_matches('.'), path)
}

fn host_pattern_matches(host: &str, pattern: &str) -> bool {
    let pat = pattern.trim().trim_end_matches('.');
    if pat.is_empty() {
        return false;
    }
    if pat.contains('*') {
        return wildcard_match_ascii_icase(pat.as_bytes(), host.as_bytes());
    }
    if let Some(suffix_no_dot) = pat.strip_prefix('.') {
        host.eq_ignore_ascii_case(suffix_no_dot)
            || (host.len() > pat.len()
                && host[host.len() - pat.len()..].eq_ignore_ascii_case(pat))
    } else {
        host.eq_ignore_ascii_case(pat)
    }
}

fn path_pattern_matches(path: &str, pattern: &str) -> bool {
    let pat = pattern.trim();
    if pat.is_empty() {
        return false;
    }
    if pat.contains('*') {
        return wildcard_match_ascii_icase(pat.as_bytes(), path.as_bytes());
    }
    let path_b = path.as_bytes();
    let pat_b = pat.as_bytes();
    path_b.len() >= pat_b.len() && path_b[..pat_b.len()].eq_ignore_ascii_case(pat_b)
}

fn wildcard_match_ascii_icase(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut match_idx = 0usize;
    while t < text.len() {
        if p < pattern.len() && pattern[p].eq_ignore_ascii_case(&text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            match_idx = t;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            match_idx += 1;
            t = match_idx;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn is_static_path(path: &str) -> bool {
    let last_segment = path.rsplit('/').next().unwrap_or("");
    let Some((_, ext)) = last_segment.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "ico"
            | "svg"
            | "avif"
            | "css"
            | "woff"
            | "woff2"
            | "ttf"
            | "eot"
            | "otf"
            | "mp4"
            | "webm"
            | "mp3"
            | "ogg"
            | "wav"
            | "flac"
            | "map"
            | "wasm"
            | "pdf"
            | "zip"
            | "gz"
            | "tar"
    )
}

fn is_static_content_type(header_value: &str) -> bool {
    let essence = header_value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    essence.starts_with("image/")
        || essence.starts_with("video/")
        || essence.starts_with("audio/")
        || essence.starts_with("font/")
        || essence == "text/css"
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookMetricsSnapshot {
    pub invoked: u64,
    pub filtered_skipped: u64,
    pub oversized_bypassed: u64,
    pub errors_or_timeouts: u64,
    pub fail_open_passes: u64,
}

#[derive(Clone, Debug)]
pub struct PluginInspection {
    pub id: Uuid,
    pub directory: PathBuf,
    pub executable: PathBuf,
    pub manifest: crate::plugins::PluginManifest,
}

#[derive(Clone, Debug)]
pub struct LoadedPlugin {
    pub identity: protocol::ChannelIdentity,
    pub directory: PathBuf,
    pub manifest: crate::plugins::PluginManifest,
    pub grants: Vec<protocol::Permission>,
    /// Each message editor owns a separate worker and its own native editor entities.
    pub tab_id: Option<TabId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPluginInfo {
    pub id: String,
    pub version: String,
    pub engine: String,
    pub directory: PathBuf,
    pub entry: PathBuf,
    pub grants: Vec<protocol::Permission>,
    pub contributions: Vec<String>,
    pub enabled: bool,
    pub auto_reload: bool,
    pub hook_filter: PluginHookFilter,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum PluginAction {
    Inspect {
        directory: PathBuf,
    },
    Load {
        inspection_id: Uuid,
        native_trusted: bool,
        grants: Vec<protocol::Permission>,
        replace_instance: Option<Uuid>,
    },
    CreateEditor {
        source_instance: Uuid,
        tab_id: TabId,
        draft_revision: u64,
    },
    Disable {
        instance_id: Uuid,
    },
    Ui {
        instance_id: Uuid,
        panel_revision: u64,
        event_seq: u64,
        event: panel::UiEvent,
    },
    OpenRequest {
        instance_id: Uuid,
        tab_id: TabId,
        draft_revision: u64,
    },
    CommitEditor {
        instance_id: Uuid,
        tab_id: TabId,
        draft_revision: u64,
        editor: panel::EditorSnapshot,
        expected_panel: Option<Arc<panel::PanelSnapshot>>,
    },
    Cancel {
        operation_id: Uuid,
    },
    /// The UI remains the sole workspace writer; the service validates and persists this proposal.
    ApplyWorkspace {
        instance_id: Uuid,
        request_id: u64,
        workspace: Workspace,
        tab_id: TabId,
    },
    RejectWorkspace {
        instance_id: Uuid,
        request_id: u64,
        message: String,
    },
    Reorder {
        instance_id: Uuid,
        move_up: bool,
    },
    OpenContextViewer {
        source_instance: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    },
    CommitContextEditor {
        instance_id: Uuid,
        context_id: Uuid,
        revision: u64,
        request: HttpRequest,
        editor: panel::EditorSnapshot,
        expected_panel: Option<Arc<panel::PanelSnapshot>>,
    },
    OpenResponseViewer {
        source_instance: PluginInstanceId,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    },
    CommitResponseEditor {
        instance_id: PluginInstanceId,
        context_id: Uuid,
        revision: u64,
        response: HttpResponse,
        editor: panel::EditorSnapshot,
        expected_panel: Option<Arc<panel::PanelSnapshot>>,
    },
    InvokeContextMenu {
        instance_id: PluginInstanceId,
        source: TrafficSource,
        action_id: String,
        selection_start: u64,
        selection_end: u64,
        selected_bytes: Vec<u8>,
        request: HttpRequest,
        response: Option<HttpResponse>,
    },
    Reload {
        instance_id: PluginInstanceId,
    },
    GeneratePayloads {
        instance_id: Uuid,
        tab_id: TabId,
        config: String,
        max_items: u32,
    },
    ProcessPayloads {
        instance_id: Uuid,
        tab_id: TabId,
        position: Option<usize>,
        config: String,
        inputs: Vec<Vec<u8>>,
    },
    SetAutoReload {
        instance_id: Uuid,
        enabled: bool,
    },
    SetHookFilter {
        instance_id: Uuid,
        filter: PluginHookFilter,
    },
    QueryHookMetrics,
    EnableInstalled {
        plugin_id: String,
    },
    Uninstall {
        plugin_id: String,
        instance_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug)]
pub enum PluginOutcome {
    Inspected(PluginInspection),
    Loaded {
        info: LoadedPlugin,
        panel: Arc<panel::PanelSnapshot>,
    },
    Opened {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        draft_revision: u64,
    },
    Committed {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        draft_revision: u64,
        request: HttpRequest,
    },
    ContextOpened {
        info: LoadedPlugin,
        context_id: Uuid,
        revision: u64,
        panel: Arc<panel::PanelSnapshot>,
    },
    ContextCommitted {
        identity: protocol::ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        request: HttpRequest,
    },
    ResponseOpened {
        info: LoadedPlugin,
        context_id: Uuid,
        revision: u64,
        panel: Arc<panel::PanelSnapshot>,
    },
    ResponseCommitted {
        identity: protocol::ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        response: HttpResponse,
    },
    ContextMenuInvoked {
        identity: protocol::ChannelIdentity,
        status_message: String,
        log_line: String,
        open_repeater: Option<HttpRequest>,
        replace_request: Option<Vec<u8>>,
    },
    Reloaded {
        old_instance: PluginInstanceId,
        info: LoadedPlugin,
        panel: Arc<panel::PanelSnapshot>,
    },
    PayloadsGenerated {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        payloads: Vec<Vec<u8>>,
    },
    PayloadsProcessed {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        position: Option<usize>,
        payloads: Vec<Vec<u8>>,
    },
    Reordered {
        order: Vec<Uuid>,
    },
    AutoReloadUpdated {
        instance_id: Uuid,
        enabled: bool,
    },
    HookFilterUpdated {
        instance_id: Uuid,
        filter: PluginHookFilter,
    },
    HookMetrics {
        metrics: Vec<(Uuid, PluginHookMetricsSnapshot)>,
    },
    InstalledEnabled {
        info: LoadedPlugin,
        panel: Arc<panel::PanelSnapshot>,
        auto_reload: bool,
        hook_filter: PluginHookFilter,
        order: Vec<Uuid>,
    },
    Uninstalled {
        plugin_id: String,
        identity: Option<protocol::ChannelIdentity>,
    },
    Disabled {
        identity: protocol::ChannelIdentity,
    },
    EventQueued,
    WorkspaceApplied,
    WorkspaceRejected,
    CancellationRequested,
}

#[derive(Clone, Debug)]
pub enum PluginWorkspaceEdit {
    UpdateDraft {
        tab_id: TabId,
        expected_revision: u64,
        request: HttpRequest,
    },
    OpenRepeater {
        request: HttpRequest,
    },
    OpenIntruder {
        request: HttpRequest,
    },
}

#[derive(Clone, Debug)]
pub struct PluginWorkspaceRequest {
    pub identity: protocol::ChannelIdentity,
    pub request_id: u64,
    pub deadline: Instant,
    pub cancel: tokio_util::sync::CancellationToken,
    pub edit: PluginWorkspaceEdit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_filter_pre_filter_evaluates_stage_scope_host_path_and_static_resources() {
        let scope = TargetScope {
            include: vec![
                ScopeRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    prefix: "https://api.example.invalid".into(),
                    include_subdomains: false,
                },
                ScopeRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    prefix: "wss://api.example.invalid".into(),
                    include_subdomains: false,
                },
            ],
            ..TargetScope::default()
        };
        let filter = PluginHookFilter {
            scope_only: true,
            request_stage: true,
            response_stage: false,
            skip_static_resources: true,
            host_allowlist: vec!["*.example.invalid".into()],
            host_denylist: vec!["blocked.example.invalid".into()],
            path_allowlist: vec!["/v1/*".into()],
            path_denylist: vec!["/v1/health".into()],
            failure_policy: PluginHookFailurePolicy::FailOpen,
        };
        assert!(filter.validate().is_ok());
        assert!(filter.should_invoke_http(
            "https://api.example.invalid/v1/orders",
            false,
            None,
            Some(&scope)
        ));
        // Response stage disabled.
        assert!(!filter.should_invoke_http(
            "https://api.example.invalid/v1/orders",
            true,
            Some("application/json"),
            Some(&scope)
        ));
        // Out of scope.
        assert!(!filter.should_invoke_http(
            "https://other.example.invalid/v1/orders",
            false,
            None,
            Some(&scope)
        ));
        // Path denylist & allowlist checks.
        assert!(!filter.should_invoke_http(
            "https://api.example.invalid/v1/health",
            false,
            None,
            Some(&scope)
        ));
        assert!(!filter.should_invoke_http(
            "https://api.example.invalid/v2/orders",
            false,
            None,
            Some(&scope)
        ));
        // Static extension skipped.
        assert!(!filter.should_invoke_http(
            "https://api.example.invalid/v1/logo.PNG?size=2",
            false,
            None,
            Some(&scope)
        ));

        let resp_filter = PluginHookFilter {
            skip_static_resources: true,
            ..Default::default()
        };
        assert!(!resp_filter.should_invoke_http(
            "https://api.example.invalid/asset",
            true,
            Some("image/png; charset=binary"),
            None
        ));
        assert!(resp_filter.should_invoke_http(
            "https://api.example.invalid/asset",
            true,
            Some("application/json"),
            None
        ));
        assert!(filter.should_invoke_ws("wss://api.example.invalid/v1/stream", Some(&scope)));
        assert!(!filter.should_invoke_ws("wss://other.example.invalid/v1/stream", Some(&scope)));
    }

    #[test]
    fn hook_filter_normalization_validation_and_pattern_parsing_work() {
        let parsed = PluginHookFilter::parse_pattern_list(" a.example.invalid ; *.b.example.invalid \n");
        assert_eq!(parsed, vec!["a.example.invalid", "*.b.example.invalid"]);
        assert_eq!(
            PluginHookFilter::format_pattern_list(&parsed),
            "a.example.invalid, *.b.example.invalid"
        );

        let filter = PluginHookFilter {
            host_allowlist: vec![" API.Example.Invalid ".into(), "api.example.invalid".into(), "".into()],
            path_allowlist: vec![" /v1 ".into(), "/v1".into()],
            ..Default::default()
        };
        let norm = filter.normalized();
        assert_eq!(norm.host_allowlist, vec!["api.example.invalid"]);
        assert_eq!(norm.path_allowlist, vec!["/v1"]);
        assert!(norm.validate().is_ok());

        let too_many = PluginHookFilter {
            host_allowlist: vec!["a.example.invalid".into(); MAX_HOOK_PATTERNS + 1],
            ..Default::default()
        };
        assert!(too_many.validate().is_err());

        let with_ctrl = PluginHookFilter {
            path_denylist: vec!["/bad\npath".into()],
            ..Default::default()
        };
        assert!(with_ctrl.validate().is_err());
    }
}
