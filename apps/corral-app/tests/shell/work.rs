//! A task's rows and operations from the shell: selecting and opening rows, replying to a
//! Blocked task, the Actions menu, confirmations, and refusals with their forced steps.

use super::*;

/// Tasks selects its first row on arrival and shows it in the inspector; a press selects
/// another row without leaving the list, a second press opens the task's page, and Escape
/// there returns to Tasks with the row still selected.
#[test]
fn a_task_row_selects_into_the_inspector_and_opens_its_page_on_the_second_press() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:inspector").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press("id:task-T-12").unwrap();
    app.wait_for("id:btn-open-page").unwrap();
    assert!(selected(&app, "task-T-12"));
    assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
    app.press("id:task-T-12").unwrap();
    app.wait_for("id:task-page").unwrap();
    assert!(selected(&app, "nav-tasks"));
    app.press_key("escape").unwrap();
    app.wait_for("id:list-tasks").unwrap();
    assert!(selected(&app, "task-T-12"));
}

/// `j` and `k` walk the rows, Enter opens the selected one's page, and with the inspector
/// hidden (`i`) one press on a row opens its page.
#[test]
fn the_keys_walk_the_tasks_list_and_a_hidden_inspector_opens_rows_on_one_press() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:inspector").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press_key("j").unwrap();
    app.wait_for("id:inspector").unwrap();
    assert!(!selected(&app, "task-T-8"));
    app.press_key("k").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press_key("enter").unwrap();
    app.wait_for("id:task-page").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("i").unwrap();
    app.wait_for_gone("id:inspector", Duration::from_secs(5))
        .unwrap();
    app.press("id:task-T-12").unwrap();
    app.wait_for("id:task-page").unwrap();
}

/// The successor of answering an Inbox question: the words typed in a Blocked task's reply
/// box on its page go out as its reply step, the task runs again and leaves Needs you.
#[test]
fn replying_from_a_blocked_tasks_page_sends_the_reply_and_the_task_leaves_needs_you() {
    let mut app = preview(desktop(), &[("corral.destination", "home")]);
    let waiting = count_of(&mut app, "count-home");
    app.press("id:task-T-8").unwrap();
    app.wait_for("id:field-reply").unwrap();
    assert!(!app.screen().enabled("id:btn-next-0").unwrap());
    app.type_text("id:field-reply", "Stay paused; never deliver twice.")
        .unwrap();
    app.press("id:btn-next-0").unwrap();
    app.wait_for_gone("id:field-reply", Duration::from_secs(5))
        .unwrap();
    assert_eq!(count_of(&mut app, "count-home"), waiting - 1);
}

/// The successor of the phone's question page: a Blocked task's page shows its reply box, and
/// Back returns to Home's list.
#[test]
fn phone_blocked_task_page_goes_back_to_homes_list() {
    let mut app = preview(phone(), &[]);
    app.press("id:task-T-8").unwrap();
    app.wait_for("id:field-reply").unwrap();
    app.press("id:page-back").unwrap();
    app.wait_for("id:task-T-8").unwrap();
    assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
    assert_text(&mut app, "page-title", "Home");
}

/// The engine refuses dispatching T-12 while T-14 is not done. The sheet offers "Dispatch
/// anyway", which names what it skips and is the one confirmation: pressing it submits the same
/// dispatch with force, T-12 runs, and Activity's Feed records the forced operation.
#[test]
fn a_refused_dispatch_offers_dispatch_anyway_and_forcing_it_runs_the_task() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    dispatch_t12(&mut app);
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:Dispatch anyway").unwrap();
    assert!(app.screen().count("id:btn-refusal-0").unwrap() > 0);
    app.wait_for("text:/^Skips unfinished dependencies/")
        .unwrap();
    app.press("id:btn-refusal-0").unwrap();
    app.wait_for_gone("id:refused", Duration::from_secs(5))
        .unwrap();
    assert_eq!(app.screen().count("id:confirm").unwrap(), 0);
    app.wait_for("text:/^Dispatch anyway T-12 · Done/").unwrap();
    app.press("id:nav-sessions").unwrap();
    app.wait_for("id:agent-T-12_R1").unwrap();
    app.press("id:nav-activity").unwrap();
    app.wait_for("text:/^dispatch · skipped unfinished dependencies/")
        .unwrap();
}

