//! A host: a tenant fleet, one tenant per cell of the sky, with ownership
//! and replication; and two batch endpoints the conductor calls every tick.
//!
//! Every cell goes through `DatabaseFleet::open`, which checks that this host
//! owns the cell's slot at its current epoch before it touches the disk. A
//! cell this host does not own is refused inside the batch (the batch's own
//! `421`), so the conductor can ask every host for every cell.
//!
//! Each write answers with the time it committed, and each read with a
//! bound: every write committed by then is in the replica
//! ([`durable_through`]). The conductor deletes a boid's older row only once
//! a newer one is under that bound (SPEC 4.6).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use autumn_web::AppState;
use autumn_web::db::fleet::{DatabaseFleet, FleetDatabase, FleetError};
use autumn_web::prelude::*;
use autumn_web::reexports::axum::response::Response;
use autumn_web::reexports::{diesel, diesel_async};
use diesel_async::{RunQueryDsl as _, SimpleAsyncConnection as _};
use sky::moves::Delete;
use sky::{Boid, BoidId, Cell};

use crate::wire::{CellRows, CellWrite, Done, ReadReq, ReadResp, WriteReq, WriteResp};

/// Cells one request works on at once.
///
/// A standby taking over a range restores every cell from the replica on its
/// first read; unbounded, that is a few hundred restores at once, enough to
/// starve the other hosts.
pub const PARALLEL: usize = 16;

fn owner(state: &AppState) -> String {
    state
        .config()
        .database
        .fleet
        .as_ref()
        .and_then(|f| f.ownership.as_ref())
        .map_or_else(|| "unowned".to_owned(), |o| o.owner.clone())
}

/// Slack for a replication tick's work between its caught-up check and its
/// success stamp.
///
/// That work is a checkpoint and a retention sweep. autumn-web 0.10 stamps
/// `last_success_at` after it, so the stamp can be later than the moment
/// the database was caught up.
pub const SETTLE_MS: u64 = 1_000;

/// Ms on this host's clock: the clock the fleet's replication stamps with.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A time by which every write this host committed is in the replica, from
/// `replication:fleet`'s details read at `now_ms` or later.
///
/// `worst_lag_seconds` is whole seconds, rounded down, of the database that
/// is furthest behind, measured from its last caught-up tick. So every open
/// database caught up after `now - (lag + 1) s`, and everything committed
/// [`SETTLE_MS`] before that has shipped. `None` when the details cannot
/// vouch for every database a write went to: none open, one closing (it
/// ships its tail outside the count), or one another process replicates.
#[must_use]
pub fn durable_through<S: std::hash::BuildHasher>(
    now_ms: u64,
    details: &HashMap<String, serde_json::Value, S>,
) -> Option<u64> {
    let n = |k: &str| details.get(k).and_then(serde_json::Value::as_u64);
    if n("databases")? == 0 || n("closing")? != 0 || n("unowned")? != 0 {
        return None;
    }
    let lag_ms = n("worst_lag_seconds")?.checked_add(1)?.checked_mul(1_000)?;
    now_ms.checked_sub(lag_ms.checked_add(SETTLE_MS)?)
}

/// This host's bound, now. A fleet without replication has no replica to
/// restore a cell from, so a commit is as durable as a write gets: now.
async fn durable_now(fleet: &DatabaseFleet) -> Option<u64> {
    let Some(replication) = fleet.replication() else {
        return Some(now_ms());
    };
    // The clock first: the details are then at least as fresh.
    let now = now_ms();
    let health = replication.indicator().check().await;
    durable_through(now, &health.details)
}

fn fleet(state: &AppState) -> Option<DatabaseFleet> {
    state.shards().and_then(|s| s.fleet()).cloned()
}

fn epoch(db: &FleetDatabase) -> u64 {
    db.epoch().map_or(0, autumn_web::lock::FencingToken::get)
}

/// Why one cell could not be served.
enum Miss {
    /// Not this host's slot.
    NotOwner,
    /// Anything else, with why.
    Failed(String),
}

async fn open(fleet: &DatabaseFleet, cell: &str) -> Result<FleetDatabase, Miss> {
    if Cell::from_key(cell).is_none() {
        return Err(Miss::Failed(format!("{cell}: not a cell")));
    }
    let key = fleet
        .key_for(cell)
        .map_err(|e| Miss::Failed(format!("{cell}: {e}")))?;
    match fleet.open(&key).await {
        Ok(db) => Ok(db),
        Err(FleetError::NotOwner { .. }) => Err(Miss::NotOwner),
        Err(e) => Err(Miss::Failed(format!("{cell}: {e}"))),
    }
}

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
    #[diesel(sql_type = diesel::sql_types::Double)]
    x: f64,
    #[diesel(sql_type = diesel::sql_types::Double)]
    y: f64,
    #[diesel(sql_type = diesel::sql_types::Double)]
    vx: f64,
    #[diesel(sql_type = diesel::sql_types::Double)]
    vy: f64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    tick: i64,
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the rows were written from f32"
)]
fn to_boid(r: &Row) -> Option<Boid> {
    Some(Boid {
        id: BoidId(u64::try_from(r.id).ok()?),
        x: r.x as f32,
        y: r.y as f32,
        vx: r.vx as f32,
        vy: r.vy as f32,
        tick: u64::try_from(r.tick).ok()?,
    })
}

