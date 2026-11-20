use std::net::{IpAddr, SocketAddr};

pub(crate) const MAX_MIME_SNIFF_BYTES: usize = 512;
pub(crate) const MAX_TITLE_SCAN_BYTES: usize = 256 * 1024;
pub(crate) const MAX_PAGE_TITLE_BYTES: usize = 16 * 1024;

mod mime;
mod title;
pub(crate) use mime::{infer_history_display_mime_type, infer_proxy_mime_type};
pub(crate) use title::extract_html_page_title_checked;

#[cfg(test)]
pub(crate) use title::extract_html_page_title;

/// Normalizes an optional IP address or socket endpoint string into its canonical
/// `IpAddr` textual form (for example `"127.0.0.1"` or `"::1"`).
///
/// Returns `None` when `ip` is `None` (connection / resolution not established yet)
/// or when the candidate is not a valid IP literal or socket address.
pub(crate) fn normalize_ip_candidate(ip: Option<&str>) -> Option<String> {
    let raw = ip?.trim();
    if raw.is_empty() || raw.len() > 128 {
        return None;
    }
    if let Ok(socket_addr) = raw.parse::<SocketAddr>() {
        return Some(canonical_ip(socket_addr.ip()));
    }
    let unbracketed = if let Some(rest) = raw.strip_prefix('[') {
        if let Some((inside, suffix)) = rest.split_once(']') {
            if !suffix.is_empty() {
                let port = suffix.strip_prefix(':')?;
                port.parse::<u16>().ok()?;
            }
            inside
        } else {
            return None;
        }
    } else {
        raw
    };
    let without_zone = match unbracketed.split_once('%') {
        Some((addr, zone)) if !zone.is_empty() && unbracketed.contains(':') => addr,
        Some(_) => return None,
        None => unbracketed,
    };
    if let Ok(addr) = without_zone.parse::<IpAddr>() {
        return Some(canonical_ip(addr));
    }
    if let Some((host, port)) = without_zone.rsplit_once(':')
        && !host.contains(':')
        && port.parse::<u16>().is_ok()
        && let Ok(v4) = host.parse::<std::net::Ipv4Addr>()
    {
        return Some(v4.to_string());
    }
    None
}

fn canonical_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => mapped.to_string(),
            None => v6.to_string(),
        },
    }
}

/// Evaluates the boolean `WasAnnotated` (`expect_annotated = true`) and
/// `WasNotAnnotated` (`expect_annotated = false`) condition against the local
/// annotation state of a flow.
#[inline]
pub(crate) const fn matches_annotation_condition(
    was_annotated: bool,
    expect_annotated: bool,
) -> bool {
    was_annotated == expect_annotated
}

#[cfg(test)]
mod tests;
