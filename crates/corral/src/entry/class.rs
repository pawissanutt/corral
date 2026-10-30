//! The one classification of every command an assistant shell (`CORRAL_ASSISTANT`) may run.
//!
//! Every `match` below is exhaustive without a wildcard arm, so a new command or subcommand
//! does not compile until it chooses a class: an assistant never reaches it by default.
use corral_core::{CommentAuthor, Origin};

use crate::agent_cmd::AgentCmd;
use crate::assistant_cmd::{AssistantCmd, ScratchCmd};
use crate::attention_cmd::AttentionCmd;
use crate::automation_cmd::AutomationCmd;
use crate::completion_cmd::{CriteriaCmd, EvidenceCmd};
use crate::dashboard_cmd::DashboardCmd;
use crate::hooks_cmd::HooksCmd;
use crate::inbox_cmd::InboxCmd;
use crate::lessons_cmd::LessonsCmd;
use crate::mode_cmd::ModeCmd;
use crate::plan_cmd::PlanCmd;
use crate::pm_cmd::PmCmd;
use crate::pm_inbox_cmd::PmInboxCmd;
use crate::pricing_cmd::PricingCmd;
use crate::question_cmd::QuestionCmd;
use crate::queue::QueueCmd;
use crate::run_cmd::RunCmd;
use crate::selection_cmd::SelectionCmd;
use crate::setup_cmd::SetupCmd;
use crate::{ArtifactCmd, Cmd, ConfigCmd, HarnessCmd, PluginCmd, SkillCmd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClass {
    /// Reads state; runs attributed to the assistant.
    Read,
    /// Files new work or session-owned records; runs attributed to the assistant.
    File,
    /// Needs the owner or the PM; refused before the store opens.
    Refused,
}

use CommandClass::{File, Read, Refused};

fn read_unless(mutates: bool) -> CommandClass {
    if mutates { Refused } else { Read }
}

fn file_if_own(own: bool) -> CommandClass {
    if own { File } else { Refused }
}

/// Classifies `cmd` for the assistant whose own session ID is `session`.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive arm per command keeps the table whole"
)]
pub fn of(cmd: &Cmd, session: &str) -> CommandClass {
    match cmd {
        Cmd::Assistant(args) => match &args.cmd {
            AssistantCmd::List { .. } | AssistantCmd::Show { .. } => Read,
            AssistantCmd::Summary { id, .. } => file_if_own(id == session),
            AssistantCmd::Create { .. } | AssistantCmd::End { .. } => Refused,
            AssistantCmd::Scratch(args) => match &args.cmd {
                // Own-session scratch only: the operation refuses another's with not_own_scratch.
                ScratchCmd::New { .. }
                | ScratchCmd::Discard { .. }
                | ScratchCmd::Promote { .. } => File,
                ScratchCmd::List { .. } => Read,
                ScratchCmd::Gc { .. } => Refused,
            },
        },
        Cmd::Mode(args) => match &args.cmd {
            ModeCmd::Status { .. } => Read,
            ModeCmd::Assist
            | ModeCmd::Autopilot { .. }
            | ModeCmd::Pause
            | ModeCmd::Resume
            | ModeCmd::Turbo { .. } => Refused,
        },
        Cmd::Inbox(args) => match &args.cmd {
            None | Some(InboxCmd::Show { .. }) => Read,
            Some(
                InboxCmd::Flush { .. }
                | InboxCmd::Retry { .. }
                | InboxCmd::InspectRetry { .. }
                | InboxCmd::IndexOwner { .. }
                | InboxCmd::Recover(_)
                | InboxCmd::Mark(_),
            ) => Refused,
        },
        Cmd::Attention(args) => match &args.cmd {
            None | Some(AttentionCmd::List | AttentionCmd::Stats { .. }) => Read,
            Some(
                AttentionCmd::Attend { .. }
                | AttentionCmd::Park { .. }
                | AttentionCmd::Unpark { .. },
            ) => Refused,
        },
        Cmd::Agent(args) => match &args.cmd {
            // The host's authenticated channel: the operation's role table decides what the
            // assistant role may do, as for every other Pi-native role.
            AgentCmd::Call { .. } => File,
            AgentCmd::Doctor { .. } | AgentCmd::State { .. } => Read,
            AgentCmd::Rewind { list, .. } => read_unless(!list),
            AgentCmd::Launch { .. }
            | AgentCmd::Install { .. }
            | AgentCmd::Login { .. }
            | AgentCmd::Cancel { .. }
            | AgentCmd::Resume { .. } => Refused,
        },
        Cmd::Question(args) => match &args.cmd {
            QuestionCmd::List { .. } | QuestionCmd::Show { .. } => Read,
            QuestionCmd::Answer { .. }
            | QuestionCmd::Escalate { .. }
            | QuestionCmd::Cancel { .. }
            | QuestionCmd::Deadline { .. } => Refused,
        },
        Cmd::Automation(args) => match &args.cmd {
            AutomationCmd::Preview(_) | AutomationCmd::Status => Read,
            AutomationCmd::Confirm { .. } => Refused,
        },
        Cmd::Selection(args) => match &args.cmd {
            SelectionCmd::Catalog { .. } => Read,
            SelectionCmd::Save { .. } => Refused,
        },
        Cmd::Setup(args) => match &args.cmd {
            SetupCmd::Procedure
            | SetupCmd::Template
            | SetupCmd::Status
            | SetupCmd::Catalog
            | SetupCmd::Discover
            | SetupCmd::Validate { .. }
            | SetupCmd::Diff => Read,
            SetupCmd::Bootstrap { .. }
            | SetupCmd::Begin { .. }
            | SetupCmd::Answers { .. }
            | SetupCmd::Propose { .. }
            | SetupCmd::Approve { .. }
            | SetupCmd::Apply
            | SetupCmd::Cancel { .. } => Refused,
        },
        Cmd::Lessons(args) => match &args.cmd {
            None | Some(LessonsCmd::Due) => Read,
            Some(
                LessonsCmd::Distill { .. }
                | LessonsCmd::Apply { .. }
                | LessonsCmd::Pin { .. }
                | LessonsCmd::Delete { .. },
            ) => Refused,
        },
        Cmd::Plan { cmd } => match cmd {
            PlanCmd::Progress { .. } | PlanCmd::Show { .. } => Read,
            PlanCmd::Verify { .. }
            | PlanCmd::Checkpoint { .. }
            | PlanCmd::Evidence { .. }
            | PlanCmd::AggregateReview { .. }
            | PlanCmd::AggregateVerdict { .. }
            | PlanCmd::Accept { .. }
            | PlanCmd::Cancel { .. }
            | PlanCmd::Submit { .. }
            | PlanCmd::Review { .. }
            | PlanCmd::Verdict { .. }
            | PlanCmd::MemberGate { .. }
            | PlanCmd::AddMember { .. }
            | PlanCmd::MoveMember { .. }
            | PlanCmd::Policy { .. } => Refused,
        },
        Cmd::Hooks { cmd } => match cmd {
            HooksCmd::List { .. } => Read,
            HooksCmd::Reconcile | HooksCmd::Retry { .. } => Refused,
        },
        Cmd::Run { cmd } => match cmd {
            RunCmd::Transcript { .. } => Read,
            RunCmd::Progress(args) | RunCmd::Checkpoint(args) => read_unless(args.file.is_some()),
            RunCmd::Wait { .. } | RunCmd::Decision { .. } | RunCmd::Resume { .. } => Refused,
        },
        Cmd::Queue { cmd } => match cmd {
            QueueCmd::List { .. } => Read,
            QueueCmd::Add { .. }
            | QueueCmd::Remove { .. }
            | QueueCmd::Pause
            | QueueCmd::Resume
            | QueueCmd::Admit { .. } => Refused,
        },
        Cmd::Pricing {
            cmd: PricingCmd::Check { .. },
        } => Read,
        Cmd::Criteria { cmd } => match cmd {
            CriteriaCmd::List { .. } => Read,
            CriteriaCmd::Register { .. } => Refused,
        },
        Cmd::Evidence { cmd } => match cmd {
            EvidenceCmd::List { .. } => Read,
            EvidenceCmd::Record { .. } => Refused,
        },
        Cmd::Dashboard { cmd } => match cmd {
            DashboardCmd::Show { .. } => Read,
            DashboardCmd::Set { .. } | DashboardCmd::Pause { .. } | DashboardCmd::Remove { .. } => {
                Refused
            }
        },
        Cmd::Artifact { cmd } => match cmd {
            ArtifactCmd::List { .. } | ArtifactCmd::Show { .. } | ArtifactCmd::Serve { .. } => Read,
            // Only to the assistant's own session; never to a task run or another session.
            ArtifactCmd::Add { session: own, .. } => file_if_own(own.as_deref() == Some(session)),
            ArtifactCmd::Gc { .. } | ArtifactCmd::SetTier { .. } => Refused,
        },
        Cmd::Pm { cmd } => match cmd {
            Some(PmCmd::Bench { .. } | PmCmd::Next { .. } | PmCmd::Brief { .. }) => Read,
            // Posting and withdrawing (the store checks authorship) route work to the PM;
            // claiming and resolving are the PM's own.
            Some(PmCmd::Inbox { cmd }) => match cmd {
                PmInboxCmd::List { .. } | PmInboxCmd::Show { .. } => Read,
                PmInboxCmd::Post { .. } | PmInboxCmd::Withdraw { .. } => File,
                PmInboxCmd::Claim { .. } | PmInboxCmd::Resolve { .. } => Refused,
            },
            None | Some(PmCmd::Act { .. } | PmCmd::Present { .. } | PmCmd::Resolve { .. }) => {
                Refused
            }
        },
        Cmd::Harness {
            cmd: HarnessCmd::List { .. },
        } => Read,
        Cmd::Plugin {
            cmd: PluginCmd::Install { .. },
        } => Refused,
        Cmd::Skill { cmd } => match cmd {
            None => Read,
            Some(SkillCmd::Install { .. }) => Refused,
        },
        Cmd::Config { cmd } => match cmd {
            ConfigCmd::Diff { .. }
            | ConfigCmd::History { .. }
            | ConfigCmd::Get { .. }
            | ConfigCmd::Validate
            | ConfigCmd::Path => Read,
            ConfigCmd::Set { .. } => Refused,
        },
        Cmd::RuntimeProbe { .. }
        | Cmd::BuildIdentity
        | Cmd::Wait(_)
        | Cmd::Stats { .. }
        | Cmd::List { .. }
        | Cmd::Search(_)
        | Cmd::Log(_)
        | Cmd::Show { .. }
        | Cmd::Pins { .. }
        | Cmd::Open { .. }
        | Cmd::Review { .. } => Read,
        Cmd::Doctor { fix, .. } => read_unless(*fix),
        Cmd::Route {
            set_reviewer,
            clear_reviewer,
            ..
        } => read_unless(set_reviewer.is_some() || *clear_reviewer),
        Cmd::Dispatch { explain, .. } => read_unless(!explain),
        Cmd::Done { check, .. } => read_unless(!check),
        Cmd::Autopilot { action, .. } => read_unless(action != "status"),
        // Filing stops short of Ready, so a filed task never dispatches on the assistant's word.
        Cmd::New { ready, origin, .. } => {
            if *ready || *origin == Origin::Pm {
                Refused
            } else {
                File
            }
        }
        // Delivery to an agent, a PM-authored note or an owner approval needs that actor.
        Cmd::Comment {
            text, author, send, ..
        } => {
            if *send || *author == CommentAuthor::Pm || text.trim().starts_with("owner-ok:") {
                Refused
            } else {
                File
            }
        }
        Cmd::Pi(_)
        | Cmd::Digest(_)
        | Cmd::Revert { .. }
        | Cmd::ReviewSprint(_)
        | Cmd::Idea(_)
        | Cmd::Up { .. }
        | Cmd::Init
        | Cmd::Import(_)
        | Cmd::Move { .. }
        | Cmd::Edit { .. }
        | Cmd::Reply { .. }
        | Cmd::Unstick { .. }
        | Cmd::Gate { .. }
        | Cmd::ReviewRun { .. }
        | Cmd::Verdict { .. }
        | Cmd::Pin { .. }
        | Cmd::Unpin { .. }
        | Cmd::Approve { .. }
        | Cmd::Changes { .. }
        | Cmd::Retry { .. }
        | Cmd::Discard { .. }
        | Cmd::Jump { .. }
        | Cmd::Sync { .. }
        | Cmd::Hook
        | Cmd::BindSession { .. }
        | Cmd::FocusBoard
        | Cmd::Board { .. }
        | Cmd::Capture { .. }
        | Cmd::Artifacts { .. }
        | Cmd::Settings { .. } => Refused,
    }
}
