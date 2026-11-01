# Findings: autumn-web 0.9's tenant fleet with ownership and replication, from building Murmuration

Apiary, Pigeonhole and Baton cover the fleet, slot mode and a light-load
failover. Murmuration puts the same ownership and replication under a
heavier load: each host keeps 150–230 SQLite files open and writes every one
of them eight times a second. Measured on a 2-CPU cloud box, release build,
`rpo_secs = 2`, `sync_interval_secs = 1`, `lease_secs = 6`.

1. **The replication lag exceeds `rpo_secs` when a host has many busy
   databases.** Fleet replication runs on one thread per process
   (`FleetReplication::spawn_loop`): each pass ticks every open database's
   replicator in turn, then sleeps `sync_interval_secs`. So a database ships
   once per *pass time + sync interval*, and the pass time grows with the
   number of databases that have new writes. With 152 and 232 hot databases,
   `replication:fleet` reported `worst_lag_seconds` of 3–7 s on host-a and
   6–14 s on host-b, against `rpo_secs = 2`. A takeover then rewound cells by
   up to 3.3–5.0 s (the birds that "came back older"), not the 0.4–0.8 s
   Baton measured with 48 busy databases.
   - The health check does its job: the indicator goes `DOWN` once the lag
     passes 3 × `rpo_secs`.
   - For the docs: the RPO bound in the production guide and next to
     `restore_missing` holds while one pass over every open database fits in
     `rpo_secs`. With many busy databases per process it does not, and
     `replication:fleet`'s `worst_lag_seconds` is the number to watch.
   - For the framework: ship databases in parallel (a small pool), or pick
     each pass's order by lag, so a hot fleet keeps its RPO.

2. **A tenant fleet's ownership follows the slot of the tenant id, as
   documented.** For all 384 cells, the host that answers is exactly the one
   whose range contains `slot_for_key(tenant id)` (`tests/sky.rs`). The
   result is a scatter: host-a served 152 cells and host-b 232, not 192
   each.

3. **Fencing held under load.** Across every run (kill -9, freeze, fail-back,
   thaw), the conductor's per-cell epoch watermark refused nothing, and no
   cell was ever answered by two hosts in one tick. A thawed zombie serves
   zero cells and reports the range under `waiting`.

4. **A standby restores a whole range at once.** host-c's first read after
   claiming touches all 152 of its cells, so it restores 152 files from the
   replica together. With one task per cell this starved the other hosts on
   a 2-CPU box: host-b stopped answering within the conductor's deadline for
   a second or two. The hosts now work on at most 16 cells at a time. Kill
   to every hole closed: 9.8 s (6 s of lease, the rest restores). Worth a
   line in the failover procedure: the first requests after a takeover pay
   for the restores, so a host may want to warm a claimed range in the
   background.

5. **Losses are the birds that moved into the dead host's cells.** Moving
   a bird between cells is two writes in two files; insert-then-delete with
   tick guards never drops one (`verus/src/moves.rs`). A takeover lost 16–78
   birds per run: birds whose upsert into one of the dead host's cells had
   not shipped, while the delete from the cell they left (on the surviving
   host) had committed. That is finding 1 seen from the client.

6. **Smaller notes.**
   - The boot-time connection warning again speaks Postgres to a SQLite
     fleet: "may open 266 connections … Ensure each Postgres server's
     max_connections …" (Pigeonhole's FINDINGS item 2).
   - During a long pass, host-b's `replication:fleet` reported `unowned: 80`
     with no other process on its volume. The doc comment says unowned means
     "open databases that another process replicates"; these were most
     likely databases opened since the pass began, not yet claimed. Worth
     checking, since it reads like a second writer.
   - `SQLite replication tick failed … the checkpoint it needs first was
     blocked by another connection` appears under this write rate. It is
     transient, as the message says, and the next tick succeeds.
