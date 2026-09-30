use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::git::GitHttpPin;
use std::path::Path;

pub(super) fn validate_git_url(value: &str) -> OperationResult<()> {
    let policy_value = windows_verbatim_prefix(value).unwrap_or(value);
    if policy_value.trim().is_empty()
        || policy_value.starts_with('-')
        || policy_value.chars().any(char::is_control)
        || policy_value.contains('?')
        || policy_value.contains('#')
    {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url is invalid",
        ));
    }
    if url_contains_credentials(value) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url must not contain credentials",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_verbatim_prefix(value: &str) -> Option<&str> {
    value
        .strip_prefix("\\\\?\\")
        .or_else(|| value.strip_prefix("//?/"))
}

#[cfg(not(windows))]
fn windows_verbatim_prefix(_value: &str) -> Option<&str> {
    None
}

/// Registration-time SSRF guard: reject Battery HTTP(S) sources whose host is a
/// **literal** private, loopback, link-local, or cloud-metadata IP. Purely
/// syntactic — no DNS — so it stays hermetic and catches the obvious
/// `https://169.254.169.254`, `https://10.0.0.5`, `https://[::1]` cases without
/// pinning registration to name resolution. The resolving guard runs at fetch
/// time (see [`assert_public_git_host`]). Non-network schemes are skipped.
pub fn assert_git_url_host_public_literal(git_url: &str) -> OperationResult<()> {
    let trimmed = git_url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Ok(());
    }
    let host = git_url_host(trimmed).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url has no host",
        )
    })?;
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip_is_private(ip) {
            return Err(private_git_host_error());
        }
    }
    Ok(())
}

/// Fetch-time SSRF guard used by HTTP policy checks. Battery sync additionally
/// pins Git/curl to the verified address so Git cannot perform a second DNS
/// lookup after this check.
pub fn assert_public_git_host(git_url: &str) -> OperationResult<()> {
    resolve_public_git_endpoint(git_url).map(|_| ())
}

pub(super) fn resolve_public_git_endpoint(git_url: &str) -> OperationResult<Option<GitHttpPin>> {
    use std::net::ToSocketAddrs;

    let trimmed = git_url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Ok(None);
    }
    let (host, port) = git_url_endpoint(trimmed).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url has no host",
        )
    })?;
    let credential_authority = git_url_authority(trimmed).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url has no authority",
        )
    })?;

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip_is_private(ip) {
            return Err(private_git_host_error());
        }
        return Ok(Some(GitHttpPin {
            host,
            port,
            address: ip,
            credential_authority,
        }));
    }

    let resolved = (host.as_str(), port).to_socket_addrs().map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("battery git host did not resolve: {err}"),
        )
    })?;
    let mut pinned = None;
    for addr in resolved {
        if ip_is_private(addr.ip()) {
            return Err(private_git_host_error());
        }
        pinned.get_or_insert(addr.ip());
    }
    let address = pinned.ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git host did not resolve to any address",
        )
    })?;
    Ok(Some(GitHttpPin {
        host,
        port,
        address,
        credential_authority,
    }))
}

fn private_git_host_error() -> OperationError {
    OperationError::new(
        OperationErrorCode::Forbidden,
        "battery git url resolves to a private, loopback, or link-local address; refused to prevent SSRF",
    )
}

/// Extract the bare host from an `scheme://` URL, stripping userinfo and port
/// and unwrapping `[ipv6]` literals.
fn git_url_host(url: &str) -> Option<String> {
    git_url_endpoint(url).map(|(host, _)| host)
}

fn git_url_endpoint(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(after_bracket) = host_port.strip_prefix('[') {
        let (host, suffix) = after_bracket.split_once(']')?;
        let port = match suffix.strip_prefix(':') {
            Some(value) => value.parse().ok()?,
            None if suffix.is_empty() => default_port,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_string(), port));
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (host_port, default_port),
    };
    (!host.is_empty()).then(|| (host.to_string(), port))
}

fn git_url_authority(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    (!authority.is_empty()).then(|| authority.to_string())
}

/// Whether an address is in a private / non-routable / metadata range that a
/// remote caller must never be able to make the node service reach.
fn ip_is_private(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => ipv4_is_private(v4),
        IpAddr::V6(v6) => ipv6_is_private(v6),
    }
}

