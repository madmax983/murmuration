//! Moving boids between cells: insert, then delete (SPEC §4).
//!
//! A tick's writes go out in two phases. Phase 1 upserts every boid into the
//! cell its new position is in. Phase 2 deletes a boid from the cell it left,
//! and only for a boid whose phase-1 upsert was acknowledged. Formal
//! (`verus/src/moves.rs`): this never drops a boid. It can leave a duplicate,
//! which [`crate::resolve`] clears.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::flock::{Boid, BoidId};
use crate::geom::Cell;

/// One boid changing cells this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    /// The boid.
    pub id: BoidId,
    /// The cell it was read from.
    pub from: Cell,
    /// The cell it moves into.
    pub to: Cell,
    /// The tick that moved it.
    pub tick: u64,
}

/// A row to delete: this id, at a tick strictly below `below`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delete {
    /// The boid.
    pub id: BoidId,
    /// Delete only rows older than this tick (SPEC 4.4).
    pub below: u64,
}

/// A tick's phase-1 writes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Every boid, by the cell it is now in.
    pub upserts: BTreeMap<Cell, Vec<Boid>>,
    /// The boids that changed cells.
    pub moves: Vec<Move>,
}

/// Plan phase 1. `was` is the cell each boid was read from; a boid not in it
/// (a birth) has no source to delete from.
#[must_use]
pub fn plan(was: &BTreeMap<BoidId, Cell>, next: &[Boid]) -> Plan {
    let mut p = Plan::default();
    for b in next {
        let to = b.cell();
        p.upserts.entry(to).or_default().push(*b);
        if let Some(&from) = was.get(&b.id)
            && from != to
        {
            p.moves.push(Move {
                id: b.id,
                from,
                to,
                tick: b.tick,
            });
        }
    }
    p
}

/// Plan phase 2: for each move whose destination acknowledged its upsert,
/// delete the boid from the cell it left. A move whose destination did not
/// answer leaves the boid where it was (SPEC 4.1).
#[must_use]
pub fn deletes(moves: &[Move], acked: &BTreeSet<Cell>) -> BTreeMap<Cell, Vec<Delete>> {
    let mut out: BTreeMap<Cell, Vec<Delete>> = BTreeMap::new();
    for m in moves {
        if acked.contains(&m.to) {
            out.entry(m.from).or_default().push(Delete {
                id: m.id,
                below: m.tick,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::CELLS;
    use proptest::prelude::*;

    /// A model of the sky: cell → id → tick.
    type Sky = BTreeMap<Cell, BTreeMap<BoidId, u64>>;

    fn at(cell: Cell, id: u64, tick: u64) -> Boid {
        let (x, y) = cell.centre();
        Boid {
            id: BoidId(id),
            x,
            y,
            vx: 1.0,
            vy: 0.0,
            tick,
        }
    }

    fn present(s: &Sky, id: BoidId) -> bool {
        s.values().any(|c| c.contains_key(&id))
    }

    #[test]
    fn a_move_upserts_at_the_destination_then_deletes_at_the_source() {
        let a = Cell::new(1, 1).unwrap();
        let b = Cell::new(2, 1).unwrap();
        let was = BTreeMap::from([(BoidId(7), a), (BoidId(8), a)]);
        let p = plan(&was, &[at(b, 7, 5), at(a, 8, 5), at(b, 9, 5)]);
        assert_eq!(p.upserts[&b].len(), 2);
        assert_eq!(p.upserts[&a].len(), 1);
        assert_eq!(
            p.moves,
            vec![Move {
                id: BoidId(7),
                from: a,
                to: b,
                tick: 5
            }]
        );
        assert_eq!(deletes(&p.moves, &BTreeSet::new()), BTreeMap::new());
        assert_eq!(
            deletes(&p.moves, &[b].into()),
            BTreeMap::from([(
                a,
                vec![Delete {
                    id: BoidId(7),
                    below: 5
                }]
            )])
        );
    }

    proptest! {
        /// SPEC 4.2, run: whatever subset of cells fails in either phase, no
        /// boid that was in the sky leaves it.
        #[test]
        fn the_protocol_never_drops_a_boid(
            start in proptest::collection::btree_map(1u64..60, 0u16..CELLS, 0..60),
            dest in proptest::collection::vec(0u16..CELLS, 60),
            fail1 in proptest::collection::btree_set(0u16..CELLS, 0..200),
            fail2 in proptest::collection::btree_set(0u16..CELLS, 0..200),
        ) {
            let c = |i: u16| Cell::from_index(i).unwrap();
            let mut sky: Sky = BTreeMap::new();
            let mut was = BTreeMap::new();
            for (&id, &cell) in &start {
                sky.entry(c(cell)).or_default().insert(BoidId(id), 1);
                was.insert(BoidId(id), c(cell));
            }
            let next: Vec<Boid> = start.keys().map(|&id| at(c(dest[usize::try_from(id).unwrap()]), id, 2)).collect();
            let p = plan(&was, &next);
            // Phase 1: failed cells write nothing.
            let mut acked = BTreeSet::new();
            for (cell, boids) in &p.upserts {
                if fail1.contains(&cell.index()) { continue; }
                let rows = sky.entry(*cell).or_default();
                for b in boids {
                    let t = rows.entry(b.id).or_insert(b.tick);
                    *t = (*t).max(b.tick);
                }
                acked.insert(*cell);
            }
            // Phase 2: deletes only after acks; some fail too.
            for (cell, ds) in deletes(&p.moves, &acked) {
                if fail2.contains(&cell.index()) { continue; }
                if let Some(rows) = sky.get_mut(&cell) {
                    for d in ds {
                        if rows.get(&d.id).is_some_and(|t| *t < d.below) {
                            rows.remove(&d.id);
                        }
                    }
                }
            }
            for &id in start.keys() {
                prop_assert!(present(&sky, BoidId(id)), "boid {} was dropped", id);
            }
        }
    }
}
