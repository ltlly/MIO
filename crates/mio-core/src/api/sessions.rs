use super::*;

pub const MAX_COOKIE_JAR_ENTRIES: usize = 4096;
pub const MAX_SESSION_RULES: usize = 128;
pub const MAX_RULE_ACTIONS: usize = 16;
pub const MAX_SESSION_MACROS: usize = 64;
pub const MAX_MACRO_STEPS: usize = 16;
pub const MAX_MACRO_EXTRACTIONS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTool {
    Proxy,
    Repeater,
    Intruder,
    Scanner,
    Sequencer,
    Extensions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieJarEntry {
    pub id: Uuid,
    pub domain: String,
    #[serde(default)]
    pub host_only: bool,
    #[serde(default = "default_cookie_path")]
    pub path: String,
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_unix_ms: Option<i64>,
}

fn default_cookie_path() -> String {
    "/".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CookieJarSettings {
    #[serde(default = "default_true_bool")]
    pub monitor_proxy: bool,
    pub monitor_repeater: bool,
    pub monitor_intruder: bool,
    #[serde(default)]
    pub monitor_scanner: bool,
    #[serde(default)]
    pub monitor_sequencer: bool,
    #[serde(default)]
    pub monitor_extensions: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub monitor_mio_ai: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<CookieJarEntry>,
}

impl Default for CookieJarSettings {
    fn default() -> Self {
        Self {
            monitor_proxy: true,
            monitor_repeater: false,
            monitor_intruder: false,
            monitor_scanner: false,
            monitor_sequencer: false,
            monitor_extensions: false,
            monitor_mio_ai: false,
            entries: Vec::new(),
        }
    }
}

impl CookieJarSettings {
    pub fn is_default(&self) -> bool {
        self.monitor_proxy
            && !self.monitor_repeater
            && !self.monitor_intruder
            && !self.monitor_scanner
            && !self.monitor_sequencer
            && !self.monitor_extensions
            && !self.monitor_mio_ai
            && self.entries.is_empty()
    }

    pub fn monitors(&self, tool: SessionTool) -> bool {
        match tool {
            SessionTool::Proxy => self.monitor_proxy,
            SessionTool::Repeater => self.monitor_repeater,
            SessionTool::Intruder => self.monitor_intruder,
            SessionTool::Scanner => self.monitor_scanner,
            SessionTool::Sequencer => self.monitor_sequencer,
            SessionTool::Extensions => self.monitor_extensions,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionToolScope {
    pub proxy: bool,
    #[serde(default = "default_true_bool")]
    pub repeater: bool,
    #[serde(default = "default_true_bool")]
    pub intruder: bool,
    #[serde(default)]
    pub scanner: bool,
    #[serde(default)]
    pub sequencer: bool,
    #[serde(default)]
    pub extensions: bool,
}

impl Default for SessionToolScope {
    fn default() -> Self {
        Self {
            proxy: false,
            repeater: true,
            intruder: true,
            scanner: false,
            sequencer: false,
            extensions: false,
        }
    }
}

impl SessionToolScope {
    pub fn matches(&self, tool: SessionTool) -> bool {
        match tool {
            SessionTool::Proxy => self.proxy,
            SessionTool::Repeater => self.repeater,
            SessionTool::Intruder => self.intruder,
            SessionTool::Scanner => self.scanner,
            SessionTool::Sequencer => self.sequencer,
            SessionTool::Extensions => self.extensions,
        }
    }
}

pub type SessionRuleTools = SessionToolScope;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SessionUrlScope {
    #[default]
    AllUrls,
    SuiteScope,
    CustomPrefixes(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "names", rename_all = "snake_case")]
pub enum SessionParamCondition {
    RequirePresent(Vec<String>),
    RequireAbsent(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRuleCondition {
    pub url_scope: SessionUrlScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_condition: Option<SessionParamCondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_contains: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionRuleAction {
    UseCookieJar {
        #[serde(default = "default_true_bool")]
        add_new_cookies: bool,
        #[serde(default = "default_true_bool")]
        update_existing_cookies: bool,
    },
    SetHeader {
        name: String,
        value: String,
        #[serde(default)]
        only_if_present: bool,
    },
    SetCookie {
        name: String,
        value: String,
        #[serde(default)]
        only_if_present: bool,
    },
    RewriteUrlParam {
        name: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        remove: bool,
        #[serde(default)]
        only_if_present: bool,
    },
    ReplaceRequestText {
        match_pattern: String,
        #[serde(default)]
        replace_with: String,
        #[serde(default)]
        regex: bool,
    },
    RunMacro {
        macro_id: Uuid,
        #[serde(default = "default_true_bool")]
        update_cookies_from_jar: bool,
        #[serde(default = "default_true_bool")]
        apply_extractions: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRule {
    pub id: Uuid,
    pub name: String,
    #[serde(default = "default_true_bool")]
    pub enabled: bool,
    #[serde(default)]
    pub tools: SessionToolScope,
    #[serde(default)]
    pub condition: SessionRuleCondition,
    pub actions: Vec<SessionRuleAction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacroExtractSource {
    Delimiter { start: String, end: String },
    Regex { pattern: String },
    ResponseHeader { header_name: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacroApplyTarget {
    #[default]
    UrlOrBodyParam,
    Header {
        header_name: String,
    },
    Cookie {
        cookie_name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroExtraction {
    pub name: String,
    pub source: MacroExtractSource,
    #[serde(default)]
    pub target: MacroApplyTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroStep {
    pub id: Uuid,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<u8>,
    #[serde(default = "default_true_bool")]
    pub use_cookie_jar: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extractions: Vec<MacroExtraction>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum MacroFailurePolicy {
    #[default]
    Abort,
    Retry {
        max_retries: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMacro {
    pub id: Uuid,
    pub name: String,
    pub steps: Vec<MacroStep>,
    #[serde(default)]
    pub failure_policy: MacroFailurePolicy,
    #[serde(default = "default_macro_timeout_ms")]
    pub timeout_ms: u64,
}

const fn default_macro_timeout_ms() -> u64 {
    10_000
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    #[serde(default, skip_serializing_if = "CookieJarSettings::is_default")]
    pub cookie_jar: CookieJarSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<SessionRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<SessionMacro>,
}

impl SessionSettings {
    pub fn is_default(&self) -> bool {
        self.cookie_jar.is_default() && self.rules.is_empty() && self.macros.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTraceEntry {
    pub id: Uuid,
    pub timestamp_ms: i64,
    pub tool: SessionTool,
    pub url: String,
    pub rule_name: String,
    pub action_summary: String,
    pub detail: String,
    pub before_preview: String,
    pub after_preview: String,
}
