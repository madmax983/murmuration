// Murmuration — moving a boid between cells never drops it, even across a
// takeover that restores cells from an older replica.
//
//   verus verus/src/moves.rs
//
// Cells are separate SQLite files, so a move is two writes: an upsert into
// the destination and a delete from the source. Each file also ships to a
// replica some time after its writes commit, and a takeover restores a cell
// from that replica: the cell goes back to what had shipped. An upsert that
// committed but had not shipped is gone after the restore.
//
// So the conductor deletes a boid's row only once a NEWER row of the same
// boid is in a replica (docs/SPEC.md §4.1). This file proves §4.2: over any
// sequence of upserts, guarded deletes, ships, restores and failed writes, a
// boid that has a row in a replica always has a row in a live cell.
//
// ── The model ───────────────────────────────────────────────────────────────
//
// A row is keyed by (cell, boid id) and holds its tick. `live` is every cell
// as its host serves it; `rep` is every cell's replica, as last shipped.
//
//   upsert   live: the row's tick goes up, never down (host SQL, §4.4)
//   delete   live: the row goes, only if its tick is below `below` (§4.4)
//   ship     rep(c) := live(c)   (a replica is a past state of its cell)
//   restore  live(c) := rep(c)   (a takeover, or a fail-back, §4.5)
//   failed   nothing changes
//
// `anchor` is ghost state: for each boid with a row in some replica, the
// newest such row (cell, tick). The guard on a delete is `below <= anchor
// tick`. The conductor checks `below <= known(id)`, where `known` is the
// highest tick it has seen confirmed in a replica; `lemma_known_is_allowed`
// shows that implies the guard, because the anchor only moves forward.
//
// Assumptions, outside the model: a host reports a tick as replicated only
// after it is (crates/murmuration-app/src/host.rs derives that bound); only
// the cell's current owner ships it (autumn's fencing); a restore restores
// the newest replica.

use vstd::prelude::*;

