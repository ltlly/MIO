use anyhow::{Result, ensure};

use super::*;
use crate::*;

pub fn domain_matches(req_host: &str, cookie_domain: &str, host_only: bool) -> bool {
    let host = req_host.trim_matches(['[', ']']).to_ascii_lowercase();
    let domain = cookie_domain.trim_start_matches('.').to_ascii_lowercase();
    if host.is_empty() || domain.is_empty() {
        return false;
    }
    if host == domain {
        return true;
    }
    if host_only || host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    host.len() > domain.len()
        && host.ends_with(&domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

pub fn path_matches(req_path: &str, cookie_path: &str) -> bool {
    let req_path = if req_path.is_empty() { "/" } else { req_path };
    let cookie_path = if cookie_path.is_empty() {
        "/"
    } else {
        cookie_path
    };
    if req_path == cookie_path {
        return true;
    }
    if let Some(rest) = req_path.strip_prefix(cookie_path) {
        return cookie_path.ends_with('/') || rest.starts_with('/');
    }
    false
}

fn default_cookie_path_for_url(uri_path: &str) -> String {
    if !uri_path.starts_with('/') {
        return "/".into();
    }
    match uri_path.rfind('/') {
        None | Some(0) => "/".into(),
        Some(idx) => uri_path[..idx].to_owned(),
    }
}

pub fn matching_cookie_entries<'a>(
    entries: &'a [CookieJarEntry],
    request_url: &str,
    now_unix_ms: i64,
) -> Vec<&'a CookieJarEntry> {
    let Ok(parsed) = url::Url::parse(request_url) else {
        return Vec::new();
    };
    let is_https = parsed.scheme().eq_ignore_ascii_case("https")
        || parsed.scheme().eq_ignore_ascii_case("wss");
    let Some(host) = parsed.host_str() else {
        return Vec::new();
    };
    let path = parsed.path();
    let mut matched: Vec<(usize, &'a CookieJarEntry)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            if entry.expires_unix_ms.is_some_and(|exp| now_unix_ms >= exp) {
                return false;
            }
            if entry.secure && !is_https {
                return false;
            }
            domain_matches(host, &entry.domain, entry.host_only) && path_matches(path, &entry.path)
        })
        .collect();
    matched.sort_by(|(idx_a, a), (idx_b, b)| {
        b.path
            .len()
            .cmp(&a.path.len())
            .then_with(|| idx_a.cmp(idx_b))
    });
    matched.into_iter().map(|(_, entry)| entry).collect()
}

fn parse_existing_cookies(headers: &[Header]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for header in headers {
        if !header.name.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&header.value) else {
            continue;
        };
        for part in raw.split(';') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((name, value)) = trimmed.split_once('=') {
                let name = name.trim();
                let value = value.trim();
                if !name.is_empty() {
                    pairs.push((name.to_owned(), value.to_owned()));
                }
            }
        }
    }
    pairs
}

fn write_cookie_pairs(headers: &mut Vec<Header>, pairs: &[(String, String)]) -> Result<()> {
    if pairs.is_empty() {
        headers.retain(|h| !h.name.eq_ignore_ascii_case("cookie"));
        return Ok(());
    }
    let serialized = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    ensure!(
        validation::valid_header_value(&serialized),
        "序列化后的 Cookie 请求头包含非法字符或超过长度上限"
    );
    let mut replaced = false;
    headers.retain_mut(|h| {
        if h.name.eq_ignore_ascii_case("cookie") {
            if !replaced {
                h.value = serialized.as_bytes().to_vec();
                replaced = true;
                true
            } else {
                false
            }
        } else {
            true
        }
    });
    if !replaced {
        headers.push(Header {
            name: "Cookie".into(),
            value: serialized.as_bytes().to_vec(),
        });
    }
    Ok(())
}

