//! The conductor: runs the flock across the hosts, eight ticks a second.
//!
//! ```text
//!   read   every cell from every host ─► served cells, holes
//!   resolve one copy per boid ─► ghosts to delete
//!   confirm what each host says reached its replica ─► known tick per boid
//!   count  against what was acknowledged ─► present / rewound / in a hole / missing / lost
//!   step   the visible boids, births
//!   write  upserts, and deletes of older copies whose boid has a newer row in
//!          a replica (SPEC 4.1); the other older copies are held
//! ```
//!
//! A boid the conductor did not have in a replica yet (released, born or
//! found) is an *egg*: it counts once a row of it is confirmed replicated.
//! Nothing about the flock lives here between ticks except what the conductor
//! expects (the last acknowledged write of each boid), the ledger of what is
//! replicated, and the counters. The flock is whatever the cells say it is.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use sky::durable::{Ack, Ledger, Report};
use sky::flock::{births, seed, step};
use sky::geom::CELLS;
use sky::headcount::{Expect, Status, classify};
use sky::moves::plan;
use sky::resolve::resolve;
use sky::watermark::Watermarks;
use sky::{Boid, BoidId, Cell, HostSpec};

use crate::wire::{CellWrite, ReadReq, ReadResp, WriteReq, WriteResp};

/// Ticks per second the conductor aims for.
pub const TICK: Duration = Duration::from_millis(125);
/// How long a host that answered last time gets.
pub const PATIENT: Duration = Duration::from_millis(1_500);
/// How long a host that did not answer last time gets, until it answers. A
/// frozen host would otherwise hold every tick for [`PATIENT`].
pub const PROBE: Duration = Duration::from_millis(400);
/// The first flock.
pub const SEED: u64 = 60;
/// How long a lost or ghost mark stays on the sky, in ms.
pub const MARK_MS: u64 = 4_000;

/// A mark on the sky where something happened.
#[derive(Debug, Clone, Serialize)]
pub struct Mark {
    /// `lost`, `unhatched` or `rewound`.
    pub kind: &'static str,
    /// Where.
    pub x: f32,
    /// Where.
    pub y: f32,
    /// When, ms on the console's clock.
    pub at_ms: u64,
}

/// A boid that was lost, with when its last write was acknowledged.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LostRec {
    /// The boid.
    pub id: u64,
    /// When its last write was acknowledged.
    pub acked_ms: u64,
    /// When it was found missing with no holes left.
    pub found_ms: u64,
}

/// A boid that came back older than its last acknowledged write.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RewindRec {
    /// The boid.
    pub id: u64,
    /// How much older, in ms: the time between the tick its row came back at
    /// and the tick of its last acknowledged write. The age of the replica the
    /// cell was restored from, at least.
    pub by_ms: u64,
    /// When it was seen, ms.
    pub seen_ms: u64,
}

/// The conductor's counters.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Counts {
    /// Ticks run.
    pub ticks: u64,
    /// Births, counted once a row of the newborn is in a replica.
    pub births: u64,
    /// Boids lost.
    pub lost: u64,
    /// Boids that came back older than their last acknowledged write.
    pub rewound: u64,
    /// Stray older copies deleted: left by a write whose answer was lost, or
    /// brought back by a restore. A move's own older copy is not one.
    pub ghosts: u64,
    /// Answers refused by the epoch watermark.
    pub stale: u64,
    /// Cells two hosts served in one tick.
    pub double: u64,
    /// Upserts whose answer did not come back (they may have landed).
    pub unacked: u64,
    /// Boids of the first flock, counted once a row of each is in a
    /// replica.
    pub released: u64,
    /// Boids read that the conductor did not expect (a write that landed
    /// but whose answer was lost, or a boid that came back after being
    /// counted lost), counted once a row of each is in a replica.
    pub found: u64,
    /// Eggs gone before any row of them reached a replica: their cell was
    /// restored without them. Never counted in the population.
    pub unhatched: u64,
}

