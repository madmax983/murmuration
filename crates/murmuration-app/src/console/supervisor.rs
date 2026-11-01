//! Starting, stopping, freezing and thawing hosts as child processes.
//!
//! Each host runs `murmuration-app host` in its own directory under the lab, with an
//! `autumn.toml` written from `host.toml`. Signals go through the system
//! `kill` command, so the console needs no `unsafe` and no libc.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use sky::HostSpec;

/// The host template, filled in per host.
pub const HOST_TEMPLATE: &str = include_str!("../../host.toml");

/// Where a host's process is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proc {
    /// Not running.
    Stopped,
    /// Running.
    Running,
    /// Stopped with SIGSTOP: alive, holding its files, answering nothing.
    Frozen,
}

impl Proc {
    /// A word for the page.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Frozen => "frozen",
        }
    }
}

/// One host's process.
#[derive(Debug)]
pub struct HostProc {
    /// What it is.
    pub spec: HostSpec,
    child: Option<Child>,
    frozen: bool,
}

/// Why a signal or a start failed.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// Filesystem or spawn error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Wrong state for the action.
    #[error("{0}")]
    State(String),
}

/// Fill the template for `spec`.
#[must_use]
pub fn host_config(spec: &HostSpec, lab: &Path) -> String {
    let shared = lab.join("shared");
    HOST_TEMPLATE
        .replace("{{port}}", &spec.port.to_string())
        .replace("{{shared}}", &shared.display().to_string())
        .replace(
            "{{volume}}",
            &lab.join(spec.name).join("volume").display().to_string(),
        )
        .replace("{{name}}", spec.name)
        .replace("{{slots}}", &spec.slots.to_string())
}

impl HostProc {
    /// A host that is not running.
    #[must_use]
    pub const fn new(spec: HostSpec) -> Self {
        Self {
            spec,
            child: None,
            frozen: false,
        }
    }

    /// Its directory.
    #[must_use]
    pub fn dir(&self, lab: &Path) -> PathBuf {
        lab.join(self.spec.name)
    }

    /// Where its process is, reaping it if it exited.
    pub fn proc(&mut self) -> Proc {
        if let Some(child) = self.child.as_mut()
            && let Ok(Some(_)) = child.try_wait()
        {
            self.child = None;
            self.frozen = false;
        }
        match (&self.child, self.frozen) {
            (None, _) => Proc::Stopped,
            (Some(_), true) => Proc::Frozen,
            (Some(_), false) => Proc::Running,
        }
    }

    /// The process id, when running.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Start it: write its config and run `exe host` in its directory.
    ///
    /// # Errors
    ///
    /// Already running, or the spawn failed.
    pub fn start(&mut self, exe: &Path, lab: &Path, secret: &str) -> Result<(), SupervisorError> {
        if self.proc() != Proc::Stopped {
            return Err(SupervisorError::State(format!(
                "{} is already running",
                self.spec.name
            )));
        }
        let dir = self.dir(lab);
        std::fs::create_dir_all(dir.join("volume"))?;
        std::fs::create_dir_all(lab.join("shared").join("replica"))?;
        std::fs::write(dir.join("autumn.toml"), host_config(&self.spec, lab))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("host.log"))?;
        let child = Command::new(exe)
            .arg("host")
            .current_dir(&dir)
            .env("AUTUMN_PROFILE", "dev")
            .env("AUTUMN_SECURITY__SIGNING_SECRET", secret)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        std::fs::write(dir.join("pid"), child.id().to_string())?;
        self.child = Some(child);
        self.frozen = false;
        Ok(())
    }

    /// Kill a host left running by an earlier console (its pid file says so,
    /// and `ps` confirms the pid is still a `murmuration-app`). Returns the pid.
    #[must_use]
    pub fn reap_orphan(&self, lab: &Path) -> Option<u32> {
        let pid: u32 = std::fs::read_to_string(self.dir(lab).join("pid"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let comm = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()?;
        if !String::from_utf8_lossy(&comm.stdout).contains("murmuration-a") {
            return None;
        }
        // A frozen orphan ignores TERM until it is continued; KILL works on both.
        let ok = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(pid)
    }

    /// Send a signal by name (`KILL`, `TERM`, `STOP`, `CONT`).
    fn signal(&self, sig: &str) -> Result<(), SupervisorError> {
        let pid = self
            .pid()
            .ok_or_else(|| SupervisorError::State(format!("{} is not running", self.spec.name)))?;
        let status = Command::new("kill")
            .arg(format!("-{sig}"))
            .arg(pid.to_string())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(SupervisorError::State(format!("kill -{sig} {pid} failed")))
        }
    }

    /// `kill -9`: the host dies with whatever it had not shipped.
    ///
    /// # Errors
    ///
    /// Not running, or the signal failed.
    pub fn kill(&mut self) -> Result<(), SupervisorError> {
        if self.frozen {
            // A stopped process takes SIGKILL fine; just forget the freeze.
            self.frozen = false;
        }
        self.signal("KILL")?;
        if let Some(mut c) = self.child.take() {
            let _ = c.wait();
        }
        Ok(())
    }

    /// `SIGTERM`: a clean shutdown that ships and releases its ranges.
    ///
    /// # Errors
    ///
    /// Not running, frozen, or the signal failed.
    pub fn term(&mut self) -> Result<(), SupervisorError> {
        if self.frozen {
            return Err(SupervisorError::State(format!(
                "{} is frozen; thaw it first, or kill it",
                self.spec.name
            )));
        }
        self.signal("TERM")
    }

    /// `SIGSTOP`: freeze it — a partitioned host, or a GC pause from hell.
    ///
    /// # Errors
    ///
    /// Not running, or the signal failed.
    pub fn freeze(&mut self) -> Result<(), SupervisorError> {
        if self.proc() != Proc::Running {
            return Err(SupervisorError::State(format!(
                "{} is not running",
                self.spec.name
            )));
        }
        self.signal("STOP")?;
        self.frozen = true;
        Ok(())
    }

    /// `SIGCONT`: wake a frozen host.
    ///
    /// # Errors
    ///
    /// Not frozen, or the signal failed.
    pub fn thaw(&mut self) -> Result<(), SupervisorError> {
        if self.proc() != Proc::Frozen {
            return Err(SupervisorError::State(format!(
                "{} is not frozen",
                self.spec.name
            )));
        }
        self.signal("CONT")?;
        self.frozen = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_is_filled_completely() {
        let lab = sky::lab();
        let cfg = host_config(&lab[0], Path::new("/lab"));
        assert!(!cfg.contains("{{"), "{cfg}");
        assert!(cfg.contains("port = 3201"));
        assert!(cfg.contains(r#"owner = "host-a""#));
        assert!(cfg.contains(r#"slots = ["0-8191"]"#));
        assert!(cfg.contains(r#"url = "sqlite:///lab/shared/control.db""#));
        assert!(cfg.contains(r#"root = "/lab/host-a/volume""#));
        assert!(cfg.contains(r#"path = "/lab/shared/replica""#));
    }

    #[test]
    fn a_host_that_never_started_is_stopped_and_refuses_signals() {
        let mut h = HostProc::new(sky::lab().remove(0));
        assert_eq!(h.proc(), Proc::Stopped);
        assert!(h.freeze().is_err());
        assert!(h.thaw().is_err());
        assert!(h.term().is_err());
    }
}
