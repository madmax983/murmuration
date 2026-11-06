# Murmuration

**A flock that lives in 384 SQLite files.** A lab for autumn-web 0.10's
tenant fleet with slot ownership and replication, where failover is
something you can see.

The sky is 24 × 16 cells. Every cell is one tenant of the fleet, so one
SQLite file, and every bird is a row in the file of the cell it is in. A
conductor runs the flock eight times a second: it reads every cell from every
host, steers the birds, and writes them back, moving a bird between files
when it crosses a cell edge. host-a serves the cells whose names hash into its
half of the slot space and host-b the rest, so the split is a scatter across
the sky, not a line down the middle.

## The moment

The flock is three hundred birds wheeling across a dusk sky. You press
**kill -9** on host-a. Half the sky goes black in one tick: 152 squares of
night with stars in them, scattered everywhere. The birds that were inside
them freeze where they were, hollow outlines in the dark. The rest of the
flock does not stop. It bends around the holes, bounces off their edges, and
threads the gaps between them.

You press **Start** on host-c, a standby with an empty volume. Six seconds
later its lease comes through and the holes start closing, each one restored
from the replica. Turn on "show who serves each cell" and the dark squares
come back lime. Then the frozen birds move, and some of them jump backwards:
the replica was a few seconds behind. Red rings show the birds that are gone
for good, the ones that flew into host-a's cells after its last write reached
the replica. The headcount says how many, and that no cell was ever served by
two hosts at once.

## Where it could go

- Rebalance a live range: move part of host-b's slots to host-c without a
  kill, and watch a patch of the sky change colour mid-flight.
- A second replica destination farther away, and a takeover from it.
- Put the conductor in the hosts: each host flies the birds in its own cells
  and hands them across the boundary, with no central loop.
