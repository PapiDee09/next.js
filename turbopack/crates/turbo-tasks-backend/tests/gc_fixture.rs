//! Shared graph shapes for the GC tests.

use anyhow::Result;
use turbo_tasks::{ResolvedVc, State, Vc};

/// A flag a test flips to disconnect a subtree. The selector-gated roots that use it live in the
/// individual test files, since each encodes its own disconnect strategy.
#[turbo_tasks::value(transparent)]
pub struct Selector(State<bool>);

#[turbo_tasks::function(operation, root)]
pub fn create_selector(initial: bool) -> Vc<Selector> {
    Selector(State::new(initial)).cell()
}

/// A long-lived State whose *value never changes*, read by leaf tasks purely to make each one
/// **mutable**, so a reader records a real dependency edge on it (an immutable constant records
/// none — see `add_cell_dependency`). The state task is a root and stays alive; the leaves reach it
/// via a dependency edge, not a child edge, so disconnecting the leaves as children still lets them
/// lose activeness.
#[turbo_tasks::value(transparent)]
pub struct Constant(State<u32>);

#[turbo_tasks::function(operation, root)]
pub fn create_constant() -> Vc<Constant> {
    Constant(State::new(0)).cell()
}

// --- Diamond fixture ---
//
// A "diamond": a reader `A` ([`diamond_reader`]) holds a **forward cell-dependency** on a target
// `B` ([`diamond_target`]) that is *not* its child — `B` is called by the root and its resolved
// `Vc` is passed into `A`, so reading it records a dep edge without a child edge. Both are children
// of [`diamond_root`].
//
// That decoupling is the crux: `B`'s only parent is the root, so collecting the root drives BOTH
// `A` and `B` to `parent_count 0` at once and they cascade-collect concurrently — while `A` still
// holds a forward-dep on `B` to scrub. Two tests exercise the two ways that scrub can land:
// same-session (`B` resident but soft-deleted) and cross-session (`B` disk-only, not yet restored).

/// The forward-dependency *target* `B`. Reading `constant`'s `State` makes it mutable, so a reader
/// records a real `cell_dependents` reverse edge on it.
#[turbo_tasks::function]
pub async fn diamond_target(constant: ResolvedVc<Constant>, index: u32) -> Result<Vc<u32>> {
    let base = *constant.await?.get();
    Ok(Vc::cell(base.wrapping_add(index).wrapping_mul(7)))
}

/// The diamond *reader* `A`: reads the cell of a [`diamond_target`] (`B`) **passed in as an
/// already-resolved `Vc`** — so `A` records a forward (cell) dependency on `B` WITHOUT connecting
/// `B` as `A`'s child (a child edge is only created by *calling* a task; here `B` was called by
/// [`diamond_root`]).
#[turbo_tasks::function]
pub async fn diamond_reader(target: ResolvedVc<u32>) -> Result<Vc<u32>> {
    Ok(Vc::cell(1 + *target.await?))
}

/// The diamond root: for each index, calls `diamond_target(index)` (`B`, the root's child) and
/// `diamond_reader(B)` (`A`, the root's child that forward-deps on `B`), parenting both as
/// siblings.
///
/// `fanout` is an argument rather than a constant so each caller picks its own width — it is part
/// of the cache key, so the tests get distinct task instances. A larger fanout gives more chances
/// for the racing interleaving where an `A` scrubs a `B` that has not been restored yet.
#[turbo_tasks::function]
pub async fn diamond_root(constant: ResolvedVc<Constant>, fanout: u32) -> Result<Vc<u32>> {
    let mut sum = 0u32;
    for index in 0..fanout {
        let target = diamond_target(*constant, index).to_resolved().await?;
        sum = sum.wrapping_add(*target.await?);
        sum = sum.wrapping_add(*diamond_reader(*target).await?);
    }
    Ok(Vc::cell(sum))
}

/// [`diamond_root`] as an `(operation, root)` op, for the cross-session test that reads it at the
/// top level of a session — it needs an `OperationVc` to take a `task_id()` from and to pin as a
/// durable root. The same-session tests instead reach the diamond through their own selector-gated
/// root, so they call [`diamond_root`] directly.
#[turbo_tasks::function(operation, root)]
pub async fn diamond_root_op(constant: ResolvedVc<Constant>, fanout: u32) -> Result<Vc<u32>> {
    Ok(diamond_root(*constant, fanout))
}