fn ipv4_is_private(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || o[0] == 0
        || is_carrier_grade_nat(o)
        || is_ietf_protocol_assignment_or_relay_anycast(o)
        || is_benchmarking_range(o)
        // Multicast and reserved/future-use space are never public unicast.
        || o[0] >= 224
        // Azure platform virtual IP is reachable only from tenant networks.
        || o == [168, 63, 129, 16]
}

/// Carrier-grade NAT `100.64.0.0/10`.
fn is_carrier_grade_nat(o: [u8; 4]) -> bool {
    o[0] == 100 && (o[1] & 0xc0) == 64
}

/// IETF protocol assignments (`192.0.0.0/24`) and the deprecated 6to4 relay
/// anycast prefix (`192.88.99.0/24`).
fn is_ietf_protocol_assignment_or_relay_anycast(o: [u8; 4]) -> bool {
    (o[0] == 192 && o[1] == 0 && o[2] == 0) || (o[0] == 192 && o[1] == 88 && o[2] == 99)
}

/// Benchmarking networks (`198.18.0.0/15`) are commonly routed inside
/// infrastructure.
fn is_benchmarking_range(o: [u8; 4]) -> bool {
    o[0] == 198 && matches!(o[1], 18 | 19)
}

fn ipv6_is_private(v6: std::net::Ipv6Addr) -> bool {
    if let Some(mapped) = v6.to_ipv4_mapped() {
        return ipv4_is_private(mapped);
    }
    let seg = v6.segments();
    // Several IPv6 forms embed an IPv4 address that a transition mechanism
    // will actually route to. `to_ipv4_mapped` only unwraps `::ffff:a.b.c.d`,
    // so classify the rest by their embedded IPv4 — otherwise e.g.
    // `[2002:0a00:0005::]` (6to4 → 10.0.0.5) or `[::7f00:1]` (127.0.0.1)
    // would slip through as "public".
    if let Some(embedded) = ipv6_embedded_ipv4(seg) {
        return ipv4_is_private(embedded);
    }
    // NAT64 local-use prefix `64:ff9b:1::/48` (RFC 8215) is local-use only and
    // never names a legitimate public host, so block the whole prefix rather
    // than trying to decode every RFC 6052 embedding.
    if is_nat64_local_use(seg) {
        return true;
    }
    ipv6_is_reserved_or_non_global(v6, seg)
}

fn ipv4_embedded_in_ipv6(hi: u16, lo: u16) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::new(
        (hi >> 8) as u8,
        (hi & 0xff) as u8,
        (lo >> 8) as u8,
        (lo & 0xff) as u8,
    )
}

/// Extract the embedded IPv4 for the transition mechanisms that carry a
/// routable address: IPv4-compatible, NAT64 WKP, 6to4, and Teredo.
fn ipv6_embedded_ipv4(seg: [u16; 8]) -> Option<std::net::Ipv4Addr> {
    if is_ipv4_compatible(seg) || is_nat64_wkp(seg) {
        return Some(ipv4_embedded_in_ipv6(seg[6], seg[7]));
    }
    if is_6to4(seg) {
        return Some(ipv4_embedded_in_ipv6(seg[1], seg[2]));
    }
    if is_teredo(seg) {
        return Some(ipv4_embedded_in_ipv6(!seg[6], !seg[7]));
    }
    None
}

/// IPv4-compatible `::a.b.c.d` (deprecated): embedded IPv4 in the low 32
/// bits, with the all-zero and loopback (`::1`) addresses excluded.
fn is_ipv4_compatible(seg: [u16; 8]) -> bool {
    seg[..6].iter().all(|s| *s == 0) && (seg[6] != 0 || seg[7] > 1)
}

/// NAT64 Well-Known Prefix `64:ff9b::/96`: embedded IPv4 in the low 32 bits.
fn is_nat64_wkp(seg: [u16; 8]) -> bool {
    seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0)
}

/// 6to4 `2002::/16`: embedded IPv4 (the 6to4 gateway) in seg[1..3].
fn is_6to4(seg: [u16; 8]) -> bool {
    seg[0] == 0x2002
}

