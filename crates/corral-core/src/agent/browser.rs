//! Browser capability settings (T-574): validated dev origins recorded as the exact WHATWG
//! `URL.origin` the Pi host compares, a per-call timeout and an optional executable.
use super::{AgentRole, CapabilitySettings};
use crate::config::agent_capabilities::validate_http_endpoint;

/// Browser: exact `http(s)://host[:port]` dev `origins` (default none: worktree HTML only), a
/// `timeout_ms` (default 30000) bounding browser start-up and then the page separately, and an
/// optional absolute Chrome/Chromium `executable`.
pub(crate) fn browser_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let mut effective = CapabilitySettings::new();
    let mut origins = Vec::new();
    let mut timeout_ms = 30_000;
    for (name, value) in table {
        match name.as_str() {
            "origins" => {
                for origin in value
                    .as_array()
                    .ok_or("origins must be a list of origins")?
                {
                    let lower = origin.as_str().unwrap_or_default().to_ascii_lowercase();
                    let text = lower.trim_end_matches('/');
                    validate_http_endpoint(text)
                        .map_err(|error| format!("origins entry {origin} {error}"))?;
                    if text.splitn(4, '/').nth(3).is_some() || text.contains('?') {
                        return Err(format!(
                            "origins entry {origin} must be scheme://host[:port] without a path"
                        ));
                    }
                    origins.push(serde_json::Value::from(canonical_origin(text)));
                }
            }
            "timeout_ms" => match value.as_u64() {
                Some(ms @ 1..=600_000) => timeout_ms = ms,
                _ => return Err("timeout_ms must be between 1 and 600000".into()),
            },
            "executable" => {
                let path = value
                    .as_str()
                    .filter(|path| path.starts_with('/') && !path.contains('\0'))
                    .ok_or("executable must be an absolute path")?;
                effective.insert(name.clone(), path.into());
            }
            other => {
                return Err(format!(
                    "has no setting {other} (known: origins, timeout_ms, executable)"
                ));
            }
        }
    }
    origins.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    origins.dedup();
    effective.insert("origins".into(), origins.into());
    effective.insert("timeout_ms".into(), timeout_ms.into());
    Ok(effective)
}

/// The WHATWG `URL.origin` serialization of a validated, lowercase `http(s)://host[:port]`: no
/// default or zero-padded port and compressed IPv6, so the host's origin comparison matches.
pub fn canonical_origin(origin: &str) -> String {
    let (scheme, authority) = origin.split_once("://").unwrap_or(("http", origin));
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => {
            let (ip, tail) = rest.split_once(']').unwrap_or((rest, ""));
            let host = ip
                .parse::<std::net::Ipv6Addr>()
                .map_or_else(|_| ip.to_owned(), whatwg_ipv6);
            (format!("[{host}]"), tail.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (authority.to_owned(), None),
        },
    };
    let default = if scheme == "https" { 443 } else { 80 };
    match port.and_then(|port| port.parse::<u16>().ok()) {
        Some(port) if port != default => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    }
}

/// Lowercase hex pieces with the first longest run of two or more zero pieces compressed.
fn whatwg_ipv6(ip: std::net::Ipv6Addr) -> String {
    let pieces = ip.segments();
    let (mut best, mut run) = ((0, 0), (0, 0));
    for (at, piece) in pieces.iter().enumerate() {
        run = if *piece == 0 {
            (if run.1 == 0 { at } else { run.0 }, run.1 + 1)
        } else {
            (0, 0)
        };
        if run.1 > best.1 {
            best = run;
        }
    }
    let hex = |part: &[u16]| {
        part.iter()
            .map(|p| format!("{p:x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    if best.1 < 2 {
        return hex(&pieces);
    }
    format!(
        "{}::{}",
        hex(&pieces[..best.0]),
        hex(&pieces[best.0 + best.1..])
    )
}

/// Worker-only: off in strict, on in standard and full; unavailable to every other role.
pub(super) fn browser_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker => Some([false, true, true]),
        AgentRole::Assistant => Some([false, false, true]),
        AgentRole::Reviewer | AgentRole::Pm | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}
