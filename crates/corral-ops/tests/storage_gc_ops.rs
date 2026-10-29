//! lifecycle-gc (T-334): `storage gc` by class, the sync lifecycle pass over finished worktrees,
//! and verified backups, on real Git worktrees and a real store with no herdr.
#![cfg(target_os = "linux")] // Process residency is read from /proc.

mod support;

#[path = "storage_gc_ops/backup.rs"]
mod backup;
#[path = "storage_gc_ops/fixture.rs"]
mod fixture;
#[path = "storage_gc_ops/gc.rs"]
mod gc;
#[path = "storage_gc_ops/lifecycle.rs"]
mod lifecycle;
#[path = "storage_gc_ops/round1.rs"]
mod round1;
