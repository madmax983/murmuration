#!/usr/bin/env bash
#
# Murmuration — the authoritative verification gate.
#
# This is the one that counts. It runs on the pinned toolchain and runs Verus
# over the verified core. The gates run in the cloud during the build are a
# weaker pre-check on a different machine with no Verus at all; a green cloud
# build says nothing about the proofs in verus/.
#
# There is no service to start. Everything is SQLite. `cargo test` already
# runs the sky test (three real host processes on ports 4201-4203); this
# script then boots the console in a scratch directory (it starts host-a and
# host-b on 3201-3202 itself), runs tools/drill.py against it exactly as the
# buttons would, and deletes the lab on the way out. Safe to run twice.
#
#   ./verify.sh          run everything
#   ./verify.sh --quick  skip the console boot and the drill
#
set -euo pipefail

cd "$(dirname "$0")"
ROOT=$(pwd)

GATES=()
STATUSES=()
FAILED=0
QUICK=0
APP_PID=""
SCRATCH=""
PORT=3000
BASE="http://127.0.0.1:$PORT"

if [ "${1:-}" = "--quick" ]; then QUICK=1; fi

if [ -t 1 ]; then
    BOLD=$(printf '\033[1m'); RED=$(printf '\033[31m')
    GREEN=$(printf '\033[32m'); YELLOW=$(printf '\033[33m'); OFF=$(printf '\033[0m')
else
    BOLD=""; RED=""; GREEN=""; YELLOW=""; OFF=""
fi

record() {
    GATES+=("$1"); STATUSES+=("$2")
    if [ "$2" = "FAIL" ]; then FAILED=1; fi
}
step() { printf '\n%s── %s %s\n' "$BOLD" "$1" "$OFF"; }
run_gate() {
    local name="$1"; shift
    step "$name"
    if "$@"; then record "$name" "PASS"; else record "$name" "FAIL"; fi
}
cleanup() {
    if [ -n "${APP_PID:-}" ]; then
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    if [ -n "${SCRATCH:-}" ] && [ -d "$SCRATCH" ]; then rm -rf "$SCRATCH"; fi
}
trap cleanup EXIT INT TERM

# ── Gate 0: the toolchain must match rust-toolchain.toml ────────────────────

step "toolchain"
EXPECTED=$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' rust-toolchain.toml | head -1)
if ! command -v cargo >/dev/null 2>&1; then
    printf '%sFAIL%s cargo is not on PATH\n' "$RED" "$OFF"; exit 1
fi
ACTUAL=$(cargo --version | awk '{print $2}')
printf 'expected %s, found cargo %s\n' "$EXPECTED" "$ACTUAL"
if [ "$ACTUAL" != "$EXPECTED" ]; then
    printf '%sFAIL%s toolchain mismatch. Install it with: rustup toolchain install %s\n' "$RED" "$OFF" "$EXPECTED"
    printf '  Refusing to run: a green result on the wrong compiler is worse than no result.\n'
    exit 1
fi
record "toolchain" "PASS"

# ── Gates 1-3: format, lint, every test ─────────────────────────────────────

run_gate "cargo fmt"    cargo fmt --all -- --check
run_gate "cargo clippy" cargo clippy --all-targets -- -D warnings
run_gate "cargo test"   cargo test --workspace

# ── Gate 4: boot the console, then the drill ────────────────────────────────

busy() {
    local p
    for p in "$PORT" 3201 3202 3203; do
        if curl -fsS --max-time 1 "http://127.0.0.1:$p/health" >/dev/null 2>&1; then
            echo "$p"; return 0
        fi
    done
    return 1
}

if [ "$QUICK" -eq 1 ]; then
    step "console + drill"; printf '%sskipped%s (--quick)\n' "$YELLOW" "$OFF"
    record "console boots" "SKIP"; record "failover drill" "SKIP"
elif ! command -v python3 >/dev/null 2>&1; then
    step "console + drill"; printf '%sskipped%s python3 not on PATH (the drill is stdlib-only)\n' "$YELLOW" "$OFF"
    record "console boots" "SKIP"; record "failover drill" "SKIP"
elif taken=$(busy); then
    step "console boots"
    printf '%sFAIL%s something is already serving on port %s. Stop it first.\n' "$RED" "$OFF" "$taken"
    record "console boots" "FAIL"; record "failover drill" "SKIP"
