//! The console: the lab's supervisor, the conductor's loop, and the page's
//! picture of both.
//!
//! ```text
//!   buttons ──► supervisor ──► murmuration-app host ×3 (child processes)
//!   poller  ──► GET /actuator/health on each host every 500 ms ──► timeline
//!   flock   ──► conductor.tick() every 125 ms ──► the frame the sky draws
//! ```

pub mod conductor;
pub mod routes;
pub mod supervisor;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use sky::{SlotRange, lab};

use conductor::{Flock, Frame, TICK};
use supervisor::{HostProc, Proc};

/// How often the poller reads every host's health.
pub const POLL: Duration = Duration::from_millis(500);
/// Timeline entries kept.
pub const TIMELINE: usize = 200;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every lock here guards a picture the next poll or tick repairs.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A range a host holds, as its health reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Held {
    /// The range.
    pub range: String,
    /// Its epoch.
    pub epoch: u64,
    /// Whether the host serves it now.
    pub serving: bool,
}

/// What the poller last saw of a host.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HostView {
    /// Whether it answered.
    pub reachable: bool,
    /// Ranges held.
    pub held: Vec<Held>,
    /// Ranges it is waiting to claim.
    pub waiting: Vec<String>,
    /// Fleet replication: how far behind its worst database is, in seconds.
    pub lag_s: Option<f64>,
    /// Fleet replication: databases it ships.
    pub replicating: Option<u64>,
}

/// A line on the timeline.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// Milliseconds since the console started.
    pub at_ms: u64,
    /// `you`, `fleet` or `flock`.
    pub kind: &'static str,
    /// What happened.
    pub text: String,
}

/// A failure you caused, for checking losses against.
#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    /// Which host.
    pub host: &'static str,
    /// `kill -9` or `freeze`.
    pub what: &'static str,
    /// When, ms.
    pub at_ms: u64,
}

/// A host on the page.
#[derive(Debug, Clone, Serialize)]
pub struct HostState {
    /// Name.
    pub name: &'static str,
    /// Role.
    pub role: &'static str,
    /// Port.
    pub port: u16,
    /// What it claims at boot.
    pub slots: String,
    /// `stopped`, `running` or `frozen`.
    pub process: &'static str,
    /// Its pid.
    pub pid: Option<u32>,
    /// What its health said.
    pub view: HostView,
}

/// Everything the page draws, but the sky.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    /// Milliseconds since the console started.
    pub now_ms: u64,
    /// Whether the flock is flying.
    pub flying: bool,
    /// The hosts.
    pub hosts: Vec<HostState>,
    /// Failures you caused, oldest first.
    pub failures: Vec<Failure>,
    /// Newest first.
    pub timeline: Vec<Event>,
}

