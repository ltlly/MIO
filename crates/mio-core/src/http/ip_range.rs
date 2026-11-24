use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IpRangeMatcher {
    V4 { start: u32, end: u32 },
    V6 { start: u128, end: u128 },
}

impl IpRangeMatcher {
    pub(crate) fn matches_ip(self, ip: IpAddr) -> bool {
        match (self, ip) {
            (Self::V4 { start, end }, IpAddr::V4(v4)) => {
                let val = u32::from(v4);
                val >= start && val <= end
            }
            (Self::V4 { start, end }, IpAddr::V6(v6)) => v6.to_ipv4_mapped().is_some_and(|v4| {
                let val = u32::from(v4);
                val >= start && val <= end
            }),
            (Self::V6 { start, end }, IpAddr::V6(v6)) => {
                let val = u128::from(v6);
                val >= start && val <= end
            }
            _ => false,
        }
    }

    pub(crate) fn matches_host(self, host: &str) -> bool {
        let clean = host.trim().trim_matches(['[', ']']);
        clean
            .parse::<IpAddr>()
            .is_ok_and(|ip| self.matches_ip(ip))
    }
}

fn strip_brackets(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed)
}

fn parse_cidr_bounds(addr_str: &str, prefix_str: &str) -> Result<IpRangeMatcher, String> {
    let ip: IpAddr = strip_brackets(addr_str)
        .parse()
        .map_err(|_| format!("CIDR 网段 IP 地址无效：{addr_str}"))?;
    let prefix: u32 = prefix_str
        .trim()
        .parse()
        .map_err(|_| format!("CIDR 前缀长度无效：{prefix_str}"))?;
    match ip {
        IpAddr::V4(v4) => {
            if prefix > 32 {
                return Err(format!("IPv4 CIDR 前缀长度必须在 0 到 32 之间：{prefix}"));
            }
            let raw = u32::from(v4);
            let (start, end) = if prefix == 0 {
                (0u32, u32::MAX)
            } else {
                let mask = !0u32 << (32 - prefix);
                let net = raw & mask;
                (net, net | !mask)
            };
            Ok(IpRangeMatcher::V4 { start, end })
        }
        IpAddr::V6(v6) => {
            if prefix > 128 {
                return Err(format!("IPv6 CIDR 前缀长度必须在 0 到 128 之间：{prefix}"));
            }
            let raw = u128::from(v6);
            let (start, end) = if prefix == 0 {
                (0u128, u128::MAX)
            } else {
                let mask = !0u128 << (128 - prefix);
                let net = raw & mask;
                (net, net | !mask)
            };
            Ok(IpRangeMatcher::V6 { start, end })
        }
    }
}

