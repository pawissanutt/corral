use super::*;
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, recv, recvmsg,
};
use std::{io::IoSliceMut, mem::MaybeUninit, thread};

fn example() -> (Enrollment, wire::Binding, wire::Limits) {
    (
        Enrollment {
            link_id: "test-link".into(),
            lite_instance: "test-lite".into(),
            corral_instance: "test-corral".into(),
            channel_epoch: "test-epoch".into(),
            prior_channel_epoch: None,
            profile_sha256: "a".repeat(64),
            corral_manifest_sha256: "b".repeat(64),
            joint_lock_sha256: "c".repeat(64),
        },
        wire::Binding {
            request_id: "test-request".into(),
            request_digest: "d".repeat(64),
            run_id: "test-run".into(),
            generation: 2,
            grant_id: "test-grant".into(),
            grant_generation: 1,
            policy_ref: wire::POLICY_REF.into(),
        },
        wire::Limits {
            calls: 8,
            response_bytes: 8192,
            total_response_bytes: 32768,
            turns: 4,
            run_cost_microusd: 4000,
        },
    )
}

fn setup_requests() -> [Value; 3] {
    [
        protocol::initialize(),
        protocol::initialized(),
        protocol::list(),
    ]
}

fn rpc_reply(id: &Value, payload: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"content":[],"isError":false,"structuredContent":payload}})
}

fn wrapped_reply(request: &Value, payload: Value) -> Value {
    json!({"run_id":request["run_id"],"generation":request["generation"],"invocation_id":request["invocation_id"],
        "response":rpc_reply(&request["request"]["id"], payload)})
}

