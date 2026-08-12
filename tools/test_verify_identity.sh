#!/usr/bin/env bash
#
# test_verify_identity.sh — the identity tier's own negative.
#
# `tools/verify.sh identity` is the proof this project reaches for whenever
# somebody claims a change is no-behavior-change. A proof that cannot fail is
# not a proof, and this tier's specific way of not failing is silent: it builds
# two trees into ONE cargo unit (see the long comment above st_identity), so
# whenever cargo decides the second build is "fresh", the tier compares a binary
# with itself and reports IDENTICAL in a minute flat. Nothing errors. The
# transcript looks exactly like a real pass.
#
# So this script asks the tier to do the two things a working tier must:
#
#   1. PASS when the ref is HEAD          — a real self-compare, twice-built.
#   2. FAIL when the ref genuinely differs — a synthetic commit whose only
#      change is one number in assets/data/units.ron (the Worker's speed).
#
# The ORDER is the load-bearing part. Case 2 runs second, with a different ref
# from case 1, which is exactly the shape of the bug: a second identity run
# against a different ref that recompiles nothing gets case 1's ref binary,
# compares HEAD with HEAD, and passes. So a regression turns case 2's expected
# DIVERGED into an IDENTICAL, and this script fails on it.
#
# Why a data file and not a line of Rust: assets/data/*.ron is `include_str!`d
# into the binary, so a one-number data edit is a genuinely different binary and
# a genuinely different sim, while being immune to every refactor that would
# move a Rust anchor out from under a sed. It also demonstrates the thing agents
# ask about — a data-only change IS caught by identity, because the compiled-in
# copy travels with the binary. (The reverse trick does not exist: BH_DATA_DIR
# would make a divergence free of any rebuild, but the tier deliberately runs
# both binaries without it, so a BH_DATA_DIR divergence would test the sim
# comparison while skipping the build path this test is here for.)
#
# The synthetic commit is written straight into the object database with
# `git commit-tree` against a temporary index: no branch, no ref, no touch of
# the working tree or the real index. `git archive` can read it; nothing else
# will ever see it.
#
# Cost: two identity runs, four crate compiles, ~10 minutes warm. This is a
# manual test — it is deliberately not a `tools/test_*.py`, because it must not
# land in the `standard` tier that every bead runs.
#
# Usage:  tools/test_verify_identity.sh
# Knobs:  CAP=120 (game-second cap for each sim), MAP=open
#
# Exit status is the result: 0 = the tier still fails when it should.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

CAP="${CAP:-120}"
MAP="${MAP:-open}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/bhidentity.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

fail() { printf 'FAIL: %s\n' "$*"; exit 1; }

head_sha="$(git -C "$ROOT" rev-parse --verify HEAD)" || fail "no HEAD"

if [ -n "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]; then
    printf 'note: working tree is dirty. The "head" side of every comparison is\n'
    printf '      your tree, so case 1 (self-compare) can legitimately diverge if\n'
    printf '      your edits change the sim. Stash them if it does.\n\n'
fi

# --- the synthetic divergent ref -------------------------------------------

units="$WORK/units.ron"
git -C "$ROOT" show "$head_sha:assets/data/units.ron" >"$units" \
    || fail "could not read assets/data/units.ron at HEAD"
# The first speed in the table is the Worker's, and workers are moving by the
# first second of every match on every map — the divergence shows up in the
# first fingerprint sample rather than in some late battle that a short cap
# might never reach.
sed -i '0,/speed: 8.0,/s//speed: 8.25,/' "$units" || fail "sed failed"
if git -C "$ROOT" show "$head_sha:assets/data/units.ron" | cmp -s - "$units"; then
    fail "the anchor 'speed: 8.0,' is gone from units.ron — pick a new one"
fi

export GIT_INDEX_FILE="$WORK/index"
git -C "$ROOT" read-tree "$head_sha" || fail "read-tree"
blob="$(git -C "$ROOT" hash-object -w "$units")" || fail "hash-object"
git -C "$ROOT" update-index --add --cacheinfo "100644,$blob,assets/data/units.ron" \
    || fail "update-index"
tree="$(git -C "$ROOT" write-tree)" || fail "write-tree"
unset GIT_INDEX_FILE
divergent="$(git -C "$ROOT" commit-tree "$tree" -p "$head_sha" \
    -m 'identity self-test: Worker speed 8.0 -> 8.25')" || fail "commit-tree"

printf 'head:      %s\n' "${head_sha:0:12}"
printf 'divergent: %s  (unreferenced; one number in units.ron)\n' "${divergent:0:12}"
printf 'map %s, cap %ss per sim\n' "$MAP" "$CAP"

# --- the two cases ----------------------------------------------------------

# run_case <name> <ref> <expect: pass|fail> <expect-in-output>
run_case() {
    local name="$1" ref="$2" expect="$3" needle="$4"
    local log="$WORK/$name.log" rc
    printf '\n=== %s: identity %s, expecting %s ===\n' "$name" "${ref:0:12}" "$expect"
    IDENT_MAPS="$MAP" SIM_CAP="$CAP" KEEP_LOGS=0 \
        "$ROOT/tools/verify.sh" identity "$ref" >"$log" 2>&1
    rc=$?
    sed -n '/=== identity/,$p' "$log" | head -n 24 | sed 's/^/  /'

    if [ "$expect" = pass ] && [ "$rc" -ne 0 ]; then
        printf '  full log: %s\n' "$log"
        fail "$name: identity exited $rc; it must pass"
    fi
    if [ "$expect" = fail ] && [ "$rc" -eq 0 ]; then
        printf '  full log: %s\n' "$log"
        fail "$name: identity PASSED against a ref that really differs — the tier is comparing a stale artifact with itself"
    fi
    if ! grep -qF "$needle" "$log"; then
        printf '  full log: %s\n' "$log"
        fail "$name: expected '$needle' in the output"
    fi
    # The positive assertion the tier makes about itself: it says which
    # directory each binary was compiled from, and the ref's directory is keyed
    # by the ref's SHA. If this line is missing, the comparison used a binary
    # nobody in this run compiled.
    if ! grep -qF "compiled bridgehead from $ROOT/target/verify-identity/ref-${ref:0:12}" "$log"; then
        printf '  full log: %s\n' "$log"
        fail "$name: the ref build did not compile ${ref:0:12}"
    fi
    printf '  %s: as expected (%s)\n' "$name" "$expect"
}

run_case self-compare "$head_sha"  pass IDENTICAL
run_case divergent    "$divergent" fail DIVERGED

printf '\nPASS: the identity tier passes on itself and fails on a real change.\n'
printf 'note: identity dropped this checkout'"'"'s crate fingerprint on the way out —\n'
printf '      the next `cargo build` here recompiles once, on purpose.\n'
exit 0
