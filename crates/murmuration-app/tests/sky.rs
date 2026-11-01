//! The lab's claims, against real host processes.
//!
//! Three `murmuration-app host` processes on spare ports (4201–4203), one
//! shared control database and replica, driven by the console's own
//! supervisor and conductor. One test on purpose: the steps are a sequence.
//!
//! What it holds the framework and the conductor to:
//!   1. the primaries split the 384 cells exactly as the slot of each cell's
//!      tenant id says, and the flock flies with no holes;
//!   2. after `kill -9` of host-a, exactly host-a's cells are holes; the
//!      boids inside are frozen where they were, and no flying boid enters a
//!      hole;
//!   3. the standby takes every hole back from the replica, at a newer epoch;
//!   4. fail-back to host-a and a thawed zombie host-c: the zombie serves
//!      nothing;
//!   5. throughout: no stale epoch, no cell served twice, every boid id at
//!      most once in the frame, and
//!      `population == released + births + found - lost`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use murmuration_app::console::conductor::{Flock, Frame};
use murmuration_app::console::supervisor::HostProc;
use murmuration_app::wire::{ReadReq, ReadResp};
use sky::{Cell, HostSpec, SlotRange};

/// Generous: a primary claims and creates its cells in seconds.
const CLAIM: Duration = Duration::from_mins(1);
/// A standby restoring every cell from the replica on a loaded machine.
const TAKEOVER: Duration = Duration::from_mins(2);
const SECRET: &str = "murmuration-test-secret-not-for-production-use-0123456789";

fn spec(name: &'static str, a: u16, b: u16, port: u16) -> HostSpec {
    HostSpec {
        name,
        slots: SlotRange::new(a, b).unwrap(),
        port,
        role: "test",
    }
}

struct Lab {
    hosts: Vec<HostProc>,
    root: PathBuf,
    /// The lab's directory, deleted on drop unless the test failed.
    tmp: Option<tempfile::TempDir>,
    client: reqwest::Client,
    flock: Flock,
    started: Instant,
}

