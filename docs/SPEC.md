# Murmuration — specification

A flock of boids whose only home is a SQLite fleet split between hosts. Kill
a host and half the sky's cells go dark; the flock steers around the holes
until a standby takes the range back from the replica.

## 1. The sky

- The sky is a torus of `24 × 16` **cells**, each `40 × 40` units
  (`960 × 640` units in all).
- Every cell is one tenant of a `[database.fleet] mode = "tenant"` fleet,
  tenant id `c-XX-YY` (column, row; two digits each). One cell, one SQLite
  file, one `boids` table.
- Ownership is by the slot of the tenant id, so host-a owns a **scattered half
  of the cells**, not the left half of the sky.

### Geometry (`crates/sky/src/geom.rs`, `verus/src/geom.rs`)

- **1.1** `cell_of(x, y)` is total over all finite coordinates: it wraps into
  the sky first, then picks the cell. Its column is in `0..24`, its row in
  `0..16` — formal (integer model), property test (floats).
- **1.2** A cell's key names it and nothing else: `Cell::from_key(c.key())
  == Some(c)` for all 384 cells, and every other string maps to `None` or to
  a cell whose key is that string — formal for the index arithmetic,
  exhaustive test for the strings.
- **1.3** `neighbours(c)` is the 3 × 3 block around `c` on the torus: 9
  distinct cells, `c` among them, and the relation is symmetric — formal.
- **1.4** The interaction radius (36 units) is less than a cell, so a boid
  sees every boid that can affect it by reading its 3 × 3 block — property
  test against a brute-force scan of the whole sky.

## 2. A tick

The conductor (the console process) runs the flock at 8 ticks a second:

1. **Read.** Ask every host for every cell. A host answers the cells it
   serves, with each cell's epoch and rows, and refuses the rest (`421`
   inside the batch). A cell no host answers is a **hole**. Each answer
   also carries the host's replication bound (§4.6), which confirms the
   writes it acknowledged earlier.
2. **Resolve.** One boid id may be in more than one cell (§4). Keep the copy
   with the highest tick; the others are **ghosts**, queued for deletion.
3. **Count.** Compare what was read with what the conductor last had
   acknowledged for each boid (§5).
4. **Step.** Move every visible boid by separation, alignment, cohesion and
   hole avoidance. Boids inside holes are frozen: nobody can read them.
   A boid never moves into a hole cell (§3.2). Births (§3.4).
5. **Write.** Upsert every boid into the cell its new position is in, and
   delete each older copy whose boid has a newer row in a replica (§4.1).
   The other older copies, including the row a move just left, are held. One
   batch per host.

## 3. The flock (`crates/sky/src/flock.rs`)

- **3.1** Speeds stay in `[1.5, 5]` units per tick; positions stay in the sky.
  Property test.
- **3.2** **No boid moves into a hole.** If its new position is in a hole
  cell, it keeps its position and reverses the velocity components that
  crossed. Property test over random flocks and random hole sets.
- **3.3** The step is deterministic: same input, same output. Test.
- **3.4** **Births.** While the population is under the cap (400), a boid with
  at least 4 neighbours spawns a child with probability 1/160 per tick, from
  a hash of `(id, tick)`. Children get fresh ids above every id seen.
  Deterministic; never above the cap. Property test.

## 4. Moving between cells (`crates/sky/src/moves.rs`, `verus/src/moves.rs`)

Files are separate databases: a move across cells is two writes, and either
can fail (the destination's host is dead, the answer times out).

- **4.1** The protocol is **insert, then delete once replicated**: a delete
  of a boid's rows older than `below` is sent only if a row of that boid at
  `below` or newer is known to be in a replica (§4.6). Until then the older
  row is **held**. A move's source row is such a copy: resolution (§5.1)
  finds it on the next read.
- **4.2** **The protocol never drops a boid.** Over any interleaving of
  upserts, guarded deletes, failed writes, ships to the replica and restores
  from it, a boid with a row in a replica has a row in a live cell after
  every step — formal (`theorem_moves_never_drop`, with a ghost anchor: each
  boid's newest replica row), and run against random ships, reports and
  restores (`durable::a_replicated_boid_is_never_dropped`).
- **4.3** It **duplicates**: every move leaves an older copy until the new
  row ships, and a failed delete leaves one too. The copy has an older tick,
  so §5.1 resolves it.
- **4.4** An upsert never overwrites a newer row (`WHERE excluded.tick >=
  boids.tick`); a delete only deletes rows older than its `below`. Host SQL,
  integration test.
- **4.5** **A restore rewinds, it does not drop.** A takeover or a fail-back
  restores a cell from its replica (autumn restores a local file that is
  older than another owner's replica). A boid whose newest rows had not
  shipped comes back from an older row: *rewound* (§5.2), by up to the delete
  wait. Only a boid with no row in any replica can vanish: an *egg* (§5.4).
- **4.6** **What is replicated.** A cell write answers with the time it
  committed, on the host's clock. A read answers with `durable_through_ms`:
  every write the host committed by then is in the replica. The conductor
  confirms an acknowledged write when it committed by the bound and the host
  still serves its cell at the same epoch; a cell served at another epoch is
  another file, and its unconfirmed writes are dropped. The known tick of a
  boid only goes up. Property tests (`durable`). On autumn-web 0.10 the bound
  is `now - (worst_lag_seconds + 1) s - 1 s` from `replication:fleet`, and
  none while a database is closing or unowned or none is open: `lag` is whole
  seconds rounded down, and the last 1 s covers the checkpoint and retention
  sweep a replication tick does after its caught-up check and before it
  stamps success. Unit test (`host::durable_through`). ADR 0003.

## 5. Resolution and the headcount (`crates/sky/src/resolve.rs`, `verus/src/resolve.rs`)

- **5.1** For every id read, exactly one copy wins: the highest tick, ties
  to the lower cell index. Every other copy is a ghost. Winners and ghosts
  partition the copies — formal for one id's copies (`pick`), property test
  for the grouping.
- **5.2** Each boid the conductor expects is exactly one of:
  - **present** — read at the tick it expects;
  - **rewound** — read at an older tick (its cell came back from a replica);
  - **in a hole** — its expected cell is a hole, so it cannot be seen;
  - **missing** — not read anywhere, and holes remain (it may be inside
    one);
  - **lost** — not read anywhere, and the sky has no holes.
  Property test (partition).
- **5.3** A boid that comes back older is reported with how far back, in
  ms: the time between the tick of the row that came back and the tick of
  its last acknowledged write. That is the age of the replica, at least.
- **5.4** **The headcount adds up.** Every boid the conductor expects was
  released, born or found, and was not lost:
  `population == released + births + found - lost`, on every tick. A boid
  new to the conductor (released, born, or *found*: read with nobody
  expecting it) is an **egg** until a row of it is confirmed in a replica
  (§4.6); only then does it count, and it is never lost after that (§4.2).
  An egg that is in no cell with no holes left is **unhatched**, counted
  apart, never in the population. Asserted on every tick of `tests/sky.rs`
  and by the drill, with `lost == 0`.

## 6. Fencing (`crates/sky/src/watermark.rs`)

Every cell answer carries the epoch it was served under. The conductor keeps
a per-cell watermark and refuses an answer below it — counted as **stale**.
Two hosts answering the same cell in one tick is a **double serve**. Both
should stay at zero.

## 7. The lab (`tests/sky.rs`, `tools/drill.py`)

As Baton: host-a (`0-8191`, `:3201`), host-b (`8192-16383`, `:3202`), host-c
(standby for `0-8191`, empty volume, `:3203`), one shared control database
and one shared replica directory, `lease_secs = 6`, `rpo_secs = 2`,
`sync_interval_secs = 1`. The console is on `:3000`. A tenant fleet with
`create_on_demand = true`, `max_open = 256`, and at most 16 cells worked on
at once per request.

The sky test and the drill check, against real processes:

| # | Claim |
|---|---|
| 7.1 | The cells each primary serves are exactly those whose tenant id's slot is in its range. |
| 7.2 | `kill -9` of host-a makes exactly its cells holes; boids inside them do not move; no flying boid is ever in a hole. |
| 7.3 | The standby fills every hole, at a newer epoch, from the replica. |
| 7.4 | Fail-back to host-a returns exactly its cells; the thawed standby serves none and waits. |
| 7.5 | Throughout: no stale epoch, no cell served twice, each boid drawn once, and 5.4. |

Observed on autumn-web 0.9 (2-CPU cloud box, release build): 20–80 ms a
tick at 400 boids; kill to every hole closed 9.8 s; fail-back 2.8 s; 16–78
boids lost and rewinds of up to 3.3–5.0 s per takeover, because replication
lagged 3–14 s behind `rpo_secs = 2` (FINDINGS 1); zero stale, zero double.

Observed with deletes that wait for the replica (autumn-web 0.10, 4-CPU
cloud box, three drills at 390–400 boids): **zero boids lost** and zero
unhatched, through the takeover and the fail-back; 166–172 came back older
per takeover, by up to 2.8–3.1 s; about 800 older copies held at any time;
89 ms a tick at the median (96 ms before); kill to every hole closed 8.4 s;
zero stale, zero double.

## 8. Scoreboard — what Verus has proved

Run by `./verify.sh` on the Mac with Verus 0.2026.08.23.fbbbbcf. `geom.rs`
and `resolve.rs` verify; `moves.rs` was rewritten for ADR 0003 and has not
been run yet. The cloud build runs no Verus, so only the Mac run counts.

| Obligation | File | Result | Notes |
|---|---|---|---|
| 1.1, 1.2, 1.3 | `geom.rs` | verified (21) | The constants are `spec const` (`int` is ghost-only). 1.3's `lemma_wrap_neg` needed `lemma_fundamental_div_mod`: `v % n + (-v) % n` is a multiple of `n` in `0..2n`, so `0` or `n`. |
| 4.2 | `moves.rs` | **not yet run** | Rewritten for ADR 0003: live cells, the replica, ships and restores, and a ghost anchor (each boid's newest replica row). Two invariants: the anchor is a replica row its cell still holds live; every replica row is at or below its boid's anchor. The cloud build could not run Verus; run `./verify.sh` on the Mac and record the count. |
| 5.1 | `resolve.rs` | verified (4) | A loop invariant with a lexicographic maximum. |

The counts are Verus's "verified" totals per file; a model that gains or
loses a lemma changes its count. Any error fails the gate.
