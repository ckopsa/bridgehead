#!/usr/bin/env bash
#
# determinism_check.sh — run the same match twice and prove it is the same match.
#
# Two headless AI-vs-AI runs with an identical BH_SEED and an identical fixed
# tick should produce byte-identical FINGERPRINT lines (a hash of every unit's
# and building's position and health, plus both economies) and the same verdict.
#
# Usage:
#   tools/determinism_check.sh                  # default: seed 42, 0.05s tick, open map
#   BH_MAP=crossings tools/determinism_check.sh
#   SEED=7 CAP=300 tools/determinism_check.sh
#
# Exit status is the result: 0 = the two runs are identical, 1 = they diverged
# (and the first differing fingerprint is printed, which is the frame to look
# at).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

SEED="${SEED:-42}"
DT="${DT:-0.05}"
INTERVAL="${INTERVAL:-10}"
CAP="${CAP:-600}"
MAP="${BH_MAP:-open}"
OUT="${OUT:-$(mktemp -d)}"

# Release if it is already built (a full match runs in seconds), otherwise
# whatever debug binary is lying around, otherwise build one. Determinism is a
# property of the schedule and the seed, not of the optimisation level — the
# release binary is a speed convenience, never part of the claim.
BIN="${BIN:-}"
if [ -z "$BIN" ]; then
  for cand in target/release/bridgehead target/debug/bridgehead; do
    [ -x "$cand" ] && BIN="$cand" && break
  done
fi
if [ -z "$BIN" ]; then
  echo "no binary found; building a debug one ..."
  cargo build --quiet || exit 1
  BIN="target/debug/bridgehead"
fi
echo "binary: $BIN"

run() {
  local n="$1"
  BH_HEADLESS=1 \
  BH_AI_BOTH=1 \
  BH_MAP="$MAP" \
  BH_SEED="$SEED" \
  BH_FIXED_DT="$DT" \
  BH_FINGERPRINT="$INTERVAL" \
  BH_MAX_GAME_SECS="$CAP" \
    "$BIN" >"$OUT/run$n.log" 2>&1
  # The fingerprints plus the verdict: the whole timeline and how it ended.
  grep -E 'FINGERPRINT|headless: (game over|time cap)' "$OUT/run$n.log" \
    | sed -E 's/^.*(FINGERPRINT|headless:)/\1/' >"$OUT/fp$n.txt"
}

echo "map=$MAP seed=$SEED tick=${DT}s fingerprint every ${INTERVAL}s, cap ${CAP}s"
echo "run 1 ..."; run 1
echo "run 2 ..."; run 2

n1=$(wc -l <"$OUT/fp1.txt")
n2=$(wc -l <"$OUT/fp2.txt")
echo "run 1: $n1 sampled lines   run 2: $n2 sampled lines"

if [ "$n1" -eq 0 ]; then
  echo "FAIL: no fingerprints — did the run start? see $OUT/run1.log"
  exit 1
fi

if diff -q "$OUT/fp1.txt" "$OUT/fp2.txt" >/dev/null; then
  echo
  echo "IDENTICAL: $n1 fingerprints match across both runs."
  echo "  first: $(head -1 "$OUT/fp1.txt")"
  echo "  last:  $(tail -2 "$OUT/fp1.txt" | head -1)"
  echo "  ended: $(tail -1 "$OUT/fp1.txt")"
  exit 0
fi

echo
echo "DIVERGED. First differing sample:"
diff "$OUT/fp1.txt" "$OUT/fp2.txt" | head -12
echo
echo "logs: $OUT/run1.log $OUT/run2.log"
exit 1