#[test]
fn rejects_foreign_binding_invocation_and_inner_id() {
    let (enrollment, binding, _) = example();
    let bound = BindingRecord::from_bound(&enrollment, &binding).unwrap();
    let call = Call::new(
        3,
        "search_materials",
        &json!({"query":"FEBRUARY","limit":2}),
        false,
    )
    .unwrap();
    let arm = Arm::new(&call).unwrap();
    let request = arm.request(&bound, &call).unwrap();
    assert_eq!(
        request["args_digest"],
        "f3b97609137e25f604dd0d9bce1dac3e3a03a73107b593017b8678a1d0a89d61"
    );
    let reply = wrapped_reply(&request, json!({"status":"ok","hits":[]}));
    assert!(arm.reply(&bound, &call, reply.clone()).is_ok());
    for (field, changed) in [
        ("run_id", json!("foreign")),
        ("generation", json!(3)),
        ("invocation_id", json!("wrong")),
    ] {
        let mut foreign = reply.clone();
        foreign[field] = changed;
        assert!(arm.reply(&bound, &call, foreign).is_err());
    }
    let mut wrong = reply;
    wrong["response"]["id"] = 2.into();
    assert!(arm.reply(&bound, &call, wrong).is_err());
    let mut bad_binding = bound.record(BindingKind::ShimBind).unwrap();
    bad_binding["channel_epoch"] = "wrong/epoch".into();
    assert!(BindingRecord::parse(bad_binding, BindingKind::ShimBind).is_err());
    assert!(wire::decode_signed::<Value>(br#"{"a":1,"a":2}"#).is_err());
}

#[test]
fn validates_pending_owner_handle_and_signed_terminal_metric() {
    let args = json!({"result":"matching"});
    let call = Call::new(4, "read_result", &args, true).unwrap();
    assert!(Call::new(4, "read_result", &args, false).is_err());
    assert!(
        protocol::structured_reply(
            &call,
            rpc_reply(&json!(4), json!({"status":"pending","result":"other"}))
        )
        .is_err()
    );
    let metric = json!({"status":"ok","result":"matching","value_minor":-17,"unit":"fixture","limitations":"", "operation_version":"version","cell":"some_cell","result_kind":"captured","citation":"some_citation"});
    let response = rpc_reply(&json!(4), metric.clone());
    let decoded = wire::decode_signed(&serde_json::to_vec(&response).unwrap()).unwrap();
    assert_eq!(protocol::structured_reply(&call, decoded).unwrap(), metric);
}

#[test]
fn no_stdio_call_until_exact_arm_ack() {
    for mutate in [false, true] {
        let (enrollment, binding, limits) = example();
        let bound = BindingRecord::from_bound(&enrollment, &binding).unwrap();
        let (stdio, child) = UnixStream::pair().unwrap();
        let (control, shim) = io::socketpair().unwrap();
        let (host_poll, _lite_poll) = io::socketpair().unwrap();
        let mut broker = Broker {
            binding: bound,
            pipes: Some(Pipes {
                stdio,
                control,
                host_poll,
                lines: Lines::default(),
            }),
            next_id: 3,
            startup_step: 3,
            budget: None,
        };
        broker.set_limits(&limits).unwrap();
        thread::scope(|s| {
            let child = s.spawn(move || {
                let mut none = || Ok(());
                let mut scope = Scope {
                    deadline: Instant::now() + Duration::from_secs(2),
                    pump: &mut none,
                };
                let arm = Arm::parse(
                    scope.recv_private(shim.as_fd(), &[child.as_fd()]).unwrap(),
                    2,
                )
                .unwrap();
                io::quiet(&[child.as_fd()]).unwrap();
                if mutate {
                    let mut ack = arm.ack().unwrap();
                    ack["id"] = 99.into();
                    scope
                        .send_private(shim.as_fd(), &ack, &[child.as_fd()])
                        .unwrap();
                }
                child
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                assert_no_bytes(child.as_fd());
            });
            let mut none = || Ok(());
            let deadline = Instant::now() + Duration::from_millis(if mutate { 1000 } else { 150 });
            assert!(
                broker
                    .invoke(
                        "search_materials",
                        &json!({"query":"february","limit":1}),
                        deadline,
                        &mut none
                    )
                    .is_err()
            );
            assert!(broker.check_live().is_err());
            child.join().unwrap();
        });
    }
}

#[test]
fn shim_rejects_unarmed_or_mismatched_call_without_forward() {
    for arm_call in [false, true] {
        let (stdio, child) = UnixStream::pair().unwrap();
        let (evidence, lite) = io::socketpair().unwrap();
        let (control, shim_control) = io::socketpair().unwrap();
        let (enrollment, binding, _) = example();
        let bound = BindingRecord::from_bound(&enrollment, &binding).unwrap();
        thread::scope(|s| {
            let shim = s.spawn(move || super::shim::run(child.as_fd()));
            let mut none = || Ok(());
            let mut scope = Scope {
                deadline: Instant::now() + Duration::from_secs(2),
                pump: &mut none,
            };
            scope
                .send_handoff(
                    stdio.as_fd(),
                    &[evidence.as_fd(), shim_control.as_fd()],
                    &[],
                )
                .unwrap();
            drop((evidence, shim_control));
            scope
                .send_private(
                    control.as_fd(),
                    &bound.record(BindingKind::ShimBind).unwrap(),
                    &[stdio.as_fd()],
                )
                .unwrap();
            assert_eq!(
                scope
                    .recv_private(control.as_fd(), &[stdio.as_fd()])
                    .unwrap(),
                bound.record(BindingKind::ShimBound).unwrap()
            );
            let mut lines = Lines::default();
            for (step, request) in setup_requests().into_iter().enumerate() {
                relay_shim_phase(
                    &mut scope,
                    stdio.as_fd(),
                    &mut lines,
                    &[control.as_fd()],
                    step,
                    &request,
                )
                .unwrap();
            }
            let call = Call::new(
                3,
                "search_materials",
                &json!({"query":"original","limit":1}),
                false,
            )
            .unwrap();
            if arm_call {
                let arm = Arm::new(&call).unwrap();
                scope
                    .send_private(
                        control.as_fd(),
                        &serde_json::to_value(arm).unwrap(),
                        &[stdio.as_fd()],
                    )
                    .unwrap();
                scope
                    .recv_private(control.as_fd(), &[stdio.as_fd()])
                    .unwrap();
            }
            let changed = Call::new(
                3,
                "search_materials",
                &json!({"query":"changed","limit":1}),
                false,
            )
            .unwrap();
            scope
                .send_line(
                    stdio.as_fd(),
                    &serde_json::to_value(changed).unwrap(),
                    &[control.as_fd()],
                )
                .unwrap();
            assert!(shim.join().unwrap().is_err());
            assert_no_bytes(lite.as_fd());
        });
    }
}

#[test]
fn attach_negotiates_evidence_first_and_only_releases_terminal_poll_payload() {
    let (stdio, child) = UnixStream::pair().unwrap();
    let (bootstrap, lite_boot) = io::socketpair().unwrap();
    let (enrollment, binding, limits) = example();
    let bound = BindingRecord::from_bound(&enrollment, &binding).unwrap();
    let payload = json!({"status":"ok","result":"run-result","value_minor":-17,"unit":"fixture","limitations":"", "operation_version":"version","cell":"some_cell","result_kind":"captured","citation":"some_citation"});
    thread::scope(|s| {
        let shim = s.spawn(move || super::shim::run(child.as_fd()));
        let (close_evidence, wait) = std::sync::mpsc::channel();
        let (closed, observe_closed) = std::sync::mpsc::channel();
        let terminal = payload.clone();
        let lite = s.spawn(move || {
            let mut none = || Ok(());
            let mut scope = Scope {
                deadline: Instant::now() + Duration::from_secs(5),
                pump: &mut none,
            };
            let [evidence, poll] = receive_attach(
                lite_boot.as_fd(),
                &bound.record(BindingKind::BrokerAttach).unwrap(),
            );
            scope
                .send_packet(
                    lite_boot.as_fd(),
                    &bound.record(BindingKind::BrokerAttached).unwrap(),
                    &[],
                    &[],
                )
                .unwrap();
            lite_startup(&mut scope, evidence.as_fd(), false, &[poll.as_fd()]);
            lite_startup(&mut scope, poll.as_fd(), true, &[evidence.as_fd()]);
            let request = scope
                .recv_packet(evidence.as_fd(), &[poll.as_fd()])
                .unwrap();
            assert_eq!(request["request"]["params"]["name"], "calculate_metric");
            let before = Instant::now();
            scope
                .send_packet(
                    evidence.as_fd(),
                    &wrapped_reply(&request, json!({"status":"pending","result":"run-result"})),
                    &[],
                    &[poll.as_fd()],
                )
                .unwrap();
            let request = scope
                .recv_packet(poll.as_fd(), &[evidence.as_fd()])
                .unwrap();
            assert!(before.elapsed() >= MIN_POLL_INTERVAL);
            assert_eq!(
                request["request"]["params"],
                json!({"name":"read_result","arguments":{"result":"run-result"}})
            );
            let before = Instant::now();
            scope
                .send_packet(
                    poll.as_fd(),
                    &wrapped_reply(&request, json!({"status":"pending","result":"run-result"})),
                    &[],
                    &[evidence.as_fd()],
                )
                .unwrap();
            let next = scope
                .recv_packet(poll.as_fd(), &[evidence.as_fd()])
                .unwrap();
            assert!(before.elapsed() >= MIN_POLL_INTERVAL);
            assert_eq!(next["request"]["params"], request["request"]["params"]);
            assert_ne!(next["request"]["id"], request["request"]["id"]);
            assert_ne!(next["invocation_id"], request["invocation_id"]);
            scope
                .send_packet(
                    poll.as_fd(),
                    &wrapped_reply(&next, terminal),
                    &[],
                    &[evidence.as_fd()],
                )
                .unwrap();
            wait.recv_timeout(Duration::from_secs(4)).unwrap();
            drop(evidence);
            closed.send(()).unwrap();
            rustix::net::sockopt::set_socket_timeout(
                &poll,
                rustix::net::sockopt::Timeout::Recv,
                Some(Duration::from_secs(2)),
            )
            .unwrap();
            // Keep the poll capability live so it cannot explain the failed evidence observation.
            assert_no_bytes(poll.as_fd());
        });
        let mut pump_steps = 0;
        let mut pump = || {
            pump_steps += 1;
            Ok(())
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut broker = Broker::attach(
            &bootstrap,
            &enrollment,
            &binding,
            stdio,
            deadline,
            &mut pump,
        )
        .unwrap();
        assert!(broker.set_limits(&limits).is_err());
        let replies = setup_requests().map(|request| {
            broker
                .negotiate_stdio(&request, deadline, &mut pump)
                .unwrap()
        });
        assert_eq!(
            replies,
            [
                Some(protocol::server_info(protocol::SHIM_SERVER)),
                None,
                Some(protocol::listing(false).unwrap())
            ]
        );
        broker.set_limits(&limits).unwrap();
        let result = broker
            .invoke(
                "calculate_metric",
                &json!({"metric":"selector","inputs":["live-source"]}),
                deadline,
                &mut pump,
            )
            .unwrap();
        assert_eq!(result, payload);
        assert!(pump_steps > 2);
        let budget = broker.budget.as_ref().unwrap();
        assert_eq!(budget.calls_left, 5);
        assert_eq!(
            budget.total_left,
            32768
                - (wire::canonical_signed(&payload).unwrap().len()
                    + 2 * wire::canonical_signed(
                        &json!({"status":"pending","result":"run-result"})
                    )
                    .unwrap()
                    .len()) as u64
        );
        assert!(broker.check_live().is_ok());
        let mut none = || Ok(());
        broker.prove_live(deadline, &mut none).unwrap();
        close_evidence.send(()).unwrap();
        observe_closed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(broker.prove_live(deadline, &mut none).is_err());
        drop(broker);
        lite.join().unwrap();
        assert!(shim.join().unwrap().is_err());
    });
}

fn receive_attach(fd: BorrowedFd<'_>, expected: &Value) -> [OwnedFd; 2] {
    rustix::net::sockopt::set_socket_timeout(
        fd,
        rustix::net::sockopt::Timeout::Recv,
        Some(Duration::from_secs(2)),
    )
    .unwrap();
    let mut bytes = [0; 4096];
    let mut area = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
    let mut control = RecvAncillaryBuffer::new(&mut area);
    let msg = recvmsg(
        fd,
        &mut [IoSliceMut::new(&mut bytes)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC,
    )
    .unwrap();
    assert!(
        !msg.flags
            .intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC)
    );
    assert_eq!(
        &wire::decode::<Value>(&bytes[..msg.bytes]).unwrap(),
        expected
    );
    let mut anc = control.drain();
    let Some(RecvAncillaryMessage::ScmRights(rights)) = anc.next() else {
        panic!("missing Lite endpoints");
    };
    let rights: [OwnedFd; 2] = rights.collect::<Vec<_>>().try_into().unwrap();
    assert!(anc.next().is_none());
    assert_ne!(
        io::socket_identity(rights[0].as_fd(), SocketType::SEQPACKET).unwrap(),
        io::socket_identity(rights[1].as_fd(), SocketType::SEQPACKET).unwrap()
    );
    rights
}

fn lite_startup(
    scope: &mut Scope<'_>,
    fd: BorrowedFd<'_>,
    host_poll: bool,
    idle: &[BorrowedFd<'_>],
) {
    assert_eq!(scope.recv_packet(fd, idle).unwrap(), protocol::initialize());
    scope
        .send_packet(fd, &protocol::server_info(protocol::LITE_SERVER), &[], idle)
        .unwrap();
    assert_eq!(
        scope.recv_packet(fd, idle).unwrap(),
        protocol::initialized()
    );
    assert_eq!(scope.recv_packet(fd, idle).unwrap(), protocol::list());
    scope
        .send_packet(fd, &protocol::listing(host_poll).unwrap(), &[], idle)
        .unwrap();
}

fn assert_no_bytes(fd: BorrowedFd<'_>) {
    let mut buf = [0u8; 64];
    let (_bytes, len) = recv(fd, &mut buf[..], RecvFlags::empty()).unwrap();
    assert_eq!(len, 0, "unexpected bytes forwarded");
}
