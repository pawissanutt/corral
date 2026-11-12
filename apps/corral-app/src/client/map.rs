use crate::model::board::Mark;
use crate::model::daemon::{
    self, Agent, DaemonHealth, DaemonLevel, Group, PmLevel, Runtime, RuntimeLevel, Step, Task,
};
use corral_protocol::board::{BoardGroup, BoardRow, CostBasis, RailGlyph};
use corral_protocol::{DaemonState, Health, PmState, RuntimeState};

pub(super) fn timestamp(text: Option<&str>) -> Option<u64> {
    text?
        .parse::<jiff::Timestamp>()
        .ok()?
        .as_second()
        .try_into()
        .ok()
}

pub(super) fn task(row: BoardRow) -> Task {
    Task {
        id: daemon::id(&row.id),
        title: row.title,
        project: row.project,
        plan: row.plan,
        goal: row.goal,
        group: match row.group {
            BoardGroup::NeedsYou => Group::NeedsYou,
            BoardGroup::Verifying => Group::Verifying,
            BoardGroup::Producing => Group::Producing,
            BoardGroup::Queued => Group::Queued,
            BoardGroup::Held => Group::Held,
            BoardGroup::Parked => Group::Parked,
            BoardGroup::Other => Group::Other,
        },
        rail: row.rail.map(|step| Step {
            mark: match step.glyph {
                RailGlyph::Pass => Mark::Done,
                RailGlyph::Active => Mark::Active,
                RailGlyph::Fail => Mark::Fail,
                RailGlyph::Stale => Mark::Stale,
                RailGlyph::Pending => Mark::Next,
                RailGlyph::Unavailable | RailGlyph::Other => Mark::Na,
            },
            sha7: step.sha7,
            at: timestamp(step.at.as_deref()),
        }),
        now: row.now.text,
        now_code: row.now.code,
        since: timestamp(row.now.since.as_deref()),
        held_reason: row.held_reason,
        questions: row.questions,
        attention: row.attention,
        agent: row.agent.map(|agent| Agent {
            session: agent.session,
            role: agent.role,
            profile: agent.profile,
            runtime: agent.runtime,
            machine: agent.machine,
        }),
        cost: row.cost_usd,
        cost_estimated: row.cost_basis != Some(CostBasis::Reported),
        rev: row.rev,
    }
}

pub(super) fn health(health: Health) -> DaemonHealth {
    DaemonHealth {
        level: match health.daemon {
            DaemonState::Healthy => DaemonLevel::Healthy,
            DaemonState::Degraded => DaemonLevel::Degraded,
            DaemonState::Down => DaemonLevel::Down,
            DaemonState::Other => DaemonLevel::Unknown,
        },
        reasons: health.reasons,
        runtimes: health
            .runtimes
            .into_iter()
            .map(|runtime| Runtime {
                kind: runtime.kind,
                level: match runtime.state {
                    RuntimeState::Ok => RuntimeLevel::Ok,
                    RuntimeState::Unreachable => RuntimeLevel::Unreachable,
                    RuntimeState::Other => RuntimeLevel::Unknown,
                },
            })
            .collect(),
        pm: match health.pm {
            PmState::Live => PmLevel::Live,
            PmState::Relaunching => PmLevel::Relaunching,
            PmState::Absent => PmLevel::Absent,
            PmState::Other => PmLevel::Unknown,
        },
        pass_seq: health.loop_facts.pass_seq,
        last_pass: timestamp(health.loop_facts.last_pass_at.as_deref()),
        last_pass_ms: health.loop_facts.last_pass_ms,
        version: health.version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::board::{BoardNow, RailStep};

    #[test]
    fn estimated_row_maps_rfc3339_seconds_and_cost_flag() {
        let row = BoardRow {
            id: "T-9".into(),
            title: "Live row".into(),
            project: None,
            plan: None,
            goal: None,
            group: BoardGroup::Verifying,
            rail: std::array::from_fn(|_| RailStep {
                glyph: RailGlyph::Unavailable,
                sha7: None,
                at: Some("2024-01-01T01:00:00+01:00".into()),
            }),
            now: BoardNow {
                code: "queued".into(),
                text: "queued".into(),
                since: Some("2024-01-01T00:00:00Z".into()),
            },
            held_reason: None,
            questions: 0,
            attention: vec![],
            agent: None,
            cost_usd: Some(1.25),
            cost_basis: Some(CostBasis::Estimated),
            last_event: None,
            rev: 1,
        };
        let mapped = task(row);
        assert_eq!(mapped.since, Some(1_704_067_200));
        assert_eq!(mapped.rail[0].at, mapped.since);
        assert_eq!(mapped.rail[0].mark, Mark::Na);
        assert_eq!(mapped.cost, Some(1.25));
        assert!(mapped.cost_estimated);
    }
}
