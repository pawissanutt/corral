use super::*;
fn covered_now(scope: &OperatingScope, task: &Task) -> bool {
    task.project
        .as_ref()
        .is_some_and(|project| scope.projects.contains_key(project))
        && !scope.tasks.contains_key(&task.id)
        && !scope.exclusions.contains(&task.id)
        && !matches!(task.status, Status::Done | Status::Dropped)
}
impl Oracle<'_> {
    pub(super) fn project_members(
        &self,
        config: &Config,
        reasons: &mut Vec<String>,
    ) -> BTreeMap<String, ScopedTask> {
        let scope = &config.automation.scope;
        let mut members = BTreeMap::new();
        if scope.projects.is_empty() {
            return members;
        }
        let since = match self.operating_state() {
            Ok(state) => state.approval.map(|approval| approval.confirmed_at),
            Err(error) => {
                reasons.push(format!("project grant state unavailable: {error:#}"));
                return members;
            }
        };
        let mut tasks = match self.list() {
            Ok(tasks) => tasks,
            Err(error) => {
                reasons.push(format!("project grant tasks unavailable: {error:#}"));
                return members;
            }
        };
        // Oldest first, so a cap covers the earliest new work and never reshuffles.
        tasks.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        let mut new_tasks = 0u32;
        let mut moved = BTreeSet::new();
        for task in tasks {
            let Some(granted) = task
                .project
                .as_ref()
                .and_then(|project| scope.projects.get(project))
            else {
                continue;
            };
            if scope.tasks.contains_key(&task.id) || scope.exclusions.contains(&task.id) {
                continue;
            }
            if since.is_some_and(|since| task.created > since) {
                if scope.max_new_tasks.is_some_and(|max| new_tasks >= max) {
                    continue;
                }
                new_tasks += 1;
            }
            if !covered_now(scope, &task) {
                continue;
            }
            match self.operating_member(config, &task) {
                Ok(member)
                    if member.repository == granted.repository
                        && member.repository_identity == granted.repository_identity =>
                {
                    members.insert(task.id.clone(), member);
                }
                Ok(member) => {
                    moved.insert(member.project);
                }
                Err(error) => reasons.push(format!("{}: {error:#}", task.id)),
            }
        }
        for project in moved {
            reasons.push(format!(
                "project {project} repository changed since the grant"
            ));
        }
        members
    }
}
