use corral_core::extension::*;
use serde_json::{Value, json};
use std::io::{self, Cursor as IoCursor, Read};

const VALID: &str = r#"
manifest_version = 1
id = "mitigraph"
version = "0.1.0"
executable = "corral-ext-mitigraph"
protocol_versions = [1]
execution_modes = ["oneshot"]
entry = ["extension", "invoke"]
concurrency = 1

[[providers]]
name = "task.context"
versions = [1]
reads = ["read.batch", "read.changes", "read.snapshot"]

[config_schema]
type = "object"
"#;

fn manifest_err(text: &str) -> ManifestError {
    Manifest::parse(text).expect_err("manifest must be rejected")
}

#[test]
fn valid_manifest_parses_with_no_activation_blockers() {
    let m = Manifest::parse(VALID).expect("valid manifest");
    assert_eq!(m.id, "mitigraph");
    assert_eq!(m.execution_modes, vec![ExecutionMode::Oneshot]);
    assert_eq!(m.entry, vec!["extension", "invoke"]);
    assert_eq!(
        m.provider("task.context").map(|p| p.versions.clone()),
        Some(vec![1])
    );
    assert_eq!(
        m.config_schema.get("type").and_then(|v| v.as_str()),
        Some("object")
    );
    assert!(m.activation_blockers().is_empty());
}

#[test]
fn unknown_keys_are_rejected_with_their_field_path() {
    let top = manifest_err(&format!("bogus = 1\n{VALID}"));
    assert_eq!(top.field, "bogus");
    let schema_keys_are_free = format!("{VALID}\nproperties = {{}}\n");
    assert!(Manifest::parse(&schema_keys_are_free).is_ok());
    let nested = VALID.replace(
        "versions = [1]\nreads",
        "versions = [1]\ncolor = \"red\"\nreads",
    );
    assert_eq!(manifest_err(&nested).field, "providers[0].color");
    let missing = VALID.replace("executable = \"corral-ext-mitigraph\"\n", "");
    assert_eq!(manifest_err(&missing).field, "executable");
}

#[test]
fn bad_values_are_rejected_with_their_field_name() {
    for bad in ["Mitigraph", "9lives", "has_underscore", "", &"a".repeat(33)] {
        let text = VALID.replace("id = \"mitigraph\"", &format!("id = \"{bad}\""));
        assert_eq!(manifest_err(&text).field, "id", "id {bad:?}");
    }
    assert!(
        Manifest::parse(&VALID.replace(
            "id = \"mitigraph\"",
            &format!("id = \"{}\"", "a".repeat(32))
        ))
        .is_ok()
    );
    let typed = VALID.replace("concurrency = 1", "concurrency = \"one\"");
    assert_eq!(manifest_err(&typed).field, "concurrency");
    let read = VALID.replace("\"read.snapshot\"]", "\"read.everything\"]");
    assert_eq!(manifest_err(&read).field, "providers[0].reads[2]");
    let version = VALID.replace("version = \"0.1.0\"", "version = \"one\"");
    assert_eq!(manifest_err(&version).field, "version");
    let err = manifest_err(&VALID.replace("manifest_version = 1", "manifest_version = 2"));
    assert_eq!(err.field, "manifest_version");
    let ext = err.to_ext_error();
    assert_eq!(ext.code, ErrorCode::InvalidRequest);
    assert_eq!(ext.data, Some(json!({"field": "manifest_version"})));
}

#[test]
fn subscriptions_and_actions_parse_but_are_not_activatable() {
    let text = format!(
        "{VALID}\n[[subscriptions]]\nname = \"tasks\"\ncursor_kind = \"event_seq\"\nfilter = [\"created\", \"moved\"]\n\n[[requested_actions]]\naction = \"task.create\"\n"
    );
    let m = Manifest::parse(&text).expect("subscriptions parse");
    assert_eq!(m.subscriptions[0].cursor_kind, CursorKind::EventSeq);
    let blockers: Vec<_> = m
        .activation_blockers()
        .into_iter()
        .map(|b| (b.field, b.stage))
        .collect();
    assert_eq!(
        blockers,
        vec![("subscriptions", 2), ("requested_actions", 3)]
    );
    let bad_filter = text.replace("[\"created\", \"moved\"]", "[\"created\", \"task\"]");
    assert_eq!(
        manifest_err(&bad_filter).field,
        "subscriptions[0].filter[1]"
    );
    let unknown = text.replace(
        "cursor_kind = \"event_seq\"",
        "cursor_kind = \"event_seq\"\nreplay = true",
    );
    assert_eq!(manifest_err(&unknown).field, "subscriptions[0].replay");
}