pub fn apply_cookie_jar_to_request(
    entries: &[CookieJarEntry],
    request: &mut HttpRequest,
    add_new: bool,
    update_existing: bool,
    now_unix_ms: i64,
) -> Result<bool> {
    if !add_new && !update_existing {
        return Ok(false);
    }
    let matched = matching_cookie_entries(entries, &request.url, now_unix_ms);
    if matched.is_empty() {
        return Ok(false);
    }
    let before = request.headers.clone();
    let mut pairs = parse_existing_cookies(&request.headers);
    // Deduplicate matched cookies by name keeping highest-specificity path first.
    let mut chosen: Vec<&CookieJarEntry> = Vec::new();
    for entry in matched {
        if !chosen.iter().any(|c| c.name == entry.name) {
            chosen.push(entry);
        }
    }
    for entry in chosen {
        if let Some(pos) = pairs.iter().position(|(name, _)| name == &entry.name) {
            if update_existing {
                pairs[pos].1 = entry.value.clone();
                // Remove any duplicate occurrences of the same cookie name.
                let mut idx = pos + 1;
                while idx < pairs.len() {
                    if pairs[idx].0 == entry.name {
                        pairs.remove(idx);
                    } else {
                        idx += 1;
                    }
                }
            }
        } else if add_new {
            pairs.push((entry.name.clone(), entry.value.clone()));
        }
    }
    write_cookie_pairs(&mut request.headers, &pairs)?;
    Ok(request.headers != before)
}

pub fn set_or_replace_request_cookie(
    request: &mut HttpRequest,
    name: &str,
    value: &str,
) -> Result<bool> {
    set_request_cookie(request, name, value, false)
}

pub fn set_request_cookie(
    request: &mut HttpRequest,
    name: &str,
    value: &str,
    only_if_present: bool,
) -> Result<bool> {
    ensure!(valid_cookie_name(name), "Cookie 名称无效：{name}");
    ensure!(valid_cookie_value(value), "Cookie 值包含非法控制字符或分号");
    let before = request.headers.clone();
    let mut pairs = parse_existing_cookies(&request.headers);
    let mut found = false;
    let mut idx = 0;
    while idx < pairs.len() {
        if pairs[idx].0 == name {
            if !found {
                pairs[idx].1 = value.to_owned();
                found = true;
                idx += 1;
            } else {
                pairs.remove(idx);
            }
        } else {
            idx += 1;
        }
    }
    if !found {
        if only_if_present {
            return Ok(false);
        }
        pairs.push((name.to_owned(), value.to_owned()));
    }
    write_cookie_pairs(&mut request.headers, &pairs)?;
    Ok(request.headers != before)
}

pub fn ingest_set_cookie_headers(
    entries: &mut Vec<CookieJarEntry>,
    request_url: &str,
    response: &HttpResponse,
    now_unix_ms: i64,
) -> bool {
    let Ok(parsed_url) = url::Url::parse(request_url) else {
        return false;
    };
    let Some(raw_host) = parsed_url.host_str() else {
        return false;
    };
    let req_host = raw_host.trim_matches(['[', ']']).to_ascii_lowercase();
    if req_host.is_empty() {
        return false;
    }
    let default_path = default_cookie_path_for_url(parsed_url.path());
    let mut changed = false;

    // Prune already-expired cookies before inserting new ones.
    let before_len = entries.len();
    entries.retain(|e| e.expires_unix_ms.is_none_or(|exp| now_unix_ms < exp));
    if entries.len() != before_len {
        changed = true;
    }

    for header in &response.headers {
        if !header.name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&header.value) else {
            continue;
        };
        if apply_single_set_cookie(entries, &req_host, &default_path, raw, now_unix_ms) {
            changed = true;
        }
    }
    changed
}

