use corral_types::{Next, Op, Refusal, Status};
use serde_json::json;

use super::*;
use crate::rows::TaskRow;
use crate::rpc::{self, Response, ResponseOutcome};
use crate::subscription::{BoardCursor, RowsDelta, SubDelta};

#[test]
fn op_submit_round_trips_with_both_force_values_and_defaults_to_false() {
    let op = Op::Move {
        id: "T-42".into(),
        status: Status::Ready,
    };
    let mut wire = json!({"op": {"move": {"id": "T-42", "status": "ready"}}});
    assert_eq!(
        serde_json::from_value::<OpSubmit>(wire.clone()).unwrap(),
        OpSubmit {
            op: op.clone(),
            force: false,
        }
    );
    for force in [false, true] {
        wire["force"] = json!(force);
        let submit = OpSubmit {
            op: op.clone(),
            force,
        };
        assert_eq!(serde_json::to_value(&submit).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<OpSubmit>(wire.clone()).unwrap(),
            submit
        );
    }
}

#[test]
fn refused_response_keeps_both_next_operations_in_order() {
    let refusal = Refusal {
        reason: "dependency not done".into(),
        next: vec![
            Next::run(Op::Move {
                id: "T-41".into(),
                status: Status::Ready,
            }),
            Next::force(Op::Dispatch {
                id: "T-42".into(),
                profile: None,
                base: None,
            }),
        ],
        problems: None,
    };
    let response = Response::<OpReply> {
        jsonrpc: rpc::JSONRPC.into(),
        id: json!(1),
        outcome: ResponseOutcome::Error {
            error: rpc::Error::refused(refusal.clone()).unwrap(),
        },
    };
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(wire["error"]["code"], "refused");
    assert_eq!(wire["error"]["message"], refusal.reason);
    let result: Response<OpReply> = serde_json::from_value(wire).unwrap();
    let ResponseOutcome::Error { error } = result.outcome else {
        panic!("refusal decoded as success");
    };
    assert_eq!(
        serde_json::from_value::<Refusal>(error.data.unwrap()).unwrap(),
        refusal
    );
}

#[test]
fn blocked_task_survives_a_tasks_delta_with_its_next_operation() {
    let task = serde_json::from_value(json!({
        "id": "T-42", "title": "Blocked task", "body": "Goal", "status": "blocked",
        "project": null, "priority": "P2", "kind": null, "size": null, "profile": null,
        "base": null, "parent": null, "depends_on": [], "round": 0,
        "blocked": {"reason": "stopped by owner", "detail": "log tail", "next": [
            {"op": {"move": {"id": "T-42", "status": "backlog"}}, "force": true}
        ]},
        "created": "2026-11-17T00:00:00Z", "updated": "2026-11-17T00:00:00Z"
    }))
    .unwrap();
    let delta = SubDelta::Tasks {
        delta: RowsDelta {
            epoch: 1,
            rows: vec![TaskRow {
                task,
                ..Default::default()
            }],
            left: vec![],
            cursor: BoardCursor::default(),
            more: false,
        },
    };
    let wire = serde_json::to_value(&delta).unwrap();
    assert_eq!(wire["topic"], "tasks");
    let task = &wire["rows"][0]["task"];
    assert_eq!(task["status"], "blocked");
    assert_eq!(task["blocked"]["detail"], "log tail");
    assert_eq!(
        task["blocked"]["next"],
        json!([{"op": {"move": {"id": "T-42", "status": "backlog"}}, "force": true}])
    );
    assert_eq!(serde_json::from_value::<SubDelta>(wire).unwrap(), delta);
}
