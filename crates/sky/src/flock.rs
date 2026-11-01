//! The boids: separation, alignment, cohesion, and holes in the sky.
//!
//! A boid only ever looks at the 3 × 3 block of cells around it: the
//! interaction radius is less than a cell (SPEC 1.4), which is what lets the
//! conductor give each boid exactly the rows of nine files.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::geom::{CELL, Cell, HEIGHT, WIDTH, cell_of, delta, wrap};

/// A boid's id. Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoidId(pub u64);

/// One boid, as a row of its cell's `boids` table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Boid {
    /// Its id.
    pub id: BoidId,
    /// Position, units.
    pub x: f32,
    /// Position, units.
    pub y: f32,
    /// Velocity, units per tick.
    pub vx: f32,
    /// Velocity, units per tick.
    pub vy: f32,
    /// The tick that wrote this row.
    pub tick: u64,
}

impl Boid {
    /// The cell it is in.
    #[must_use]
    pub fn cell(&self) -> Cell {
        cell_of(self.x, self.y)
    }

    /// Whether every field is a finite number (the host refuses others).
    #[must_use]
    pub const fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.vx.is_finite() && self.vy.is_finite()
    }
}

/// How far a boid sees, in units. Less than a cell (SPEC 1.4).
pub const RADIUS: f32 = 36.0;
/// Closer than this, boids push apart.
pub const PERSONAL: f32 = 13.0;
/// Fastest, units per tick.
pub const MAX_SPEED: f32 = 5.0;
/// Slowest, units per tick.
pub const MIN_SPEED: f32 = 1.5;
/// How far from a hole's edge a boid starts to turn away.
pub const HOLE_FEAR: f32 = 26.0;
/// Population cap for births.
pub const CAP: usize = 400;
/// Neighbours needed to breed.
pub const CROWD: u32 = 4;
/// One birth per this many crowded boid-ticks, on average.
pub const BIRTH_ODDS: u64 = 160;

/// splitmix64: a small, good, deterministic hash.
#[must_use]
pub const fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A uniform number in `-1..1` from `seed`.
#[must_use]
#[allow(clippy::cast_precision_loss, reason = "a 24-bit fraction")]
pub fn unit(seed: u64) -> f32 {
    ((mix(seed) >> 40) as f32 / (1u64 << 23) as f32) - 1.0
}

/// What a tick of flocking produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Stepped {
    /// Every boid, moved, stamped with the tick, in the input order.
    pub boids: Vec<Boid>,
    /// How many boids each one saw (for births).
    pub crowd: Vec<u32>,
}

fn index(boids: &[Boid]) -> BTreeMap<Cell, Vec<usize>> {
    let mut by: BTreeMap<Cell, Vec<usize>> = BTreeMap::new();
    for (i, b) in boids.iter().enumerate() {
        by.entry(b.cell()).or_default().push(i);
    }
    by
}

/// Push away from every hole cell near `(x, y)`. Holes are read from the
/// 5 × 5 block, so a boid starts turning a cell before it arrives.
fn hole_push(x: f32, y: f32, holes: &BTreeSet<Cell>) -> (f32, f32) {
    if holes.is_empty() {
        return (0.0, 0.0);
    }
    let here = cell_of(x, y);
    let (mut px, mut py) = (0.0, 0.0);
    for dr in [-2.0f32, -1.0, 0.0, 1.0, 2.0] {
        for dc in [-2.0f32, -1.0, 0.0, 1.0, 2.0] {
            let cell = cell_of(
                (f32::from(here.col()) + dc + 0.5) * CELL,
                (f32::from(here.row()) + dr + 0.5) * CELL,
            );
            if !holes.contains(&cell) {
                continue;
            }
            // The nearest point of the hole's square, relative to the boid.
            let (cx, cy) = cell.centre();
            let (ox, oy) = (delta(x, cx, WIDTH), delta(y, cy, HEIGHT));
            let nx = ox.clamp(-CELL / 2.0, CELL / 2.0);
            let ny = oy.clamp(-CELL / 2.0, CELL / 2.0);
            let (ax, ay) = (ox - nx, oy - ny);
            let dist = ax.hypot(ay).max(0.5);
            if dist < HOLE_FEAR {
                let fear = (HOLE_FEAR - dist) / HOLE_FEAR;
                px = (ax / dist).mul_add(-fear * 0.9, px);
                py = (ay / dist).mul_add(-fear * 0.9, py);
            }
        }
    }
    (px, py)
}

