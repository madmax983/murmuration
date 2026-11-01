//! Who is where (SPEC 5.2): every boid the conductor expects is present,
//! rewound, in a hole, missing or lost.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::flock::{Boid, BoidId};
use crate::geom::Cell;

/// What the conductor last had acknowledged for one boid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expect {
    /// The cell its last acknowledged upsert went to.
    pub cell: Cell,
    /// That upsert's tick.
    pub tick: u64,
    /// When it was acknowledged, ms on the conductor's clock.
    pub at_ms: u64,
}

/// One boid's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Read at the tick expected (or newer: a write that landed but whose
    /// answer did not come back).
    Present,
    /// Read at an older tick: its cell came back from a replica.
    Rewound {
        /// Ticks it went back.
        by: u64,
    },
    /// Its expected cell is a hole.
    InHole,
    /// Not read anywhere, and holes remain: it may be inside one.
    Missing,
    /// Not read anywhere, and the sky has no holes.
    Lost,
}

/// Classify every expected boid.
#[must_use]
pub fn classify(
    expect: &BTreeMap<BoidId, Expect>,
    alive: &BTreeMap<BoidId, (Cell, Boid)>,
    holes: &BTreeSet<Cell>,
) -> BTreeMap<BoidId, Status> {
    expect
        .iter()
        .map(|(id, e)| {
            let s = match alive.get(id) {
                Some((_, b)) if b.tick >= e.tick => Status::Present,
                Some((_, b)) => Status::Rewound {
                    by: e.tick - b.tick,
                },
                None if holes.contains(&e.cell) => Status::InHole,
                None if !holes.is_empty() => Status::Missing,
                None => Status::Lost,
            };
            (*id, s)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::CELLS;
    use proptest::prelude::*;

    fn boid(id: u64, tick: u64) -> Boid {
        Boid {
            id: BoidId(id),
            x: 0.0,
            y: 0.0,
            vx: 1.0,
            vy: 0.0,
            tick,
        }
    }

    #[test]
    fn each_case() {
        let c = |i| Cell::from_index(i).unwrap();
        let e = |cell, tick| Expect {
            cell: c(cell),
            tick,
            at_ms: 0,
        };
        let expect = BTreeMap::from([
            (BoidId(1), e(1, 10)),
            (BoidId(2), e(2, 10)),
            (BoidId(3), e(3, 10)),
            (BoidId(4), e(4, 10)),
        ]);
        let alive = BTreeMap::from([
            (BoidId(1), (c(1), boid(1, 10))),
            (BoidId(2), (c(2), boid(2, 7))),
        ]);
        let holes: BTreeSet<Cell> = [c(3)].into();
        let s = classify(&expect, &alive, &holes);
        assert_eq!(s[&BoidId(1)], Status::Present);
        assert_eq!(s[&BoidId(2)], Status::Rewound { by: 3 });
        assert_eq!(s[&BoidId(3)], Status::InHole);
        assert_eq!(s[&BoidId(4)], Status::Missing);
        let s = classify(&expect, &alive, &BTreeSet::new());
        assert_eq!(s[&BoidId(3)], Status::Lost);
        assert_eq!(s[&BoidId(4)], Status::Lost);
    }

    proptest! {
        /// SPEC 5.2: every expected boid gets exactly one status, and "lost"
        /// only when there are no holes.
        #[test]
        fn a_partition(
            ex in proptest::collection::btree_map(0u64..80, (0u16..CELLS, 1u64..50), 0..80),
            al in proptest::collection::btree_map(0u64..80, 0u64..60, 0..80),
            holes in proptest::collection::btree_set(0u16..CELLS, 0..20),
        ) {
            let c = |i| Cell::from_index(i).unwrap();
            let expect: BTreeMap<_, _> = ex.iter().map(|(&id, &(cell, tick))| (BoidId(id), Expect { cell: c(cell), tick, at_ms: 0 })).collect();
            let alive: BTreeMap<_, _> = al.iter().map(|(&id, &t)| (BoidId(id), (c(0), boid(id, t)))).collect();
            let holes: BTreeSet<Cell> = holes.into_iter().map(c).collect();
            let s = classify(&expect, &alive, &holes);
            prop_assert_eq!(s.len(), expect.len());
            for st in s.values() {
                if *st == Status::Lost { prop_assert!(holes.is_empty()); }
            }
        }
    }
}
