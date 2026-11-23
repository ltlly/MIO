use super::*;
use std::sync::{Mutex, OnceLock};

const MAX_GREP_REGEX_BYTES: usize = 1 << 20;
const GREP_REGEX_CACHE_LIMIT: usize = 256;
type GrepRegexCache = crate::lru::BoundedLruCache<(String, bool), Option<regex::bytes::Regex>>;

fn grep_regex_cache() -> &'static Mutex<GrepRegexCache> {
    static CACHE: OnceLock<Mutex<GrepRegexCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(crate::lru::BoundedLruCache::new(GREP_REGEX_CACHE_LIMIT)))
}

fn cached_grep_regex(pattern: &str, case_insensitive: bool) -> Option<regex::bytes::Regex> {
    if pattern.is_empty() || pattern.len() > MAX_GREP_REGEX_BYTES {
        return None;
    }
    let key = (pattern.to_owned(), case_insensitive);
    {
        let mut guard = grep_regex_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = guard.get(&key) {
            return cached;
        }
    }
    let compiled = regex::bytes::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(MAX_GREP_REGEX_BYTES)
        .dfa_size_limit(MAX_GREP_REGEX_BYTES)
        .build()
        .ok();
    {
        let mut guard = grep_regex_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(key, compiled.clone());
    }
    compiled
}

pub fn evaluate_grep_config(
    response_raw: &[u8],
    config: &crate::IntruderGrepConfig,
) -> (Vec<String>, Vec<String>) {
    let raw = &response_raw[..response_raw.len().min(MAX_REQUEST_BYTES)];
    let matched =
        evaluate_grep_matches(raw, &config.match_expressions, config.match_case_sensitive);
    let pairs = evaluate_grep_extract_pairs(response_raw, config);
    let extracted_list = pairs
        .into_iter()
        .map(|(key, val)| format!("{key}: {val}"))
        .collect();
    (matched, extracted_list)
}

pub fn evaluate_grep_extract_pairs(
    response_raw: &[u8],
    config: &crate::IntruderGrepConfig,
) -> Vec<(String, String)> {
    let input_truncated = response_raw.len() > MAX_REQUEST_BYTES;
    let raw = &response_raw[..response_raw.len().min(MAX_REQUEST_BYTES)];
    let mut out = Vec::new();
    for rule in &config.extract_rules {
        if let Some(pattern) = rule.regex_pattern() {
            if let Some(re) = cached_grep_regex(pattern, false)
                && let Some(caps) = re.captures(raw)
                && let Some(m) = caps.get(1).or_else(|| caps.get(0))
            {
                let (mut extracted, value_truncated) = format_extracted_bytes(m.as_bytes(), 256);
                if value_truncated {
                    extracted.push('…');
                }
                if input_truncated {
                    extracted.push_str(" [响应超限截断]");
                }
                out.push((rule.key_label(), extracted));
            }
            continue;
        }
        if rule.start_delimiter.is_empty() || rule.end_delimiter.is_empty() {
            continue;
        }
        let start_bytes = rule.start_delimiter.as_bytes();
        let end_bytes = rule.end_delimiter.as_bytes();
        if let Some(start_idx) = find_subslice(raw, start_bytes, true) {
            let after_start = &raw[start_idx + start_bytes.len()..];
            if let Some(end_idx) = find_subslice(after_start, end_bytes, true) {
                let between = &after_start[..end_idx];
                let (mut extracted, value_truncated) = format_extracted_bytes(between, 256);
                if value_truncated {
                    extracted.push('…');
                }
                if input_truncated {
                    extracted.push_str(" [响应超限截断]");
                }
                out.push((rule.key_label(), extracted));
            }
        }
    }
    out
}

pub fn evaluate_grep_payload_hits(
    response_raw: &[u8],
    payloads: &[&[u8]],
    case_sensitive: bool,
) -> Vec<bool> {
    let raw = &response_raw[..response_raw.len().min(MAX_REQUEST_BYTES)];
    payloads
        .iter()
        .map(|payload| !payload.is_empty() && find_subslice(raw, payload, case_sensitive).is_some())
        .collect()
}