fn apply_single_set_cookie(
    entries: &mut Vec<CookieJarEntry>,
    req_host: &str,
    default_path: &str,
    raw_header: &str,
    now_unix_ms: i64,
) -> bool {
    let mut parts = raw_header.split(';');
    let Some(name_value) = parts.next() else {
        return false;
    };
    let Some((raw_name, raw_value)) = name_value.split_once('=') else {
        return false;
    };
    let name = raw_name.trim();
    let value = raw_value.trim();
    if !valid_cookie_name(name) || !valid_cookie_value(value) {
        return false;
    }

    let mut domain_attr: Option<String> = None;
    let mut path_attr: Option<String> = None;
    let mut secure = false;
    let mut http_only = false;
    let mut max_age_ms: Option<i64> = None;
    let mut expires_ms: Option<i64> = None;

    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        if let Some((attr_name, attr_val)) = attr.split_once('=') {
            let key = attr_name.trim();
            let val = attr_val.trim();
            if key.eq_ignore_ascii_case("domain") {
                if let Ok(norm) = normalize_cookie_domain(val) {
                    domain_attr = Some(norm);
                } else {
                    return false;
                }
            } else if key.eq_ignore_ascii_case("path") {
                if val.starts_with('/') && val.len() <= 2048 && !val.chars().any(char::is_control) {
                    path_attr = Some(val.to_owned());
                }
            } else if key.eq_ignore_ascii_case("max-age") {
                if let Ok(secs) = val.parse::<i64>() {
                    max_age_ms = Some(if secs <= 0 {
                        now_unix_ms.saturating_sub(1)
                    } else {
                        now_unix_ms.saturating_add(secs.saturating_mul(1000))
                    });
                }
            } else if key.eq_ignore_ascii_case("expires")
                && let Some(parsed_ms) = parse_http_cookie_date_ms(val)
            {
                expires_ms = Some(parsed_ms);
            }
        } else if attr.eq_ignore_ascii_case("secure") {
            secure = true;
        } else if attr.eq_ignore_ascii_case("httponly") {
            http_only = true;
        }
    }

    let is_ip = req_host.parse::<std::net::IpAddr>().is_ok();
    let (domain, host_only) = match domain_attr {
        Some(dom) => {
            if is_ip {
                if dom != req_host {
                    return false;
                }
                (req_host.to_owned(), true)
            } else {
                // Reject single-label public suffix domain attributes unless exact host (e.g. localhost).
                if !dom.contains('.') && dom != req_host {
                    return false;
                }
                if !domain_matches(req_host, &dom, false) {
                    return false;
                }
                (dom, false)
            }
        }
        None => (req_host.to_owned(), true),
    };
    let path = path_attr.unwrap_or_else(|| default_path.to_owned());
    // RFC 6265: Max-Age takes precedence over Expires.
    let effective_expires = max_age_ms.or(expires_ms);

    let existing_pos = entries.iter().position(|e| {
        e.domain == domain && e.host_only == host_only && e.path == path && e.name == name
    });

    if effective_expires.is_some_and(|exp| exp <= now_unix_ms) {
        if let Some(pos) = existing_pos {
            entries.remove(pos);
            return true;
        }
        return false;
    }

    if let Some(pos) = existing_pos {
        let entry = &mut entries[pos];
        let updated = entry.value != value
            || entry.secure != secure
            || entry.http_only != http_only
            || entry.expires_unix_ms != effective_expires;
        entry.value = value.to_owned();
        entry.secure = secure;
        entry.http_only = http_only;
        entry.expires_unix_ms = effective_expires;
        return updated;
    }

    if entries.len() >= MAX_COOKIE_JAR_ENTRIES {
        entries.remove(0);
    }
    entries.push(CookieJarEntry {
        id: Uuid::new_v4(),
        domain,
        host_only,
        path,
        name: name.to_owned(),
        value: value.to_owned(),
        secure,
        http_only,
        expires_unix_ms: effective_expires,
    });
    true
}

/// Parses IMF-fixdate / RFC 6265 cookie-date (`Wed, 21 Oct 2015 07:28:00 GMT` or `21-Oct-2015 07:28:00 GMT`).
pub(crate) fn parse_http_cookie_date_ms(input: &str) -> Option<i64> {
    let cleaned = input.trim();
    let after_weekday = match cleaned.split_once(',') {
        Some((_, rest)) => rest.trim(),
        None => cleaned,
    };
    let tokens: Vec<&str> = after_weekday
        .split(|c: char| c.is_ascii_whitespace() || c == '-')
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.len() < 4 {
        return None;
    }
    let day: u32 = tokens[0].parse().ok()?;
    let month: u32 = match tokens[1].to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let raw_year: i64 = tokens[2].parse().ok()?;
    let year = match raw_year {
        0..=69 => 2000 + raw_year,
        70..=99 => 1900 + raw_year,
        1601..=9999 => raw_year,
        _ => return None,
    };
    let mut hms = tokens[3].split(':');
    let hour: u32 = hms.next()?.parse().ok()?;
    let minute: u32 = hms.next()?.parse().ok()?;
    let second: u32 = hms.next()?.parse().ok()?;
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second.min(59)))?;
    secs.checked_mul(1000)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    let max_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            if leap { 29 } else { 28 }
        }
        _ => return None,
    };
    if day > max_days {
        return None;
    }
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = u64::from(month);
    let d = u64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + (doe as i64) - 719_468)
}