async fn read_cell(fleet: &DatabaseFleet, cell: &str) -> Result<CellRows, Miss> {
    let db = open(fleet, cell).await?;
    let _lease = db.lease();
    let mut conn = db
        .pool()
        .get()
        .await
        .map_err(|e| Miss::Failed(format!("{cell}: {e}")))?;
    let rows: Vec<Row> = diesel::sql_query("SELECT id, x, y, vx, vy, tick FROM boids")
        .load(&mut *conn)
        .await
        .map_err(|e| Miss::Failed(format!("{cell}: {e}")))?;
    Ok(CellRows {
        cell: cell.to_owned(),
        epoch: epoch(&db),
        boids: rows.iter().filter_map(to_boid).collect(),
    })
}

/// Read cells. Answers the ones this host serves; counts the rest.
#[post("/api/cells/read")]
#[public]
pub async fn read(State(state): State<AppState>, Json(req): Json<ReadReq>) -> Response {
    let host = owner(&state);
    let Some(fleet) = fleet(&state) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no fleet").into_response();
    };
    // Before any cell is opened: a cell this read opens afresh was not part
    // of the bound, and its epoch tells the conductor so.
    let durable_through_ms = durable_now(&fleet).await;
    // Each cell in its own task: they run together, and a read that is still
    // restoring a cell from the replica finishes even if the conductor stops
    // waiting (a dropped JoinHandle does not abort its task).
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let tasks: Vec<_> = req
        .cells
        .into_iter()
        .map(|cell| {
            let (fleet, gate) = (fleet.clone(), std::sync::Arc::clone(&gate));
            tokio::spawn(async move {
                let _permit = gate.acquire_owned().await;
                read_cell(&fleet, &cell).await
            })
        })
        .collect();
    let mut resp = ReadResp {
        host,
        cells: Vec::new(),
        refused: 0,
        failed: Vec::new(),
        durable_through_ms,
    };
    for t in tasks {
        match t.await {
            Ok(Ok(rows)) => resp.cells.push(rows),
            Ok(Err(Miss::NotOwner)) => resp.refused += 1,
            Ok(Err(Miss::Failed(why))) => {
                tracing::warn!(%why, "cell read failed");
                resp.failed.push(why);
            }
            Err(e) => {
                tracing::warn!(error = %e, "cell read task failed");
                resp.failed.push(e.to_string());
            }
        }
    }
    Json(resp).into_response()
}

/// The SQL for one cell's writes, as one transaction. Every value is a
/// number the server parsed from JSON, checked finite, so formatting it
/// into the statement cannot inject anything.
///
/// # Errors
///
/// A non-finite number, or an id or tick past `i64`.
pub fn cell_sql(upserts: &[Boid], deletes: &[Delete]) -> Result<String, String> {
    use std::fmt::Write as _;
    let mut sql = String::from("BEGIN IMMEDIATE;\n");
    if !upserts.is_empty() {
        sql.push_str("INSERT INTO boids (id, x, y, vx, vy, tick) VALUES ");
        for (i, b) in upserts.iter().enumerate() {
            if !b.is_finite() {
                return Err(format!("boid {} is not finite", b.id.0));
            }
            let id = i64::try_from(b.id.0).map_err(|e| e.to_string())?;
            let tick = i64::try_from(b.tick).map_err(|e| e.to_string())?;
            if i > 0 {
                sql.push(',');
            }
            let _ = write!(sql, "({id},{},{},{},{},{tick})", b.x, b.y, b.vx, b.vy);
        }
        // Never over a newer row (SPEC 4.4).
        sql.push_str(
            " ON CONFLICT(id) DO UPDATE SET x = excluded.x, y = excluded.y, vx = excluded.vx, \
             vy = excluded.vy, tick = excluded.tick WHERE excluded.tick >= boids.tick;\n",
        );
    }
    for d in deletes {
        let id = i64::try_from(d.id.0).map_err(|e| e.to_string())?;
        let below = i64::try_from(d.below).map_err(|e| e.to_string())?;
        // Only rows older than the tick that moved or superseded it (SPEC 4.4).
        let _ = writeln!(sql, "DELETE FROM boids WHERE id = {id} AND tick < {below};");
    }
    sql.push_str("COMMIT;");
    Ok(sql)
}