pub fn evaluate_grep_row(
    response_raw: &[u8],
    config: &crate::IntruderGrepConfig,
    payloads: &[&[u8]],
) -> (Vec<String>, Vec<(String, String)>, Vec<bool>) {
    let raw = &response_raw[..response_raw.len().min(MAX_REQUEST_BYTES)];
    let matched =
        evaluate_grep_matches(raw, &config.match_expressions, config.match_case_sensitive);
    let extracts = evaluate_grep_extract_pairs(response_raw, config);
    let payload_hits = if config.grep_payloads {
        evaluate_grep_payload_hits(raw, payloads, config.grep_payloads_case_sensitive)
    } else {
        Vec::new()
    };
    (matched, extracts, payload_hits)
}

pub fn evaluate_grep_matches(
    response_bytes: &[u8],
    expressions: &[String],
    case_sensitive: bool,
) -> Vec<String> {
    let raw = &response_bytes[..response_bytes.len().min(MAX_REQUEST_BYTES)];
    let text_utf8 = std::str::from_utf8(raw).ok();
    let has_literal = expressions
        .iter()
        .any(|expr| !expr.is_empty() && !expr.starts_with("re:"));
    let lower_utf8 = (!case_sensitive && has_literal)
        .then(|| text_utf8.map(str::to_lowercase))
        .flatten();
    let lower_lossy = (text_utf8.is_none()
        && !case_sensitive
        && expressions
            .iter()
            .any(|e| !e.is_empty() && !e.starts_with("re:") && !e.is_ascii()))
    .then(|| String::from_utf8_lossy(raw).to_lowercase());
    let mut matched = Vec::new();
    for expr in expressions {
        if expr.is_empty() {
            continue;
        }
        if let Some(pattern) = expr.strip_prefix("re:") {
            if let Some(re) = cached_grep_regex(pattern, !case_sensitive)
                && re.is_match(raw)
            {
                matched.push(expr.clone());
            }
            continue;
        }
        let needle = expr.as_bytes();
        let is_match = if case_sensitive {
            find_subslice(raw, needle, true).is_some()
        } else if expr.is_ascii() {
            find_subslice(raw, needle, false).is_some()
        } else if let Some(lower) = lower_utf8.as_deref() {
            lower.contains(&expr.to_lowercase())
        } else {
            find_subslice(raw, needle, false).is_some()
                || lower_lossy
                    .as_deref()
                    .is_some_and(|lower| lower.contains(&expr.to_lowercase()))
        };
        if is_match {
            matched.push(expr.clone());
        }
    }
    matched
}

pub fn find_echoed_inputs(response_bytes: &[u8], inputs: &[String]) -> Vec<String> {
    let raw = &response_bytes[..response_bytes.len().min(MAX_REQUEST_BYTES)];
    let mut seen = std::collections::HashSet::new();
    let mut echoed = Vec::new();
    for input in inputs {
        if input.is_empty() || !seen.insert(input.as_str()) {
            continue;
        }
        if find_subslice(raw, input.as_bytes(), true).is_some() {
            echoed.push(input.clone());
        }
    }
    echoed
}

fn find_subslice(haystack: &[u8], needle: &[u8], case_sensitive: bool) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    if case_sensitive {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    } else {
        haystack
            .windows(needle.len())
            .position(|window| window.eq_ignore_ascii_case(needle))
    }
}

fn format_extracted_bytes(bytes: &[u8], max_units: usize) -> (String, bool) {
    let max_scan = max_units.saturating_mul(4).saturating_add(4);
    let over_scan = bytes.len() > max_scan;
    let mut bytes = &bytes[..bytes.len().min(max_scan)];
    let mut out = String::new();
    let mut units = 0usize;
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                for ch in valid.chars() {
                    if units == max_units {
                        return (out, true);
                    }
                    if ch == '\0' {
                        out.push_str("\\x00");
                    } else {
                        out.push(ch);
                    }
                    units += 1;
                }
                return (out, over_scan);
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    let valid = &bytes[..valid_up_to];
                    let valid_str = std::str::from_utf8(valid).expect("valid UTF-8 prefix");
                    for ch in valid_str.chars() {
                        if units == max_units {
                            return (out, true);
                        }
                        if ch == '\0' {
                            out.push_str("\\x00");
                        } else {
                            out.push(ch);
                        }
                        units += 1;
                    }
                    bytes = &bytes[valid_up_to..];
                    continue;
                }
                if units == max_units {
                    return (out, true);
                }
                let b = bytes[0];
                out.push_str(&format!("\\x{b:02x}"));
                units += 1;
                bytes = &bytes[1..];
            }
        }
    }
    (out, over_scan)
}
