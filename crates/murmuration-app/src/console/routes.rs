//! The console's page and buttons.

use std::sync::Arc;

use autumn_web::AppState;
use autumn_web::prelude::*;
use maud::{DOCTYPE, PreEscaped};

use super::conductor::Frame;
use super::{Console, State as LabState};

fn console(state: &AppState) -> AutumnResult<Arc<Console>> {
    state
        .extension::<Console>()
        .ok_or_else(|| AutumnError::service_unavailable_msg("the console is still starting"))
}

/// The stylesheet.
pub const STYLE: &str = include_str!("sky.css");
/// The script.
pub const SCRIPT: &str = include_str!("sky.js");

/// The sky.
#[get("/")]
#[public]
pub async fn index(csrf: Option<CsrfToken>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Murmuration" }
                @if let Some(t) = csrf.as_ref() { meta name="csrf-token" content=(t.token()); }
                style { (PreEscaped(STYLE)) }
                script src="/sky.js" defer {}
            }
            body {
                div class="wrap" {
                    header class="top" {
                        h1 { "Murmuration" }
                        span class="tag" { "a flock that lives in 384 SQLite files" }
                        nav {
                            label class="toggle" {
                                input type="checkbox" id="show-owners";
                                " show who serves each cell"
                            }
                            button id="fly" class="ghost" data-post="/flock/stop" { "Pause" }
                            button class="ghost" data-post="/reset" data-confirm="Kill every host, wipe the lab and start over?" { "Reset the lab" }
                        }
                    }
                    section class="sky-frame" {
                        canvas id="sky" width="960" height="640" aria-label="The sky: 24 by 16 cells, each one SQLite file, with the flock flying through them" {}
                        div class="readout" id="readout" {}
                    }
                    p class="lede" {
                        "Every square of this sky is one SQLite file, and every bird is a row in the file of the square it is in. "
                        "host-a serves the cells whose names hash into its half of the slot space, host-b the rest — a scatter, not a split. "
                        "Kill a host and its cells go dark: the birds inside freeze where they were, and the flock steers around the holes. "
                        "Start host-c and it takes the dark cells back from the replica."
                    }
                    section class="grid" {
                        div id="hosts" class="hosts" {}
                        div id="headcount" class="panel" {}
                    }
                    section class="panel" {
                        h3 { "Timeline" }
                        ol id="timeline" class="timeline" {}
                    }
                    footer {
                        "Every host is a real autumn-web 0.9 process: a tenant fleet with "
                        code { "[database.fleet.ownership]" } " and " code { "[replication]" }
                        ". The lab lives in " code { "murmuration-lab/" } "; each host's log is "
                        code { "murmuration-lab/<host>/host.log" } "."
                    }
                }
            }
        }
    }
}

/// The last tick's sky.
///
/// # Errors
///
/// 503 while the console starts.
#[get("/_frame.json")]
#[public]
pub async fn frame(State(state): State<AppState>) -> AutumnResult<Json<Frame>> {
    Ok(Json((*console(&state)?.frame()).clone()))
}

/// The hosts and the timeline.
///
/// # Errors
///
/// 503 while the console starts.
#[get("/_state.json")]
#[public]
pub async fn lab_state(State(state): State<AppState>) -> AutumnResult<Json<LabState>> {
    Ok(Json(console(&state)?.state()))
}

/// A host button.
///
/// # Errors
///
/// 409 if the action does not apply to the host as it is now.
#[post("/hosts/{name}/{action}")]
#[public]
pub async fn host_action(
    State(state): State<AppState>,
    Path((name, action)): Path<(String, String)>,
) -> AutumnResult<Json<LabState>> {
    let c = console(&state)?;
    c.act(&name, &action)
        .map_err(|e| AutumnError::conflict_msg(e.to_string()))?;
    Ok(Json(c.state()))
}

/// Release or pause the flock.
///
/// # Errors
///
/// 400 for anything but `start` or `stop`.
#[post("/flock/{on}")]
#[public]
pub async fn flock(
    State(state): State<AppState>,
    Path(on): Path<String>,
) -> AutumnResult<Json<LabState>> {
    let c = console(&state)?;
    match on.as_str() {
        "start" => c.set_flying(true),
        "stop" => c.set_flying(false),
        other => {
            return Err(AutumnError::bad_request_msg(format!(
                "no flock action {other}"
            )));
        }
    }
    Ok(Json(c.state()))
}

/// Start over.
///
/// # Errors
///
/// 500 if the lab directory could not be cleared.
#[post("/reset")]
#[public]
pub async fn reset(State(state): State<AppState>) -> AutumnResult<Json<LabState>> {
    let c = console(&state)?;
    c.reset()
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;
    c.set_flying(true);
    Ok(Json(c.state()))
}

/// The script.
#[get("/sky.js")]
#[public]
pub async fn script() -> impl IntoResponse {
    (
        [
            ("content-type", "text/javascript; charset=utf-8"),
            ("cache-control", "no-cache"),
        ],
        SCRIPT,
    )
}