/// Why a console action failed.
#[derive(Debug, thiserror::Error)]
pub enum ConsoleError {
    /// No such host.
    #[error("no host called {0}")]
    NoHost(String),
    /// No such action.
    #[error("no action called {0}")]
    NoAction(String),
    /// The supervisor refused.
    #[error(transparent)]
    Supervisor(#[from] supervisor::SupervisorError),
    /// The lab directory.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The console.
#[derive(Debug)]
pub struct Console {
    lab: PathBuf,
    exe: PathBuf,
    secret: String,
    started: Instant,
    hosts: Mutex<Vec<HostProc>>,
    views: Mutex<Vec<HostView>>,
    timeline: Mutex<VecDeque<Event>>,
    failures: Mutex<Vec<Failure>>,
    flock: tokio::sync::Mutex<Flock>,
    frame: Mutex<Arc<Frame>>,
    flying: AtomicBool,
    client: reqwest::Client,
}

impl Console {
    /// A console for the lab in `lab`, starting hosts with `exe`.
    ///
    /// # Errors
    ///
    /// The HTTP client could not be built.
    pub fn new(lab: PathBuf, exe: PathBuf, secret: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            lab,
            exe,
            secret,
            started: Instant::now(),
            hosts: Mutex::new(sky::lab().into_iter().map(HostProc::new).collect()),
            views: Mutex::new(vec![HostView::default(); sky::lab().len()]),
            timeline: Mutex::new(VecDeque::new()),
            failures: Mutex::new(Vec::new()),
            flock: tokio::sync::Mutex::new(Flock::new(sky::lab())),
            frame: Mutex::new(Arc::new(Frame::default())),
            flying: AtomicBool::new(false),
            client: reqwest::Client::builder().no_proxy().build()?,
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Add a timeline line.
    pub fn event(&self, kind: &'static str, text: impl Into<String>) {
        let mut t = lock(&self.timeline);
        t.push_front(Event {
            at_ms: self.now_ms(),
            kind,
            text: text.into(),
        });
        t.truncate(TIMELINE);
    }

    /// Run `action` on host `name`: `start`, `kill`, `term`, `freeze`, `thaw`.
    ///
    /// # Errors
    ///
    /// Unknown host or action, or the supervisor refused.
    pub fn act(&self, name: &str, action: &str) -> Result<(), ConsoleError> {
        let mut hosts = lock(&self.hosts);
        let h = hosts
            .iter_mut()
            .find(|h| h.spec.name == name)
            .ok_or_else(|| ConsoleError::NoHost(name.to_owned()))?;
        let (what, failure) = match action {
            "start" => {
                h.start(&self.exe, &self.lab, &self.secret)?;
                ("started", None)
            }
            "kill" => {
                h.kill()?;
                (
                    "killed with SIGKILL: its cells are holes now",
                    Some("kill -9"),
                )
            }
            "term" => {
                h.term()?;
                ("sent SIGTERM (ships, releases its ranges, exits)", None)
            }
            "freeze" => {
                h.freeze()?;
                ("frozen with SIGSTOP", Some("freeze"))
            }
            "thaw" => {
                h.thaw()?;
                ("thawed with SIGCONT", None)
            }
            other => return Err(ConsoleError::NoAction(other.to_owned())),
        };
        let host = h.spec.name;
        drop(hosts);
        if let Some(what) = failure {
            lock(&self.failures).push(Failure {
                host,
                what,
                at_ms: self.now_ms(),
            });
        }
        self.event("you", format!("{name} {what}"));
        Ok(())
    }

    /// Stop every host, wipe the lab, forget the flock, and start the two
    /// primaries.
    ///
    /// # Errors
    ///
    /// The lab directory could not be removed or a host could not start.
    pub async fn reset(&self) -> Result<(), ConsoleError> {
        self.flying.store(false, Ordering::Release);
        let mut flock = self.flock.lock().await;
        {
            let mut hosts = lock(&self.hosts);
            for h in hosts.iter_mut() {
                if h.proc() != Proc::Stopped {
                    let _ = h.kill();
                }
            }
        }
        if self.lab.exists() {
            std::fs::remove_dir_all(&self.lab)?;
        }
        *flock = Flock::new(sky::lab());
        drop(flock);
        *lock(&self.frame) = Arc::new(Frame::default());
        lock(&self.timeline).clear();
        lock(&self.failures).clear();
        self.event("you", "reset the lab");
        self.act("host-a", "start")?;
        self.act("host-b", "start")?;
        Ok(())
    }

    /// Stop every host cleanly (console shutdown).
    pub fn stop_all(&self) {
        let mut hosts = lock(&self.hosts);
        for h in hosts.iter_mut() {
            match h.proc() {
                Proc::Running => {
                    let _ = h.term();
                }
                Proc::Frozen => {
                    let _ = h.kill();
                }
                Proc::Stopped => {}
            }
        }
    }

    /// Start or pause the flock.
    pub fn set_flying(self: &Arc<Self>, on: bool) {
        let was = self.flying.swap(on, Ordering::AcqRel);
        if on && !was {
            self.event("you", "released the flock");
            let me = Arc::clone(self);
            tokio::spawn(async move { me.fly().await });
        } else if !on && was {
            self.event("you", "paused the flock");
        }
    }

    async fn fly(self: Arc<Self>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        while self.flying.load(Ordering::Acquire) {
            tick.tick().await;
            let mut flock = self.flock.lock().await;
            let (frame, notes) = flock.tick(&self.client, || self.now_ms()).await;
            drop(flock);
            *lock(&self.frame) = Arc::new(frame);
            for n in notes {
                self.event("flock", n);
            }
        }
    }

    /// The last frame.
    #[must_use]
    pub fn frame(&self) -> Arc<Frame> {
        Arc::clone(&lock(&self.frame))
    }

    /// Poll every host's health once, and note what changed.
    pub async fn poll(&self) {
        let specs = lab();
        let mut fresh = Vec::with_capacity(specs.len());
        for h in &specs {
            fresh.push(self.health(h.port).await);
        }
        let old = std::mem::replace(&mut *lock(&self.views), fresh.clone());
        for ((spec, before), after) in specs.iter().zip(old).zip(fresh) {
            for e in diff(spec.name, &before, &after) {
                self.event("fleet", e);
            }
        }
    }

    async fn health(&self, port: u16) -> HostView {
        let url = format!("http://127.0.0.1:{port}/actuator/health");
        let Ok(r) = self
            .client
            .get(url)
            .timeout(Duration::from_millis(400))
            .send()
            .await
        else {
            return HostView::default();
        };
        let Ok(v) = r.json::<serde_json::Value>().await else {
            return HostView::default();
        };
        parse_health(&v)
    }

    /// Everything the page draws but the sky.
    pub fn state(&self) -> State {
        let views = lock(&self.views).clone();
        let hosts = lock(&self.hosts)
            .iter_mut()
            .zip(views)
            .map(|(h, view)| HostState {
                name: h.spec.name,
                role: h.spec.role,
                port: h.spec.port,
                slots: h.spec.slots.to_string(),
                process: h.proc().label(),
                pid: h.pid(),
                view,
            })
            .collect();
        State {
            now_ms: self.now_ms(),
            flying: self.flying.load(Ordering::Acquire),
            hosts,
            failures: lock(&self.failures).clone(),
            timeline: lock(&self.timeline).iter().cloned().collect(),
        }
    }
}

/// The development signing secret handed to hosts when none is set.
const DEV_SECRET: &str = "murmuration-lab-development-secret-not-for-production-use";

/// The console's startup hook: a fresh lab with the two primaries running, a
/// poller, the flock released, and every host stopped on Ctrl-C.
///
/// # Errors
///
/// The executable or the lab directory could not be found, or a host could
/// not start.
pub async fn boot(state: autumn_web::AppState) -> autumn_web::AutumnResult<()> {
    use autumn_web::error::AutumnError;
    let fail = |e: &dyn std::fmt::Display| AutumnError::service_unavailable_msg(e.to_string());
    let lab = std::env::current_dir()
        .map_err(|e| fail(&e))?
        .join(std::env::var("MURMURATION_LAB").unwrap_or_else(|_| "murmuration-lab".to_owned()));
    let exe = std::env::current_exe().map_err(|e| fail(&e))?;
    let secret =
        std::env::var("AUTUMN_SECURITY__SIGNING_SECRET").unwrap_or_else(|_| DEV_SECRET.to_owned());
    let console = Console::new(lab.clone(), exe, secret).map_err(|e| fail(&e))?;
    for h in lock(&console.hosts).iter() {
        if let Some(pid) = h.reap_orphan(&lab) {
            tracing::warn!(
                host = h.spec.name,
                pid,
                "killed a host left running by an earlier console"
            );
        }
    }
    console.reset().await.map_err(|e| fail(&e))?;
    state.insert_extension(console);
    let console = state
        .extension::<Console>()
        .ok_or_else(|| fail(&"the console did not install"))?;

    let poller = Arc::clone(&console);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            poller.poll().await;
        }
    });
    if std::env::var("MURMURATION_PAUSED").is_err() {
        console.set_flying(true);
    }
    let stopper = Arc::clone(&console);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stopper.flying.store(false, Ordering::Release);
            stopper.stop_all();
        }
    });
    tracing::info!(lab = %lab.display(), "Murmuration console ready: host-a and host-b are starting");
    Ok(())
}

