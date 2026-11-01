# ADR 0001: A cell of the sky is a tenant, in a tenant fleet

Status: accepted, 2026-10-31

## Context

The lab needs space partitioned across hosts so that a host's death removes
part of the world. Slot mode would put several cells in one file whenever
their keys collide, and Pigeonhole already covers that. Ownership in
autumn-web 0.9 is by slot range, and the fleet guide says a tenant fleet
uses the slot of the tenant id, "so the same ranges work in both modes".

## Decision

`[database.fleet] mode = "tenant"`, `create_on_demand = true`. Each of the
384 cells is a tenant, id `c-CC-RR`, so one SQLite file per cell. host-a and
host-b claim `0-8191` and `8192-16383`; which cells each serves follows from
the hash of the tenant id.

## Consequences

- The split is a scatter: host-a serves 152 cells all over the sky, host-b
  232. Losing a host punches holes everywhere instead of removing one half.
- `tests/sky.rs` checks the guide's claim: for all 384 cells, the host that
  answers is the one `autumn_web::sharding::slot_for_key(tenant id)` names.
- Each host keeps up to 232 files open and written eight times a second,
  which is what exposed the replication pass time (FINDINGS 1).