else
    step "console boots"
    SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/murmuration-verify.XXXXXX")
    cp autumn.toml autumn-dev.toml "$SCRATCH/"
    cargo build --release --bin murmuration-app
    # 384 cell files per host volume, open at once.
    ulimit -n 8192 2>/dev/null || ulimit -n 4096 2>/dev/null || true
    printf 'open-file limit: %s\n' "$(ulimit -n)"
    export AUTUMN_ENV=dev AUTUMN_PROFILE=dev
    : "${AUTUMN_SECURITY__SIGNING_SECRET:=verify-sh-development-secret-not-for-production-use}"
    export AUTUMN_SECURITY__SIGNING_SECRET
    (cd "$SCRATCH" && exec "${CARGO_TARGET_DIR:-$ROOT/target}/release/murmuration-app") > "$SCRATCH/console.log" 2>&1 &
    APP_PID=$!
    BOOTED=0
    for _ in $(seq 1 90); do
        kill -0 "$APP_PID" 2>/dev/null || break
        if curl -fsS "$BASE/health" >/dev/null 2>&1; then BOOTED=1; break; fi
        sleep 1
    done
    if [ "$BOOTED" -eq 1 ] && kill -0 "$APP_PID" 2>/dev/null; then
        record "console boots" "PASS"
        step "failover drill (about a minute)"
        sleep 2
        if python3 tools/drill.py "$BASE"; then record "failover drill" "PASS"; else
            record "failover drill" "FAIL"; tail -40 "$SCRATCH/console.log" || true
        fi
    else
        printf '%sFAIL%s the console did not boot. Last 40 lines:\n' "$RED" "$OFF"
        tail -40 "$SCRATCH/console.log" || true
        record "console boots" "FAIL"; record "failover drill" "SKIP"
    fi
    # SIGINT, not SIGTERM: the console stops every host it started on Ctrl-C.
    kill -INT "$APP_PID" 2>/dev/null || true; wait "$APP_PID" 2>/dev/null || true; APP_PID=""
    sleep 1
fi

# ── Gate 5: every route is classified ───────────────────────────────────────

step "autumn routes audit"
if command -v autumn >/dev/null 2>&1; then
    if (cd crates/murmuration-app && autumn routes audit); then record "routes audit" "PASS"
    else record "routes audit" "FAIL"; fi
else
    printf '%sskipped%s autumn CLI not on PATH (cargo install autumn-cli --version 0.10.0)\n' "$YELLOW" "$OFF"
    record "routes audit" "SKIP"
fi

# ── Gate 6: Verus over the verified core ────────────────────────────────────

step "verus"
VERUS=""
for candidate in "$(command -v verus 2>/dev/null || true)" "${VERUS_BIN:-}" \
    "$HOME/verus/verus" "$HOME/verus/source/target-verus/release/verus"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] && { VERUS="$candidate"; break; }
done
if [ -z "$VERUS" ]; then
    printf '%sFAIL%s no Verus binary found (PATH, $VERUS_BIN, ~/verus/verus, ~/verus/source/target-verus/release/verus)\n' "$RED" "$OFF"
    record "verus" "FAIL"
else
    printf 'using %s\n' "$VERUS"
    # docs/SPEC.md section 8 records what each one proves.
    for source in verus/src/moves.rs verus/src/resolve.rs verus/src/geom.rs; do
        name="verus $(basename "$source" .rs)"
        printf '\n  %s\n' "$source"
        if "$VERUS" "$source"; then printf '  %sverified%s\n' "$GREEN" "$OFF"; record "$name" "PASS"
        else printf '  %snot verified%s — see docs/SPEC.md section 8\n' "$RED" "$OFF"; record "$name" "FAIL"; fi
    done
fi

# ── Summary ─────────────────────────────────────────────────────────────────

printf '\n%s──────── murmuration verify ────────%s\n' "$BOLD" "$OFF"
i=0
while [ "$i" -lt "${#GATES[@]}" ]; do
    case "${STATUSES[$i]}" in PASS) c="$GREEN" ;; FAIL) c="$RED" ;; *) c="$YELLOW" ;; esac
    printf '  %-22s %s%s%s\n' "${GATES[$i]}" "$c" "${STATUSES[$i]}" "$OFF"
    i=$((i + 1))
done
printf '%s─────────────────────────────────────%s\n' "$BOLD" "$OFF"
if [ "$FAILED" -ne 0 ]; then printf '%sverification failed%s\n' "$RED" "$OFF"; exit 1; fi
printf '%sall gates passed%s\n' "$GREEN" "$OFF"