impl Lab {
    fn specs() -> Vec<HostSpec> {
        vec![
            spec("host-a", 0, 8_191, 4201),
            spec("host-b", 8_192, 16_383, 4202),
            spec("host-c", 0, 8_191, 4203),
        ]
    }

    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        reap_on_interrupt(root.clone());
        Self {
            hosts: Self::specs().into_iter().map(HostProc::new).collect(),
            root,
            tmp: Some(tmp),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            flock: Flock::new(Self::specs()),
            started: Instant::now(),
        }
    }

    fn start(&mut self, i: usize) {
        let exe = Path::new(env!("CARGO_BIN_EXE_murmuration-app"));
        let root = self.root.clone();
        self.hosts[i].start(exe, &root, SECRET).unwrap();
    }

    async fn tick(&mut self) -> Frame {
        let t0 = self.started;
        let now = move || u64::try_from(t0.elapsed().as_millis()).unwrap();
        let (f, _) = self.flock.tick(&self.client, now).await;
        check(&f);
        f
    }

    /// Tick until `done`, or panic after `within`.
    async fn until(
        &mut self,
        within: Duration,
        what: &str,
        done: impl Fn(&Frame) -> bool,
    ) -> Frame {
        let t = Instant::now();
        loop {
            let f = self.tick().await;
            if done(&f) {
                return f;
            }
            if t.elapsed() >= within {
                let holes: Vec<String> = Cell::all()
                    .filter(|c| f.cells[usize::from(c.index())] == -1)
                    .map(|c| c.to_string())
                    .collect();
                panic!(
                    "never: {what} (holes {} [{}], population {})",
                    f.holes,
                    holes.join(" "),
                    f.population
                );
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    }

    async fn served_by(&self, i: usize) -> usize {
        let req = ReadReq {
            cells: Cell::all().map(Cell::key).collect(),
        };
        let r: ReadResp = self
            .client
            .post(format!(
                "http://127.0.0.1:{}/api/cells/read",
                self.hosts[i].spec.port
            ))
            .json(&req)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        r.cells.len()
    }
}

/// Ctrl-C or a TERM ends the test before `Lab`'s `Drop` runs. The running
/// hosts take the terminal's SIGINT and shut down, but one frozen with
/// SIGSTOP never acts on it: it outlives the run, stopped, holding its port,
/// and the next run cannot bind it. So on either signal, kill every host from
/// its pid file (KILL works on a stopped process), drop the lab, and exit.
fn reap_on_interrupt(root: PathBuf) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut int = signal(SignalKind::interrupt()).unwrap();
    let mut term = signal(SignalKind::terminate()).unwrap();
    tokio::spawn(async move {
        let code = tokio::select! {
            _ = int.recv() => 130,
            _ = term.recv() => 143,
        };
        for spec in Lab::specs() {
            let name = spec.name;
            if let Some(pid) = HostProc::new(spec).reap_orphan(&root) {
                eprintln!("interrupted: killed {name} (pid {pid})");
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        std::process::exit(code);
    });
}

impl Drop for Lab {
    fn drop(&mut self) {
        for h in &mut self.hosts {
            let _ = h.kill();
        }
        // On a failure, keep the hosts' logs and databases to look at.
        if std::thread::panicking()
            && let Some(tmp) = self.tmp.take()
        {
            eprintln!("lab kept at {}", tmp.keep().display());
        }
    }
}

/// The invariants every frame keeps (5).
fn check(f: &Frame) {
    let c = f.counts;
    assert_eq!(c.stale, 0, "an answer below the epoch watermark");
    assert_eq!(c.double, 0, "a cell served by two hosts in one tick");
    let ids: BTreeSet<u64> = f.boids.iter().map(|b| b.0).collect();
    assert_eq!(ids.len(), f.boids.len(), "a boid drawn twice");
    assert_eq!(
        f.population as u64,
        c.released + c.births + c.found - c.lost,
        "population does not add up: {c:?}"
    );
    // Flying boids are never in a hole.
    for b in f.boids.iter().filter(|b| b.5 != 1) {
        let cell = sky::geom::cell_of(b.1, b.2);
        assert!(
            f.cells[usize::from(cell.index())] >= 0,
            "boid {} flies in hole {cell}",
            b.0
        );
    }
}

fn owner_of(cell: Cell) -> i8 {
    let slot = autumn_web::sharding::slot_for_key(cell.key().as_str().into()).0;
    i8::from(slot >= 8_192)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one drill, each step depends on the last"
)]
async fn holes_open_and_close_and_the_count_adds_up() {
    let mut lab = Lab::new();
    lab.start(0);
    lab.start(1);

    // 1. The split, and the flock in flight.
    let f = lab
        .until(CLAIM, "the primaries serve every cell", |f| f.holes == 0)
        .await;
    for cell in Cell::all() {
        assert_eq!(f.cells[usize::from(cell.index())], owner_of(cell), "{cell}");
    }
    let a_cells = Cell::all().filter(|c| owner_of(*c) == 0).count();
    assert!(
        a_cells > 100 && a_cells < 284,
        "a lopsided split: {a_cells}"
    );
    let f = lab
        .until(CLAIM, "the flock is up", |f| f.population >= 60)
        .await;
    assert_eq!(f.counts.released, 60);
    for _ in 0..40 {
        lab.tick().await;
    }

    // 2. kill -9 host-a: exactly its cells go dark; boids inside freeze.
    lab.hosts[0].kill().unwrap();
    let f = lab
        .until(Duration::from_secs(30), "host-a's cells are holes", |f| {
            f.holes > 0
        })
        .await;
    assert_eq!(f.holes, a_cells);
    for cell in Cell::all() {
        let o = f.cells[usize::from(cell.index())];
        assert_eq!(o == -1, owner_of(cell) == 0, "{cell}");
    }
    let frozen: Vec<(u64, f32, f32)> = f
        .boids
        .iter()
        .filter(|b| b.5 == 1)
        .map(|b| (b.0, b.1, b.2))
        .collect();
    assert!(!frozen.is_empty(), "nobody was in host-a's cells");
    for _ in 0..16 {
        let g = lab.tick().await;
        for (id, x, y) in &frozen {
            if let Some(b) = g.boids.iter().find(|b| b.0 == *id && b.5 == 1) {
                assert!(
                    (b.1 - x).abs() < f32::EPSILON && (b.2 - y).abs() < f32::EPSILON,
                    "frozen boid {id} moved"
                );
            }
        }
    }

    // 3. The standby fills every hole from the replica.
    lab.start(2);
    let f = lab
        .until(TAKEOVER, "host-c takes the holes back", |f| f.holes == 0)
        .await;
    assert_eq!(f.cells.iter().filter(|o| **o == 2).count(), a_cells);
    assert_eq!(f.frozen, 0);
    eprintln!(
        "takeover: {} lost, {} came back older (up to {} ms), {} ghosts cleared",
        f.counts.lost,
        f.counts.rewound,
        f.rewinds.iter().map(|r| r.by_ms).max().unwrap_or(0),
        f.counts.ghosts
    );
    for _ in 0..16 {
        lab.tick().await;
    }

    // 4. Freeze the standby, fail back to host-a, thaw the zombie.
    lab.hosts[2].freeze().unwrap();
    tokio::time::sleep(Duration::from_secs(8)).await;
    lab.start(0);
    let f = lab
        .until(TAKEOVER, "host-a takes its cells back", |f| {
            f.holes == 0 && f.cells.iter().filter(|o| **o == 0).count() == a_cells
        })
        .await;
    assert_eq!(f.frozen, 0);
    lab.hosts[2].thaw().unwrap();
    for _ in 0..24 {
        lab.tick().await;
    }
    assert_eq!(
        lab.served_by(2).await,
        0,
        "the thawed zombie still serves cells"
    );
    let f = lab.tick().await;
    assert_eq!(f.cells.iter().filter(|o| **o == 2).count(), 0);
}