/// One tick: every boid steers by its neighbours and the holes, moves, and is
/// stamped with `tick`. A boid never moves into a hole (SPEC 3.2).
#[must_use]
#[allow(clippy::cast_precision_loss, reason = "neighbour counts are small")]
pub fn step(boids: &[Boid], holes: &BTreeSet<Cell>, tick: u64) -> Stepped {
    let by = index(boids);
    let mut out = Vec::with_capacity(boids.len());
    let mut crowd = Vec::with_capacity(boids.len());
    for (i, b) in boids.iter().enumerate() {
        let (mut n, mut ax, mut ay, mut cx, mut cy, mut sx, mut sy) =
            (0u32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for c in b.cell().neighbours() {
            for &j in by.get(&c).map_or(&[][..], Vec::as_slice) {
                if j == i {
                    continue;
                }
                let o = &boids[j];
                let (dx, dy) = (delta(b.x, o.x, WIDTH), delta(b.y, o.y, HEIGHT));
                let d2 = dx.mul_add(dx, dy * dy);
                if d2 >= RADIUS * RADIUS {
                    continue;
                }
                n += 1;
                ax += o.vx;
                ay += o.vy;
                cx += dx;
                cy += dy;
                if d2 < PERSONAL * PERSONAL {
                    let d2 = d2.max(0.25);
                    sx -= dx / d2;
                    sy -= dy / d2;
                }
            }
        }
        let (mut vx, mut vy) = (b.vx, b.vy);
        if n > 0 {
            let k = n as f32;
            // Alignment, cohesion, separation.
            vx += sx.mul_add(1.6, (cx / k).mul_add(0.006, (ax / k - vx) * 0.06));
            vy += sy.mul_add(1.6, (cy / k).mul_add(0.006, (ay / k - vy) * 0.06));
        }
        let (hx, hy) = hole_push(b.x, b.y, holes);
        vx += hx;
        vy += hy;
        // A little wander, so a lone boid does not fly a straight line forever.
        let seed = b.id.0.wrapping_mul(0x1000_0000_01B3) ^ tick;
        vx = unit(seed).mul_add(0.12, vx);
        vy = unit(seed ^ 0xA5A5).mul_add(0.12, vy);
        let s = vx.hypot(vy);
        if s > MAX_SPEED {
            vx *= MAX_SPEED / s;
            vy *= MAX_SPEED / s;
        } else if s < MIN_SPEED {
            if s < 1e-3 {
                (vx, vy) = (MIN_SPEED, 0.0);
            } else {
                vx *= MIN_SPEED / s;
                vy *= MIN_SPEED / s;
            }
        }
        let (mut x, mut y) = (wrap(b.x + vx, WIDTH), wrap(b.y + vy, HEIGHT));
        let to = cell_of(x, y);
        if holes.contains(&to) {
            // Bounce off the hole: stay put, reverse what crossed.
            let from = b.cell();
            if to.col() != from.col() {
                vx = -vx;
            }
            if to.row() != from.row() {
                vy = -vy;
            }
            (x, y) = (b.x, b.y);
        }
        out.push(Boid {
            id: b.id,
            x,
            y,
            vx,
            vy,
            tick,
        });
        crowd.push(n);
    }
    Stepped { boids: out, crowd }
}

/// Births for this tick (SPEC 3.4).
///
/// Crowded boids spawn a child with odds 1 in [`BIRTH_ODDS`], from a hash of
/// `(id, tick)`, until the population reaches [`CAP`]. `population` counts
/// every boid, including those frozen in holes that are not in `stepped`.
/// Children take ids from `next_id` up.
#[must_use]
pub fn births(
    stepped: &Stepped,
    holes: &BTreeSet<Cell>,
    tick: u64,
    next_id: u64,
    population: usize,
) -> Vec<Boid> {
    let mut room = CAP.saturating_sub(population.max(stepped.boids.len()));
    let mut id = next_id;
    let mut out = Vec::new();
    for (b, &n) in stepped.boids.iter().zip(&stepped.crowd) {
        if room == 0 {
            break;
        }
        if n < CROWD || !mix(b.id.0 ^ tick.rotate_left(29)).is_multiple_of(BIRTH_ODDS) {
            continue;
        }
        let seed = mix(b.id.0.wrapping_add(tick));
        let (x, y) = (
            wrap(unit(seed).mul_add(6.0, b.x), WIDTH),
            wrap(unit(seed ^ 1).mul_add(6.0, b.y), HEIGHT),
        );
        if holes.contains(&cell_of(x, y)) {
            continue;
        }
        out.push(Boid {
            id: BoidId(id),
            x,
            y,
            vx: b.vy.mul_add(-0.3, b.vx),
            vy: b.vx.mul_add(0.3, b.vy),
            tick,
        });
        id += 1;
        room -= 1;
    }
    out
}

/// The first flock: `n` boids in four loose clusters, heading roughly together.
#[must_use]
#[allow(clippy::cast_precision_loss, reason = "small counts")]
pub fn seed(n: u64) -> Vec<Boid> {
    let centres = [
        (200.0, 160.0),
        (700.0, 180.0),
        (260.0, 470.0),
        (740.0, 500.0),
    ];
    (0..n)
        .map(|i| {
            let (cx, cy) = centres[usize::from(u8::try_from(i % 4).unwrap_or(0))];
            let h = mix(i + 1);
            let angle = ((i % 4) as f32).mul_add(1.4, unit(h ^ 7).mul_add(0.5, 0.6));
            Boid {
                id: BoidId(i + 1),
                x: wrap(unit(h).mul_add(60.0, cx), WIDTH),
                y: wrap(unit(h ^ 3).mul_add(45.0, cy), HEIGHT),
                vx: angle.cos() * 3.0,
                vy: angle.sin() * 3.0,
                tick: 0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::CELLS;
    use proptest::prelude::*;

    fn boid(id: u64, x: f32, y: f32, vx: f32, vy: f32) -> Boid {
        Boid {
            id: BoidId(id),
            x,
            y,
            vx,
            vy,
            tick: 0,
        }
    }

    fn arb_flock() -> impl Strategy<Value = Vec<Boid>> {
        proptest::collection::vec((0f32..960.0, 0f32..640.0, -5f32..5.0, -5f32..5.0), 0..120)
            .prop_map(|v| {
                v.into_iter()
                    .enumerate()
                    .map(|(i, (x, y, vx, vy))| boid(i as u64 + 1, x, y, vx, vy))
                    .collect()
            })
    }

    fn arb_holes() -> impl Strategy<Value = BTreeSet<Cell>> {
        proptest::collection::btree_set(0u16..CELLS, 0..200)
            .prop_map(|s| s.into_iter().filter_map(Cell::from_index).collect())
    }

    #[test]
    fn the_step_is_deterministic() {
        let f = seed(80);
        let holes: BTreeSet<Cell> = [Cell::new(5, 4).unwrap()].into();
        assert_eq!(step(&f, &holes, 9), step(&f, &holes, 9));
    }

    #[test]
    fn a_boid_heading_into_a_hole_bounces() {
        // At the right edge of cell (3,3), heading right into (4,3).
        let b = boid(1, 159.0, 140.0, 4.0, 0.0);
        let holes: BTreeSet<Cell> = [Cell::new(4, 3).unwrap()].into();
        let s = step(&[b], &holes, 1);
        let n = s.boids[0];
        assert_eq!(n.cell(), Cell::new(3, 3).unwrap());
        assert!(n.vx < 0.0, "{n:?}");
    }

    #[test]
    fn neighbours_steer_together() {
        let a = boid(1, 100.0, 100.0, 3.0, 0.0);
        let b = boid(2, 110.0, 120.0, 0.0, 3.0);
        let s = step(&[a, b], &BTreeSet::new(), 1);
        assert!(s.boids[0].vy > 0.0 && s.boids[1].vx > 0.0);
        assert_eq!(s.crowd, vec![1, 1]);
    }

    #[test]
    fn the_seed_is_valid_and_births_stop_at_the_cap() {
        let f = seed(CAP as u64 + 10);
        assert!(f.iter().all(Boid::is_finite));
        let s = Stepped {
            crowd: vec![99; f.len()],
            boids: f,
        };
        assert!(births(&s, &BTreeSet::new(), 3, 10_000, 0).is_empty());
        // Frozen boids count toward the cap too.
        let few = Stepped {
            crowd: vec![99; 10],
            boids: seed(10),
        };
        assert!(births(&few, &BTreeSet::new(), 3, 10_000, CAP).is_empty());
    }

    proptest! {
        /// SPEC 3.1 and 3.2.
        #[test]
        fn speeds_bounded_positions_in_sky_and_no_boid_enters_a_hole(
            flock in arb_flock(), holes in arb_holes(), tick in 0u64..10_000,
        ) {
            // Boids are read from served cells, so none starts in a hole.
            let flock: Vec<Boid> = flock.into_iter().filter(|b| !holes.contains(&b.cell())).collect();
            let s = step(&flock, &holes, tick);
            prop_assert_eq!(s.boids.len(), flock.len());
            for (b, n) in flock.iter().zip(&s.boids) {
                prop_assert_eq!(b.id, n.id);
                prop_assert_eq!(n.tick, tick);
                let v = n.vx.hypot(n.vy);
                prop_assert!((MIN_SPEED - 1e-3..=MAX_SPEED + 1e-3).contains(&v), "speed {}", v);
                prop_assert!((0.0..WIDTH).contains(&n.x) && (0.0..HEIGHT).contains(&n.y));
                prop_assert!(!holes.contains(&n.cell()), "{:?} entered a hole", n);
            }
        }

        /// SPEC 1.4: the 3 × 3 block sees every boid within the radius.
        #[test]
        fn the_neighbourhood_sees_everything_in_range(flock in arb_flock()) {
            let s = step(&flock, &BTreeSet::new(), 0);
            for (i, b) in flock.iter().enumerate() {
                let brute = flock.iter().enumerate().filter(|(j, o)| {
                    *j != i && {
                        let (dx, dy) = (delta(b.x, o.x, WIDTH), delta(b.y, o.y, HEIGHT));
                        dx.mul_add(dx, dy * dy) < RADIUS * RADIUS
                    }
                }).count();
                prop_assert_eq!(s.crowd[i] as usize, brute);
            }
        }

        /// SPEC 3.4.
        #[test]
        fn births_are_deterministic_capped_fresh_and_outside_holes(
            flock in arb_flock(), holes in arb_holes(), tick in 0u64..10_000, frozen in 0u16..400,
        ) {
            let s = step(&flock, &holes, tick);
            let crowded = Stepped { crowd: vec![CROWD; s.boids.len()], boids: s.boids };
            let frozen = frozen as usize;
            let pop = crowded.boids.len() + frozen;
            let born = births(&crowded, &holes, tick, 1_000, pop);
            prop_assert_eq!(&born, &births(&crowded, &holes, tick, 1_000, pop));
            prop_assert!(pop + born.len() <= CAP.max(pop));
            for (k, b) in born.iter().enumerate() {
                prop_assert_eq!(b.id, BoidId(1_000 + k as u64));
                prop_assert!(!holes.contains(&b.cell()));
                prop_assert!(b.is_finite());
            }
        }
    }
}
