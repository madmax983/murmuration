// Murmuration — the sky's geometry, in integers.
//
//   verus verus/src/geom.rs
//
// The runtime takes float coordinates, floors them and hands the integers to
// exactly this arithmetic (`crates/sky/src/geom.rs`, `cell_of_units`). This
// file proves (docs/SPEC.md §1):
//
//   1.1  wrapping any integer coordinate lands in the sky, and the cell it
//        picks is in range;
//   1.2  a cell's index and its (column, row) round-trip;
//   1.3  the 3 x 3 neighbourhood on the torus is symmetric, and contains the
//        cell itself.

use vstd::arithmetic::div_mod::lemma_fundamental_div_mod;
use vstd::prelude::*;

verus! {

pub spec const COLS: int = 24;
pub spec const ROWS: int = 16;
pub spec const CELL: int = 40;
pub spec const WIDTH: int = 960;   // COLS * CELL
pub spec const HEIGHT: int = 640;  // ROWS * CELL

/// Euclidean wrap into `0..n`.
pub open spec fn wrap(v: int, n: int) -> int
    recommends n > 0,
{
    let r = v % n;
    if r < 0 { r + n } else { r }
}

/// SPEC 1.1 (part). `wrap` lands in `0..n`.
pub proof fn lemma_wrap_in_range(v: int, n: int)
    requires
        n > 0,
    ensures
        0 <= wrap(v, n) < n,
{
    // Verus's `%` on int is Euclidean already; this records it.
    assert(0 <= v % n < n) by (nonlinear_arith)
        requires n > 0;
}

pub open spec fn col_of(x: int) -> int {
    wrap(x, WIDTH) / CELL
}

pub open spec fn row_of(y: int) -> int {
    wrap(y, HEIGHT) / CELL
}

/// SPEC 1.1. Every integer point lands in a cell of the sky.
pub proof fn theorem_cell_in_range(x: int, y: int)
    ensures
        0 <= col_of(x) < COLS,
        0 <= row_of(y) < ROWS,
{
    lemma_wrap_in_range(x, WIDTH);
    lemma_wrap_in_range(y, HEIGHT);
    let wx = wrap(x, WIDTH);
    let wy = wrap(y, HEIGHT);
    assert(0 <= wx / CELL < COLS) by (nonlinear_arith)
        requires 0 <= wx < WIDTH, WIDTH == COLS * CELL, CELL > 0;
    assert(0 <= wy / CELL < ROWS) by (nonlinear_arith)
        requires 0 <= wy < HEIGHT, HEIGHT == ROWS * CELL, CELL > 0;
}

pub open spec fn index(col: int, row: int) -> int {
    row * COLS + col
}

/// SPEC 1.2. Index and (column, row) round-trip, for every cell.
pub proof fn theorem_index_round_trip(col: int, row: int)
    requires
        0 <= col < COLS,
        0 <= row < ROWS,
    ensures
        0 <= index(col, row) < COLS * ROWS,
        index(col, row) % COLS == col,
        index(col, row) / COLS == row,
{
    assert(0 <= row * COLS + col < COLS * ROWS) by (nonlinear_arith)
        requires 0 <= col < COLS, 0 <= row < ROWS, COLS > 0;
    assert((row * COLS + col) % COLS == col) by (nonlinear_arith)
        requires 0 <= col < COLS, COLS > 0;
    assert((row * COLS + col) / COLS == row) by (nonlinear_arith)
        requires 0 <= col < COLS, COLS > 0;
}

/// `d` is a neighbour of `c` (including `c` itself) on the torus: each
/// coordinate differs by -1, 0 or +1, modulo the sky.
pub open spec fn near(a: int, b: int, n: int) -> bool {
    wrap(a - b, n) == 0 || wrap(a - b, n) == 1 || wrap(a - b, n) == n - 1
}

pub open spec fn neighbour(c_col: int, c_row: int, d_col: int, d_row: int) -> bool {
    near(c_col, d_col, COLS) && near(c_row, d_row, ROWS)
}

proof fn lemma_wrap_neg(v: int, n: int)
    requires
        n > 2,
    ensures
        wrap(v, n) == 0 <==> wrap(-v, n) == 0,
        wrap(v, n) == 1 <==> wrap(-v, n) == n - 1,
        wrap(v, n) == n - 1 <==> wrap(-v, n) == 1,
{
    // r + s is a multiple of n in 0..2n, so it is 0 or n: s == 0 when r == 0,
    // and s == n - r otherwise.
    let r = v % n;
    let s = (-v) % n;
    assert(0 <= v % n < n) by (nonlinear_arith) requires n > 0;
    assert(0 <= (-v) % n < n) by (nonlinear_arith) requires n > 0;
    assert(wrap(v, n) == r && wrap(-v, n) == s);
    lemma_fundamental_div_mod(v, n);
    lemma_fundamental_div_mod(-v, n);
    let k = -(v / n + (-v) / n);
    assert(r + s == n * k) by (nonlinear_arith)
        requires v == n * (v / n) + r, -v == n * ((-v) / n) + s, k == -(v / n + (-v) / n);
    assert(k == 0 || k == 1) by (nonlinear_arith)
        requires r + s == n * k, 0 <= r < n, 0 <= s < n, n > 0;
}

/// SPEC 1.3. The neighbourhood is symmetric and reflexive.
pub proof fn theorem_neighbours_symmetric(c_col: int, c_row: int, d_col: int, d_row: int)
    ensures
        neighbour(c_col, c_row, c_col, c_row),
        neighbour(c_col, c_row, d_col, d_row) == neighbour(d_col, d_row, c_col, c_row),
{
    lemma_wrap_neg(c_col - d_col, COLS);
    lemma_wrap_neg(c_row - d_row, ROWS);
    assert(-(c_col - d_col) == d_col - c_col);
    assert(-(c_row - d_row) == d_row - c_row);
    assert(wrap(0, COLS) == 0);
    assert(wrap(0, ROWS) == 0);
}

fn main() {
}

} // verus!
