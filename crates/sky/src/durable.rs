//! What has reached a replica (SPEC 4.6), and the guard on every delete
//! (SPEC 4.1).
//!
//! A host acknowledges a cell's write when it commits, on its own disk. A
//! takeover restores the cell from the replica, so a commit that had not
//! shipped is gone after it. The conductor therefore deletes a boid's row
//! only once a newer row of that boid is known to be in a replica: formal in
//! `verus/src/moves.rs` (the guard is `below <= known`).
//!
//! ```text
//!   write ack   host, cell, epoch, committed_ms, rows ─► pending, per host
//!   read        host, durable_through_ms, cells served at epochs
//!               ─► pending acks committed by then, same host, same epoch:
//!                  confirmed; known(id) = max(known(id), tick)
//!   delete      sent only if below <= known(id); otherwise held
//! ```
//!
//! Both times are the host's own clock; the conductor never compares them
//! with its own.

use std::collections::{BTreeMap, BTreeSet};

use crate::flock::BoidId;
use crate::geom::Cell;
use crate::moves::Delete;

/// An acknowledged write older than this (on its host's clock, behind the
/// host's newest acknowledged write) is dropped unconfirmed. Its rows stay
/// held until a newer write of the same boids is confirmed.
pub const EXPIRE_MS: u64 = 60_000;

/// One cell's acknowledged write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    /// The cell.
    pub cell: Cell,
    /// The epoch the host wrote it under.
    pub epoch: u64,
    /// When it committed, ms on the host's clock (read after the commit).
    pub committed_ms: u64,
    /// The rows upserted: id and tick.
    pub rows: Vec<(BoidId, u64)>,
}

/// One host's read answer, as far as durability goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Everything the host committed at or before this, ms on its clock, is
    /// in the replica, for the databases it had open. `None`: it cannot say.
    pub durable_through_ms: Option<u64>,
    /// The cells it served in this read, with their epochs.
    pub serving: BTreeMap<Cell, u64>,
}

/// The deletes a tick may send, and the ones it holds back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guarded {
    /// Deletes whose boid has a row at `below` or newer in a replica.
    pub send: BTreeMap<Cell, Vec<Delete>>,
    /// Rows kept: no newer row of the boid is known to be replicated yet.
    pub held: usize,
}

/// The conductor's ledger of acknowledged writes and of what is replicated.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pending: BTreeMap<String, Vec<Ack>>,
    known: BTreeMap<BoidId, u64>,
}

impl Ledger {
    /// Record a write `host` acknowledged.
    pub fn acked(&mut self, host: &str, ack: Ack) {
        let acks = self.pending.entry(host.to_owned()).or_default();
        let newest = acks.iter().map(|a| a.committed_ms).max().unwrap_or(0);
        let newest = newest.max(ack.committed_ms);
        acks.push(ack);
        acks.retain(|a| a.committed_ms.saturating_add(EXPIRE_MS) >= newest);
    }

    /// Apply `host`'s report. An ack is confirmed when it committed at or
    /// before `durable_through_ms` and the host still serves its cell at the
    /// same epoch, so it is the same database file. An ack whose cell the
    /// host now serves at another epoch can never be confirmed, and is
    /// dropped. Returns the boids whose known tick rose.
    pub fn report(&mut self, host: &str, r: &Report) -> BTreeSet<BoidId> {
        let mut rose = BTreeSet::new();
        let Some(acks) = self.pending.get_mut(host) else {
            return rose;
        };
        let mut keep = Vec::with_capacity(acks.len());
        for a in acks.drain(..) {
            match r.serving.get(&a.cell) {
                Some(&e) if e != a.epoch => {}
                Some(_) if r.durable_through_ms.is_some_and(|t| a.committed_ms <= t) => {
                    for &(id, tick) in &a.rows {
                        let k = self.known.entry(id).or_insert(tick);
                        if tick >= *k {
                            *k = tick;
                            rose.insert(id);
                        }
                    }
                }
                _ => keep.push(a),
            }
        }
        *acks = keep;
        rose
    }

    /// The newest tick of `id` known to be in a replica.
    #[must_use]
    pub fn known(&self, id: BoidId) -> Option<u64> {
        self.known.get(&id).copied()
    }

    /// Stop tracking a boid that is gone.
    pub fn forget(&mut self, id: BoidId) {
        self.known.remove(&id);
    }

