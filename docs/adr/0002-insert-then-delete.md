# ADR 0002: Move a bird with insert-then-delete, and let the newest copy win

Status: accepted, 2026-10-31. Phase 2 amended by ADR 0003: a delete waits for the replica.

## Context

A bird crossing a cell edge leaves one SQLite file for another, often on
another host. There is no transaction across two files, let alone two
processes. Either write can fail: the destination's host is dead, frozen, or
answers too late.

## Decision

- Phase 1 upserts every bird into the cell it is now in. Phase 2 deletes a
  bird from the cell it left, and only if its phase-1 upsert was
  acknowledged.
- Every row carries the tick that wrote it. An upsert never replaces a newer
  row; a delete removes only rows older than the tick that moved the bird.
- On read, if a bird is in two cells, the copy with the highest tick wins
  (ties to the lower cell). The others are ghosts, deleted in the next
  phase 1.

`verus/src/moves.rs` proves the protocol never drops a bird;
`verus/src/resolve.rs` proves the pick is the newest copy.

## Consequences

- A failed delete leaves a duplicate, never a gap; resolution cleans it up.
- A bird can only vanish if a cell goes back in time: a restore from a replica
  that had not received the latest writes. That turns the RPO into a count of
  birds, which is the point of the lab.
- A crossing costs two round trips per tick. At 8 ticks a second and a few
  hundred birds this is cheap.
