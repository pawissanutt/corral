use corral_protocol::cmd::CmdState;
use corral_protocol::rpc::ErrorCode;

use super::{Decision, LocalReason, Reply, decide};

fn refused(code: &str) -> Decision {
    Decision::Local(LocalReason::Refused(code.to_owned()))
}

/// Defect: a command the daemon may have accepted also runs locally (double effect), or one it
/// provably did not accept is reported as a daemon result instead of running.
#[test]
fn only_provably_unaccepted_commands_run_locally() {
    let rejected = |code, message: &str| Reply::Rejected(code, message.to_owned());
    let cases = [
        (
            "absent",
            Reply::Absent,
            Decision::Local(LocalReason::Absent),
        ),
        (
            "no capability",
            Reply::NoCapability,
            Decision::Local(LocalReason::NoCapability),
        ),
        (
            "refused",
            rejected(ErrorCode::Refused, "client_mismatch"),
            refused("client_mismatch"),
        ),
        (
            "busy",
            rejected(ErrorCode::Busy, "queue_timeout"),
            refused("queue_timeout"),
        ),
        (
            "expired",
            rejected(ErrorCode::Expired, "expired"),
            refused("expired"),
        ),
        (
            "unknown method",
            rejected(ErrorCode::MethodNotFound, "method not found"),
            refused("method_not_found"),
        ),
        (
            "unavailable",
            rejected(ErrorCode::Unavailable, "unavailable"),
            refused("unavailable"),
        ),
        (
            "too large",
            Reply::TooLarge,
            Decision::Local(LocalReason::TooLarge),
        ),
        (
            "invalid params",
            rejected(ErrorCode::InvalidParams, "cmd_id_reused"),
            Decision::Finish,
        ),
        (
            "unauthenticated",
            rejected(ErrorCode::Unauthenticated, "unauthenticated"),
            Decision::Finish,
        ),
        ("settled", Reply::State(CmdState::Done), Decision::Finish),
        (
            "settled busy",
            Reply::State(CmdState::Busy),
            Decision::Finish,
        ),
        (
            "running",
            Reply::State(CmdState::Running),
            Decision::Requery,
        ),
        (
            "orphaned",
            Reply::State(CmdState::Unknown),
            Decision::Finish,
        ),
        (
            "accepted then lost, tombstoned",
            Reply::State(CmdState::Abandoned),
            Decision::Local(LocalReason::Abandoned),
        ),
        ("accepted then lost", Reply::Lost, Decision::Recover),
        (
            "accepted then lost, never resolved",
            Reply::OutOfTime,
            Decision::Finish,
        ),
    ];
    for (case, reply, expected) in cases {
        assert_eq!(decide(&reply), expected, "{case}");
    }
}
