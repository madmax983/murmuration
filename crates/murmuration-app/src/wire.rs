//! What the conductor and the hosts send each other. A tick is one read of
//! every cell and up to two batches of writes, per host.

use serde::{Deserialize, Serialize};
use sky::Boid;
use sky::moves::Delete;

/// Read these cells.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadReq {
    /// Tenant ids (`c-CC-RR`).
    pub cells: Vec<String>,
}

/// One cell, as a host serves it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellRows {
    /// Tenant id.
    pub cell: String,
    /// The ownership epoch it is open under.
    pub epoch: u64,
    /// Its rows.
    pub boids: Vec<Boid>,
}

/// A host's answer to a read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResp {
    /// The host.
    pub host: String,
    /// The cells it serves.
    pub cells: Vec<CellRows>,
    /// How many it does not own (`421` for each, inside the batch).
    pub refused: u32,
    /// Cells it owns but could not read, with why.
    pub failed: Vec<String>,
}

/// One cell's writes, applied in one transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellWrite {
    /// Tenant id.
    pub cell: String,
    /// Rows to insert or update (never over a newer row).
    #[serde(default)]
    pub upserts: Vec<Boid>,
    /// Rows to delete (only rows older than each `below`).
    #[serde(default)]
    pub deletes: Vec<Delete>,
}

/// Write these cells.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteReq {
    /// The cells.
    pub cells: Vec<CellWrite>,
}

/// A cell written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Done {
    /// Tenant id.
    pub cell: String,
    /// The epoch it was written under.
    pub epoch: u64,
}

/// A host's answer to a write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResp {
    /// The host.
    pub host: String,
    /// Cells committed.
    pub done: Vec<Done>,
    /// Cells it does not own.
    pub refused: Vec<String>,
    /// Cells it owns but could not write.
    pub failed: Vec<String>,
}
