//! A plan review answered through the real client against a scripted corrald on a Unix socket:
//! the full session page and the side pane's session tab each Execute, retry after a refusal,
//! hold a compaction until it starts or fails, and show and retire the wait for an approval
//! already being answered. The script answers what the app reads (device, session, assistant
//! rows, the live session, cards, the plan document, the host's `plan_status`) and hands every
//! `session.card.answer` to the test, which decides when and how it settles.
#![cfg(unix)]

#[path = "plan_review/support.rs"]
mod support;
#[path = "plan_review/world.rs"]
mod world;

use std::time::Duration;

use nsh_testing::Headless;

use support::{Daemon, Entry, assert_muted, chose, connected, shows, wait_armed};
use world::{COMPACT, DUPLICATE, EXECUTE, World, card, idle};

/// Every way an approval can go, answered from `entry`, one submitted plan after another.
fn approvals_settle(entry: &Entry, daemon: &Daemon, app: &mut Headless) {
    // Ordinary Execute starts the plan, and the review closes.
    entry.open(app, 1);
    app.press(&entry.option(1, EXECUTE)).unwrap();
    let answer = daemon.answer(app);
    chose(&answer, "execute", 1);
    daemon.reply(&answer, "preparing");
    daemon.change(World::started);
    entry.closed(app, 1);

    // A refused answer says why and leaves Execute to retry.
    daemon.change(|world| world.submit(2));
    entry.open(app, 2);
    app.press(&entry.option(2, EXECUTE)).unwrap();
    let answer = daemon.answer(app);
    daemon.refuse(
        &answer,
        "The execution model changed or is unknown; review the model again",
    );
    app.wait_for(&entry.banner("Answer not taken")).unwrap();
    assert!(shows(app, &entry.at("id:plan-review")));
    app.press(&entry.option(2, EXECUTE)).unwrap();
    let retried = daemon.answer(app);
    chose(&retried, "execute", 2);
    daemon.reply(&retried, "preparing");
    daemon.change(World::started);
    entry.closed(app, 2);
    entry.clear_banners(app);

    // A held compaction keeps the options muted until it starts; the host's status, read once
    // after the ACK, says it is still pending and is not read again while it stays so.
    daemon.change(|world| world.submit(3));
    entry.open(app, 3);
    let probed = daemon.probed();
    app.press(&entry.option(3, COMPACT)).unwrap();
    let answer = daemon.answer(app);
    chose(&answer, "execute_compact", 3);
    daemon.reply(&answer, "preparing");
    daemon.change(|world| world.preparing(Some("compact")));
    app.wait_for(&entry.at("id:plan-preparing")).unwrap();
    daemon.probed_since(app, probed);
    let probed = daemon.probed();
    assert_muted(app, &entry.option(3, COMPACT));
    daemon.no_answer(app, Duration::from_millis(600), "preparing must hold");
    assert!(shows(app, &entry.at("id:plan-preparing")));
    assert_eq!(
        daemon.probed(),
        probed,
        "a pending status was read again and again"
    );
    daemon.change(World::started);
    entry.closed(app, 3);

    // A compaction that fails says so and leaves the review to retry.
    daemon.change(|world| world.submit(4));
    entry.open(app, 4);
    app.type_text(&entry.at("id:field-answer-plan-4-0"), "Retain this note")
        .unwrap();
    app.press(&entry.option(4, COMPACT)).unwrap();
    let answer = daemon.answer(app);
    assert_eq!(
        answer.answer.answers[0].text.as_deref(),
        Some("Retain this note")
    );
    daemon.reply(&answer, "preparing");
    app.wait_for(&entry.at("id:plan-preparing")).unwrap();
    // Preparation completes before its transient live state is delivered.
    daemon.change(|world| world.failed("Context could not be prepared"));
    app.wait_for(&entry.banner("The plan was not started"))
        .unwrap();
    app.wait_for_gone(&entry.at("id:plan-preparing"), Duration::from_secs(5))
        .unwrap();
    assert!(shows(app, &entry.at("id:plan-review")));
    app.press(&entry.option(4, COMPACT)).unwrap();
    let retried = daemon.answer(app);
    chose(&retried, "execute_compact", 4);
    assert_eq!(
        retried.answer.answers[0].text.as_deref(),
        Some("Retain this note")
    );
    daemon.reply(&retried, "preparing");
    daemon.change(|world| world.preparing(Some("compact")));
    app.wait_for(&entry.at("id:plan-preparing")).unwrap();
    daemon.change(World::started);
    entry.closed(app, 4);
    entry.clear_banners(app);
}

