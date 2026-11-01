//! The sky: a torus of 24 × 16 cells, 40 units square. Each cell is one
//! tenant of the fleet, so one `SQLite` file.
//!
//! Formal (`verus/src/geom.rs`): wrapping lands in the sky and picks a cell
//! in range; a cell's index and (column, row) round-trip; the 3 × 3
//! neighbourhood is reflexive and symmetric (docs/SPEC.md §1).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Columns of cells.
pub const COLS: u16 = 24;
/// Rows of cells.
pub const ROWS: u16 = 16;
/// Cells in the sky.
pub const CELLS: u16 = COLS * ROWS;
/// A cell's side, in units.
pub const CELL: f32 = 40.0;
/// The sky's width, in units.
pub const WIDTH: f32 = 960.0;
/// The sky's height, in units.
pub const HEIGHT: f32 = 640.0;

/// One cell of the sky, by index `row · 24 + column`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cell(u16);

impl Cell {
    /// The cell at (`col`, `row`), if that is in the sky.
    #[must_use]
    pub const fn new(col: u16, row: u16) -> Option<Self> {
        if col < COLS && row < ROWS {
            Some(Self(row * COLS + col))
        } else {
            None
        }
    }

    /// The cell with this index, if there is one.
    #[must_use]
    pub const fn from_index(i: u16) -> Option<Self> {
        if i < CELLS { Some(Self(i)) } else { None }
    }

    /// Its index, `0..384`.
    #[must_use]
    pub const fn index(self) -> u16 {
        self.0
    }

    /// Its column.
    #[must_use]
    pub const fn col(self) -> u16 {
        self.0 % COLS
    }

    /// Its row.
    #[must_use]
    pub const fn row(self) -> u16 {
        self.0 / COLS
    }

    /// Its tenant id in the fleet: `c-CC-RR`.
    #[must_use]
    pub fn key(self) -> String {
        format!("c-{:02}-{:02}", self.col(), self.row())
    }

    /// The cell a tenant id names. Only the exact `c-CC-RR` spelling of a cell
    /// in the sky parses.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        let rest = key.strip_prefix("c-")?;
        let (c, r) = rest.split_once('-')?;
        if c.len() != 2 || r.len() != 2 || !c.bytes().chain(r.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        Self::new(c.parse().ok()?, r.parse().ok()?)
    }

    /// Every cell, in index order.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..CELLS).map(Self)
    }

    /// The 3 × 3 block around this cell on the torus, this cell included.
    #[must_use]
    pub fn neighbours(self) -> [Self; 9] {
        let (c, r) = (self.col(), self.row());
        let mut out = [self; 9];
        let mut k = 0;
        for dr in [ROWS - 1, 0, 1] {
            for dc in [COLS - 1, 0, 1] {
                out[k] = Self(((r + dr) % ROWS) * COLS + (c + dc) % COLS);
                k += 1;
            }
        }
        out
    }

    /// The centre of the cell, in units.
    #[must_use]
    pub fn centre(self) -> (f32, f32) {
        (
            (f32::from(self.col()) + 0.5) * CELL,
            (f32::from(self.row()) + 0.5) * CELL,
        )
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// Wrap `v` into `0..n`. Non-finite values go to 0.
#[must_use]
pub fn wrap(v: f32, n: f32) -> f32 {
    if !v.is_finite() {
        return 0.0;
    }
    let r = v.rem_euclid(n);
    // rem_euclid can round a tiny negative up to exactly `n`.
    if r >= n { 0.0 } else { r }
}

/// The shortest signed offset from `a` to `b` on a circle of length `n`.
#[must_use]
pub fn delta(a: f32, b: f32, n: f32) -> f32 {
    let d = wrap(b - a, n);
    if d > n / 2.0 { d - n } else { d }
}

/// The cell an integer point (whole units) is in: exactly the arithmetic
/// `verus/src/geom.rs` proves in range.
#[must_use]
pub fn cell_of_units(x: i64, y: i64) -> Cell {
    // WIDTH and HEIGHT are whole units; COLS and ROWS fit in u16.
    let wx = x.rem_euclid(960);
    let wy = y.rem_euclid(640);
    let col = u16::try_from(wx / 40).unwrap_or(0);
    let row = u16::try_from(wy / 40).unwrap_or(0);
    Cell(row * COLS + col)
}

/// The cell a point is in, after wrapping it into the sky (SPEC 1.1).
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "wrapped into 0..960, floored"
)]
pub fn cell_of(x: f32, y: f32) -> Cell {
    cell_of_units(
        wrap(x, WIDTH).floor() as i64,
        wrap(y, HEIGHT).floor() as i64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_cell_round_trips_through_its_key_and_index() {
        let mut keys = BTreeSet::new();
        for c in Cell::all() {
            assert_eq!(Cell::from_key(&c.key()), Some(c));
            assert_eq!(Cell::new(c.col(), c.row()), Some(c));
            assert_eq!(Cell::from_index(c.index()), Some(c));
            keys.insert(c.key());
        }
        assert_eq!(keys.len(), usize::from(CELLS));
        assert_eq!(Cell::new(0, 0).map(Cell::key), Some("c-00-00".into()));
        assert_eq!(Cell::new(23, 15).map(Cell::key), Some("c-23-15".into()));
    }

    #[test]
    fn only_exact_keys_parse() {
        for bad in [
            "", "c-", "c-1-1", "c-001-01", "c-24-00", "c-00-16", "x-00-00", "c-0a-00", "c-00-00-",
            "C-00-00", "c-+1-00",
        ] {
            assert_eq!(Cell::from_key(bad), None, "{bad}");
        }
        assert_eq!(Cell::new(24, 0), None);
        assert_eq!(Cell::from_index(CELLS), None);
    }

    #[test]
    fn neighbours_are_nine_distinct_reflexive_and_symmetric() {
        for c in Cell::all() {
            let n = c.neighbours();
            let set: BTreeSet<Cell> = n.iter().copied().collect();
            assert_eq!(set.len(), 9, "{c}");
            assert!(set.contains(&c));
            for d in n {
                assert!(d.neighbours().contains(&c), "{c} sees {d} but not back");
            }
        }
    }

    #[test]
    fn corners_wrap() {
        let c = Cell::new(0, 0).unwrap();
        let n: BTreeSet<String> = c.neighbours().iter().map(|x| x.key()).collect();
        assert!(n.contains("c-23-15") && n.contains("c-01-01") && n.contains("c-23-00"));
        assert_eq!(cell_of(-0.001, -0.001).key(), "c-23-15");
        assert_eq!(cell_of(960.0, 640.0).key(), "c-00-00");
        assert_eq!(cell_of(f32::NAN, f32::INFINITY).key(), "c-00-00");
    }

    proptest! {
        /// SPEC 1.1: total and in range.
        #[test]
        fn any_point_lands_in_a_cell(x in -1e7f32..1e7, y in -1e7f32..1e7) {
            let c = cell_of(x, y);
            prop_assert!(c.col() < COLS && c.row() < ROWS);
            let (wx, wy) = (wrap(x, WIDTH), wrap(y, HEIGHT));
            prop_assert!((0.0..WIDTH).contains(&wx) && (0.0..HEIGHT).contains(&wy));
        }

        #[test]
        fn delta_is_shortest(a in 0f32..960.0, b in 0f32..960.0) {
            let d = delta(a, b, WIDTH);
            prop_assert!(d.abs() <= WIDTH / 2.0 + 1e-3);
            prop_assert!((wrap(a + d, WIDTH) - b).abs() < 1e-2 || (wrap(a + d, WIDTH) - b).abs() > WIDTH - 1e-2);
        }
    }
}
