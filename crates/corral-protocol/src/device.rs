//! Enrolled app identities and trusted-side browser pairing.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[path = "device_push.rs"]
mod push;
pub use push::*;

pub const DEVICE_V1: &str = "device.v1";
pub const DEVICE_ENROL: &str = "device.enrol";
pub const DEVICE_ATTACH: &str = "device.attach";
pub const DEVICE_LIST: &str = "device.list";
pub const DEVICE_PAGE: &str = "device.page";
pub const DEVICE_PAGE_LIMIT_MAX: u16 = 50;
pub const DEVICE_REVOKE: &str = "device.revoke";
pub const PAIR_BEGIN: &str = "pair.begin";
pub const PAIR_STATUS: &str = "pair.status";
pub const PAIR_CONFIRM: &str = "pair.confirm";
pub const PAIR_CANCEL: &str = "pair.cancel";
pub const PAIR_CLAIM: &str = "pair.claim";
pub const PAIR_DONE: &str = "pair.done";
pub const PAIR_REFUSED: &str = "pair.refused";
pub const SESSION_PATH: &str = "/v1/session";
pub const PAIR_PATH_PREFIX: &str = "/v1/pair/";
pub const PAIR_LINK_FRAGMENT: &str = "corral-pair=";
pub const NOISE_SESSION: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
pub const NOISE_PAIR: &str = "Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
pub const NOISE_TAG: usize = 16;
pub const HANDSHAKE_FIRST_LEN: usize = 96;
pub const CLOSE_REVOKED: &str = "revoked";
pub const CLOSE_DEVICE_UNKNOWN: &str = "device_unknown";
pub const CLOSE_PAIRING_UNAVAILABLE: &str = "pairing_unavailable";
pub const CLOSE_HANDSHAKE_FAILED: &str = "handshake_failed";
pub const CLOSE_PROTOCOL: &str = "protocol_violation";
pub const CLOSE_SESSION_LIFETIME: &str = "session_lifetime";
pub const CLOSE_DAEMON_STOPPING: &str = "daemon_stopping";
pub const CLOSE_PAIRED: &str = "paired";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceReach {
    Unix,
    Loopback,
    Lan,
    Tailscale,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReachHint {
    pub reach: DeviceReach,
    /// WebSocket base URL, without an application path or credentials.
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Native,
    Web,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: DevicePlatform,
    pub app_version: String,
    pub enrolled_at: String,
    pub revoked_at: Option<String>,
    /// Coarse RFC3339 time of authenticated activity, not an online indicator.
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub reach: Option<DeviceReach>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceEnrol {
    pub name: String,
    pub app_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceAttach {
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceList {}

/// App-safe device enumeration without changing the Unix device.list contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePageQuery {
    /// Resume strictly after this canonical device ID.
    pub after: Option<String>,
    /// At most DEVICE_PAGE_LIMIT_MAX; replies also obey APP_CONTROL_MAX.
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePage {
    pub devices: Vec<DeviceInfo>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceRevoke {
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairBegin {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairOffer {
    pub pair_id: String,
    pub code: String,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub reach: Vec<ReachHint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairStatusQuery {
    pub pair_id: String,
    pub wait_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairStatus {
    pub pair_id: String,
    pub state: PairState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairConfirm {
    pub pair_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairCancel {
    pub pair_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairClaim {
    pub name: String,
    pub app_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairClaimed {
    pub sas: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairDone {
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairRefused {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairState {
    Offered {
        expires_at: String,
    },
    Claimed {
        name: String,
        app_version: String,
        sas: String,
        expires_at: String,
    },
    Paired {
        device: DeviceInfo,
    },
    Ended {
        reason: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairCode {
    pub daemon_public: [u8; 32],
    pub token: [u8; 32],
    pub port: u16,
    pub pair_id: [u8; 8],
    pub endpoint: Option<String>,
}

impl PairCode {
    pub fn encode(&self) -> String {
        let mut bytes = Vec::with_capacity(74 + self.endpoint.as_ref().map_or(0, String::len));
        bytes.extend_from_slice(&self.daemon_public);
        bytes.extend_from_slice(&self.token);
        bytes.extend_from_slice(&self.port.to_be_bytes());
        bytes.extend_from_slice(&self.pair_id);
        let version = if let Some(endpoint) = &self.endpoint {
            bytes.extend_from_slice(endpoint.as_bytes());
            2
        } else {
            1
        };
        format!("corral-pair-{version}.{}", URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (encoded, remote) = if let Some(encoded) = text.strip_prefix("corral-pair-1.") {
            (encoded, false)
        } else if let Some(encoded) = text.strip_prefix("corral-pair-2.") {
            (encoded, true)
        } else {
            return Err("not a Corral pairing code".into());
        };
        if encoded.len() > 784 {
            return Err("the pairing code is damaged".into());
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| "the pairing code is damaged")?;
        if (!remote && bytes.len() != 74) || (remote && !(75..=586).contains(&bytes.len())) {
            return Err("the pairing code is damaged".into());
        }
        let endpoint = if remote {
            let endpoint =
                std::str::from_utf8(&bytes[74..]).map_err(|_| "the pairing endpoint is damaged")?;
            if !valid_endpoint(endpoint) {
                return Err("the pairing endpoint is invalid".into());
            }
            Some(endpoint.to_owned())
        } else {
            None
        };
        let mut daemon_public = [0; 32];
        let mut token = [0; 32];
        let mut pair_id = [0; 8];
        daemon_public.copy_from_slice(&bytes[..32]);
        token.copy_from_slice(&bytes[32..64]);
        pair_id.copy_from_slice(&bytes[66..74]);
        Ok(Self {
            daemon_public,
            token,
            port: u16::from_be_bytes([bytes[64], bytes[65]]),
            pair_id,
            endpoint,
        })
    }

    pub fn pair_id_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(16);
        for byte in self.pair_id {
            text.push(HEX[(byte >> 4) as usize] as char);
            text.push(HEX[(byte & 15) as usize] as char);
        }
        text
    }
}

/// A secure browser endpoint, or a local-development WebSocket. Paths are fixed by
/// the protocol so a scanned offer cannot smuggle credentials, queries or redirects.
pub fn valid_endpoint(endpoint: &str) -> bool {
    let (authority, secure) = if let Some(authority) = endpoint.strip_prefix("wss://") {
        (authority, true)
    } else if let Some(authority) = endpoint.strip_prefix("ws://") {
        (authority, false)
    } else {
        return false;
    };
    if endpoint.len() > 512
        || authority.is_empty()
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/?#@\\%".contains(c))
    {
        return false;
    }
    let (host, port) = if authority.starts_with('[') {
        let Some(end) = authority.find(']') else {
            return false;
        };
        if authority[1..end].parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        (&authority[..=end], &authority[end + 1..])
    } else {
        let end = authority.find(':').unwrap_or(authority.len());
        let host = &authority[..end];
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return false;
        }
        (host, &authority[end..])
    };
    if !port.is_empty()
        && port
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .is_none_or(|p| p == 0)
    {
        return false;
    }
    secure || matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

pub fn sas(handshake_hash: &[u8]) -> String {
    let n = handshake_hash.get(..4).map_or(0, |bytes| {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000
    });
    format!("{:03} {:03}", n / 1000, n % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_code_round_trips_and_rejects_damage() {
        let code = PairCode {
            daemon_public: [17; 32],
            token: [29; 32],
            port: 7841,
            pair_id: [0xab; 8],
            endpoint: None,
        };
        let mut remote = code.clone();
        remote.endpoint = Some("wss://workstation.tailnet.ts.net".into());
        assert_eq!(PairCode::decode(&remote.encode()).unwrap(), remote);
        remote.endpoint = Some("ws://192.168.1.2:7841".into());
        assert!(PairCode::decode(&remote.encode()).is_err());
        let text = code.encode();
        assert_eq!(PairCode::decode(&format!(" {text}\n")).unwrap(), code);
        assert_eq!(code.pair_id_hex(), "abababababababab");
        assert_eq!(
            PairCode::decode(&text[..text.len() - 2]).unwrap_err(),
            "the pairing code is damaged"
        );
        assert_eq!(
            PairCode::decode(&text.replacen("corral", "other", 1)).unwrap_err(),
            "not a Corral pairing code"
        );
        assert_eq!(sas(&[0, 0, 4, 210]), "001 234");
        assert_eq!(sas(&[]), "000 000");
    }
}
