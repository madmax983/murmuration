// Murmuration — moving a boid between cells never drops it.
//
//   verus verus/src/moves.rs
//
// Cells are separate SQLite files, so a move is two writes: an upsert into
// the destination and a delete from the source. Either can fail. The
// conductor sends the delete only after the destination acknowledged the
// upsert (docs/SPEC.md §4.1). This file proves §4.2: over any sequence of
// upserts, guarded deletes and failed writes, a boid that was in some cell
// stays in some cell. It may be in two (§4.3); resolution handles that.
//
// ── The model ───────────────────────────────────────────────────────────────
//
// The sky's contents are a map from cell to the set of boid ids in it. Ticks
// and positions do not affect presence, so they are left out. A failed write
// is a step that changes nothing. A rewind (a restore from an older replica)
// is NOT a step here: §4.5 says loss needs one, and this file is why.

use vstd::prelude::*;

verus! {

pub type Sky = Map<int, Set<int>>;

pub open spec fn in_cell(s: Sky, c: int, id: int) -> bool {
    s.contains_key(c) && s[c].contains(id)
}

pub open spec fn present(s: Sky, id: int) -> bool {
    exists|c: int| in_cell(s, c, id)
}

pub enum Step {
    /// Upsert `id` into cell `c` (acknowledged).
    Upsert { c: int, id: int },
    /// Delete `id` from `src`. The conductor sends it only once `dst` has
    /// acknowledged the upsert: `id` is in `dst`, and `dst != src`.
    Delete { src: int, dst: int, id: int },
    /// A write that failed or timed out: nothing changed.
    Failed,
}

pub open spec fn cells_of(s: Sky, c: int) -> Set<int> {
    if s.contains_key(c) { s[c] } else { Set::empty() }
}

/// The guard the conductor enforces before a delete (SPEC 4.1).
pub open spec fn allowed(s: Sky, st: Step) -> bool {
    match st {
        Step::Delete { src, dst, id } => src != dst && in_cell(s, dst, id),
        _ => true,
    }
}

pub open spec fn apply(s: Sky, st: Step) -> Sky {
    match st {
        Step::Upsert { c, id } => s.insert(c, cells_of(s, c).insert(id)),
        Step::Delete { src, dst, id } => s.insert(src, cells_of(s, src).remove(id)),
        Step::Failed => s,
    }
}

/// One allowed step keeps every present boid present.
pub proof fn lemma_step_keeps(s: Sky, st: Step, b: int)
    requires
        allowed(s, st),
        present(s, b),
    ensures
        present(apply(s, st), b),
{
    let c = choose|c: int| in_cell(s, c, b);
    let n = apply(s, st);
    match st {
        Step::Upsert { c: u, id } => {
            if c == u {
                assert(in_cell(n, u, b));
            } else {
                assert(n[c] == s[c]);
                assert(in_cell(n, c, b));
            }
        },
        Step::Delete { src, dst, id } => {
            if b == id {
                // The guard: `id` is in `dst`, and `dst` is not touched.
                assert(n.contains_key(dst));
                assert(n[dst] == s[dst]);
                assert(in_cell(n, dst, b));
            } else if c == src {
                assert(n[src] == cells_of(s, src).remove(id));
                assert(in_cell(n, src, b));
            } else {
                assert(n[c] == s[c]);
                assert(in_cell(n, c, b));
            }
        },
        Step::Failed => {
            assert(in_cell(n, c, b));
        },
    }
}

pub open spec fn all_allowed(s: Sky, steps: Seq<Step>) -> bool
    decreases steps.len(),
{
    if steps.len() == 0 {
        true
    } else {
        allowed(s, steps[0]) && all_allowed(apply(s, steps[0]), steps.skip(1))
    }
}

pub open spec fn run(s: Sky, steps: Seq<Step>) -> Sky
    decreases steps.len(),
{
    if steps.len() == 0 { s } else { run(apply(s, steps[0]), steps.skip(1)) }
}

/// SPEC 4.2. **The theorem.** Any sequence of allowed steps keeps every boid
/// that was present, present.
pub proof fn theorem_moves_never_drop(s: Sky, steps: Seq<Step>, b: int)
    requires
        all_allowed(s, steps),
        present(s, b),
    ensures
        present(run(s, steps), b),
    decreases steps.len(),
{
    if steps.len() > 0 {
        lemma_step_keeps(s, steps[0], b);
        theorem_moves_never_drop(apply(s, steps[0]), steps.skip(1), b);
    }
}

fn main() {
}

} // verus!
