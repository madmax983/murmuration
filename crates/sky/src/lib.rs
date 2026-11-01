//! Murmuration's engine: the sky, the flock, and the rules that keep a boid
//! in exactly one cell of a `SQLite` fleet. No IO, no async, no clock.
//!
//! - [`geom`] — 24 × 16 cells on a torus; each cell is one tenant file
//!   (`verus/src/geom.rs`).
//! - [`flock`] — separation, alignment, cohesion, holes, births.
//! - [`moves`] — insert-then-delete between cells; never drops a boid
//!   (`verus/src/moves.rs`).
//! - [`resolve`] — one copy per boid, the newest (`verus/src/resolve.rs`).
//! - [`headcount`] — present, rewound, in a hole, missing, lost.
//! - [`watermark`] — per-cell epoch fencing on the conductor's side.
//! - [`ranges`] — slot ranges and the lab's three hosts.
#![forbid(unsafe_code)]

pub mod flock;
pub mod geom;
pub mod headcount;
pub mod moves;
pub mod ranges;
pub mod resolve;
pub mod watermark;

pub use flock::{Boid, BoidId};
pub use geom::Cell;
pub use ranges::{HostSpec, SLOTS, SlotRange, lab};
