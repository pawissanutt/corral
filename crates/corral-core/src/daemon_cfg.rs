//! `[daemon]`: workspace daemon switches.
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// `[daemon]` in `corral.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonCfg {
    /// Whether `corrald` executes owner/agent commands (`cmd.submit`); `false` refuses them,
    /// so every client runs its commands locally.
    #[serde(default = "yes")]
    pub commands: bool,
    /// How the daemon's sync passes skip work. The runtime switch files can only make it
    /// more conservative.
    #[serde(default)]
    pub pass: DaemonPass,
    /// Optional explicit numeric interface; never binds a wildcard address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<ListenAddr>,
    /// Exact browser Origins; listener changes require a daemon restart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web_origins: Vec<WebOrigin>,
    /// Browser-visible TLS WebSocket endpoint when using a reverse proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_endpoint: Option<WebEndpoint>,
    #[serde(default, skip_serializing_if = "PushCfg::is_default")]
    pub push: PushCfg,
}

/// `[daemon] pass`, ordered from least to most conservative.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DaemonPass {
    /// Steps and tasks run only when an input changed or came due.
    #[default]
    Incremental,
    /// Every pass is full and reports the steps an incremental pass would have wrongly skipped.
    Audit,
    /// Every pass is full; nothing is skipped.
    Full,
}

impl DaemonPass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Audit => "audit",
            Self::Full => "full",
        }
    }
}

fn yes() -> bool {
    true
}

impl Default for DaemonCfg {
    fn default() -> Self {
        Self {
            commands: true,
            pass: DaemonPass::Incremental,
            listen: None,
            web_origins: Vec::new(),
            web_endpoint: None,
            push: PushCfg::default(),
        }
    }
}

impl DaemonCfg {
    /// Omitted from serialized configuration while it holds the defaults.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ListenAddr(SocketAddr);

impl ListenAddr {
    pub fn addr(&self) -> SocketAddr {
        self.0
    }
}

impl<'de> Deserialize<'de> for ListenAddr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<SocketAddr>()
            .ok()
            .filter(|addr| {
                !addr.ip().is_unspecified() && !addr.ip().is_multicast()
                    && !matches!(addr.ip(), std::net::IpAddr::V4(ip) if ip.is_broadcast())
            })
            .map(Self)
            .ok_or_else(|| {
                serde::de::Error::custom(
                    "daemon.listen must name one numeric unicast interface, such as \"127.0.0.1:7841\" or \"100.64.0.2:7841\"; wildcard binds and hostnames are refused",
                )
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WebOrigin(String);

impl WebOrigin {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn origin_host(authority: &str) -> Option<&str> {
    if authority.is_empty()
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/?#@\\".contains(c))
    {
        return None;
    }
    let (host, rest) = if let Some(bracketed) = authority.strip_prefix('[') {
        let end = bracketed.find(']')? + 2;
        let host = &authority[..end];
        host[1..host.len() - 1].parse::<std::net::Ipv6Addr>().ok()?;
        (host, &authority[end..])
    } else {
        let end = authority.find(':').unwrap_or(authority.len());
        let host = &authority[..end];
        if !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        {
            return None;
        }
        (host, &authority[end..])
    };
    if host.is_empty() {
        return None;
    }
    if !rest.is_empty() {
        let port = rest.strip_prefix(':')?;
        if port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || port.parse::<u16>().ok()? == 0
        {
            return None;
        }
    }
    Some(host)
}

impl<'de> Deserialize<'de> for WebOrigin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let valid = if let Some(authority) = text.strip_prefix("https://") {
            origin_host(authority).is_some()
        } else if let Some(authority) = text.strip_prefix("http://") {
            matches!(
                origin_host(authority),
                Some("127.0.0.1" | "localhost" | "[::1]")
            )
        } else {
            false
        };
        if valid {
            Ok(Self(text))
        } else {
            Err(serde::de::Error::custom(
                "daemon.web_origins entries must be \"https://host[:port]\" or \"http://127.0.0.1[:port]\", \"http://localhost[:port]\" or \"http://[::1][:port]\" with no path",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WebEndpoint(String);

impl WebEndpoint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for WebEndpoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let origin = text
            .strip_prefix("wss://")
            .map(|s| format!("https://{s}"))
            .or_else(|| text.strip_prefix("ws://").map(|s| format!("http://{s}")))
            .ok_or_else(|| {
                serde::de::Error::custom(
                    "daemon.web_endpoint must use wss:// (ws:// is loopback-only)",
                )
            })?;
        let _: WebOrigin = WebOrigin::deserialize(
            serde::de::value::StringDeserializer::<D::Error>::new(origin),
        )?;
        if text.len() > 512 {
            return Err(serde::de::Error::custom(
                "daemon.web_endpoint exceeds 512 bytes",
            ));
        }
        Ok(Self(text))
    }
}

/// Web Push is off until a private VAPID PEM and explicit push-service origins are set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushCfg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vapid_private_key: Option<std::path::PathBuf>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub origins: Vec<WebOrigin>,
}

impl PushCfg {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listeners_are_explicit_and_browser_origins_stay_secure() {
        for value in [
            "0.0.0.0:7841",
            "localhost:7841",
            "[::]:7841",
            "224.0.0.1:7841",
            "255.255.255.255:7841",
        ] {
            assert!(serde_json::from_value::<ListenAddr>(serde_json::json!(value)).is_err());
        }
        assert!(DaemonCfg::default().listen.is_none());
        for value in [
            "127.0.0.1:7841",
            "127.0.0.1:0",
            "[::1]:7841",
            "192.168.1.2:7841",
            "100.64.0.2:7841",
        ] {
            let addr: ListenAddr = serde_json::from_value(serde_json::json!(value)).unwrap();
            assert_eq!(addr.addr().to_string(), value);
        }
        for value in [
            "http://192.168.1.2:8790",
            "http://127.0.0.1:8790/",
            "null",
            "https://",
            "https://user@host",
            "https://host:0",
            "https://host:65536",
            "https://host?q",
            "https://host#x",
        ] {
            assert!(
                serde_json::from_value::<WebOrigin>(serde_json::json!(value)).is_err(),
                "{value}"
            );
        }
        for value in [
            "http://127.0.0.1:8790",
            "https://corral.example",
            "http://localhost",
            "http://[::1]:8790",
            "https://corral.example:443",
        ] {
            let origin: WebOrigin = serde_json::from_value(serde_json::json!(value)).unwrap();
            assert_eq!(origin.as_str(), value);
        }
        for endpoint in [
            "ws://192.168.1.2:7841",
            "wss://host/path",
            "wss://host?query",
            "wss://user@host",
        ] {
            assert!(serde_json::from_value::<WebEndpoint>(serde_json::json!(endpoint)).is_err());
        }
        for endpoint in [
            "ws://127.0.0.1:7841",
            "wss://workstation.tailnet.ts.net",
            "wss://corral.example:443",
        ] {
            assert!(serde_json::from_value::<WebEndpoint>(serde_json::json!(endpoint)).is_ok());
        }
    }
}