verus! {

pub struct Sky {
    /// (cell, id) → tick, as the hosts serve the cells.
    pub live: Map<(int, int), int>,
    /// (cell, id) → tick, as last shipped to the replica.
    pub rep: Map<(int, int), int>,
    /// id → (cell, tick) of its newest replica row. Ghost.
    pub anchor: Map<int, (int, int)>,
}

pub enum Step {
    /// Upsert `id` into cell `c` at tick `t`.
    Upsert { c: int, id: int, t: int },
    /// Delete `id`'s row from cell `c`, if older than `below`.
    Delete { c: int, id: int, below: int },
    /// Cell `c`'s replica catches up with it.
    Ship { c: int },
    /// Cell `c` is restored from its replica.
    Restore { c: int },
    /// A write that failed or timed out: nothing changed.
    Failed,
}

pub open spec fn present(s: Sky, id: int) -> bool {
    exists|c: int| s.live.contains_key((c, id))
}

pub open spec fn anchored(s: Sky, id: int) -> bool {
    s.anchor.contains_key(id)
}

/// The guard the conductor enforces before a delete (SPEC 4.1): a row of
/// `id` at `below` or newer is in a replica.
pub open spec fn allowed(s: Sky, st: Step) -> bool {
    match st {
        Step::Delete { c, id, below } => anchored(s, id) && below <= s.anchor[id].1,
        _ => true,
    }
}

pub open spec fn upserted(s: Sky, c: int, id: int, t: int) -> int {
    if s.live.contains_key((c, id)) && s.live[(c, id)] > t {
        s.live[(c, id)]
    } else {
        t
    }
}

pub open spec fn removes(s: Sky, c: int, id: int, below: int) -> bool {
    s.live.contains_key((c, id)) && s.live[(c, id)] < below
}

/// Shipping cell `c` makes its row of `id` the anchor: when `id` had none,
/// when the anchor was already in `c`, or when the row is newer.
pub open spec fn takes(s: Sky, c: int, id: int) -> bool {
    s.live.contains_key((c, id)) && (!s.anchor.contains_key(id) || s.anchor[id].0 == c
        || s.live[(c, id)] > s.anchor[id].1)
}

pub open spec fn ship(s: Sky, c: int) -> Sky {
    Sky {
        live: s.live,
        rep: Map::new(
            |k: (int, int)|
                if k.0 == c {
                    s.live.contains_key(k)
                } else {
                    s.rep.contains_key(k)
                },
            |k: (int, int)|
                if k.0 == c {
                    s.live[k]
                } else {
                    s.rep[k]
                },
        ),
        anchor: Map::new(
            |id: int| s.anchor.contains_key(id) || s.live.contains_key((c, id)),
            |id: int|
                if takes(s, c, id) {
                    (c, s.live[(c, id)])
                } else {
                    s.anchor[id]
                },
        ),
    }
}

pub open spec fn restore(s: Sky, c: int) -> Sky {
    Sky {
        live: Map::new(
            |k: (int, int)|
                if k.0 == c {
                    s.rep.contains_key(k)
                } else {
                    s.live.contains_key(k)
                },
            |k: (int, int)|
                if k.0 == c {
                    s.rep[k]
                } else {
                    s.live[k]
                },
        ),
        rep: s.rep,
        anchor: s.anchor,
    }
}

pub open spec fn apply(s: Sky, st: Step) -> Sky {
    match st {
        Step::Upsert { c, id, t } => Sky {
            live: s.live.insert((c, id), upserted(s, c, id, t)),
            rep: s.rep,
            anchor: s.anchor,
        },
        Step::Delete { c, id, below } => if removes(s, c, id, below) {
            Sky { live: s.live.remove((c, id)), rep: s.rep, anchor: s.anchor }
        } else {
            s
        },
        Step::Ship { c } => ship(s, c),
        Step::Restore { c } => restore(s, c),
        Step::Failed => s,
    }
}

// ── The invariant ───────────────────────────────────────────────────────────

/// An anchor is a replica row, and its cell still holds the boid live, at
/// the anchor's tick or newer.
pub open spec fn anchor_ok(s: Sky, id: int) -> bool {
    s.anchor.contains_key(id) ==> {
        let a = s.anchor[id];
        &&& s.rep.contains_key((a.0, id))
        &&& s.rep[(a.0, id)] == a.1
        &&& s.live.contains_key((a.0, id))
        &&& s.live[(a.0, id)] >= a.1
    }
}

/// Every replica row belongs to an anchored boid, at the anchor's tick or
/// older.
pub open spec fn row_ok(s: Sky, c: int, id: int) -> bool {
    s.rep.contains_key((c, id)) ==> s.anchor.contains_key(id) && s.rep[(c, id)]
        <= s.anchor[id].1
}

pub open spec fn inv(s: Sky) -> bool {
    &&& forall|id: int| #[trigger] anchor_ok(s, id)
    &&& forall|c: int, id: int| #[trigger] row_ok(s, c, id)
}

pub open spec fn empty() -> Sky {
    Sky { live: Map::empty(), rep: Map::empty(), anchor: Map::empty() }
}

/// An empty sky satisfies the invariant.
pub proof fn lemma_empty_inv()
    ensures
        inv(empty()),
{
    assert forall|id: int| #[trigger] anchor_ok(empty(), id) by {}
    assert forall|c: int, id: int| #[trigger] row_ok(empty(), c, id) by {}
}

/// The invariant makes an anchored boid present: its anchor cell holds it.
pub proof fn lemma_anchored_is_present(s: Sky, id: int)
    requires
        inv(s),
        anchored(s, id),
    ensures
        present(s, id),
{
    assert(anchor_ok(s, id));
    let a = s.anchor[id];
    assert(s.live.contains_key((a.0, id)));
}

/// Every replica row bounds the anchor from below. With
/// `lemma_step_keeps`'s monotonicity, a tick the conductor once saw in a
/// replica stays at or below the anchor: `below <= known` is enough.
pub proof fn lemma_known_is_allowed(s: Sky, c: int, id: int, below: int)
    requires
        inv(s),
        s.rep.contains_key((c, id)),
        below <= s.rep[(c, id)],
    ensures
        allowed(s, Step::Delete { c: c, id: id, below: below }),
{
    assert(row_ok(s, c, id));
}

proof fn lemma_ship_at(s: Sky, c: int, d: int, id: int)
    ensures
        ship(s, c).rep.contains_key((d, id)) == (if d == c {
            s.live.contains_key((d, id))
        } else {
            s.rep.contains_key((d, id))
        }),
        ship(s, c).rep.contains_key((d, id)) ==> ship(s, c).rep[(d, id)] == (if d == c {
            s.live[(d, id)]
        } else {
            s.rep[(d, id)]
        }),
        ship(s, c).anchor.contains_key(id) == (s.anchor.contains_key(id) || s.live.contains_key(
            (c, id),
        )),
        ship(s, c).anchor.contains_key(id) ==> ship(s, c).anchor[id] == (if takes(s, c, id) {
            (c, s.live[(c, id)])
        } else {
            s.anchor[id]
        }),
{
}

proof fn lemma_restore_at(s: Sky, c: int, d: int, id: int)
    ensures
        restore(s, c).live.contains_key((d, id)) == (if d == c {
            s.rep.contains_key((d, id))
        } else {
            s.live.contains_key((d, id))
        }),
        restore(s, c).live.contains_key((d, id)) ==> restore(s, c).live[(d, id)] == (if d == c {
            s.rep[(d, id)]
        } else {
            s.live[(d, id)]
        }),
{
}

/// A ship keeps the invariant, and moves no anchor back.
proof fn lemma_ship_keeps(s: Sky, c: int)
    requires
        inv(s),
    ensures
        inv(ship(s, c)),
        forall|id: int|
            #![trigger s.anchor.contains_key(id)]
            s.anchor.contains_key(id) ==> ship(s, c).anchor.contains_key(id) && ship(
                s,
                c,
            ).anchor[id].1 >= s.anchor[id].1,
{
    let n = ship(s, c);
    assert forall|id: int| #[trigger] anchor_ok(n, id) by {
        lemma_ship_at(s, c, c, id);
        assert(anchor_ok(s, id));
        if n.anchor.contains_key(id) {
            if takes(s, c, id) {
                // The anchor is the row just shipped: in the replica and live.
                assert(n.anchor[id] == (c, s.live[(c, id)]));
            } else {
                // Not taken: the boid had an anchor, in another cell.
                assert(s.anchor.contains_key(id));
                let a = s.anchor[id];
                assert(a.0 != c);
                lemma_ship_at(s, c, a.0, id);
                assert(n.rep.contains_key((a.0, id)));
            }
        }
    }
    assert forall|d: int, id: int| #[trigger] row_ok(n, d, id) by {
        lemma_ship_at(s, c, d, id);
        lemma_ship_at(s, c, c, id);
        assert(anchor_ok(s, id));
        assert(row_ok(s, d, id));
        if n.rep.contains_key((d, id)) {
            if d == c {
                assert(s.live.contains_key((c, id)));
                assert(n.anchor.contains_key(id));
            } else {
                assert(s.anchor.contains_key(id));
                assert(n.anchor.contains_key(id));
                assert(n.anchor[id].1 >= s.anchor[id].1);
            }
        }
    }
    assert forall|id: int| #![trigger s.anchor.contains_key(id)] s.anchor.contains_key(id) implies n.anchor.contains_key(id) && n.anchor[id].1 >= s.anchor[id].1 by {
        lemma_ship_at(s, c, c, id);
        assert(anchor_ok(s, id));
    }
}