/// Read a host's `/actuator/health` body.
#[must_use]
pub fn parse_health(v: &serde_json::Value) -> HostView {
    let d = &v["components"]["fleet:ownership"]["details"];
    let held = d["held"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    let range = h["range"].as_str()?.to_owned();
                    SlotRange::parse(&range).ok()?;
                    Some(Held {
                        range,
                        epoch: h["epoch"].as_u64()?,
                        serving: h["serving"].as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let waiting = d["waiting"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let r = &v["components"]["replication:fleet"]["details"];
    HostView {
        reachable: true,
        held,
        waiting,
        lag_s: r["worst_lag_seconds"].as_f64(),
        replicating: r["databases"].as_u64(),
    }
}

/// What changed between two polls of one host, as timeline lines.
#[must_use]
pub fn diff(name: &str, before: &HostView, after: &HostView) -> Vec<String> {
    let mut out = Vec::new();
    match (before.reachable, after.reachable) {
        (true, false) => out.push(format!("{name} stopped answering")),
        (false, true) => out.push(format!("{name} is answering")),
        _ => {}
    }
    for h in &after.held {
        match before
            .held
            .iter()
            .find(|b| b.range == h.range && b.epoch == h.epoch)
        {
            None => out.push(format!("{name} holds {} at epoch {}", h.range, h.epoch)),
            Some(b) if b.serving && !h.serving => {
                out.push(format!(
                    "{name} stopped serving {} (local lease ran out)",
                    h.range
                ));
            }
            Some(b) if !b.serving && h.serving => {
                out.push(format!("{name} serves {} again", h.range));
            }
            Some(_) => {}
        }
    }
    if after.reachable {
        for b in &before.held {
            if !after
                .held
                .iter()
                .any(|h| h.range == b.range && h.epoch == b.epoch)
            {
                out.push(format!("{name} let go of {} (epoch {})", b.range, b.epoch));
            }
        }
        for w in &after.waiting {
            if !before.waiting.contains(w) {
                out.push(format!("{name} is waiting to claim {w}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(held: &[(&str, u64, bool)], waiting: &[&str]) -> HostView {
        HostView {
            reachable: true,
            held: held
                .iter()
                .map(|(r, e, s)| Held {
                    range: (*r).into(),
                    epoch: *e,
                    serving: *s,
                })
                .collect(),
            waiting: waiting.iter().map(|w| (*w).to_owned()).collect(),
            lag_s: None,
            replicating: None,
        }
    }

    #[test]
    fn parses_the_ownership_details() {
        let v = serde_json::json!({"components":{"fleet:ownership":{"details":{
            "owner":"host-a","held":[{"range":"0-8191","epoch":4,"serving":true}],
            "waiting":["8192-16383"],"lost":[],"failed":[]}},
            "replication:fleet":{"details":{"worst_lag_seconds":3,"databases":152}}}});
        let h = parse_health(&v);
        assert!(h.reachable);
        assert_eq!(h.held, view(&[("0-8191", 4, true)], &[]).held);
        assert_eq!(h.waiting, vec!["8192-16383".to_owned()]);
        assert_eq!((h.lag_s, h.replicating), (Some(3.0), Some(152)));
        assert_eq!(parse_health(&serde_json::json!({})).held, vec![]);
    }

    #[test]
    fn the_timeline_names_claims_losses_and_lapses() {
        let a1 = view(&[("0-8191", 1, true)], &[]);
        assert_eq!(
            diff("host-a", &HostView::default(), &a1),
            vec!["host-a is answering", "host-a holds 0-8191 at epoch 1"]
        );
        let lapsed = view(&[("0-8191", 1, false)], &[]);
        assert_eq!(
            diff("host-a", &a1, &lapsed),
            vec!["host-a stopped serving 0-8191 (local lease ran out)"]
        );
        assert_eq!(
            diff("host-a", &lapsed, &a1),
            vec!["host-a serves 0-8191 again"]
        );
        let lost = view(&[], &["0-8191"]);
        assert_eq!(
            diff("host-a", &lapsed, &lost),
            vec![
                "host-a let go of 0-8191 (epoch 1)",
                "host-a is waiting to claim 0-8191"
            ]
        );
        assert_eq!(
            diff("host-a", &a1, &HostView::default()),
            vec!["host-a stopped answering"]
        );
    }
}