/// An approval already being answered: the wait shows, × hides it without touching that
/// approval, and settlement retires it whether or not it was dismissed.
fn approval_waits_settle(entry: &Entry, daemon: &Daemon, app: &mut Headless) {
    let wait = entry.banner("already being answered");

    daemon.change(|world| world.submit(5));
    entry.open(app, 5);
    app.press(&entry.option(5, COMPACT)).unwrap();
    let answer = daemon.answer(app);
    // Another answer's compaction holds the review before this one is refused as a duplicate.
    daemon.change(|world| world.preparing(Some("compact")));
    daemon.refuse(&answer, DUPLICATE);
    app.wait_for(&wait).unwrap();
    app.wait_for(&entry.at("id:plan-preparing")).unwrap();
    app.press(&entry.at("id:btn-dismiss-notice")).unwrap();
    app.wait_for_gone(&wait, Duration::from_secs(5)).unwrap();
    assert!(shows(app, &entry.at("id:plan-preparing")));
    assert!(shows(app, &entry.at("id:plan-review")));
    daemon.no_answer(app, Duration::from_millis(300), "dismissal sends nothing");
    daemon.change(World::started);
    entry.closed(app, 5);
    assert!(!shows(app, &wait), "a dismissed wait came back");

    // Undismissed, the wait leaves when the approval it waited on fails, and the failure shows.
    daemon.change(|world| world.submit(6));
    entry.open(app, 6);
    app.press(&entry.option(6, COMPACT)).unwrap();
    let answer = daemon.answer(app);
    daemon.change(|world| world.preparing(Some("compact")));
    daemon.refuse(&answer, DUPLICATE);
    app.wait_for(&wait).unwrap();
    daemon.change(|world| world.failed("Context could not be prepared"));
    app.wait_for_gone(&wait, Duration::from_secs(5)).unwrap();
    app.wait_for(&entry.banner("The plan was not started"))
        .unwrap();
    wait_armed(app, &entry.option(6, EXECUTE));
    app.press(&entry.option(6, EXECUTE)).unwrap();
    let retried = daemon.answer(app);
    chose(&retried, "execute", 6);
    daemon.reply(&retried, "preparing");
    daemon.change(World::started);
    entry.closed(app, 6);
}

#[test]
fn the_session_page_settles_every_plan_approval() {
    let (_env, daemon, mut app) = connected();
    approvals_settle(&Entry::PAGE, &daemon, &mut app);
    approval_waits_settle(&Entry::PAGE, &daemon, &mut app);
}

#[test]
fn the_side_pane_settles_every_plan_approval() {
    let (_env, daemon, mut app) = connected();
    app.press("id:btn-dock-side").unwrap();
    app.wait_for(&Entry::PANE.at("id:dock-transcript")).unwrap();
    app.wait_for("id:conversation-beside").unwrap();
    approvals_settle(&Entry::PANE, &daemon, &mut app);
    approval_waits_settle(&Entry::PANE, &daemon, &mut app);
}

/// corrald acknowledges a compaction (`preparing`) before the live state reports it: until
/// that state arrives, the review must not take a second Execute.
#[test]
fn a_preparing_reply_ahead_of_live_state_does_not_rearm_execute() {
    for pane in [false, true] {
        let (_env, daemon, mut app) = connected();
        let entry = Entry::of(pane, &mut app);
        entry.open(&mut app, 1);
        app.press(&entry.option(1, COMPACT)).unwrap();
        let answer = daemon.answer(&mut app);
        daemon.reply(&answer, "preparing");
        app.wait_for(&entry.at("id:plan-preparing")).unwrap();
        assert_muted(&app, &entry.option(1, COMPACT));
        daemon.no_answer(
            &mut app,
            Duration::from_millis(600),
            "a second Execute went out between the preparing reply and the live state",
        );
        daemon.change(|world| world.preparing(Some("compact")));
        app.wait_for(&entry.at("id:plan-preparing")).unwrap();
        daemon.change(World::started);
        entry.closed(&mut app, 1);
    }
}

