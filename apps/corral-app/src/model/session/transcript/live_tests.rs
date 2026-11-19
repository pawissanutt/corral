use super::*;

fn event(seq: u64, generation: u32, kind: SessionLiveKind) -> SessionLiveEvent {
    SessionLiveEvent {
        seq,
        generation,
        at: String::new(),
        kind,
    }
}

fn started(seq: u64, generation: u32, turn: u64) -> SessionLiveEvent {
    event(
        seq,
        generation,
        SessionLiveKind::LiveTurn {
            turn,
            state: SessionLiveTurnState::Started,
        },
    )
}

#[test]
fn replayed_text_chunk_is_not_appended_twice() {
    let mut live = LiveSession::default();
    let chunk = event(
        2,
        1,
        SessionLiveKind::LiveText {
            turn: 1,
            message: 1,
            offset: 0,
            text: "hé".into(),
        },
    );
    live.apply_page(Some(1), vec![chunk.clone(), started(1, 1, 1)], false);
    live.apply_page(Some(1), vec![chunk.clone()], false);
    live.apply_page(Some(1), vec![SessionLiveEvent { seq: 3, ..chunk }], false);
    live.apply_page(
        Some(1),
        vec![event(
            4,
            1,
            SessionLiveKind::LiveText {
                turn: 1,
                message: 1,
                offset: 3,
                text: "!".into(),
            },
        )],
        false,
    );
    assert_eq!(live.messages[0].text, "hé!");
    assert!(live.open);
}

#[test]
fn ended_tool_leaves_overlay_when_recorded_transcript_arrives() {
    let mut live = LiveSession::default();
    live.apply_page(
        Some(1),
        vec![event(
            2,
            1,
            SessionLiveKind::LiveTool {
                turn: 1,
                call: "c".into(),
                tool: "read".into(),
                state: SessionLiveToolState::Started {
                    args: None,
                    args_truncated: None,
                },
            },
        )],
        false,
    );
    let before_end = live.reread;
    live.apply_page(
        Some(1),
        vec![event(
            3,
            1,
            SessionLiveKind::LiveTool {
                turn: 1,
                call: "c".into(),
                tool: "read".into(),
                state: SessionLiveToolState::Ended {
                    failed: false,
                    elapsed_ms: Some(7),
                },
            },
        )],
        false,
    );
    live.settle(before_end);
    assert_eq!(live.tools.len(), 1);
    assert_eq!(live.tools[0].state, LiveToolState::Done);
    let recorded = live.reread;
    live.settle(recorded);
    assert!(live.tools.is_empty());
}

#[test]
fn generation_change_drops_old_turn_text() {
    let mut live = LiveSession::default();
    live.apply_page(
        Some(1),
        vec![
            started(9, 1, 1),
            event(
                10,
                1,
                SessionLiveKind::LiveText {
                    turn: 1,
                    message: 1,
                    offset: 0,
                    text: "old".into(),
                },
            ),
        ],
        false,
    );
    let before_change = live.reread;
    live.apply_page(Some(2), vec![started(1, 2, 2)], false);
    assert!(live.messages.is_empty());
    assert!(live.tools.is_empty());
    assert_eq!(live.turn, Some(2));
    assert_eq!(live.generation, Some(2));
    assert_eq!(live.reread, before_change + 1);
}
