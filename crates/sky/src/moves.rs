//! Moving boids between cells: insert, then delete (SPEC §4).
//!
//! A tick upserts every boid into the cell its new position is in. The row
//! it left stays until a newer row of the boid is in a replica: then it is
//! an older copy that [`crate::resolve`] finds and [`crate::durable`] lets
//! the conductor delete. Formal (`verus/src/moves.rs`): this never drops a
//! boid, even across a restore from a replica that is behind.

use std::collections::BTreeMap;

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

/// A tick's upserts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Every boid, by the cell it is now in.
    pub upserts: BTreeMap<Cell, Vec<Boid>>,
    /// The boids that changed cells: each leaves an older copy behind.
    pub moves: Vec<Move>,
}

/// Plan a tick's upserts. `was` is the cell each boid was read from; a boid not in it
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::CELLS;
    use proptest::prelude::*;

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

    #[test]
    fn a_move_upserts_at_the_destination_and_records_the_source() {
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
    }

    proptest! {
        /// Every boid is upserted once, into the cell it is in; a boid read
        /// in another cell is a move from there; a boid not read (a birth)
        /// is not a move.
        #[test]
        fn a_plan_upserts_each_boid_once_and_names_every_move(
            was in proptest::collection::btree_map(0u64..60, 0u16..CELLS, 0..60),
            now in proptest::collection::btree_map(0u64..60, 0u16..CELLS, 0..60),
        ) {
            let c = |i: u16| Cell::from_index(i).unwrap();
            let was: BTreeMap<BoidId, Cell> = was.into_iter().map(|(id, i)| (BoidId(id), c(i))).collect();
            let next: Vec<Boid> = now.iter().map(|(&id, &i)| at(c(i), id, 9)).collect();
            let p = plan(&was, &next);
            let upserted: usize = p.upserts.values().map(Vec::len).sum();
            prop_assert_eq!(upserted, next.len());
            for (cell, bs) in &p.upserts {
                for b in bs {
                    prop_assert_eq!(b.cell(), *cell);
                }
            }
            for b in &next {
                let moved = p.moves.iter().find(|m| m.id == b.id);
                match was.get(&b.id) {
                    Some(&from) if from != b.cell() => {
                        prop_assert_eq!(moved, Some(&Move { id: b.id, from, to: b.cell(), tick: 9 }));
                    }
                    _ => prop_assert!(moved.is_none()),
                }
            }
        }
    }
}
