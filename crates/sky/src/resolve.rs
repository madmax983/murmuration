//! One copy per boid (SPEC 5.1): the highest tick wins, ties to the lower
//! cell. Every other copy is a ghost.
//!
//! Formal (`verus/src/resolve.rs`): [`pick`] returns a copy that beats every
//! copy.

use std::collections::BTreeMap;

use crate::flock::{Boid, BoidId};
use crate::geom::Cell;
use crate::moves::Delete;

/// The index of the winning copy among `(cell, tick)` pairs: the highest tick,
/// ties to the lowest cell. `None` for no copies.
#[must_use]
pub fn pick(copies: &[(Cell, u64)]) -> Option<usize> {
    let mut w = 0;
    for (i, &(cell, tick)) in copies.iter().enumerate().skip(1) {
        let (wc, wt) = copies[w];
        if tick > wt || (tick == wt && cell < wc) {
            w = i;
        }
    }
    (!copies.is_empty()).then_some(w)
}

/// What a read of the sky resolves to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    /// Every boid once, with the cell its winning copy is in.
    pub alive: BTreeMap<BoidId, (Cell, Boid)>,
    /// Older copies to delete, by cell.
    pub ghosts: BTreeMap<Cell, Vec<Delete>>,
}

impl Resolved {
    /// Ghost rows to delete.
    #[must_use]
    pub fn ghost_count(&self) -> usize {
        self.ghosts.values().map(Vec::len).sum()
    }
}

/// Resolve every copy read this tick.
#[must_use]
pub fn resolve(copies: impl IntoIterator<Item = (Cell, Boid)>) -> Resolved {
    let mut by: BTreeMap<BoidId, Vec<(Cell, Boid)>> = BTreeMap::new();
    for (cell, b) in copies {
        by.entry(b.id).or_default().push((cell, b));
    }
    let mut r = Resolved::default();
    for (id, cs) in by {
        let keys: Vec<(Cell, u64)> = cs.iter().map(|(c, b)| (*c, b.tick)).collect();
        let Some(w) = pick(&keys) else { continue };
        for (i, (cell, b)) in cs.iter().enumerate() {
            if i == w {
                r.alive.insert(id, (*cell, *b));
            } else {
                // Delete this exact copy: rows at or below its tick.
                r.ghosts.entry(*cell).or_default().push(Delete {
                    id,
                    below: b.tick + 1,
                });
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::CELLS;
    use proptest::prelude::*;

    fn b(id: u64, tick: u64) -> Boid {
        Boid {
            id: BoidId(id),
            x: 1.0,
            y: 1.0,
            vx: 1.0,
            vy: 0.0,
            tick,
        }
    }

    #[test]
    fn newest_wins_then_lowest_cell() {
        let c = |i| Cell::from_index(i).unwrap();
        assert_eq!(pick(&[]), None);
        assert_eq!(pick(&[(c(5), 3), (c(1), 9), (c(2), 9)]), Some(1));
        let r = resolve([(c(4), b(1, 3)), (c(2), b(1, 7)), (c(3), b(2, 1))]);
        assert_eq!(r.alive[&BoidId(1)].0, c(2));
        assert_eq!(r.alive.len(), 2);
        assert_eq!(
            r.ghosts,
            BTreeMap::from([(
                c(4),
                vec![Delete {
                    id: BoidId(1),
                    below: 4
                }]
            )])
        );
        assert_eq!(r.ghost_count(), 1);
    }

    proptest! {
        /// SPEC 5.1, run: one winner per id, at the highest tick; winners and
        /// ghosts partition the copies.
        #[test]
        fn winners_and_ghosts_partition_the_copies(
            copies in proptest::collection::btree_map((0u16..CELLS, 1u64..20), 0u64..50, 0..200),
        ) {
            let input: Vec<(Cell, Boid)> = copies
                .iter()
                .map(|(&(cell, id), &tick)| (Cell::from_index(cell).unwrap(), b(id, tick)))
                .collect();
            let r = resolve(input.clone());
            prop_assert_eq!(r.alive.len() + r.ghost_count(), input.len());
            for (id, (cell, w)) in &r.alive {
                for (c, o) in input.iter().filter(|(_, o)| o.id == *id) {
                    prop_assert!(w.tick > o.tick || (w.tick == o.tick && cell <= c));
                }
            }
        }
    }
}