fn offer(caps: &[(&str, &[u32])]) -> InitializeParams {
    InitializeParams {
        protocol_versions: vec![1],
        capabilities: caps
            .iter()
            .map(|(name, versions)| CapabilityOffer {
                name: (*name).into(),
                versions: versions.to_vec(),
            })
            .collect(),
        host: HostInfo {
            corral_version: "0.3.0".into(),
            workspace_id: "ws".into(),
        },
        limits: Value::Null,
    }
}

fn chose(version: u32, caps: &[(&str, u32)]) -> InitializeResult {
    InitializeResult {
        protocol_version: version,
        capabilities: caps
            .iter()
            .map(|(n, v)| CapabilityChoice {
                name: (*n).into(),
                version: *v,
            })
            .collect(),
        unsupported: vec![],
    }
}

#[test]
fn initialize_result_is_checked_against_manifest_and_offer() {
    let m = Manifest::parse(VALID).expect("valid manifest");
    let host = offer(&[("task.context", &[1]), ("task.summary", &[1])]);
    assert!(
        m.check_initialize(&host, &chose(1, &[("task.context", 1)]))
            .is_ok()
    );
    let undeclared = m
        .check_initialize(&host, &chose(1, &[("task.summary", 1)]))
        .unwrap_err();
    assert_eq!(undeclared.code, ErrorCode::ManifestMismatch);
    assert_eq!(
        undeclared.data,
        Some(json!({"capability": "task.summary@1"}))
    );
    let wrong_version = m
        .check_initialize(&host, &chose(1, &[("task.context", 2)]))
        .unwrap_err();
    assert_eq!(wrong_version.code, ErrorCode::ManifestMismatch);
    let not_offered = m
        .check_initialize(&offer(&[]), &chose(1, &[("task.context", 1)]))
        .unwrap_err();
    assert_eq!(not_offered.code, ErrorCode::UnsupportedCapability);
    let protocol = m.check_initialize(&host, &chose(2, &[])).unwrap_err();
    assert_eq!(protocol.code, ErrorCode::UnsupportedProtocol);
    let twice = m
        .check_initialize(
            &host,
            &chose(1, &[("task.context", 1), ("task.context", 1)]),
        )
        .unwrap_err();
    assert_eq!(twice.reason(), Some("duplicate_capability"));
}

struct Counting<R> {
    inner: R,
    taken: usize,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.taken += n;
        Ok(n)
    }
}

fn violation(bytes: &[u8]) -> ExtError {
    let mut reader = FrameReader::new(bytes);
    loop {
        match reader.read_frame() {
            Ok(Some(_)) => {}
            Ok(None) => return reader.finish().expect_err("stream must be rejected"),
            Err(e) => return e,
        }
    }
}

fn assert_violation(bytes: &[u8], reason: &str) {
    let err = violation(bytes);
    assert_eq!(err.code, ErrorCode::ProtocolViolation, "{err}");
    assert_eq!(err.reason(), Some(reason), "{err}");
    assert!(!err.detail.is_empty());
}

#[test]
fn frames_round_trip_and_end_cleanly() {
    let mut wire = Vec::new();
    let env = Envelope::new(
        "A-1",
        Body::Response {
            result: json!({"ok": true}),
        },
    );
    write_envelope(&mut wire, &env).expect("write");
    write_frame(&mut wire, "{}", MAX_FRAME_BYTES).expect("write");
    assert!(wire.starts_with(b"Content-Length: "));
    let mut reader = FrameReader::new(wire.as_slice());
    assert_eq!(reader.read_envelope().expect("read"), Some(env));
    assert_eq!(reader.read_frame().expect("read").as_deref(), Some("{}"));
    assert_eq!(reader.read_frame().expect("eof"), None);
    reader.finish().expect("nothing trails");
    let too_big = write_frame(&mut Vec::new(), "xx", 1).unwrap_err();
    assert_eq!(too_big.code, ErrorCode::LimitExceeded);
}

