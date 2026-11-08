# CLAUDE.md — Murmuration

A flock of boids whose only home is a SQLite tenant fleet split between
hosts, with ownership and replication. Read `PITCH.md`, then `docs/SPEC.md`.

- `crates/sky` — pure (no IO, async or clock): `geom` (cells, torus),
  `flock` (steering, holes, births), `moves` (the upserts and moves of a
  tick), `durable` (what is replicated; the guard on every delete),
  `resolve` (newest copy wins), `headcount`, `watermark`, `ranges`. Rules go
  here, with property tests.
- `crates/murmuration-app/src/host.rs` — the two batch endpoints. Every cell
  goes through `DatabaseFleet::open`; `NotOwner` is counted, not an error.
  Each cell's writes are one `BEGIN IMMEDIATE … COMMIT`.
- `crates/murmuration-app/src/console/conductor.rs` — one tick: read
  (with each host's replication bound), resolve, count, step, write. `Flock::new` takes the hosts, so
  the test drives it against its own ports.
- `crates/murmuration-app/host.toml` — every host's config template.
- `verus/src/` — models of `geom`, `moves` and `resolve`. Change the runtime
  and the model together.

Rules that matter here:

- Never delete a bird's row until a newer row of it is in a replica
  (`durable::Ledger::guard`; ADR 0003). An acknowledgement is not enough: a
  takeover restores the cell from the replica. That rule is what
  `verus/src/moves.rs` proves never drops a bird, restores included.
- A host's `durable_through_ms` (`host::durable_through`) must never claim
  a write is replicated before it is. It is derived from autumn-web 0.10's
  `replication:fleet` health; switch it to a per-database API when one
  exists (FINDINGS 7).
- A bird new to the conductor is an egg until it is replicated; only then
  does it count. `lost` must stay 0: the sky test and the drill assert it.
- Upserts never overwrite a newer row and deletes only remove older rows
  (`host::cell_sql`); resolution relies on ticks only going up per cell.
- The population identity `population == released + births + found - lost`
  is asserted on every tick of `tests/sky.rs` and by the drill. A new way for
  a bird to appear or vanish must be counted.
- The host builds SQL by formatting numbers it parsed and checked finite;
  never format a string into it.
- Hosts are real processes on purpose (Baton's ADR 0001 applies).
- Ports: console 3000, hosts 3201–3203; `tests/sky.rs` uses 4201–4203.

Gates: `./verify.sh` (authoritative, on the Mac). In the cloud:
`cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`.