async fn write_cell(fleet: &DatabaseFleet, w: &CellWrite) -> Result<Done, Miss> {
    let sql = cell_sql(&w.upserts, &w.deletes).map_err(Miss::Failed)?;
    let db = open(fleet, &w.cell).await?;
    let _lease = db.lease();
    let mut conn = db
        .pool()
        .get()
        .await
        .map_err(|e| Miss::Failed(format!("{}: {e}", w.cell)))?;
    if let Err(e) = conn.batch_execute(&sql).await {
        let _ = conn.batch_execute("ROLLBACK;").await;
        return Err(Miss::Failed(format!("{}: {e}", w.cell)));
    }
    Ok(Done {
        cell: w.cell.clone(),
        epoch: epoch(&db),
        committed_ms: now_ms(),
    })
}

/// Write cells: each cell's upserts and deletes in one transaction.
#[post("/api/cells/write")]
#[public]
pub async fn write(State(state): State<AppState>, Json(req): Json<WriteReq>) -> Response {
    let host = owner(&state);
    let Some(fleet) = fleet(&state) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no fleet").into_response();
    };
    // As for reads: concurrent, and a write runs to its commit or rollback
    // even if the conductor has stopped waiting for the answer.
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let tasks: Vec<_> = req
        .cells
        .into_iter()
        .map(|w| {
            let (fleet, gate) = (fleet.clone(), std::sync::Arc::clone(&gate));
            tokio::spawn(async move {
                let _permit = gate.acquire_owned().await;
                let cell = w.cell.clone();
                (cell, write_cell(&fleet, &w).await)
            })
        })
        .collect();
    let mut resp = WriteResp {
        host,
        done: Vec::new(),
        refused: Vec::new(),
        failed: Vec::new(),
    };
    for t in tasks {
        match t.await {
            Ok((_, Ok(d))) => resp.done.push(d),
            Ok((cell, Err(Miss::NotOwner))) => resp.refused.push(cell),
            Ok((_, Err(Miss::Failed(why)))) => {
                tracing::warn!(%why, "cell write failed");
                resp.failed.push(why);
            }
            Err(e) => {
                tracing::warn!(error = %e, "cell write task failed");
                resp.failed.push(e.to_string());
            }
        }
    }
    Json(resp).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: u64, tick: u64) -> Boid {
        Boid {
            id: BoidId(id),
            x: 1.5,
            y: 2.0,
            vx: -0.25,
            vy: 3.0,
            tick,
        }
    }

    #[test]
    fn the_sql_is_one_transaction_and_guards_ticks() {
        let sql = cell_sql(
            &[b(1, 4), b(2, 4)],
            &[Delete {
                id: BoidId(9),
                below: 4,
            }],
        )
        .unwrap();
        assert!(sql.starts_with("BEGIN IMMEDIATE;") && sql.ends_with("COMMIT;"));
        assert!(sql.contains("(1,1.5,2,-0.25,3,4),(2,1.5,2,-0.25,3,4)"));
        assert!(sql.contains("WHERE excluded.tick >= boids.tick"));
        assert!(sql.contains("DELETE FROM boids WHERE id = 9 AND tick < 4;"));
        assert_eq!(cell_sql(&[], &[]).unwrap(), "BEGIN IMMEDIATE;\nCOMMIT;");
    }

    fn details(pairs: &[(&str, u64)]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|&(k, v)| (k.to_owned(), v.into()))
            .collect()
    }

    #[test]
    fn the_bound_is_the_worst_lag_rounded_up_plus_the_settle_margin() {
        let ok = [
            ("databases", 152),
            ("closing", 0),
            ("unowned", 0),
            ("worst_lag_seconds", 1),
        ];
        assert_eq!(
            durable_through(10_000, &details(&ok)),
            Some(10_000 - 2_000 - SETTLE_MS)
        );
        let mut lagging = ok;
        lagging[3].1 = 0;
        assert_eq!(
            durable_through(10_000, &details(&lagging)),
            Some(10_000 - 1_000 - SETTLE_MS)
        );
        // Too early in the clock to say anything.
        assert_eq!(durable_through(500, &details(&ok)), None);
    }

    #[test]
    fn no_bound_when_a_database_is_outside_the_count() {
        for (key, value) in [("databases", 0), ("closing", 1), ("unowned", 2)] {
            let mut d = details(&[
                ("databases", 152),
                ("closing", 0),
                ("unowned", 0),
                ("worst_lag_seconds", 1),
            ]);
            d.insert(key.to_owned(), value.into());
            assert_eq!(durable_through(10_000, &d), None, "{key} = {value}");
        }
        // A field missing: no bound.
        assert_eq!(
            durable_through(
                10_000,
                &details(&[("databases", 1), ("closing", 0), ("unowned", 0)])
            ),
            None
        );
    }

    #[test]
    fn non_finite_and_huge_values_are_refused() {
        let mut nan = b(1, 1);
        nan.x = f32::NAN;
        assert!(cell_sql(&[nan], &[]).is_err());
        assert!(cell_sql(&[b(u64::MAX, 1)], &[]).is_err());
        assert!(
            cell_sql(
                &[],
                &[Delete {
                    id: BoidId(1),
                    below: u64::MAX
                }]
            )
            .is_err()
        );
    }
}
