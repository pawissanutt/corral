//! Dependency heads and acyclicity shared by every check of one pinned pass. A pinned pass
//! reads each head once and walks each subgraph once; a live store derives both per call.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::{Context, Result, bail};
use corral_core::{Status, Store, Task, TaskFieldsRef, TaskHead};

/// Heads as `Store::head` returned them, and the dependencies whose whole stored subgraph
/// was walked without a cycle or a missing head. Every completed node's dependencies are
/// completed too, so the set is closed under stored edges.
#[derive(Default)]
struct Graph {
    heads: HashMap<String, Arc<TaskHead>>,
    acyclic: HashSet<String>,
}

fn graph(store: &Store) -> Result<Arc<Mutex<Graph>>> {
    store.pass_derived("dependency-graph", || Ok(Mutex::new(Graph::default())))
}

fn lock(graph: &Mutex<Graph>) -> MutexGuard<'_, Graph> {
    graph.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `store.head(id)`, read at most once per pinned pass. Errors are never kept.
pub(crate) fn head(store: &Store, id: &str) -> Result<Arc<TaskHead>> {
    let graph = graph(store)?;
    head_in(&graph, store, id)
}

fn head_in(graph: &Mutex<Graph>, store: &Store, id: &str) -> Result<Arc<TaskHead>> {
    if let Some(head) = lock(graph).heads.get(id) {
        return Ok(Arc::clone(head));
    }
    let head = Arc::new(store.head(id)?);
    lock(graph).heads.insert(id.to_owned(), Arc::clone(&head));
    Ok(head)
}

/// `head.to_shell_task()`, converted once per pinned pass: a dependency shared by several
/// Plan members is qualified once per member. A live store converts on every call.
pub(crate) fn shell(store: &Store, head: &TaskHead) -> Result<Arc<Task>> {
    store.pass_derived(
        &format!("dependency-shell\0{}\0{}", head.id, head.rev),
        || head.to_shell_task(),
    )
}

/// Reject malformed graphs before ordinary dependency waiting; Done alone satisfies a dependency.
pub(crate) fn dependency_check<T: TaskFieldsRef + ?Sized>(
    store: &Store,
    task: &T,
) -> Result<Option<String>> {
    let _span = corral_core::timing::span("queue.dependency_check");
    let graph = graph(store)?;
    // The root walks its given dependencies, which may differ from its stored ones. A
    // completed subgraph that contains the root may therefore not be skipped; one without
    // it cannot reach the root, so it holds no cycle through this walk's path.
    let shared = !lock(&graph).acyclic.contains(task.id().as_str());
    Walk {
        store,
        graph: &graph,
        root: task.id(),
        shared,
        path: Vec::new(),
        checked: Vec::new(),
    }
    .visit(task.id(), task.depends_on())?;
    for id in task.depends_on() {
        let dep = head_in(&graph, store, id)?;
        if dep.task.status != Status::Done {
            return Ok(Some(format!(
                "waiting on {} ({})",
                dep.task.id,
                dep.task.status.label()
            )));
        }
    }
    Ok(None)
}

struct Walk<'a> {
    store: &'a Store,
    graph: &'a Mutex<Graph>,
    root: &'a str,
    shared: bool,
    path: Vec<String>,
    checked: Vec<String>,
}

impl Walk<'_> {
    fn visit(&mut self, id: &str, depends_on: &[String]) -> Result<()> {
        if self.path.iter().any(|seen| seen == id) {
            self.path.push(id.to_owned());
            bail!("dependency cycle: {}", self.path.join(" -> "));
        }
        if self.checked.iter().any(|seen| seen == id)
            || (self.shared && id != self.root && lock(self.graph).acyclic.contains(id))
        {
            return Ok(());
        }
        self.path.push(id.to_owned());
        for dep in depends_on {
            let head = head_in(self.graph, self.store, dep)
                .with_context(|| format!("{id}: missing dependency {dep}"))?;
            self.visit(&head.task.id, &head.task.depends_on)?;
        }
        self.path.pop();
        self.checked.push(id.to_owned());
        if id != self.root {
            lock(self.graph).acyclic.insert(id.to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::Task;
    use corral_core::work::{self, Counter, HotScope};

    #[test]
    fn candidates_sharing_a_dependency_chain_read_each_head_once_per_pass() {
        // Given a Done chain C -> B -> A and three Ready candidates that each depend on C.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let mut top: Option<String> = None;
        for title in ["A", "B", "C"] {
            let task = store.create(title, Some("app"), Status::Done, "").unwrap();
            let below = top.clone();
            store
                .update(&task.id, |t| {
                    t.depends_on = below.iter().cloned().collect();
                    Ok(())
                })
                .unwrap();
            top = Some(task.id);
        }
        let top = top.unwrap();
        let candidates: Vec<Task> = (0..3)
            .map(|n| {
                let task = store
                    .create(&format!("Candidate {n}"), Some("app"), Status::Ready, "")
                    .unwrap();
                store
                    .update(&task.id, |t| {
                        t.depends_on = vec![top.clone()];
                        Ok(())
                    })
                    .unwrap()
            })
            .collect();
        // When one pinned pass checks one candidate and another pass checks all three.
        let decodes = |checked: &[Task]| {
            let pass = store.read_only_snapshot().unwrap();
            let _scope = HotScope::enter();
            let before = work::snapshot();
            for task in checked {
                assert_eq!(dependency_check(&pass, task).unwrap(), None);
            }
            work::snapshot()
                .since(&before)
                .get(Counter::TaskHeadDecodes)
        };
        let one = decodes(&candidates[..1]);
        let all = decodes(&candidates);
        // Then the shared chain is decoded once per pass, whatever the number of candidates.
        assert!(one > 0);
        assert_eq!(all, one);
    }
}