    /// Acknowledged writes not confirmed yet.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.values().map(Vec::len).sum()
    }

    /// Keep the deletes this ledger allows (SPEC 4.1): a delete of rows of
    /// `id` older than `below` only if a row at `below` or newer is in a
    /// replica.
    #[must_use]
    pub fn guard(&self, deletes: &BTreeMap<Cell, Vec<Delete>>) -> Guarded {
        let mut g = Guarded::default();
        for (cell, ds) in deletes {
            for d in ds {
                if self.known(d.id).is_some_and(|k| d.below <= k) {
                    g.send.entry(*cell).or_default().push(*d);
                } else {
                    g.held += 1;
                }
            }
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::Boid;
    use crate::resolve::resolve;
    use proptest::prelude::*;

    fn c(i: u16) -> Cell {
        Cell::from_index(i).unwrap()
    }

    fn ack(cell: u16, epoch: u64, at: u64, rows: &[(u64, u64)]) -> Ack {
        Ack {
            cell: c(cell),
            epoch,
            committed_ms: at,
            rows: rows.iter().map(|&(id, t)| (BoidId(id), t)).collect(),
        }
    }

    fn report(through: Option<u64>, serving: &[(u16, u64)]) -> Report {
        Report {
            durable_through_ms: through,
            serving: serving.iter().map(|&(cell, e)| (c(cell), e)).collect(),
        }
    }

    #[test]
    fn an_ack_is_confirmed_once_its_commit_is_replicated() {
        let mut l = Ledger::default();
        l.acked("a", ack(1, 3, 100, &[(7, 5)]));
        assert_eq!(l.known(BoidId(7)), None);
        // Not yet: committed after the bound, or no bound at all.
        assert!(l.report("a", &report(Some(99), &[(1, 3)])).is_empty());
        assert!(l.report("a", &report(None, &[(1, 3)])).is_empty());
        // Another host's report says nothing about host a.
        assert!(l.report("b", &report(Some(1_000), &[(1, 3)])).is_empty());
        assert_eq!(l.pending(), 1);
        assert_eq!(
            l.report("a", &report(Some(100), &[(1, 3)])),
            [BoidId(7)].into()
        );
        assert_eq!(l.known(BoidId(7)), Some(5));
        assert_eq!(l.pending(), 0);
    }

    #[test]
    fn a_cell_at_another_epoch_drops_the_ack_and_a_missing_cell_keeps_it() {
        let mut l = Ledger::default();
        l.acked("a", ack(1, 3, 100, &[(7, 5)]));
        // Not served this read (a failed read): kept, not confirmed.
        assert!(l.report("a", &report(Some(500), &[])).is_empty());
        assert_eq!(l.pending(), 1);
        // Served at a newer epoch: another file's lineage. Dropped.
        assert!(l.report("a", &report(Some(500), &[(1, 4)])).is_empty());
        assert_eq!(l.pending(), 0);
        assert_eq!(l.known(BoidId(7)), None);
    }

    #[test]
    fn known_never_goes_down_and_old_acks_expire() {
        let mut l = Ledger::default();
        l.acked("a", ack(1, 1, 200, &[(7, 9)]));
        l.report("a", &report(Some(200), &[(1, 1)]));
        l.acked("a", ack(2, 1, 210, &[(7, 4)]));
        l.report("a", &report(Some(210), &[(2, 1)]));
        assert_eq!(l.known(BoidId(7)), Some(9));
        l.acked("a", ack(1, 1, 300, &[(8, 1)]));
        l.acked("a", ack(1, 1, 300 + EXPIRE_MS + 1, &[(8, 2)]));
        assert_eq!(l.pending(), 1);
        l.forget(BoidId(7));
        assert_eq!(l.known(BoidId(7)), None);
    }

    #[test]
    fn the_guard_sends_only_deletes_below_a_replicated_tick() {
        let mut l = Ledger::default();
        l.acked("a", ack(1, 1, 10, &[(7, 5)]));
        l.report("a", &report(Some(10), &[(1, 1)]));
        let d = |id, below| Delete {
            id: BoidId(id),
            below,
        };
        let ds = BTreeMap::from([(c(2), vec![d(7, 5), d(7, 6), d(8, 1)])]);
        let g = l.guard(&ds);
        assert_eq!(g.send, BTreeMap::from([(c(2), vec![d(7, 5)])]));
        assert_eq!(g.held, 2);
    }

    /// A boid in the simulation: the tick of its row in a cell.
    type Rows = BTreeMap<(Cell, BoidId), u64>;

    fn present(live: &Rows, id: BoidId) -> bool {
        live.keys().any(|(_, i)| *i == id)
    }

    fn boid(id: BoidId, tick: u64) -> Boid {
        Boid {
            id,
            x: 0.0,
            y: 0.0,
            vx: 1.0,
            vy: 0.0,
            tick,
        }
    }

    /// What one step of the simulation does.
    #[derive(Debug, Clone)]
    enum Op {
        /// A conductor tick: read, resolve, guarded deletes, every boid
        /// upserted into the cell `dest[id]` picks, acks.
        Tick(Vec<u16>),
        /// A cell ships to its replica.
        Ship(u16),
        /// A host reports.
        Report(u16),
        /// A takeover: a cell is restored from its replica, at a new epoch.
        Restore(u16),
    }

    fn op() -> impl Strategy<Value = Op> {
        let cell = 0u16..8;
        prop_oneof![
            proptest::collection::vec(0u16..8, 12).prop_map(Op::Tick),
            cell.clone().prop_map(Op::Ship),
            (0u16..2).prop_map(Op::Report),
            cell.prop_map(Op::Restore),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// SPEC 4.2, run: the conductor's protocol over hosts that ship and
        /// restore at random. Two hosts, eight cells, twelve boids; a host's
        /// bound is the oldest last-ship of its cells, as a fleet's worst lag
        /// gives it. After every step, every boid with a confirmed row in a
        /// replica is in some live cell.
        #[test]
        fn a_replicated_boid_is_never_dropped(ops in proptest::collection::vec(op(), 1..120)) {
            let host = |cell: u16| cell % 2;
            let name = |h: u16| if h == 0 { "a" } else { "b" };
            let mut live: Rows = BTreeMap::new();
            let mut rep: Rows = BTreeMap::new();
            let mut shipped_at = [0u64; 8];
            let mut epoch = [1u64; 8];
            let mut ledger = Ledger::default();
            let mut clock = 0u64;
            let mut tick = 0u64;
            for id in 0..12u64 {
                live.insert((c(u16::try_from(id % 8).unwrap()), BoidId(id)), 0);
            }
            for o in ops {
                clock += 1;
                match o {
                    Op::Tick(dest) => {
                        tick += 1;
                        let copies: Vec<(Cell, Boid)> =
                            live.iter().map(|(&(cell, id), &t)| (cell, boid(id, t))).collect();
                        let r = resolve(copies);
                        for (cell, ds) in ledger.guard(&r.ghosts).send {
                            for d in ds {
                                if live.get(&(cell, d.id)).is_some_and(|t| *t < d.below) {
                                    live.remove(&(cell, d.id));
                                }
                            }
                        }
                        let mut writes: BTreeMap<Cell, Vec<(BoidId, u64)>> = BTreeMap::new();
                        for id in r.alive.keys() {
                            let to = c(dest[usize::try_from(id.0).unwrap()]);
                            let t = live.entry((to, *id)).or_insert(tick);
                            *t = (*t).max(tick);
                            writes.entry(to).or_default().push((*id, tick));
                        }
                        for (cell, rows) in writes {
                            let i = cell.index();
                            ledger.acked(name(host(i)), Ack {
                                cell, epoch: epoch[usize::from(i)], committed_ms: clock, rows,
                            });
                        }
                    }
                    Op::Ship(i) => {
                        rep.retain(|(cell, _), _| cell.index() != i);
                        for (&(cell, id), &t) in &live {
                            if cell.index() == i {
                                rep.insert((cell, id), t);
                            }
                        }
                        shipped_at[usize::from(i)] = clock;
                    }
                    Op::Report(h) => {
                        let mine: Vec<u16> = (0..8).filter(|i| host(*i) == h).collect();
                        let through = mine.iter().map(|i| shipped_at[usize::from(*i)]).min();
                        let serving = mine.iter().map(|i| (c(*i), epoch[usize::from(*i)])).collect();
                        ledger.report(name(h), &Report { durable_through_ms: through, serving });
                    }
                    Op::Restore(i) => {
                        live.retain(|(cell, _), _| cell.index() != i);
                        for (&(cell, id), &t) in &rep {
                            if cell.index() == i {
                                live.insert((cell, id), t);
                            }
                        }
                        epoch[usize::from(i)] += 1;
                        shipped_at[usize::from(i)] = clock;
                    }
                }
                for id in 0..12u64 {
                    if ledger.known(BoidId(id)).is_some() {
                        prop_assert!(present(&live, BoidId(id)), "boid {} dropped", id);
                    }
                }
            }
        }
    }
}
