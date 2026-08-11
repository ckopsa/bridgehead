#!/usr/bin/env bash
#
# verify.sh — the definition of done, as one executable.
#
# Every agent that touches this repo has to answer "is it still working?", and
# left to itself every agent invents a different answer: a release build nobody
# asked for, a `cargo run` that silently recompiles something other than what it
# just tested, a bridge waiter that deadlocks against its own engine, a sim
# declared green because it hit the time cap instead of a verdict. This script
# is the answer, written down once, so that "verified" means the same thing in
# every handoff.
#
#   tools/verify.sh smoke        fastest honest "did I break it"
#   tools/verify.sh standard     the default bead-level bar (bare invocation too)
#   tools/verify.sh full         everything, including the live-engine bridges
#   tools/verify.sh identity     prove a change is byte-for-byte no-behavior-change
#   tools/verify.sh --list       one line per tier
#
# ---------------------------------------------------------------------------
# The tiers
# ---------------------------------------------------------------------------
#
# smoke      cargo build (dev) + cargo test + ONE headless open-map sim that
#            must reach a real verdict. ~4 min on a warm target.
#
# standard   smoke + a crossings sim + every tools/test_*.py suite. This is the
#            bar for closing a bead. ~5 min warm.
#
# full       standard + a 2-seed x 2-map sim matrix + a determinism pair (same
#            seed twice, fingerprints must match) + all four bridge verifiers
#            against a live engine. Tens of minutes; the bridges dominate.
#
# identity   [ref] — for "this changes no behavior" claims. Builds `ref`
#            (default: merge-base with master) and HEAD, runs seeded fixed-dt
#            fingerprint sims on both maps with both binaries, and byte-compares
#            the fingerprint streams. Cheaper and stricter than re-reading a
#            diff: identical fingerprints ARE the no-behavior-change proof.
#
# Tiers are cumulative and ordered cheapest-first WITHIN a tier, so the first
# failure is usually the cheapest one to reproduce. It stops at the first
# failing stage and names it.
#
# ---------------------------------------------------------------------------
# What it refuses to get wrong
# ---------------------------------------------------------------------------
#
# * Dev profile, always. Determinism is a property of the schedule and the
#   seed, not of the optimisation level, and a release detour is minutes of
#   compile for no extra evidence.
# * The BUILT BINARY is what runs the sims — target/debug/wc3clone by path,
#   never `cargo run`. `cargo run` can rebuild between your test and your sim
#   and leave you unable to say which code produced the verdict. The build
#   stage prints the binary's mtime so a stale binary is visible, not implied.
# * Decisive-or-fail. A sim that hits WC3_MAX_GAME_SECS is a FAILURE here, not
#   a pass with a caveat, and the `headless: game over` line is echoed so the
#   verdict is in the transcript rather than in a log nobody opens.
# * Every engine this script starts is tracked by PID and dies with the script,
#   via an EXIT/INT/TERM trap. It only ever signals PIDs it started itself.
# * It only ever deletes ./bridge under THIS checkout. Bridge seat directories
#   are resolved from the engine's cwd (src/bridge.rs `BRIDGE_DIR`), so
#   concurrent runs need separate worktrees, not separate env vars.
# * Runs from any cwd inside the repo — it locates the root from its own path.
# * Exit status is the result: 0 only if every stage of the tier passed.
#
# ---------------------------------------------------------------------------
# Knobs (all optional, all env)
# ---------------------------------------------------------------------------
#
#   SIM_DT=0.05        fixed tick for sims; the determinism harness's tick
#   SIM_CAP=900        WC3_MAX_GAME_SECS safety cap, in GAME seconds
#   SIM_SEEDS="42 7"   seeds for full's matrix
#   IDENT_SEED=42      seed for identity's fingerprint sims
#   FP_INTERVAL=10     WC3_FINGERPRINT sampling interval, in game seconds
#   BRIDGE_SPEED=4     WC3_SPEED for the engine verify_intent_bridge.py drives
#   BRIDGE_CAP=20000   that engine's WC3_MAX_GAME_SECS — high on purpose, so the
#                      match cannot time-cap out from under the verifier; the
#                      engine is stopped by PID when the verifier returns
#   KEEP_LOGS=1        keep the log directory (and identity's extracted ref tree)
#
# Logs for every stage land in one temp directory, printed at the end.
#
# ---------------------------------------------------------------------------
# Notes on the bridge verifiers (why `full` looks asymmetric)
# ---------------------------------------------------------------------------
#
# The four verifiers do not agree on who owns the engine, so this script does
# not pretend they do:
#
#   verify_intent_bridge.py     needs an engine ALREADY RUNNING. This script
#                               launches it (WC3_BRIDGE=1), waits for the
#                               script to finish, then stops it by saved PID.
#                               The verifier performs the t0d ready handshake
#                               itself — it sees `waiting_for` in the snapshot
#                               and sends {"type":"ready"} — so the engine gets
#                               a GENEROUS WC3_READY_TIMEOUT, not a low one:
#                               the point is to let the handshake happen, not
#                               to time it out before the verifier can test it.
#   verify_territory_bridge.py  launches its own engine; takes `--bin PATH`, so
#                               it gets the binary this script just built.
#   verify_research_bridge.py   launches its own engine via `cargo run`.
#   verify_r9_legibility.py     launches its own engine via `cargo run --quiet`.
#
# The last two are the reason the build stage runs first even in `full`: their
# "did the engine come up?" budgets are 20s and 30s of wall clock, which a cold
# compile blows straight through. With the build already done, `cargo run` is a
# no-op and they start instantly.
#
# Each bridge stage wipes ./bridge first, because two of the verifiers assume
# they own that tree.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