/// The review is shown from host H1 and the answer is acknowledged by H2, which never reaches
/// the app live. A live frame from H3, queued before the host moved on, then shows the review
/// idle, while H4 actually serves the same review, idle. The stale frame must not re-arm the
/// review. Only a status read from the host as it is now may do that, and it says the outcome
/// is unknown.
#[test]
fn a_stale_live_host_does_not_unlock_what_a_fresh_status_read_settles() {
    for pane in [false, true] {
        let (_env, daemon, mut app) = connected();
        let entry = Entry::of(pane, &mut app);
        daemon.change(|world| world.restart("H1"));
        entry.open(&mut app, 1);

        daemon.hold_probes(true);
        app.press(&entry.option(1, COMPACT)).unwrap();
        let answer = daemon.answer(&mut app);
        chose(&answer, "execute_compact", 1);
        // H2 takes the answer; its ACK names H2 and the epoch of this approval.
        daemon.quietly(|world| world.restart("H2"));
        daemon.reply(&answer, "preparing");
        app.wait_for(&entry.at("id:plan-preparing")).unwrap();
        let probe = daemon.probe(&mut app);
        assert_eq!(probe.card, card(1));

        // H3's stale frame: the review open and idle, as though nothing were underway.
        daemon.stale(|state| {
            if let Some(plan) = state.plan_mode.as_mut() {
                idle(plan, "H3");
            }
        });
        daemon.no_answer(
            &mut app,
            Duration::from_millis(600),
            "a stale live host re-armed the review",
        );
        assert!(shows(&app, &entry.at("id:plan-preparing")));
        assert_muted(&app, &entry.option(1, COMPACT));
        assert_muted(&app, &entry.option(1, EXECUTE));
        assert!(
            !shows(&app, &entry.banner("outcome is unavailable")),
            "a stale live host settled the approval"
        );

        // H4 serves a pending answer without a corresponding live frame. The owner can
        // recheck without resubmitting if that host completes without another live update.
        daemon.quietly(|world| {
            world.restart("H4");
            world.preparing(Some("compact"));
        });
        daemon.hold_probes(false);
        daemon.status(&probe);
        let retry = entry.at("id:btn-plan-status-retry");
        app.wait_for(&retry).unwrap();
        assert_muted(&app, &entry.option(1, EXECUTE));
        daemon.quietly(|world| world.preparing(None));
        app.press(&retry).unwrap();
        app.wait_for(&entry.banner("outcome is unavailable"))
            .unwrap();
        app.wait_for_gone(&entry.at("id:plan-preparing"), Duration::from_secs(5))
            .unwrap();
        assert!(shows(&app, &entry.at("id:plan-review")));
        wait_armed(&mut app, &entry.option(1, EXECUTE));
        entry.clear_banners(&mut app);

        // A clipped snapshot can omit the card; its proof must survive the omission.
        daemon.stale(|state| {
            state.cards.clear();
            state.plan_mode = None;
        });
        app.wait_for_gone(&entry.at("id:plan-review"), Duration::from_secs(5))
            .unwrap();

        // A delayed, busy H3 projection must be rechecked rather than sticking the review.
        let prior = daemon.probed();
        daemon.stale(|state| {
            if let Some(plan) = state.plan_mode.as_mut() {
                idle(plan, "H3");
                plan.preparing = Some("compact".into());
                plan.approval_pending = Some(true);
            }
        });
        daemon.probed_since(&mut app, prior);
        entry.open(&mut app, 1);
        wait_armed(&mut app, &entry.option(1, EXECUTE));
        assert!(
            !shows(&app, &entry.banner("outcome is unavailable")),
            "revalidation must not repeat a dismissed outcome"
        );

        // A refusal must not restore H3's stale busy state; Execute remains retryable.
        app.press(&entry.option(1, EXECUTE)).unwrap();
        let refused = daemon.answer(&mut app);
        daemon.refuse(&refused, "The execution model changed");
        app.wait_for(&entry.banner("Answer not taken")).unwrap();
        wait_armed(&mut app, &entry.option(1, EXECUTE));
        entry.clear_banners(&mut app);

        // Execute goes out again, and H4 starts it.
        app.press(&entry.option(1, EXECUTE)).unwrap();
        let retried = daemon.answer(&mut app);
        chose(&retried, "execute", 1);
        daemon.reply(&retried, "preparing");
        daemon.change(World::started);
        entry.closed(&mut app, 1);
    }
}

/// A status read that fails leaves the approval guarded and offers to read again; the retry
/// reads the host without answering the review a second time.
#[test]
fn a_failed_status_read_can_be_retried_without_reanswering() {
    for pane in [false, true] {
        let (_env, daemon, mut app) = connected();
        let entry = Entry::of(pane, &mut app);
        entry.open(&mut app, 1);
        daemon.hold_probes(true);
        app.press(&entry.option(1, COMPACT)).unwrap();
        let answer = daemon.answer(&mut app);
        daemon.reply(&answer, "preparing");
        app.wait_for(&entry.at("id:plan-preparing")).unwrap();
        let probe = daemon.probe(&mut app);
        daemon.refuse_status(&probe, "the session host did not answer");

        let retry = entry.at("id:btn-plan-status-retry");
        app.wait_for(&retry).unwrap();
        assert!(shows(&app, &entry.at("id:plan-preparing")));
        assert_muted(&app, &entry.option(1, COMPACT));
        daemon.no_answer(
            &mut app,
            Duration::from_millis(300),
            "a failed status read re-answered the review",
        );

        app.press(&retry).unwrap();
        let reread = daemon.probe(&mut app);
        assert_eq!(reread.card, card(1));
        // The host reports pending, but the live snapshot has not confirmed it yet.
        daemon.status(&reread);
        app.wait_for(&entry.banner("host reports an answer pending"))
            .unwrap();
        assert!(shows(&app, &retry));
        daemon.change(|_| {});
        app.wait_for_gone(&retry, Duration::from_secs(5)).unwrap();
        assert!(shows(&app, &entry.at("id:plan-preparing")));
        assert_muted(&app, &entry.option(1, COMPACT));
        daemon.no_answer(&mut app, Duration::from_millis(300), "preparing must hold");

        daemon.hold_probes(false);
        daemon.change(World::started);
        entry.closed(&mut app, 1);
    }
}