/// A restore keeps the invariant: the anchor's row comes back from the
/// replica it is in.
proof fn lemma_restore_keeps(s: Sky, c: int)
    requires
        inv(s),
    ensures
        inv(restore(s, c)),
{
    let n = restore(s, c);
    assert forall|id: int| #[trigger] anchor_ok(n, id) by {
        assert(anchor_ok(s, id));
        if n.anchor.contains_key(id) {
            let a = s.anchor[id];
            lemma_restore_at(s, c, a.0, id);
        }
    }
    assert forall|d: int, id: int| #[trigger] row_ok(n, d, id) by {
        assert(row_ok(s, d, id));
    }
}

/// One allowed step keeps the invariant, and moves no anchor back.
pub proof fn lemma_step_keeps(s: Sky, st: Step)
    requires
        inv(s),
        allowed(s, st),
    ensures
        inv(apply(s, st)),
        forall|id: int|
            #![trigger s.anchor.contains_key(id)]
            s.anchor.contains_key(id) ==> apply(s, st).anchor.contains_key(id) && apply(
                s,
                st,
            ).anchor[id].1 >= s.anchor[id].1,
{
    let n = apply(s, st);
    match st {
        Step::Upsert { c, id, t } => {
            assert forall|b: int| #[trigger] anchor_ok(n, b) by {
                assert(anchor_ok(s, b));
                if n.anchor.contains_key(b) {
                    let a = s.anchor[b];
                    if a.0 == c && b == id {
                        assert(n.live[(c, id)] == upserted(s, c, id, t));
                    } else {
                        assert(n.live[(a.0, b)] == s.live[(a.0, b)]);
                    }
                }
            }
            assert forall|d: int, b: int| #[trigger] row_ok(n, d, b) by {
                assert(row_ok(s, d, b));
            }
        },
        Step::Delete { c, id, below } => {
            if removes(s, c, id, below) {
                assert forall|b: int| #[trigger] anchor_ok(n, b) by {
                    assert(anchor_ok(s, b));
                    if n.anchor.contains_key(b) {
                        let a = s.anchor[b];
                        // The guard: the anchor's row is at `below` or newer,
                        // so it is not the row this delete removes.
                        if a.0 == c && b == id {
                            assert(s.live[(c, id)] >= s.anchor[id].1);
                            assert(false);
                        }
                        assert(n.live.contains_key((a.0, b)));
                        assert(n.live[(a.0, b)] == s.live[(a.0, b)]);
                    }
                }
                assert forall|d: int, b: int| #[trigger] row_ok(n, d, b) by {
                    assert(row_ok(s, d, b));
                }
            }
        },
        Step::Ship { c } => {
            lemma_ship_keeps(s, c);
        },
        Step::Restore { c } => {
            lemma_restore_keeps(s, c);
        },
        Step::Failed => {},
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
    if steps.len() == 0 {
        s
    } else {
        run(apply(s, steps[0]), steps.skip(1))
    }
}

/// SPEC 4.2. **The theorem.** Over any sequence of allowed steps, including
/// restores from the replica, a boid with a row in a replica keeps one, at
/// the same tick or newer, and is present in a live cell after every step.
pub proof fn theorem_moves_never_drop(s: Sky, steps: Seq<Step>, b: int)
    requires
        inv(s),
        all_allowed(s, steps),
        anchored(s, b),
    ensures
        inv(run(s, steps)),
        anchored(run(s, steps), b),
        run(s, steps).anchor[b].1 >= s.anchor[b].1,
        present(run(s, steps), b),
    decreases steps.len(),
{
    if steps.len() == 0 {
        lemma_anchored_is_present(s, b);
    } else {
        lemma_step_keeps(s, steps[0]);
        let n = apply(s, steps[0]);
        assert(s.anchor.contains_key(b));
        theorem_moves_never_drop(n, steps.skip(1), b);
    }
}

fn main() {
}

} // verus!