/// What the sky looks like after a tick.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Frame {
    /// The tick.
    pub tick: u64,
    /// How long it took, ms.
    pub tick_ms: u64,
    /// `[id, x, y, vx, vy, state]`; state 0 flying, 1 frozen in a hole,
    /// 2 rewound this tick, 3 born this tick.
    pub boids: Vec<(u64, f32, f32, f32, f32, u8)>,
    /// For each cell, the lab index of the host that served it, or -1: a hole.
    pub cells: Vec<i8>,
    /// Recent marks.
    pub marks: Vec<Mark>,
    /// Every boid the conductor expects with a row in a replica:
    /// `released + births + found - lost`.
    pub population: usize,
    /// Boids acknowledged but not yet in a replica.
    pub eggs: usize,
    /// Older copies kept this tick until a newer row is in a replica.
    pub held: usize,
    /// Acknowledged cell writes not yet confirmed replicated.
    pub unconfirmed: usize,
    /// Flying.
    pub flying: usize,
    /// In holes or unaccounted for.
    pub frozen: usize,
    /// Holes now.
    pub holes: usize,
    /// Counters.
    pub counts: Counts,
    /// The most recent losses.
    pub lost: Vec<LostRec>,
    /// The most recent rewinds.
    pub rewinds: Vec<RewindRec>,
}

/// The flock's state between ticks.
#[derive(Debug, Default)]
pub struct Flock {
    hosts: Vec<HostSpec>,
    tick: u64,
    next_id: u64,
    expect: BTreeMap<BoidId, Expect>,
    eggs: BTreeMap<BoidId, Origin>,
    ledger: Ledger,
    moved_from: BTreeSet<(BoidId, Cell)>,
    last_seen: BTreeMap<BoidId, Boid>,
    water: Watermarks,
    marks: VecDeque<Mark>,
    lost: VecDeque<LostRec>,
    rewinds: VecDeque<RewindRec>,
    tick_ms: BTreeMap<u64, u64>,
    counts: Counts,
    slow: Vec<bool>,
    seeded: bool,
}

/// Where an egg came from: the counter it adds to when it hatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Released,
    Born,
    Found,
}

/// A cell write a host acknowledged.
struct Acked {
    host: String,
    epoch: u64,
    committed_ms: u64,
}

/// Something worth a line on the timeline.
pub type Note = String;

