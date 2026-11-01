-- The boids in one cell of the sky. Every cell is its own fleet file; the
-- control database gets this table too and never uses it.
--
-- `tick` is the conductor tick that wrote the row. An upsert never replaces a
-- newer row, and a delete only removes rows older than the tick that moved the
-- boid (docs/SPEC.md §4.4).
CREATE TABLE boids (
    id INTEGER PRIMARY KEY,
    x REAL NOT NULL,
    y REAL NOT NULL,
    vx REAL NOT NULL,
    vy REAL NOT NULL,
    tick INTEGER NOT NULL
);
