//! kill -9 crash safety of SqliteBackend (plan T-253 sqlite-backend): a child process stops at
//! a barrier inside an operation's transaction and is killed; the reopened database passes its
//! integrity checks and holds exactly the operations committed before. The normal suite kills at
//! every barrier of every operation kind; the ignored stress test runs 30 randomized rounds.

mod backend {
    pub use corral_core::backend::*;
}
mod blob_ref {
    pub use corral_core::blob_ref::*;
}
mod sqlite_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail, ensure};
use corral_core::backend::OpKind;
use sqlite_support::child_role;
use sqlite_support::crash::{self, Script};

const ENTRY: &str = "zz_child_process_role";

/// The entry child processes run; a no-op in a normal test run.
#[test]
fn zz_child_process_role() -> Result<()> {
    match child_role() {
        None => Ok(()),
        Some((role, dir)) if role == "kill" => crash::child(&dir),
        Some((role, _)) => bail!("unknown child role {role}"),
    }
}

/// Runs `jobs` on up to eight threads, collecting every failure.
fn run_all(jobs: Vec<(Script, Arc<crash::Plan>, usize)>) -> Result<usize> {
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some((script, plan, point)) = jobs.get(i) else {
                        break;
                    };
                    if let Err(error) = crash::kill_at(ENTRY, *script, plan, *point) {
                        failures
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(format!("{error:#}"));
                    }
                }
            });
        }
    });
    let failures = failures
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ensure!(
        failures.is_empty(),
        "{} kill(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(jobs.len())
}

#[test]
fn a_kill_at_any_barrier_of_any_operation_leaves_it_all_or_nothing() -> Result<()> {
    let mut jobs = Vec::new();
    for kind in OpKind::ALL {
        let script = Script { kind, fillers: 0 };
        let plan = Arc::new(crash::plan(script)?);
        let points = plan.final_points();
        ensure!(!points.is_empty(), "{kind:?} has no barrier points");
        for point in points {
            jobs.push((script, Arc::clone(&plan), point));
        }
    }
    let killed = run_all(jobs)?;
    println!(
        "killed {killed} children at barriers inside all {} operation kinds",
        OpKind::ALL.len()
    );
    Ok(())
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
#[ignore = "stress: 30 randomized kill rounds; run with --ignored"]
fn randomized_kill_rounds() -> Result<()> {
    let seed = match std::env::var("CORRAL_STRESS_SEED") {
        Ok(seed) => seed.parse()?,
        Err(_) => {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos() as u64
                | 1
        }
    };
    println!("seed {seed}");
    let mut rng = XorShift(seed);
    let mut jobs = Vec::new();
    for _ in 0..30 {
        let kind = OpKind::ALL[(rng.next() % OpKind::ALL.len() as u64) as usize];
        let script = Script {
            kind,
            fillers: rng.next() % 4,
        };
        let plan = crash::plan(script)?;
        let point = (rng.next() % plan.points.len() as u64) as usize;
        println!(
            "round: {script:?} killed at point {point} ({})",
            plan.points[point].2
        );
        jobs.push((script, Arc::new(plan), point));
    }
    let killed = run_all(jobs)?;
    println!("{killed}/30 randomized rounds passed (seed {seed})");
    Ok(())
}
