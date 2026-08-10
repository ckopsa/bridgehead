#!/usr/bin/env python3
"""Event-driven pacing for bridge commanders.

Usage: bridge_wait.py --seat bridge/red [--max 15]

Polls the seat's state.json once a second for up to --max seconds, returning
EARLY the moment something noteworthy appears: a new event, a new error, or
game over. Prints why it woke. Use instead of a blind sleep so reactions to
attacks and bounty spawns take ~2s, not a full cycle.

Uses its own marker (separate from bridge_view's) so the two never fight over
read position: this tool decides WHEN to look, bridge_view decides WHAT is new.
"""
import json
import sys
import time


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
    marker = "/tmp/claude-1000/bridge_wait_" + seat.replace("/", "_")

    def read():
        try:
            with open(state_path) as f:
                return json.load(f)
        except Exception:
            return None

    def last_seen():
        try:
            with open(marker) as f:
                return float(f.read().strip())
        except Exception:
            return 0.0

    def remember(t):
        try:
            with open(marker, "w") as f:
                f.write(str(t))
        except Exception:
            pass

    seen = last_seen()
    deadline = time.monotonic() + max_wait
    while True:
        s = read()
        if s is not None:
            if s.get("game_over"):
                remember(s["t"])
                print(f"WAKE: game over ({s['game_over']})")
                return
            fresh = [m for t, m in s.get("events", []) if t > seen]
            if fresh:
                remember(s["t"])
                print(f"WAKE after {max_wait - (deadline - time.monotonic()):.0f}s: " + " | ".join(fresh[-4:]))
                return
            if s.get("errors"):
                remember(s["t"])
                print("WAKE: command errors — " + " | ".join(s["errors"][:3]))
                return
        if time.monotonic() >= deadline:
            if s is not None:
                remember(s["t"])
            print(f"WAKE: quiet cycle ({max_wait:.0f}s)")
            return
        time.sleep(1.0)


if __name__ == "__main__":
    main()