#[test]
fn malformed_frames_are_protocol_violations_with_reasons() {
    assert_violation(
        b"Content-Length: 2\r\nContent-Type: json\r\n\r\n{}",
        "unexpected_header",
    );
    assert_violation(
        b"Content-Type: json\r\nContent-Length: 2\r\n\r\n{}",
        "unexpected_header",
    );
    assert_violation(b"Content-Length: 2\n\n{}", "unexpected_header");
    assert_violation(b"Content-Length: -2\r\n\r\n{}", "unexpected_header");
    assert_violation(b"Content-Length: 10\r\n\r\n{}", "truncated_body");
    assert_violation(b"Content-Length: 2\r\n", "truncated_header");
    assert_violation(b"Content-Length: 2\r\n\r\n{}Content", "truncated_header");
    assert_violation(b"Content-Length: 2\r\n\r\n\xff\xfe", "invalid_utf8");
    for trailing in [&b"x"[..], b"\n", b"Content-Length: 2\r\n\r\n{}"] {
        let stream = [&b"Content-Length: 2\r\n\r\n{}"[..], trailing].concat();
        let mut reader = FrameReader::new(stream.as_slice());
        assert!(reader.read_frame().expect("frame").is_some());
        let err = reader.finish().unwrap_err();
        assert_eq!(err.code, ErrorCode::ProtocolViolation);
        assert_eq!(err.reason(), Some("trailing_bytes"));
    }
}

#[test]
fn oversized_frames_are_refused_before_the_body_is_read() {
    let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1);
    let mut source = Counting {
        inner: header.as_bytes().chain(io::repeat(b'a')),
        taken: 0,
    };
    let err = FrameReader::new(&mut source).read_frame().unwrap_err();
    assert_eq!(err.code, ErrorCode::ProtocolViolation);
    assert_eq!(err.reason(), Some("frame_too_large"));
    assert!(
        source.taken <= header.len(),
        "read {} bytes past the header",
        source.taken
    );
    let huge = Counting {
        inner: &b"Content-Length: 99999999999999999999999\r\n\r\n"[..],
        taken: 0,
    };
    assert_eq!(
        FrameReader::new(huge).read_frame().unwrap_err().reason(),
        Some("frame_too_large")
    );
    let mut endless = Counting {
        inner: io::repeat(b'a'),
        taken: 0,
    };
    let err = FrameReader::new(&mut endless).read_frame().unwrap_err();
    assert_eq!(err.reason(), Some("header_too_long"));
    assert!(
        endless.taken <= 65,
        "header read consumed {} bytes",
        endless.taken
    );
    let exact = "Content-Length: 4\r\n\r\n\"ab\"".to_string();
    let mut small = FrameReader::with_limit(IoCursor::new(exact.into_bytes()), 4);
    assert_eq!(
        small.read_frame().expect("at limit").as_deref(),
        Some("\"ab\"")
    );
}

