# CLAUDE.md — Murmuration

A flock of boids whose only home is a SQLite tenant fleet split between
hosts, with ownership and replication. Read `PITCH.md`, then `docs/SPEC.md`.

- `crates/sky` — pure (no IO, async or clock): `geom` (cells, torus),
  `flock` (steering, holes, births), `moves` (insert-then-delete), `resolve`
  (newest copy wins), `headcount`, `watermark`, `ranges`. Rules go here, with
  property tests.
- `crates/murmuration-app/src/host.rs` — the two batch endpoints. Every cell
  goes through `DatabaseFleet::open`; `NotOwner` is counted, not an error.
  Each cell's writes are one `BEGIN IMMEDIATE … COMMIT`.
- `crates/murmuration-app/src/console/conductor.rs` — one tick: read,
  resolve, count, step, write in two phases. `Flock::new` takes the hosts, so
  the test drives it against its own ports.
- `crates/murmuration-app/host.toml` — every host's config template.
- `verus/src/` — models of `geom`, `moves` and `resolve`. Change the runtime
  and the model together.

Rules that matter here:

- Never delete a bird from the cell it left until the destination
  acknowledged the upsert (`moves::deletes` takes the acked set). That order
  is what `verus/src/moves.rs` proves never drops a bird.
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
