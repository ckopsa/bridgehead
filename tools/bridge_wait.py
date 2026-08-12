#!/usr/bin/env python3
"""Event-driven pacing for bridge commanders.

Usage: bridge_wait.py --seat bridge/red [--max 15]

Polls the seat's state.json once a second for up to --max seconds, returning
EARLY the moment something noteworthy appears: a new event, a NEW error, or
game over. Prints why it woke. Use instead of a blind sleep so reactions to
attacks and bounty spawns take ~2s, not a full cycle.

Uses its own marker (separate from bridge_view's) so the two never fight over
read position: this tool decides WHEN to look, bridge_view decides WHAT is new.

## Novelty, and why `errors` needed its own kind of it

Events were always edge-triggered: the marker remembers the game time `t` of
the last state this tool woke on, and only events stamped later than that
count. `errors` had no such test — a non-empty array woke the tool, every call,
for as long as the array stayed non-empty. That is fine for the case it was
written for (a batch's refusals, which arrive once and are cleared when the
next batch lands) and wrong for the case that actually happened.

Arena round 17: a plan step blocked on `cannot afford Footman (135g 0l)` kept
that string in the seat's `errors` array, so `bridge_wait` returned instantly on
every call. The commander's loop became a fire hose, they chained waits to get
away from it, went ~100 game-seconds without issuing an order, and lost. The
engine side of that is fixed (plan.rs emits transitions, not repeats), but a
pacing tool that trusts its input to be well-behaved is one bad channel away
from the same failure. So the novelty test lives here too, and it is the same
test the events channel already applies, spelled for a channel with no
timestamps: a **content fingerprint of the error SET**, remembered in the same
marker file.

  * an identical error, re-emitted or merely still standing → no wake;
  * any error the last-seen set did not contain → wake;
  * the set emptying and later refilling with the same error → wake, because
    the fingerprint of an empty set is not the fingerprint of that error.

A set rather than a list: the same two refusals in the other order is the same
news. Belt and braces with the engine fix, deliberately — the two defend
against different mistakes, and the cheap one lives on this side.
"""
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from markers import marker_path  # noqa: E402

#: Marker schema version. Bumping it is how a future change to what gets
#: remembered invalidates stale markers instead of misreading them.
MARKER_VERSION = 2


def fingerprint(errors):
    """A stable digest of an error SET.

    Empty in, empty out — so "no errors" is a distinguishable state rather
    than a hash that happens to collide with nothing, and a returning error
    reads as a change.
    """
    unique = sorted({str(e) for e in (errors or [])})
    if not unique:
        return ""
    return hashlib.sha1("\n".join(unique).encode("utf-8")).hexdigest()


def read_marker(path):
    """`(t, errors_fingerprint)` from the marker, tolerantly.

    Accepts the pre-v2 format — a bare float, and nothing else — so a marker
    written by an older checkout degrades to "events as before, no error
    memory" rather than to a crash or to a spurious first wake.
    """
    try:
        with open(path) as f:
            raw = f.read().strip()
    except Exception:
        return 0.0, ""
    if not raw:
        return 0.0, ""
    try:
        loaded = json.loads(raw)
    except Exception:
        try:
            return float(raw), ""
        except ValueError:
            return 0.0, ""
    if isinstance(loaded, dict):
        try:
            t = float(loaded.get("t", 0.0))
        except (TypeError, ValueError):
            t = 0.0
        return t, str(loaded.get("errors", ""))
    try:
        return float(loaded), ""
    except (TypeError, ValueError):
        return 0.0, ""


def write_marker(path, t, errors_fp):
    """Remember both halves, always together.

    Every exit writes the CURRENT error fingerprint, including a wake caused by
    something else entirely. An error that arrived alongside the event that
    woke us has been delivered — the commander is about to read the whole
    snapshot — and announcing it again on the next call would be the repeat
    this tool exists to stop.
    """
    try:
        with open(path, "w") as f:
            json.dump({"v": MARKER_VERSION, "t": t, "errors": errors_fp}, f)
    except Exception:
        pass


def main():
    args = sys.argv[1:]
    seat, max_wait = "bridge/red", 15.0
    while args:
        a = args.pop(0)
        if a == "--seat":
            seat = args.pop(0).rstrip("/")
        elif a == "--max":
            max_wait = float(args.pop(0))

    state_path = f"{seat}/state.json"
    marker = marker_path("bridge_wait", seat)

    def read():
        try:
            with open(state_path) as f:
                return json.load(f)
        except Exception:
            return None

    # The seat's own name, as it appears in a held snapshot's `waiting_for`:
    # `bridge/red` -> `red`.
    me = seat.rsplit("/", 1)[-1]

    seen, seen_errors = read_marker(marker)
    deadline = time.monotonic() + max_wait
    # Did we see the match being held during THIS call? The pre-match hold is
    # the one condition where the event channel cannot speak for itself: the
    # game clock is frozen at 0, so `t > seen` is false for every event in the
    # snapshot including `match start` itself. Tracking the hold in-process is
    # what lets the start still wake a commander early.
    was_held = False
    while True:
        s = read()
        if s is not None:
            errors = s.get("errors") or []
            errors_fp = fingerprint(errors)
            if s.get("game_over"):
                write_marker(marker, s["t"], errors_fp)
                print(f"WAKE: game over ({s['game_over']})")
                return
            # --- the ready handshake (docs/INTENT.md) -----------------------
            waiting = s.get("waiting_for")
            if waiting is not None:
                was_held = True
                if me in waiting:
                    # The engine is waiting on US, and no amount of sleeping
                    # fixes that. This is the one wake that is an instruction
                    # rather than news, and it must be immediate: every second
                    # spent here is a second the OTHER commander spends reading
                    # the map, which is precisely the asymmetry the handshake
                    # exists to remove.
                    print(
                        "WAKE: the match has not started — send "
                        "'[{\"type\":\"ready\"}]' when you have read the map and "
                        f"set your opening (waiting for: {' '.join(waiting)})"
                    )
                    return
                # We have been heard; the hold is somebody else's. Keep
                # waiting — this is a real quiet cycle, not a busy loop.
            elif was_held:
                write_marker(marker, s.get("t", 0.0), errors_fp)
                print("WAKE: match started — the clock is running from t=0")
                return
            fresh = [m for t, m in s.get("events", []) if t > seen]
            if fresh:
                write_marker(marker, s["t"], errors_fp)
                print(f"WAKE after {max_wait - (deadline - time.monotonic()):.0f}s: " + " | ".join(fresh[-4:]))
                return
            # NEW errors only. A blocked plan step that keeps failing the same
            # way is a condition, not an event — read it off `plans[].status`
            # when you next look, and do not let it decide your cadence.
            if errors and errors_fp != seen_errors:
                write_marker(marker, s["t"], errors_fp)
                print("WAKE: new command errors — " + " | ".join(errors[:3]))
                return
        if time.monotonic() >= deadline:
            if s is not None:
                write_marker(marker, s["t"], fingerprint(s.get("errors") or []))
            waiting = (s or {}).get("waiting_for")
            if waiting:
                # Named rather than folded into "quiet cycle": a commander that
                # has readied and is waiting on its opponent should be able to
                # tell that from a match that is simply uneventful, or it will
                # start hunting for a bug in its own loop.
                print(
                    f"WAKE: still held at t=0 after {max_wait:.0f}s — "
                    f"waiting for: {' '.join(waiting)}"
                )
            else:
                print(f"WAKE: quiet cycle ({max_wait:.0f}s)")
            return
        time.sleep(1.0)


if __name__ == "__main__":
    main()
