// Murmuration — which copy of a boid wins.
//
//   verus verus/src/resolve.rs
//
// A boid can be read from more than one cell (docs/SPEC.md §4.3). For one
// id's copies, the winner is the copy with the highest tick, ties to the
// lower cell index (§5.1). This file proves the executable `pick` returns
// exactly that copy, so every other copy is strictly older or a later tie,
// and it can be deleted as a ghost without losing the newest state.

use vstd::prelude::*;

verus! {

pub struct Copy {
    pub cell: u16,
    pub tick: u64,
}

/// `a` beats or equals `b`: a higher tick, or the same tick in a cell with a
/// lower or equal index.
pub open spec fn better(a: Copy, b: Copy) -> bool {
    a.tick > b.tick || (a.tick == b.tick && a.cell <= b.cell)
}

/// SPEC 5.1. `pick` returns an index whose copy beats every copy.
pub fn pick(copies: &Vec<Copy>) -> (w: usize)
    requires
        copies@.len() > 0,
    ensures
        w < copies@.len(),
        forall|i: int| 0 <= i < copies@.len() ==> better(copies@[w as int], #[trigger] copies@[i]),
{
    let mut w: usize = 0;
    let mut i: usize = 1;
    while i < copies.len()
        invariant
            0 < copies@.len(),
            1 <= i <= copies@.len(),
            w < i,
            forall|j: int| 0 <= j < i ==> better(copies@[w as int], #[trigger] copies@[j]),
        decreases copies.len() - i,
    {
        let a = &copies[w];
        let b = &copies[i];
        if b.tick > a.tick || (b.tick == a.tick && b.cell < a.cell) {
            // `b` strictly beats `a`, and `better` is transitive.
            assert forall|j: int| 0 <= j < i + 1 implies better(copies@[i as int], #[trigger] copies@[j]) by {
                if j < i as int {
                    assert(better(copies@[w as int], copies@[j]));
                }
            }
            w = i;
        }
        i = i + 1;
    }
    w
}

/// SPEC 5.1. The winner is unique up to equal (tick, cell): two copies that
/// each beat the other are the same copy, as far as resolution can tell.
pub proof fn lemma_better_antisymmetric(a: Copy, b: Copy)
    requires
        better(a, b),
        better(b, a),
    ensures
        a.tick == b.tick && a.cell == b.cell,
{
}

fn main() {
}

} // verus!
