//! A per-cell epoch watermark (SPEC §6).
//!
//! The conductor refuses an answer served under an epoch older than one it
//! has already seen for that cell. Baton's ledger, keyed by cell (the same
//! argument, proved there).

use std::collections::BTreeMap;

use crate::geom::Cell;

/// An answer refused for its epoch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{host} served {cell} at epoch {epoch}, but epoch {watermark} was already seen")]
pub struct Stale {
    /// Who answered.
    pub host: String,
    /// The cell.
    pub cell: Cell,
    /// The epoch it claimed.
    pub epoch: u64,
    /// The highest seen.
    pub watermark: u64,
}

/// The watermarks.
#[derive(Debug, Clone, Default)]
pub struct Watermarks(BTreeMap<Cell, u64>);

impl Watermarks {
    /// Accept `epoch` for `cell`, raising the watermark, unless it is older.
    ///
    /// # Errors
    ///
    /// [`Stale`] when `epoch` is below the watermark.
    pub fn observe(&mut self, host: &str, cell: Cell, epoch: u64) -> Result<(), Stale> {
        let w = self.0.entry(cell).or_insert(0);
        if epoch < *w {
            return Err(Stale {
                host: host.to_owned(),
                cell,
                epoch,
                watermark: *w,
            });
        }
        *w = epoch;
        Ok(())
    }

    /// The watermark of `cell`.
    #[must_use]
    pub fn get(&self, cell: Cell) -> u64 {
        self.0.get(&cell).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn older_is_refused_newer_raises() {
        let c = Cell::from_index(3).unwrap();
        let mut w = Watermarks::default();
        w.observe("a", c, 2).unwrap();
        w.observe("a", c, 2).unwrap();
        assert_eq!(w.observe("c", c, 1).unwrap_err().watermark, 2);
        w.observe("c", c, 5).unwrap();
        assert_eq!(w.get(c), 5);
    }

    proptest! {
        #[test]
        fn monotone(events in proptest::collection::vec((0u16..8, 0u64..6), 0..100)) {
            let mut w = Watermarks::default();
            let mut prev = [0u64; 8];
            for (cell, e) in events {
                let c = Cell::from_index(cell).unwrap();
                let ok = w.observe("h", c, e).is_ok();
                prop_assert_eq!(ok, e >= prev[cell as usize]);
                prop_assert!(w.get(c) >= prev[cell as usize]);
                prev[cell as usize] = w.get(c);
            }
        }
    }
}