BIN="$ROOT/target/debug/wc3clone"

SIM_DT="${SIM_DT:-0.05}"
SIM_CAP="${SIM_CAP:-900}"
SIM_SEEDS="${SIM_SEEDS:-42 7}"
IDENT_SEED="${IDENT_SEED:-42}"
FP_INTERVAL="${FP_INTERVAL:-10}"
# Speed 4 is a compromise the intent verifier forces: it waits up to 120 WALL
# seconds for red's economy to afford a tier-up, so the game clock has to move
# briskly — but red is a bridged seat sitting still while blue's scripted AI
# builds an army, so too fast and the match is razed out from under the script.
BRIDGE_SPEED="${BRIDGE_SPEED:-4}"
BRIDGE_CAP="${BRIDGE_CAP:-20000}"
BRIDGE_READY_TIMEOUT="${BRIDGE_READY_TIMEOUT:-600}"
KEEP_LOGS="${KEEP_LOGS:-0}"

LOGDIR="$(mktemp -d "${TMPDIR:-/tmp}/wc3verify.XXXXXX")"

# ---------------------------------------------------------------------------
# Engine lifecycle: nothing we start outlives us
# ---------------------------------------------------------------------------

ENGINE_PIDS=()
IDENT_CHECKOUT=""

track_engine() { ENGINE_PIDS+=("$1"); }

untrack_engine() {
    local keep=() p
    for p in ${ENGINE_PIDS[@]+"${ENGINE_PIDS[@]}"}; do
        [ -n "$p" ] || continue
        [ "$p" = "$1" ] || keep+=("$p")
    done
    ENGINE_PIDS=(${keep[@]+"${keep[@]}"})
}

# Stop one engine by exact PID: TERM, up to 5s of grace, then KILL. Never a
# process group, never a pattern — a pkill here would reap a sibling agent's
# engine, and this script is supposed to be the thing you can trust.
stop_engine() {
    local pid="$1" i
    [ -n "$pid" ] || return 0
    if kill -0 "$pid" 2>/dev/null; then
        kill -TERM "$pid" 2>/dev/null
        for i in 1 2 3 4 5 6 7 8 9 10; do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid" 2>/dev/null
        fi
    fi
    wait "$pid" 2>/dev/null
    untrack_engine "$pid"
}

