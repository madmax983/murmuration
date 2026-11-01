//! Murmuration: a flock of boids whose only home is a `SQLite` fleet split
//! between hosts.
//!
//! One binary, two roles. `murmuration-app host` is a host: a tenant fleet,
//! one tenant per cell of the sky, with ownership and replication, and two
//! batch endpoints ([`host`]). `murmuration-app` alone is the console: it
//! starts and stops hosts as child processes and runs the flock through them
//! ([`console`]).
#![warn(missing_docs)]

pub mod console;
pub mod host;
pub mod wire;

/// Every console handler `main.rs` must register (checked by a test).
pub const CONSOLE_ROUTES: &[&str] = &[
    "routes::index",
    "routes::frame",
    "routes::lab_state",
    "routes::host_action",
    "routes::flock",
    "routes::reset",
    "routes::script",
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_route_is_registered() {
        let main_source = include_str!("main.rs");
        for handler in crate::CONSOLE_ROUTES {
            assert!(main_source.contains(handler), "{handler} is not registered");
        }
        assert!(main_source.contains("host::read") && main_source.contains("host::write"));
    }

    #[test]
    fn the_host_template_is_a_tenant_fleet_with_ownership_and_replication() {
        let t = include_str!("../host.toml");
        for needle in [
            r#"mode = "tenant""#,
            "create_on_demand = true",
            "restore_missing = true",
            "[database.fleet.ownership]",
            "[replication]",
            "enabled = true",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
    }
}