async fn post<B: Serialize + Sync, R: DeserializeOwned>(
    client: &reqwest::Client,
    port: u16,
    path: &str,
    body: &B,
    timeout: Duration,
) -> Option<R> {
    let r = client
        .post(format!("http://127.0.0.1:{port}{path}"))
        .timeout(timeout)
        .json(body)
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

/// One tick's reads, resolved.
struct Read {
    /// Which host served each cell, and at what epoch.
    served: BTreeMap<Cell, (usize, u64)>,
    /// Every copy read.
    copies: Vec<(Cell, Boid)>,
}

impl Flock {
    /// A flock that has not started, flying through `hosts`.
    #[must_use]
    pub fn new(hosts: Vec<HostSpec>) -> Self {
        Self {
            slow: vec![false; hosts.len()],
            hosts,
            next_id: SEED + 1,
            ..Self::default()
        }
    }

    fn timeout(&self, host: usize) -> Duration {
        if self.slow[host] { PROBE } else { PATIENT }
    }

    async fn read(&mut self, client: &reqwest::Client, notes: &mut Vec<Note>) -> Read {
        let keys: Vec<String> = Cell::all().map(Cell::key).collect();
        let req = ReadReq { cells: keys };
        let hosts = self.hosts.clone();
        let mut set = tokio::task::JoinSet::new();
        for (i, h) in hosts.iter().enumerate() {
            let (client, req, port, t) = (client.clone(), req.clone(), h.port, self.timeout(i));
            set.spawn(async move {
                (
                    i,
                    post::<_, ReadResp>(&client, port, "/api/cells/read", &req, t).await,
                )
            });
        }
        let mut answers: Vec<(usize, ReadResp)> = Vec::new();
        while let Some(Ok((i, r))) = set.join_next().await {
            self.slow[i] = r.is_none();
            if let Some(r) = r {
                answers.push((i, r));
            }
        }
        answers.sort_by_key(|(i, _)| *i);
        let mut served: BTreeMap<Cell, (usize, u64)> = BTreeMap::new();
        let mut rows: BTreeMap<Cell, Vec<Boid>> = BTreeMap::new();
        for (i, r) in answers {
            let mut report = Report {
                durable_through_ms: r.durable_through_ms,
                serving: BTreeMap::new(),
            };
            for c in r.cells {
                let Some(cell) = Cell::from_key(&c.cell) else {
                    continue;
                };
                if let Err(e) = self.water.observe(&r.host, cell, c.epoch) {
                    self.counts.stale += 1;
                    notes.push(format!("refused {e}"));
                    continue;
                }
                if let Some(&(j, _)) = served.get(&cell) {
                    self.counts.double += 1;
                    notes.push(format!(
                        "{} and {} both served {cell}",
                        hosts[j].name, r.host
                    ));
                }
                served.insert(cell, (i, c.epoch));
                report.serving.insert(cell, c.epoch);
                rows.insert(cell, c.boids);
            }
            let _ = self.ledger.report(&r.host, &report);
        }
        let copies = rows
            .into_iter()
            .flat_map(|(cell, bs)| bs.into_iter().map(move |b| (cell, b)))
            .collect();
        Read { served, copies }
    }

    /// Send each host its cells' writes; the cells acknowledged, with who
    /// committed them, under which epoch, and when.
    async fn write(
        &mut self,
        client: &reqwest::Client,
        by_cell: BTreeMap<Cell, CellWrite>,
        served: &BTreeMap<Cell, (usize, u64)>,
    ) -> BTreeMap<Cell, Acked> {
        let hosts = self.hosts.clone();
        let mut per: BTreeMap<usize, Vec<CellWrite>> = BTreeMap::new();
        for (cell, w) in by_cell {
            if let Some(&(i, _)) = served.get(&cell) {
                per.entry(i).or_default().push(w);
            }
        }
        let mut set = tokio::task::JoinSet::new();
        for (i, cells) in per {
            let (client, port, t) = (client.clone(), hosts[i].port, self.timeout(i));
            set.spawn(async move {
                let req = WriteReq { cells };
                let n = req.cells.len();
                (
                    i,
                    n,
                    post::<_, WriteResp>(&client, port, "/api/cells/write", &req, t).await,
                )
            });
        }
        let mut acked = BTreeMap::new();
        while let Some(Ok((i, n, r))) = set.join_next().await {
            let Some(r) = r else {
                self.slow[i] = true;
                self.counts.unacked += u64::try_from(n).unwrap_or(0);
                continue;
            };
            for d in r.done {
                let Some(cell) = Cell::from_key(&d.cell) else {
                    continue;
                };
                if self.water.observe(&r.host, cell, d.epoch).is_ok() {
                    acked.insert(
                        cell,
                        Acked {
                            host: r.host.clone(),
                            epoch: d.epoch,
                            committed_ms: d.committed_ms,
                        },
                    );
                } else {
                    self.counts.stale += 1;
                }
            }
        }
        acked
    }

    /// Count every egg that has a row in a replica now.
    fn hatch(&mut self) {
        let ready: Vec<(BoidId, Origin)> = self
            .eggs
            .iter()
            .filter(|(id, _)| self.ledger.known(**id).is_some())
            .map(|(id, o)| (*id, *o))
            .collect();
        for (id, origin) in ready {
            self.eggs.remove(&id);
            match origin {
                Origin::Released => self.counts.released += 1,
                Origin::Born => self.counts.births += 1,
                Origin::Found => self.counts.found += 1,
            }
        }
    }

    fn mark(&mut self, kind: &'static str, b: &Boid, now: u64) {
        self.marks.push_back(Mark {
            kind,
            x: b.x,
            y: b.y,
            at_ms: now,
        });
    }

    /// Run one tick. `now` is ms on the console's clock. Returns the frame and
    /// any timeline notes.
    #[allow(
        clippy::too_many_lines,
        reason = "one tick, in the order of the module doc"
    )]
    pub async fn tick(
        &mut self,
        client: &reqwest::Client,
        now: impl Fn() -> u64,
    ) -> (Frame, Vec<Note>) {
        let started = Instant::now();
        let mut notes = Vec::new();
        self.tick += 1;
        let t = self.tick;

        // 1. Read.
        let read = self.read(client, &mut notes).await;
        let holes: BTreeSet<Cell> = Cell::all()
            .filter(|c| !read.served.contains_key(c))
            .collect();

        // 2. Resolve.
        let copies: BTreeSet<(BoidId, Cell)> =
            read.copies.iter().map(|(c, b)| (b.id, *c)).collect();
        let resolved = resolve(read.copies);

        // 3. Count. An egg with a row in a replica hatches; a boid read that
        // nobody expected is adopted, as an egg.
        self.hatch();
        let seen_at = now();
        for (id, (cell, b)) in &resolved.alive {
            if !self.expect.contains_key(id) {
                self.expect.insert(
                    *id,
                    Expect {
                        cell: *cell,
                        tick: b.tick,
                        at_ms: seen_at,
                    },
                );
                self.eggs.insert(*id, Origin::Found);
            }
        }
        let statuses = classify(&self.expect, &resolved.alive, &holes);
        let mut rewound_now = BTreeSet::new();
        let mut lost_now = Vec::new();
        let mut unhatched_now = Vec::new();
        let seen = now();
        for (id, st) in &statuses {
            if self.eggs.contains_key(id) {
                // Never in a replica: a restore can take it, and that is
                // not a loss of a boid the sky had (SPEC 5.4).
                if *st == Status::Lost {
                    unhatched_now.push(*id);
                }
                continue;
            }
            match st {
                Status::Rewound { by } => {
                    rewound_now.insert(*id);
                    if let (Some(e), Some((_, b))) = (self.expect.get(id), resolved.alive.get(id)) {
                        let then = self.tick_ms.get(&b.tick).copied();
                        let tick_len = u64::try_from(TICK.as_millis()).unwrap_or(125);
                        let by_ms = then.map_or(by * tick_len, |t0| e.at_ms.saturating_sub(t0));
                        self.rewinds.push_back(RewindRec {
                            id: id.0,
                            by_ms,
                            seen_ms: seen,
                        });
                    }
                }
                Status::Lost => lost_now.push(*id),
                _ => {}
            }
        }
        if !rewound_now.is_empty() {
            self.counts.rewound += rewound_now.len() as u64;
            notes.push(format!(
                "{} birds came back older than their last acknowledged write",
                rewound_now.len()
            ));
        }
        let at = now();
        for id in &unhatched_now {
            self.expect.remove(id);
            self.eggs.remove(id);
            self.ledger.forget(*id);
            if let Some(b) = self.last_seen.remove(id) {
                self.mark("unhatched", &b, at);
            }
        }
        if !unhatched_now.is_empty() {
            self.counts.unhatched += unhatched_now.len() as u64;
            notes.push(format!(
                "{} eggs never reached a replica: their cells came back without them",
                unhatched_now.len()
            ));
        }
        for id in &lost_now {
            self.ledger.forget(*id);
            if let Some(e) = self.expect.remove(id) {
                self.lost.push_back(LostRec {
                    id: id.0,
                    acked_ms: e.at_ms,
                    found_ms: at,
                });
            }
            if let Some(b) = self.last_seen.remove(id) {
                self.mark("lost", &b, at);
            }
        }
        if !lost_now.is_empty() {
            self.counts.lost += lost_now.len() as u64;
            notes.push(format!(
                "{} birds lost: in no cell, and the sky has no holes",
                lost_now.len()
            ));
        }
        for (id, (_, b)) in &resolved.alive {
            self.next_id = self.next_id.max(id.0 + 1);
            if rewound_now.contains(id) {
                self.mark("rewound", b, at);
            }
        }

        // 4. Step, or seed an empty sky once every cell is served.
        let visible: Vec<Boid> = resolved.alive.values().map(|(_, b)| *b).collect();
        if !visible.is_empty() {
            self.seeded = true;
        }
        let mut first = BTreeSet::new();
        let (next, born) = if !self.seeded && self.expect.is_empty() && holes.is_empty() {
            self.seeded = true;
            notes.push(format!("released {SEED} birds"));
            let s: Vec<Boid> = seed(SEED)
                .into_iter()
                .map(|b| Boid { tick: t, ..b })
                .collect();
            first = s.iter().map(|b| b.id).collect();
            (s, Vec::new())
        } else {
            let stepped = step(&visible, &holes, t);
            let born = births(&stepped, &holes, t, self.next_id, self.expect.len());
            (stepped.boids, born)
        };
        self.next_id += born.len() as u64;
        let born_ids: BTreeSet<BoidId> = born.iter().map(|b| b.id).collect();

        // 5. Write: upserts, and the older copies a replica lets go.
        let was: BTreeMap<BoidId, Cell> = resolved
            .alive
            .iter()
            .map(|(id, (c, _))| (*id, *c))
            .collect();
        let all: Vec<Boid> = next.iter().chain(&born).copied().collect();
        let p = plan(&was, &all);
        let guarded = self.ledger.guard(&resolved.ghosts);
        let mut by_cell: BTreeMap<Cell, CellWrite> = BTreeMap::new();
        for (cell, boids) in &p.upserts {
            by_cell
                .entry(*cell)
                .or_insert_with(|| CellWrite {
                    cell: cell.key(),
                    upserts: vec![],
                    deletes: vec![],
                })
                .upserts
                .clone_from(boids);
        }
        for (cell, ds) in &guarded.send {
            by_cell
                .entry(*cell)
                .or_insert_with(|| CellWrite {
                    cell: cell.key(),
                    upserts: vec![],
                    deletes: vec![],
                })
                .deletes
                .extend(ds.iter().copied());
        }
        let acked = self.write(client, by_cell, &read.served).await;
        let at = now();
        for (cell, a) in &acked {
            let Some(boids) = p.upserts.get(cell) else {
                continue;
            };
            self.ledger.acked(
                &a.host,
                Ack {
                    cell: *cell,
                    epoch: a.epoch,
                    committed_ms: a.committed_ms,
                    rows: boids.iter().map(|b| (b.id, b.tick)).collect(),
                },
            );
            for b in boids {
                // A boid new to the conductor is an egg until a row of it is
                // replicated; one that never lands never existed (its id is
                // not reused).
                if born_ids.contains(&b.id) {
                    self.eggs.insert(b.id, Origin::Born);
                } else if first.contains(&b.id) {
                    self.eggs.insert(b.id, Origin::Released);
                }
                self.expect.insert(
                    b.id,
                    Expect {
                        cell: *cell,
                        tick: b.tick,
                        at_ms: at,
                    },
                );
            }
        }
        // A move leaves its older copy behind, for the guard to delete later;
        // that delete is not a ghost.
        for m in &p.moves {
            if acked.contains_key(&m.to) {
                self.moved_from.insert((m.id, m.from));
            }
        }
        for (cell, ds) in &guarded.send {
            if !acked.contains_key(cell) {
                continue;
            }
            for d in ds {
                if !self.moved_from.remove(&(d.id, *cell)) {
                    self.counts.ghosts += 1;
                }
            }
        }
        self.moved_from.retain(|(id, cell)| {
            self.expect.contains_key(id)
                && (!read.served.contains_key(cell) || copies.contains(&(*id, *cell)))
        });

        // The frame.
        for b in &all {
            self.last_seen.insert(b.id, *b);
        }
        let mut boids: Vec<(u64, f32, f32, f32, f32, u8)> = all
            .iter()
            .map(|b| {
                let state = if born_ids.contains(&b.id) {
                    3
                } else if rewound_now.contains(&b.id) {
                    2
                } else {
                    0
                };
                (b.id.0, b.x, b.y, b.vx, b.vy, state)
            })
            .collect();
        let flying = boids.len();
        for (id, st) in &statuses {
            if matches!(st, Status::InHole | Status::Missing)
                && let Some(b) = self.last_seen.get(id)
            {
                boids.push((id.0, b.x, b.y, b.vx, b.vy, 1));
            }
        }
        let frozen = boids.len() - flying;
        let mut cells = vec![-1i8; usize::from(CELLS)];
        for (cell, (i, _)) in &read.served {
            cells[usize::from(cell.index())] = i8::try_from(*i).unwrap_or(-1);
        }
        let cutoff = at.saturating_sub(MARK_MS);
        while self.marks.front().is_some_and(|m| m.at_ms < cutoff) {
            self.marks.pop_front();
        }
        while self.lost.len() > 400 {
            self.lost.pop_front();
        }
        while self.rewinds.len() > 400 {
            self.rewinds.pop_front();
        }
        self.tick_ms.insert(t, at);
        while self.tick_ms.len() > 4_000 {
            self.tick_ms.pop_first();
        }
        self.counts.ticks += 1;
        let frame = Frame {
            tick: t,
            tick_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            boids,
            cells,
            marks: self.marks.iter().cloned().collect(),
            population: self.expect.len() - self.eggs.len(),
            eggs: self.eggs.len(),
            held: guarded.held,
            unconfirmed: self.ledger.pending(),
            flying,
            frozen,
            holes: holes.len(),
            counts: self.counts,
            lost: self.lost.iter().copied().collect(),
            rewinds: self.rewinds.iter().copied().collect(),
        };
        (frame, notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_that_timed_out_is_probed_briefly_until_it_answers() {
        let mut f = Flock::new(sky::lab());
        assert_eq!(f.timeout(0), PATIENT);
        f.slow[0] = true;
        assert_eq!(f.timeout(0), PROBE);
        assert_eq!(f.timeout(1), PATIENT);
    }

    #[tokio::test]
    async fn with_no_hosts_every_cell_is_a_hole_and_nothing_is_seeded() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        // Nothing listens on these ports; every read fails.
        let hosts = sky::lab()
            .into_iter()
            .enumerate()
            .map(|(i, h)| HostSpec {
                port: 1 + u16::try_from(i).unwrap(),
                ..h
            })
            .collect();
        let mut f = Flock::new(hosts);
        let (frame, _) = f.tick(&client, || 0).await;
        assert_eq!(frame.holes, usize::from(CELLS));
        assert!(frame.boids.is_empty());
        assert!(frame.cells.iter().all(|c| *c == -1));
        assert_eq!(frame.population, 0);
    }
}