cleanup() {
    local rc=$? p
    for p in ${ENGINE_PIDS[@]+"${ENGINE_PIDS[@]}"}; do
        [ -n "$p" ] || continue
        kill -TERM "$p" 2>/dev/null
    done
    sleep 0.3
    for p in ${ENGINE_PIDS[@]+"${ENGINE_PIDS[@]}"}; do
        [ -n "$p" ] || continue
        kill -KILL "$p" 2>/dev/null
    done
    if [ -n "$IDENT_CHECKOUT" ] && [ -d "$IDENT_CHECKOUT" ] && [ "$KEEP_LOGS" != "1" ]; then
        rm -rf "$IDENT_CHECKOUT"
    fi
    if [ "$rc" -eq 0 ] && [ "$KEEP_LOGS" != "1" ]; then
        rm -rf "$LOGDIR"
    fi
    return $rc
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# ---------------------------------------------------------------------------
# Stage bookkeeping and the summary table
# ---------------------------------------------------------------------------

S_NAME=()
S_RESULT=()
S_MS=()

now_ms() { date +%s%3N; }

fmt_ms() { printf '%d.%01d' "$(( $1 / 1000 ))" "$(( ($1 % 1000) / 100 ))"; }

summary() {
    [ "${#S_NAME[@]}" -gt 0 ] || return 0
    local i total=0 rule
    rule="$(printf '%.0s-' $(seq 1 42))"
    printf '\n'
    printf '%-42s  %-6s  %9s\n' "STAGE" "RESULT" "SECONDS"
    printf '%-42s  %-6s  %9s\n' "$rule" "------" "---------"
    for i in "${!S_NAME[@]}"; do
        printf '%-42s  %-6s  %9s\n' "${S_NAME[$i]}" "${S_RESULT[$i]}" "$(fmt_ms "${S_MS[$i]}")"
        total=$(( total + S_MS[i] ))
    done
    printf '%-42s  %-6s  %9s\n' "$rule" "------" "---------"
    printf '%-42s  %-6s  %9s\n' "TOTAL" "" "$(fmt_ms "$total")"
}

# run_stage <name> <command...> — time it, record it, and abort the whole run
# on failure with the stage named. Fail fast is the whole point: a summary
# table full of later stages that never ran is noise.
run_stage() {
    local name="$1"; shift
    printf '\n=== %s ===\n' "$name"
    local t0 t1 rc
    t0="$(now_ms)"
    "$@"
    rc=$?
    t1="$(now_ms)"
    S_NAME+=("$name")
    S_MS+=("$(( t1 - t0 ))")
    if [ "$rc" -eq 0 ]; then
        S_RESULT+=("PASS")
        printf -- '--- %s: PASS in %ss\n' "$name" "$(fmt_ms "$(( t1 - t0 ))")"
        return 0
    fi
    S_RESULT+=("FAIL")
    printf -- '--- %s: FAIL in %ss\n' "$name" "$(fmt_ms "$(( t1 - t0 ))")"
    summary
    printf '\nFAILED AT STAGE: %s\n' "$name"
    printf 'logs: %s\n' "$LOGDIR"
    KEEP_LOGS=1
    exit 1
}

# ---------------------------------------------------------------------------
# Stages
# ---------------------------------------------------------------------------

st_build() {
    local log="$LOGDIR/cargo-build.log"
    cargo build >"$log" 2>&1
    local rc=$?
    if [ "$rc" -ne 0 ]; then
        tail -n 40 "$log"
        printf 'full log: %s\n' "$log"
        return "$rc"
    fi
    if [ ! -x "$BIN" ]; then
        printf 'build succeeded but there is no binary at %s\n' "$BIN"
        return 1
    fi
    printf '  binary: %s\n' "$BIN"
    printf '  built:  %s\n' "$(date -r "$BIN" '+%Y-%m-%d %H:%M:%S')"
    printf '  %s\n' "$(grep -m1 'Finished' "$log" | sed 's/^[[:space:]]*//')"
    return 0
}

st_cargo_test() {
    local log="$LOGDIR/cargo-test.log"
    cargo test >"$log" 2>&1
    local rc=$?
    grep -E '^test result:' "$log" | sed 's/^/  /'
    if [ "$rc" -ne 0 ]; then
        printf '  --- failures ---\n'
        grep -E '^(failures:|    [a-z_]+::|---- )' "$log" | head -n 30 | sed 's/^/  /'
        printf '  full log: %s\n' "$log"
    fi
    return "$rc"
}

st_py_suites() {
    local f name log rc=0 found=0
    for f in "$ROOT"/tools/test_*.py; do
        [ -e "$f" ] || continue
        found=1
        name="$(basename "$f")"
        log="$LOGDIR/$name.log"
        if python3 "$f" >"$log" 2>&1; then
            printf '  %-26s %s\n' "$name" "$(tail -n 1 "$log")"
        else
            printf '  %-26s FAILED\n' "$name"
            tail -n 20 "$log" | sed 's/^/      /'
            printf '      full log: %s\n' "$log"
            rc=1
        fi
    done
    if [ "$found" -eq 0 ]; then
        printf '  no tools/test_*.py suites found — that is itself suspicious\n'
        return 1
    fi
    return "$rc"
}

# sim <map> <seed> — one headless AI-vs-AI match on the built binary. Decisive
# or it is a failure; the verdict line is echoed either way.
sim() {
    local map="$1" seed="$2"
    local log="$LOGDIR/sim-$map-$seed.log"
    env WC3_HEADLESS=1 WC3_AI_BOTH=1 \
        WC3_MAP="$map" WC3_SEED="$seed" \
        WC3_FIXED_DT="$SIM_DT" WC3_MAX_GAME_SECS="$SIM_CAP" \
        "$BIN" >"$log" 2>&1 &
    local pid=$!
    track_engine "$pid"
    wait "$pid"
    local rc=$?
    untrack_engine "$pid"
    if [ "$rc" -ne 0 ]; then
        printf '  %-10s seed %-4s engine exited %d — %s\n' "$map" "$seed" "$rc" "$log"
        return 1
    fi
    local over
    over="$(grep -m1 'headless: game over' "$log" | sed -E 's/.*headless: //')"
    if [ -n "$over" ]; then
        printf '  %-10s seed %-4s %s\n' "$map" "$seed" "$over"
        return 0
    fi
    local capped
    capped="$(grep -m1 'headless: time cap' "$log" | sed -E 's/.*headless: //')"
    if [ -n "$capped" ]; then
        printf '  %-10s seed %-4s NOT DECISIVE: %s\n' "$map" "$seed" "$capped"
    else
        printf '  %-10s seed %-4s no verdict line at all — see %s\n' "$map" "$seed" "$log"
    fi
    return 1
}

st_sim_open()      { sim open 42; }
st_sim_crossings() { sim crossings 42; }

st_sim_matrix() {
    local map seed rc=0
    for map in open crossings; do
        for seed in $SIM_SEEDS; do
            sim "$map" "$seed" || rc=1
        done
    done
    return "$rc"
}

st_determinism() {
    BIN="$BIN" SEED=42 DT="$SIM_DT" INTERVAL="$FP_INTERVAL" CAP="$SIM_CAP" \
    WC3_MAP=open OUT="$LOGDIR/determinism" \
        "$ROOT/tools/determinism_check.sh" >"$LOGDIR/determinism.log" 2>&1
    local rc=$?
    tail -n 8 "$LOGDIR/determinism.log" | sed 's/^/  /'
    [ "$rc" -eq 0 ] || printf '  full log: %s\n' "$LOGDIR/determinism.log"
    return "$rc"
}

# --- bridges --------------------------------------------------------------

# Only ever this checkout's bridge tree. Seat dirs come from the engine's cwd,
# so this is exactly the set of seats this script could have created.
clean_bridge() { rm -rf "$ROOT/bridge"; }

st_bridge_intent() {
    clean_bridge
    mkdir -p "$ROOT/bridge/red"
    local log="$LOGDIR/engine-intent.log"
    env WC3_HEADLESS=1 WC3_BRIDGE=1 WC3_MAP=open \
        WC3_SPEED="$BRIDGE_SPEED" WC3_MAX_GAME_SECS="$BRIDGE_CAP" \
        WC3_READY_TIMEOUT="$BRIDGE_READY_TIMEOUT" \
        "$BIN" >"$log" 2>&1 &
    local pid=$!
    track_engine "$pid"
    printf '  engine pid %s, log %s\n' "$pid" "$log"
    python3 "$ROOT/tools/verify_intent_bridge.py"
    local rc=$?
    stop_engine "$pid"
    printf '  engine pid %s stopped\n' "$pid"
    return "$rc"
}

st_bridge_territory() {
    clean_bridge
    python3 "$ROOT/tools/verify_territory_bridge.py" --bin "$BIN"
}

st_bridge_research() {
    clean_bridge
    python3 "$ROOT/tools/verify_research_bridge.py"
}

st_bridge_r9() {
    clean_bridge
    python3 "$ROOT/tools/verify_r9_legibility.py"
}

# --- identity -------------------------------------------------------------

# fingerprints <binary> <cwd> <map> <seed> <outfile>
# The binary runs with ITS OWN checkout as cwd, because assets/data/*.ron are
# read relative to it and are as much "behavior" as the code is.
fingerprints() {
    local bin="$1" cwd="$2" map="$3" seed="$4" out="$5"
    local log="$out.log" prev="$PWD"
    cd "$cwd" || return 1
    env WC3_HEADLESS=1 WC3_AI_BOTH=1 \
        WC3_MAP="$map" WC3_SEED="$seed" \
        WC3_FIXED_DT="$SIM_DT" WC3_FINGERPRINT="$FP_INTERVAL" \
        WC3_MAX_GAME_SECS="$SIM_CAP" \
        "$bin" >"$log" 2>&1 &
    local pid=$!
    cd "$prev" || return 1
    track_engine "$pid"
    wait "$pid"
    untrack_engine "$pid"
    grep -E 'FINGERPRINT|headless: (game over|time cap)' "$log" \
        | sed -E 's/^.*(FINGERPRINT|headless:)/\1/' >"$out"
    if [ ! -s "$out" ]; then
        printf '  no fingerprints from %s (%s/%s) — see %s\n' "$bin" "$map" "$seed" "$log"
        return 1
    fi
    return 0
}

st_identity() {
    local ref="$IDENT_REF"
    if [ -z "$ref" ]; then
        ref="$(git -C "$ROOT" merge-base HEAD master 2>/dev/null)"
        if [ -z "$ref" ]; then
            printf '  no merge-base with master; pass a ref explicitly\n'
            return 1
        fi
    fi
    local ref_sha head_sha
    ref_sha="$(git -C "$ROOT" rev-parse --verify --quiet "$ref^{commit}")" || {
        printf '  not a commit: %s\n' "$ref"; return 1; }
    head_sha="$(git -C "$ROOT" rev-parse --verify HEAD)" || return 1
    printf '  ref  %s  %s\n' "${ref_sha:0:12}" "$(git -C "$ROOT" log -1 --format=%s "$ref_sha")"
    printf '  head %s  %s\n' "${head_sha:0:12}" "$(git -C "$ROOT" log -1 --format=%s "$head_sha")"
    if [ "$ref_sha" = "$head_sha" ]; then
        printf '  (same commit — this is the self-compare, and it must pass)\n'
    fi
    # "HEAD" here means the working tree, which is the honest thing to compare
    # when the change under test is not committed yet — but say so, because a
    # dirty tree makes the word "HEAD" in this output a small lie.
    if [ -n "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]; then
        printf '  note: working tree is dirty — the "head" side is your tree, not %s\n' "${head_sha:0:12}"
    fi

    # HEAD first, out of the checkout we are standing in, then copied aside so
    # the ref build cannot clobber it. Both builds share one target dir on
    # purpose: a second target dir is a cold build of every dependency, and the
    # only thing that actually recompiles here is this crate.
    printf '  building HEAD ...\n'
    cargo build >"$LOGDIR/build-head.log" 2>&1 || {
        tail -n 30 "$LOGDIR/build-head.log"; return 1; }
    cp "$BIN" "$LOGDIR/bin-head" || return 1

    # `git archive | tar -x`, deliberately, and NOT `git worktree add`: adding a
    # worktree writes into the shared .git directory that every sibling agent's
    # checkout hangs off. Materialising the ref as a plain directory is
    # read-only on the repo, needs no cleanup registration, and there is no
    # build.rs here that would miss the missing .git.
    IDENT_CHECKOUT="$LOGDIR/ref-checkout"
    printf '  extracting ref into %s ...\n' "$IDENT_CHECKOUT"
    mkdir -p "$IDENT_CHECKOUT" || return 1
    git -C "$ROOT" archive --format=tar "$ref_sha" 2>"$LOGDIR/archive.log" \
        | tar -x -C "$IDENT_CHECKOUT" 2>>"$LOGDIR/archive.log"
    if [ ! -f "$IDENT_CHECKOUT/Cargo.toml" ]; then
        printf '  could not materialise %s\n' "${ref_sha:0:12}"
        tail -n 20 "$LOGDIR/archive.log"
        return 1
    fi
    printf '  building ref ...\n'
    ( cd "$IDENT_CHECKOUT" && CARGO_TARGET_DIR="$ROOT/target" cargo build ) \
        >"$LOGDIR/build-ref.log" 2>&1 || {
        tail -n 30 "$LOGDIR/build-ref.log"; return 1; }
    cp "$BIN" "$LOGDIR/bin-ref" || return 1

    local map rc=0
    for map in open crossings; do
        fingerprints "$LOGDIR/bin-head" "$ROOT" "$map" "$IDENT_SEED" \
            "$LOGDIR/fp-head-$map" || { rc=1; continue; }
        fingerprints "$LOGDIR/bin-ref" "$IDENT_CHECKOUT" "$map" "$IDENT_SEED" \
            "$LOGDIR/fp-ref-$map" || { rc=1; continue; }
        local n
        n="$(wc -l <"$LOGDIR/fp-head-$map")"
        if cmp -s "$LOGDIR/fp-head-$map" "$LOGDIR/fp-ref-$map"; then
            printf '  %-10s seed %-4s IDENTICAL over %s samples\n' "$map" "$IDENT_SEED" "$n"
            printf '  %-10s %s\n' "" "$(tail -n 1 "$LOGDIR/fp-head-$map")"
        else
            printf '  %-10s seed %-4s DIVERGED — first difference:\n' "$map" "$IDENT_SEED"
            diff "$LOGDIR/fp-ref-$map" "$LOGDIR/fp-head-$map" | head -n 8 | sed 's/^/      /'
            rc=1
        fi
    done

    # The ref build left target/debug/wc3clone belonging to the ref. Put HEAD's
    # back so the next thing to use this checkout is not quietly running old code.
    cp "$LOGDIR/bin-head" "$BIN" 2>/dev/null
    return "$rc"
}

# ---------------------------------------------------------------------------
# Tiers
# ---------------------------------------------------------------------------

tier_smoke() {
    run_stage "build (dev)"            st_build
    run_stage "cargo test"             st_cargo_test
    run_stage "sim: open, decisive"    st_sim_open
}

tier_standard() {
    tier_smoke
    run_stage "sim: crossings, decisive" st_sim_crossings
    run_stage "python suites"            st_py_suites
}

tier_full() {
    tier_standard
    run_stage "sim matrix (2 seeds x 2 maps)" st_sim_matrix
    run_stage "determinism pair"              st_determinism
    run_stage "bridge: intent"                st_bridge_intent
    run_stage "bridge: territory"             st_bridge_territory
    run_stage "bridge: r9 legibility"         st_bridge_r9
    run_stage "bridge: research"              st_bridge_research
}

tier_identity() {
    run_stage "identity: fingerprint compare" st_identity
}

usage() {
    cat <<'EOF'
tools/verify.sh — the definition of done, as one executable.

usage: tools/verify.sh [smoke|standard|full|identity [ref]] [--list]

  smoke      cargo build (dev) + cargo test + one decisive open-map sim.
             The fastest honest "did I break it".
  standard   smoke + a decisive crossings sim + every tools/test_*.py suite.
             The default bead-level bar, and what a bare invocation runs.
  full       standard + a 2-seed x 2-map sim matrix + a determinism pair +
             all four bridge verifiers driven against a live engine.
  identity   [ref] build ref (default: merge-base with master) and HEAD, run
             seeded fixed-dt fingerprint sims on both maps, byte-compare —
             the cheap proof that a change is no-behavior-change.

Read the header of this file for the knobs and the reasoning.
EOF
}

list_tiers() {
    printf 'smoke     build + cargo test + one decisive open-map sim on the built binary.\n'
    printf 'standard  smoke + a decisive crossings sim + every tools/test_*.py suite.\n'
    printf 'full      standard + 2-seed x 2-map sim matrix + a determinism pair + all four bridge verifiers.\n'
    printf 'identity  [ref] build ref and HEAD, fingerprint both maps with each, byte-compare.\n'
}

IDENT_REF=""
TIER="${1:-standard}"
case "$TIER" in
    --list|-l|list)   list_tiers; exit 0 ;;
    -h|--help|help)   usage; exit 0 ;;
    smoke)            ;;
    standard|"")      TIER=standard ;;
    full)             ;;
    identity)         IDENT_REF="${2:-}" ;;
    *)                printf 'unknown tier: %s\n\n' "$TIER" >&2; usage >&2; exit 2 ;;
esac

printf 'verify.sh %s\n' "$TIER"
printf 'repo:  %s\n' "$ROOT"
printf 'head:  %s\n' "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
printf 'logs:  %s\n' "$LOGDIR"

RUN_START="$(now_ms)"

case "$TIER" in
    smoke)    tier_smoke ;;
    standard) tier_standard ;;
    full)     tier_full ;;
    identity) tier_identity ;;
esac

summary
printf '\nverify.sh %s: PASS (%ss wall)\n' "$TIER" "$(fmt_ms "$(( $(now_ms) - RUN_START ))")"
exit 0