/// Teredo `2001:0000::/32`: client IPv4 is the bitwise complement of the low
/// 32 bits.
fn is_teredo(seg: [u16; 8]) -> bool {
    seg[0] == 0x2001 && seg[1] == 0x0000
}

fn is_nat64_local_use(seg: [u16; 8]) -> bool {
    seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0x0001
}

fn ipv6_is_reserved_or_non_global(v6: std::net::Ipv6Addr, seg: [u16; 8]) -> bool {
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        // Only global-unicast 2000::/3 is accepted after transition forms.
        || (seg[0] & 0xe000) != 0x2000
        // Documentation, benchmarking, and ORCHID are not public endpoints.
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)
        || (seg[0] == 0x2001 && seg[1] == 0x0002 && seg[2] == 0)
        || (seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0x0010)
        || (seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0x0020)
        // Unique-local fc00::/7
        || (seg[0] & 0xfe00) == 0xfc00
        // Link-local fe80::/10
        || (seg[0] & 0xffc0) == 0xfe80
}

/// Reject local / file Battery sources when deploy policy disallows them.
pub fn assert_local_battery_allowed(allow_local: bool, git_url: &str) -> OperationResult<()> {
    if allow_local {
        return Ok(());
    }
    let lower = git_url.trim().to_ascii_lowercase();
    let is_local = lower.starts_with("file://")
        || Path::new(git_url.trim()).is_absolute()
        || (!lower.contains("://") && !lower.contains('@'));
    if is_local {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy sources.allow_local_batteries=false",
        ));
    }
    Ok(())
}

pub(super) fn normalize_git_url(value: &str) -> OperationResult<String> {
    if windows_verbatim_prefix(value).is_none() {
        if let Some((scheme, _)) = value.split_once("://") {
            let scheme = scheme.to_ascii_lowercase();
            if matches!(scheme.as_str(), "https" | "http" | "file") {
                return Ok(value.to_string());
            }
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "battery git url scheme is not allowed",
            ));
        }
    }

    let local_value = windows_verbatim_path(value);
    let path = Path::new(&local_value);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| {
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to resolve current directory: {err}"),
                )
            })?
            .join(path)
    };
    let canonical = path.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("battery local git source must exist: {err}"),
        )
    })?;
    Ok(strip_windows_verbatim_owned(
        canonical.to_string_lossy().into_owned(),
    ))
}

fn windows_verbatim_path(value: &str) -> String {
    #[cfg(windows)]
    if let Some(stripped) = windows_verbatim_prefix(value) {
        if let Some(unc) = stripped
            .strip_prefix("UNC\\")
            .or_else(|| stripped.strip_prefix("UNC/"))
        {
            return format!("\\\\{unc}");
        }
        return stripped.to_string();
    }
    value.to_string()
}

pub(super) fn strip_windows_verbatim_owned(value: String) -> String {
    #[cfg(windows)]
    if let Some(stripped) = windows_verbatim_prefix(&value) {
        if let Some(unc) = stripped
            .strip_prefix("UNC\\")
            .or_else(|| stripped.strip_prefix("UNC/"))
        {
            return format!("\\\\{unc}");
        }
        return stripped.to_string();
    }
    value
}

pub(super) fn validate_git_ref(value: &str) -> OperationResult<()> {
    if value.trim().is_empty()
        || value.starts_with('-')
        || value.starts_with('+')
        || value.chars().any(|ch| {
            ch.is_control() || ch.is_whitespace() || matches!(ch, ':' | '*' | '?' | '[' | '\\')
        })
        || value.contains("..")
        || value.contains("@{")
    {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery ref is invalid",
        ));
    }
    Ok(())
}

pub(super) fn url_contains_credentials(value: &str) -> bool {
    let Some((_, rest)) = value.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority.contains('@')
}

pub(super) fn redacted_git_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let mut parts = rest.splitn(2, ['/', '?', '#']);
    let authority = parts.next().unwrap_or_default();
    if !authority.contains('@') {
        return value.to_string();
    }
    let suffix = &rest[authority.len()..];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    format!("{scheme}://<redacted>@{host}{suffix}")
}