/// Parses an explicit CIDR (`192.0.2.0/24`, `2001:db8::/32`) or IP range
/// (`192.0.2.1-192.0.2.50`, `2001:db8::1-2001:db8::10`) pattern.
///
/// Returns `Ok(None)` when `pattern` does not look like a CIDR or IP range so hostnames and
/// regexes containing `-` or `/` (e.g. `api-v1.example.invalid` or `^[0-9]+$`) are untouched.
pub(crate) fn parse_cidr_or_ip_range(pattern: &str) -> Result<Option<IpRangeMatcher>, String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return Ok(None);
    }

    if let Some((addr_part, prefix_part)) = trimmed.split_once('/')
        && strip_brackets(addr_part).parse::<IpAddr>().is_ok()
    {
        return parse_cidr_bounds(addr_part, prefix_part).map(Some);
    }

    if let Some((left_raw, right_raw)) = trimmed.split_once('-') {
        let left_clean = strip_brackets(left_raw);
        let right_clean = strip_brackets(right_raw);
        let left_ip = left_clean.parse::<IpAddr>();
        let right_ip = right_clean.parse::<IpAddr>();
        match (left_ip, right_ip) {
            (Ok(IpAddr::V4(start)), Ok(IpAddr::V4(end))) => {
                let s = u32::from(start);
                let e = u32::from(end);
                if s > e {
                    return Err(format!("IP 区间起始地址不能大于结束地址：{trimmed}"));
                }
                return Ok(Some(IpRangeMatcher::V4 { start: s, end: e }));
            }
            (Ok(IpAddr::V6(start)), Ok(IpAddr::V6(end))) => {
                let s = u128::from(start);
                let e = u128::from(end);
                if s > e {
                    return Err(format!("IPv6 区间起始地址不能大于结束地址：{trimmed}"));
                }
                return Ok(Some(IpRangeMatcher::V6 { start: s, end: e }));
            }
            (Ok(_), Ok(_)) => {
                return Err(format!("IP 区间两端必须同为 IPv4 或 IPv6：{trimmed}"));
            }
            (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                return Err(format!("IP 区间两端必须是有效的 IP 地址：{trimmed}"));
            }
            (Err(_), Err(_)) => {}
        }
    }

    let _ = (Ipv4Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED);
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_and_ip_range_handle_ipv4_ipv6_boundaries_and_non_ip_patterns() {
        let cidr4 = parse_cidr_or_ip_range("192.0.2.0/24").unwrap().unwrap();
        assert!(cidr4.matches_host("192.0.2.0"));
        assert!(cidr4.matches_host("192.0.2.255"));
        assert!(!cidr4.matches_host("192.0.3.1"));
        assert!(!cidr4.matches_host("api.example.invalid"));

        let cidr_zero = parse_cidr_or_ip_range("0.0.0.0/0").unwrap().unwrap();
        assert!(cidr_zero.matches_host("127.0.0.1"));
        assert!(cidr_zero.matches_host("192.0.2.10"));
        assert!(!cidr_zero.matches_host("::1"));

        let cidr_32 = parse_cidr_or_ip_range("192.0.2.42/32").unwrap().unwrap();
        assert!(cidr_32.matches_host("192.0.2.42"));
        assert!(!cidr_32.matches_host("192.0.2.43"));

        let range4 = parse_cidr_or_ip_range("192.0.2.10-192.0.2.20")
            .unwrap()
            .unwrap();
        assert!(range4.matches_host("192.0.2.10"));
        assert!(range4.matches_host("192.0.2.15"));
        assert!(range4.matches_host("192.0.2.20"));
        assert!(!range4.matches_host("192.0.2.9"));
        assert!(!range4.matches_host("192.0.2.21"));

        let cidr6 = parse_cidr_or_ip_range("2001:db8::/64").unwrap().unwrap();
        assert!(cidr6.matches_host("[2001:db8::1]"));
        assert!(!cidr6.matches_host("2001:db8:0:1::1"));

        let cidr6_128 = parse_cidr_or_ip_range("::1/128").unwrap().unwrap();
        assert!(cidr6_128.matches_host("[0:0::1]"));
        assert!(!cidr6_128.matches_host("::2"));

        let range6 = parse_cidr_or_ip_range("[2001:db8::1]-[2001:db8::10]")
            .unwrap()
            .unwrap();
        assert!(range6.matches_host("2001:db8::8"));
        assert!(!range6.matches_host("2001:db8::11"));

        assert!(parse_cidr_or_ip_range("192.0.2.0/33").is_err());
        assert!(parse_cidr_or_ip_range("2001:db8::/129").is_err());
        assert!(parse_cidr_or_ip_range("192.0.2.20-192.0.2.10").is_err());
        assert!(parse_cidr_or_ip_range("192.0.2.1-::1").is_err());
        assert!(parse_cidr_or_ip_range("192.0.2.1-notanip").is_err());

        // Hostnames and regexes with `-` are not mistaken for IP ranges.
        assert_eq!(parse_cidr_or_ip_range("api-dev.example.invalid").unwrap(), None);
        assert_eq!(parse_cidr_or_ip_range("^192\\.0\\.2\\.[0-9]+$").unwrap(), None);
    }
}
