//! Bridge v1 client (corral to host over the generation's Unix socket, `ctl_cap` per request,
//! peer uid checked): hello, get_state, submit_input, cancel, events_since and shutdown.
//! Owner: T-252 child host-bridge (recovery extends reconnect and reconciliation). Until it
//! lands no corral process ever opens a host socket.

/// The T-252 child that lands the bridge client.
pub const OWNER: &str = "host-bridge";
