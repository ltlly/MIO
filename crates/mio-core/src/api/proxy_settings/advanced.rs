use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SslPassThroughRule {
    pub id: Uuid,
    pub enabled: bool,
    pub host_pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub is_regex: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsReplaceDirection {
    #[default]
    Both,
    ClientToServer,
    ServerToClient,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WsMatchAndReplaceRule {
    pub id: Uuid,
    pub enabled: bool,
    #[serde(default)]
    pub direction: WsReplaceDirection,
    pub match_pattern: String,
    pub replace_with: String,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub comment: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyResponseModificationSettings {
    #[serde(default)]
    pub unhide_hidden_form_fields: bool,
    #[serde(default)]
    pub enable_disabled_form_fields: bool,
    #[serde(default)]
    pub highlight_unhidden_fields: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_input_field_length_limits: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_javascript_form_validation: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_all_javascript: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_object_tags: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub convert_https_links_to_http: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_secure_flag_from_cookies: bool,
}

impl ProxyResponseModificationSettings {
    pub fn is_default(&self) -> bool {
        !self.unhide_hidden_form_fields
            && !self.enable_disabled_form_fields
            && !self.highlight_unhidden_fields
            && !self.remove_input_field_length_limits
            && !self.remove_javascript_form_validation
            && !self.remove_all_javascript
            && !self.remove_object_tags
            && !self.convert_https_links_to_http
            && !self.remove_secure_flag_from_cookies
    }
}

pub const WS_MATCH_REPLACE_COMMENT_PREFIX: &str = "mio-ws-mr:";
pub const SSL_PASSTHROUGH_COMMENT_PREFIX: &str = "mio-ssl-pt:";

impl WsReplaceDirection {
    pub fn as_code(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::ClientToServer => "c2s",
            Self::ServerToClient => "s2c",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code {
            "c2s" | "client_to_server" => Self::ClientToServer,
            "s2c" | "server_to_client" => Self::ServerToClient,
            _ => Self::Both,
        }
    }
}

impl SslPassThroughRule {
    pub fn to_match_and_replace_rule(&self) -> MatchAndReplaceRule {
        MatchAndReplaceRule {
            id: self.id,
            enabled: self.enabled,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: self.host_pattern.clone(),
            replace_with: self.port.map(|p| p.to_string()).unwrap_or_default(),
            regex: self.is_regex,
            comment: SSL_PASSTHROUGH_COMMENT_PREFIX.to_owned(),
            script_name: String::new(),
            filter_script: String::new(),
        }
    }

    pub fn from_match_and_replace_rule(rule: &MatchAndReplaceRule) -> Option<Self> {
        if !rule.comment.starts_with(SSL_PASSTHROUGH_COMMENT_PREFIX) {
            return None;
        }
        let port = if rule.replace_with.trim().is_empty() {
            None
        } else {
            rule.replace_with
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
        };
        Some(Self {
            id: rule.id,
            enabled: rule.enabled,
            host_pattern: rule.match_pattern.clone(),
            port,
            is_regex: rule.regex,
        })
    }
}

impl WsMatchAndReplaceRule {
    pub fn to_match_and_replace_rule(&self) -> MatchAndReplaceRule {
        MatchAndReplaceRule {
            id: self.id,
            enabled: self.enabled,
            item_type: MatchReplaceTarget::RequestBody,
            match_pattern: self.match_pattern.clone(),
            replace_with: self.replace_with.clone(),
            regex: self.is_regex,
            comment: format!(
                "{WS_MATCH_REPLACE_COMMENT_PREFIX}{}:{}",
                self.direction.as_code(),
                self.comment
            ),
            script_name: String::new(),
            filter_script: String::new(),
        }
    }

    pub fn from_match_and_replace_rule(rule: &MatchAndReplaceRule) -> Option<Self> {
        let rest = rule.comment.strip_prefix(WS_MATCH_REPLACE_COMMENT_PREFIX)?;
        let (dir_code, user_comment) = rest.split_once(':').unwrap_or((rest, ""));
        Some(Self {
            id: rule.id,
            enabled: rule.enabled,
            direction: WsReplaceDirection::from_code(dir_code),
            match_pattern: rule.match_pattern.clone(),
            replace_with: rule.replace_with.clone(),
            is_regex: rule.regex,
            comment: user_comment.to_owned(),
        })
    }
}

pub fn is_advanced_proxy_rule(rule: &MatchAndReplaceRule) -> bool {
    rule.comment.starts_with(SSL_PASSTHROUGH_COMMENT_PREFIX)
        || rule.comment.starts_with(WS_MATCH_REPLACE_COMMENT_PREFIX)
}

pub fn extract_ssl_pass_through_rules(rules: &[MatchAndReplaceRule]) -> Vec<SslPassThroughRule> {
    rules
        .iter()
        .filter_map(SslPassThroughRule::from_match_and_replace_rule)
        .collect()
}

pub fn extract_ws_match_and_replace_rules(
    rules: &[MatchAndReplaceRule],
) -> Vec<WsMatchAndReplaceRule> {
    rules
        .iter()
        .filter_map(WsMatchAndReplaceRule::from_match_and_replace_rule)
        .collect()
}

pub fn replace_ssl_pass_through_rules(
    rules: &mut Vec<MatchAndReplaceRule>,
    ssl_rules: &[SslPassThroughRule],
) {
    rules.retain(|r| !r.comment.starts_with(SSL_PASSTHROUGH_COMMENT_PREFIX));
    rules.extend(
        ssl_rules
            .iter()
            .map(SslPassThroughRule::to_match_and_replace_rule),
    );
}

pub fn replace_ws_match_and_replace_rules(
    rules: &mut Vec<MatchAndReplaceRule>,
    ws_rules: &[WsMatchAndReplaceRule],
) {
    rules.retain(|r| !r.comment.starts_with(WS_MATCH_REPLACE_COMMENT_PREFIX));
    rules.extend(
        ws_rules
            .iter()
            .map(WsMatchAndReplaceRule::to_match_and_replace_rule),
    );
}