/// The keyboard's Actions menu (`.`) took no arrows, so Down three times and Enter ran its first
/// item, Edit. Down moves through the items and Enter runs the one it reached, the fourth
/// (Dispatch); End and Home reach the last and the first.
#[test]
fn arrow_keys_walk_the_actions_menu_and_enter_runs_the_item_reached() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    app.press("id:task-T-8").unwrap();
    app.press_key(".").unwrap();
    app.wait_for("id:verbs").unwrap();
    app.press_key("end").unwrap();
    app.press_key("home").unwrap();
    for _ in 0..3 {
        app.press_key("down").unwrap();
    }
    app.press_key("enter").unwrap();
    app.wait_for("id:btn-verb-send").unwrap();
    app.wait_for("text:Dispatch T-8").unwrap();
    assert_eq!(app.screen().count("text:Edit T-8").unwrap(), 0);
}

/// A destructive operation asks once, locally, with the focus on Cancel: Enter cancels, the
/// receipt says nothing was sent, and the task is still there.
#[test]
fn the_confirm_sheet_takes_the_focus_so_enter_cancels_it() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    select(&mut app, "T-12 Timeline zoom presets");
    select(&mut app, "Drop T-12");
    app.wait_for("id:confirm").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for_gone("id:confirm", Duration::from_secs(5))
        .unwrap();
    app.wait_for("text:/ · cancelled, nothing was sent$/")
        .unwrap();
}

/// T-10's gate is red, so approving it from More is refused, and the refusal offers Land
/// anyway, which names what it skips and is the one confirmation: pressing it submits approve
/// with force and the task lands and leaves Review.
#[test]
fn land_anyway_over_a_red_gate_is_offered_by_the_refusal_and_lands() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    select(&mut app, "T-10 Usage page explains missing records");
    app.wait_for("id:task-page").unwrap();
    select(&mut app, "Approve and land… T-10");
    app.wait_for("id:btn-verb-send").unwrap();
    app.press("id:btn-verb-send").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:/the gate check/").unwrap();
    // The page's decide bar offers Land anyway too; this is the refusal's own step.
    app.press("id:btn-refusal-1").unwrap();
    app.wait_for("text:/^Land anyway T-10 · Done/").unwrap();
}

/// A review the engine marked ready shows Approve as its one next step: one press asks once,
/// and confirming lands the task.
#[test]
fn a_ready_review_lands_from_its_one_next_step() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:next-T-11").unwrap();
    app.press("id:next-T-11").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/^Approve and land T-11 · Done/")
        .unwrap();
}

/// Approve lands onto the branch typed in Onto. A branch the repository lacks is refused, and the
/// refusal offers one Approve per existing branch; choosing one asks once and lands there.
#[test]
fn approving_onto_a_missing_branch_offers_each_existing_branch() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    select(&mut app, "T-11 Collapse the sidebar on narrow screens");
    app.wait_for("id:task-page").unwrap();
    select(&mut app, "Approve and land… T-11");
    app.wait_for("id:field-verb-0").unwrap();
    app.clear("id:field-verb-0").unwrap();
    app.type_text("id:field-verb-0", "release/1.0").unwrap();
    app.press("id:btn-verb-send").unwrap();
    app.wait_for("text:/--onto release\\/1.0/").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:Approve onto release/0.9").unwrap();
    app.press("id:btn-refusal-1").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/^Approve onto release\\/0.9 T-11 · Done/")
        .unwrap();
}