#[test]
fn envelopes_decode_strictly() {
    let bad_json = Envelope::decode("notjson").unwrap_err();
    assert_eq!(
        (bad_json.code, bad_json.reason()),
        (ErrorCode::ProtocolViolation, Some("invalid_envelope"))
    );
    let version =
        Envelope::decode(r#"{"corral_ext":2,"kind":"response","id":"x","result":{}}"#).unwrap_err();
    assert_eq!(version.code, ErrorCode::UnsupportedProtocol);
    let kind = Envelope::decode(r#"{"corral_ext":1,"kind":"notify","id":"x"}"#).unwrap_err();
    assert_eq!(kind.code, ErrorCode::ProtocolViolation);
}

fn binding(spec_rev: &str) -> Binding {
    Binding {
        v: BINDING_VERSION,
        scope: BindingScope {
            workspace_id: "ws-1".into(),
            project: "corral".into(),
            repo: "/abs/repo".into(),
        },
        task: BindingTask {
            id: "T-381".into(),
            spec_rev: spec_rev.into(),
        },
        code: CodeBinding::DispatchBase {
            base: "main".into(),
            base_sha: "a".repeat(40),
            tree: "b".repeat(40),
        },
        workflow: WorkflowBinding {
            mode: Freshness::CurrentAtCapture,
            lineage: "lin-1".into(),
            w_min: Cursor::new("ws-1", "lin-1", "corral", 10, 4).encode(),
            generation: None,
        },
        provider: ProviderBinding {
            ext: "mitigraph".into(),
            registration_rev: 3,
            manifest: sha256_tag(b"manifest"),
            executable: sha256_tag(b"exe"),
            capability: capability_id("task.context", 1),
            config: sha256_tag(b"config"),
        },
        policy: PolicyBinding {
            freshness: Freshness::CurrentAtCapture,
            required: false,
        },
    }
}

fn request(id: &str, deadline: u64, brief_max: u64, spec_rev: &str) -> ContextRequest {
    ContextRequest {
        request_id: id.into(),
        brief_max_bytes: brief_max,
        params: TaskContextParams {
            binding: binding(spec_rev),
            budget: ContextBudget {
                max_items: 20,
                max_bytes: 16384,
                max_evidence_items: 8,
                max_excerpt_bytes: 320,
            },
            deadline_unix_ms: deadline,
        },
    }
}

#[test]
fn binding_digest_ignores_deadline_request_id_and_ceilings() {
    let base = request("A-1", 1_000, 16_384, "rev-1")
        .binding_digest()
        .expect("digest");
    assert!(base.starts_with("sha256:") && base.len() == 71);
    assert_eq!(
        request("A-1", 9_999, 16_384, "rev-1")
            .binding_digest()
            .expect("digest"),
        base
    );
    assert_eq!(
        request("A-2", 1_000, 16_384, "rev-1")
            .binding_digest()
            .expect("digest"),
        base
    );
    assert_eq!(
        request("A-1", 1_000, 4_096, "rev-1")
            .binding_digest()
            .expect("digest"),
        base
    );
    let mut budget = request("A-1", 1_000, 16_384, "rev-1");
    budget.params.budget.max_items = 1;
    assert_eq!(budget.binding_digest().expect("digest"), base);
    assert_ne!(
        request("A-1", 1_000, 16_384, "rev-2")
            .binding_digest()
            .expect("digest"),
        base
    );
}

#[test]
fn binding_digest_is_sha256_of_documented_canonical_json() {
    let b = binding("rev-1");
    let value = serde_json::to_value(&b).expect("value");
    let canonical = canonical_json(&value);
    assert!(
        canonical.starts_with(r#"{"code":{"base":"main","base_sha":"#),
        "{canonical}"
    );
    assert!(!canonical.contains(' ') && !canonical.contains('\n'));
    assert_eq!(
        b.digest().expect("digest"),
        sha256_tag(canonical.as_bytes())
    );
    let reordered: Value =
        serde_json::from_str(r#"{ "z": 1, "a": { "y": 2, "b": [3, "x"] } }"#).expect("json");
    assert_eq!(
        canonical_json(&reordered),
        r#"{"a":{"b":[3,"x"],"y":2},"z":1}"#
    );
    let escapes = canonical_json(&json!({"b": "q\"\\\n\u{1}é", "a": [1, true, null]}));
    assert_eq!(
        escapes,
        "{\"a\":[1,true,null],\"b\":\"q\\\"\\\\\\n\\u0001é\"}"
    );
    assert_eq!(
        sha256_tag(b""),
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let back: Binding = serde_json::from_str(&canonical).expect("binding round trip");
    assert_eq!(back, b);
}

#[test]
fn every_error_code_round_trips_and_unknown_codes_map_to_internal() {
    for code in ErrorCode::ALL {
        let wire = serde_json::to_value(code).expect("code");
        assert_eq!(wire, Value::String(code.as_str().into()));
        assert_eq!(
            serde_json::from_value::<ErrorCode>(wire).expect("code"),
            code
        );
        let err = ExtError::new(code, "detail").with_data(json!({"k": [1]}));
        let text = serde_json::to_string(&err).expect("error");
        assert_eq!(serde_json::from_str::<ExtError>(&text).expect("error"), err);
        let flipped = ExtError {
            retryable: !code.default_retryable(),
            ..ExtError::new(code, "")
        };
        assert_eq!(
            serde_json::from_value::<ExtError>(serde_json::to_value(&flipped).expect("v"))
                .expect("e"),
            flipped
        );
    }
    assert_eq!(ErrorCode::ALL.len(), 22);
    let defaulted: ExtError = serde_json::from_value(json!({"code": "superseded"})).expect("error");
    assert!(defaulted.retryable);
    let unknown: ExtError = serde_json::from_value(
        json!({"code": "contract_rejected", "detail": "d", "data": {"x": 1}}),
    )
    .expect("error");
    assert_eq!(unknown.code, ErrorCode::Internal);
    assert_eq!(unknown.detail, "d");
    assert_eq!(
        unknown.data,
        Some(json!({"original_code": "contract_rejected", "original_data": {"x": 1}}))
    );
    let bare: ExtError =
        serde_json::from_value(json!({"code": "nope", "retryable": false})).expect("error");
    assert_eq!((bare.code, bare.retryable), (ErrorCode::Internal, false));
    assert_eq!(bare.data, Some(json!({"original_code": "nope"})));
}

fn outcomes() -> Vec<TerminalOutcome> {
    let mut all = vec![
        TerminalOutcome::Accepted,
        TerminalOutcome::Superseded,
        TerminalOutcome::Cancelled,
        TerminalOutcome::DeadlineExceeded,
        TerminalOutcome::process_lost(),
    ];
    all.extend(
        ErrorCode::ALL
            .into_iter()
            .map(|c| TerminalOutcome::failed(c, None)),
    );
    all
}

#[test]
fn every_attempt_state_and_outcome_round_trips() {
    let mut states = vec![AttemptState::Starting, AttemptState::Running];
    for outcome in outcomes() {
        let text = serde_json::to_string(&outcome).expect("outcome");
        assert_eq!(
            serde_json::from_str::<TerminalOutcome>(&text).expect("outcome"),
            outcome
        );
        let state = AttemptState::from(outcome.clone());
        assert_eq!(state.outcome(), Some(outcome));
        states.push(state);
    }
    for state in &states {
        let text = serde_json::to_string(state).expect("state");
        assert_eq!(
            &serde_json::from_str::<AttemptState>(&text).expect("state"),
            state
        );
    }
    assert_eq!(
        serde_json::to_value(AttemptState::Starting).expect("v"),
        json!({"state": "starting"})
    );
    assert_eq!(
        serde_json::to_value(AttemptState::from(TerminalOutcome::process_lost())).expect("v"),
        json!({"state": "failed", "code": "internal", "reason": PROCESS_LOST})
    );
    assert_eq!(
        TerminalOutcome::failed(ErrorCode::Cancelled, Some("x")),
        TerminalOutcome::Cancelled
    );
}

#[test]
fn attempt_transitions_follow_the_lifecycle() {
    use AttemptState::*;
    assert!(Starting.can_transition_to(&Running));
    assert!(Starting.can_transition_to(&Cancelled));
    assert!(Starting.can_transition_to(&TerminalOutcome::process_lost().into()));
    assert!(!Starting.can_transition_to(&Accepted));
    assert!(Running.can_transition_to(&Accepted));
    assert!(Running.can_transition_to(&Superseded));
    assert!(!Running.can_transition_to(&Starting));
    for outcome in outcomes() {
        let terminal = AttemptState::from(outcome);
        assert!(terminal.is_terminal());
        assert!(!terminal.can_transition_to(&Running));
        assert!(!terminal.can_transition_to(&Cancelled));
    }
}

#[test]
fn every_envelope_kind_and_method_round_trips() {
    let mut envelopes: Vec<Envelope> = Method::ALL
        .into_iter()
        .map(|method| {
            Envelope::new(
                "A-1",
                Body::Request {
                    method,
                    params: json!({"n": 1}),
                },
            )
        })
        .collect();
    envelopes.push(Envelope::new(
        "A-1",
        Body::Response {
            result: json!({"items": []}),
        },
    ));
    envelopes.push(Envelope::new(
        "A-1",
        Body::Error {
            error: ExtError::new(ErrorCode::InputMismatch, "d"),
        },
    ));
    envelopes.push(Envelope::new(
        "A-1",
        Body::Progress {
            progress: Progress {
                stage: "rank".into(),
                done: Some(3),
                total: Some(9),
            },
        },
    ));
    let kinds: Vec<_> = envelopes.iter().map(|e| e.body.kind()).collect();
    for kind in EnvelopeKind::ALL {
        assert!(kinds.contains(&kind), "{kind:?} not covered");
    }
    for env in envelopes {
        let text = env.encode().expect("encode");
        let wire: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(wire["corral_ext"], 1);
        assert_eq!(
            wire["kind"],
            serde_json::to_value(env.body.kind()).expect("kind")
        );
        assert_eq!(Envelope::decode(&text).expect("decode"), env);
    }
    let methods: Vec<_> = Method::ALL
        .into_iter()
        .map(|m| serde_json::to_value(m).expect("m"))
        .collect();
    assert_eq!(methods[0], "initialize");
    assert_eq!(methods[1], "provider.invoke");
    assert!(Method::ProviderInvoke.in_pilot() && !Method::EventsDeliver.in_pilot());
}

fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(A[(n >> (18 - 6 * i)) as usize & 63]));
        }
    }
    out
}

#[test]
fn cursors_round_trip_both_positions_and_reject_bad_shapes() {
    let cursor = Cursor::new("ws-1", "lin-1", "corral", 42, 7);
    let text = cursor.encode();
    assert!(
        text.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    let back = Cursor::decode(&text).expect("decode");
    assert_eq!(back, cursor);
    assert_eq!((back.seq, back.eseq), (42, 7));
    assert_ne!(Cursor::new("ws-1", "lin-1", "corral", 7, 42).encode(), text);
    let invalid = |json: Value| {
        Cursor::decode(&b64(json.to_string().as_bytes()))
            .unwrap_err()
            .code
    };
    let full = json!({"v": 1, "workspace_id": "ws-1", "lineage_id": "lin-1", "project": "corral", "projection": 1, "seq": 1, "eseq": 2});
    assert!(Cursor::decode(&b64(full.to_string().as_bytes())).is_ok());
    let mut no_eseq = full.clone();
    no_eseq.as_object_mut().expect("object").remove("eseq");
    assert_eq!(invalid(no_eseq), ErrorCode::InvalidRequest);
    let mut no_seq = full.clone();
    no_seq.as_object_mut().expect("object").remove("seq");
    assert_eq!(invalid(no_seq), ErrorCode::InvalidRequest);
    let mut projection = full.clone();
    projection["projection"] = json!(2);
    assert_eq!(invalid(projection), ErrorCode::InvalidRequest);
    let mut version = full.clone();
    version["v"] = json!(2);
    assert_eq!(invalid(version), ErrorCode::InvalidRequest);
    let mut extra = full.clone();
    extra["mac"] = json!("x");
    assert_eq!(invalid(extra), ErrorCode::InvalidRequest);
    for garbage in [
        "",
        "!!!!",
        "abc=",
        "a",
        &format!("{text}="),
        &"A".repeat(5000),
    ] {
        assert_eq!(
            Cursor::decode(garbage).unwrap_err().code,
            ErrorCode::InvalidRequest,
            "{garbage:?}"
        );
    }
    let non_canonical = format!("{}B", &b64(b"{}")[..2]);
    assert_eq!(
        Cursor::decode(&non_canonical).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn cursor_scope_checks_distinguish_lineage_from_scope() {
    let cursor = Cursor::new("ws-1", "lin-1", "corral", 1, 1);
    assert!(cursor.check_scope("ws-1", "lin-1", "corral").is_ok());
    assert_eq!(
        cursor
            .check_scope("ws-1", "lin-2", "corral")
            .unwrap_err()
            .code,
        ErrorCode::LineageChanged
    );
    assert_eq!(
        cursor
            .check_scope("ws-2", "lin-1", "corral")
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        cursor
            .check_scope("ws-1", "lin-1", "other")
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
}
