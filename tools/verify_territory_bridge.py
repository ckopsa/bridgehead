#!/usr/bin/env python3
"""End-to-end proof that TERRITORY works over the live bridge.

    python3 tools/verify_territory_bridge.py [--bin target/debug/wc3clone]

Boots a real game with a bridge seat, then drives the whole feature through the
wire exactly as a commander would — no test harness, no direct resource pokes:

  1. the seat reads `map.places` and can speak a ford's name with NOTHING armed
  2. `region_set` names ground; it comes back in `regions`, own-team only
  3. a posture NAMES the region and the engine resolves it (checked against the
     squad's live posture in the seat's own snapshot)
  4. an `enemy_in` trigger armed on that region FIRES when the enemy walks in,
     and says so in `events` with the place NAMED rather than spelled in floats
  5. a misspelled name is refused with the list of known places

Exits non-zero with the failing check named. Everything it asserts is something
a commander could have read from its own snapshot.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SEAT = os.path.join(ROOT, "bridge", "red")
STATE = os.path.join(SEAT, "state.json")
COMMANDS = os.path.join(SEAT, "commands.json")

FAILURES = []


def check(name, ok, detail=""):
    mark = "ok  " if ok else "FAIL"
    print(f"  {mark} {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        FAILURES.append(name)
    return ok


POSTURE_RE = re.compile(
    r"^(?P<word>defend|push|forage)@\((?P<x>-?[\d.]+),\s*(?P<z>-?[\d.]+)\)"
    r"(?:r=(?P<r>[\d.]+))?")


def parse_posture(text):
    """`"defend@(-60.0, 60.0)r=20"` -> a dict. The snapshot spells a squad's
    posture as one string; this is the seat's own reading of it."""
    if not text:
        return {}
    m = POSTURE_RE.match(text)
    if not m:
        return {"raw": text}
    out = {"type": m.group("word"), "x": float(m.group("x")),
           "z": float(m.group("z"))}
    if m.group("r"):
        out["radius"] = float(m.group("r"))
    return out


def read_state():
    for _ in range(50):
        try:
            with open(STATE) as f:
                return json.load(f)
        except Exception:
            time.sleep(0.1)
    raise SystemExit("no snapshot from the game — is the seat wired?")


def send(commands):
    """One batch, at the next seq, the way bridge_send.py does it."""
    seq = 0
    for path in (STATE, COMMANDS):
        try:
            with open(path) as f:
                d = json.load(f)
            seq = max(seq, d.get("seq_applied", 0), d.get("seq", 0))
        except Exception:
            pass
        tmp = COMMANDS + ".tmp"
    with open(tmp, "w") as f:
        json.dump({"seq": seq + 1, "commands": commands}, f)
    os.replace(tmp, COMMANDS)
    return seq + 1


def wait_applied(seq, timeout=20.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        s = read_state()
        if s.get("seq_applied", 0) >= seq:
            return s
        time.sleep(0.2)
    return read_state()


def wait_for(predicate, timeout=40.0):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = read_state()
        if predicate(last):
            return last, True
        time.sleep(0.25)
    return last, False


def main():
    binary = os.path.join(ROOT, "target", "debug", "wc3clone")
    args = sys.argv[1:]
    if args and args[0] == "--bin":
        binary = args[1]
    if not os.path.exists(binary):
        raise SystemExit(f"no binary at {binary} — cargo build first")

    shutil.rmtree(os.path.join(ROOT, "bridge"), ignore_errors=True)
    os.makedirs(SEAT, exist_ok=True)

    env = dict(os.environ)
    env.update({
        "WC3_MAP": "crossings",
        "WC3_BRIDGE": "red",
        "WC3_HEADLESS": "1",
        "WC3_SPEED": "4",
        "WC3_FOG": "0",           # the fog rules are unit-tested; this run is
                                   # about the vocabulary, and a scout hunt
                                   # would make it flaky rather than honest.
        "WC3_MAX_GAME_SECS": "1800",
        "WC3_SEED": "7",
    })
    print(f"launching {binary} (crossings, headless, bridge/red)")
    game = subprocess.Popen([binary], cwd=ROOT, env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        state = read_state()

        # A bridged seat now gates the start (docs/INTENT.md, "The ready
        # handshake"): the world is spawned and photographed, but the clock
        # does not move until every seat says the word. This script's checks
        # are about vocabulary rather than elapsed time, but `wait_applied`
        # still needs the engine to be compiling batches, so say it. Harmless
        # against an engine built before the handshake — `ready` is then an
        # unrecognized verb that the next batch clears.
        if "waiting_for" in state:
            print(f"held at t=0, waiting for {state['waiting_for']} — sending ready")
            send([{"type": "ready"}])
            state, started = wait_for(lambda s: "waiting_for" not in s)
            check("the ready handshake starts the match", started,
                  f"still waiting for {(state or {}).get('waiting_for')}")

        # --- 1. the map's own vocabulary, before anything is armed --------
        places = {p["name"]: p for p in state.get("map", {}).get("places", [])}
        check("map.places carries the map's vocabulary",
              len(places) == 10, f"{len(places)} names: {', '.join(sorted(places))}")
        check("the fords are nameable",
              {"northwest ford", "center ford", "southeast ford"} <= set(places))
        check("the mines are nameable by compass corner",
              {"northwest mine", "southeast mine", "southwest mine",
               "northeast mine"} <= set(places))
        check("no regions armed yet", not state.get("regions"))

        # A ford, named, with nothing armed. This is the "built-ins exist
        # without arming" claim, made over the wire.
        seq = send([{"type": "posture", "id": 3,
                     "posture": {"type": "defend", "region": "center ford"}}])
        state = wait_applied(seq)
        squads = {s["id"]: s for s in state.get("squads", [])}
        centre = places["center ford"]
        posture3 = parse_posture(squads.get(3, {}).get("posture"))
        check("a built-in place is speakable with nothing armed",
              not state.get("errors") and posture3.get("type") == "defend",
              json.dumps(posture3))
        check("...and the ford's own radius became the ring",
              abs(float(posture3.get("radius", 0)) - centre["radius"]) < 0.51,
              f"ring {posture3.get('radius')} vs ford {centre['radius']}")

        # --- 2. naming ground --------------------------------------------
        seq = send([
            {"type": "region_set", "name": "north-pass",
             "x": -60.0, "z": 60.0, "radius": 20.0},
        ])
        state = wait_applied(seq)
        regions = {r["name"]: r for r in state.get("regions", [])}
        check("region_set names ground", "north-pass" in regions,
              json.dumps(state.get("regions")))
        check("...at the shape we asked for",
              regions.get("north-pass", {}).get("radius") == 20.0
              and regions.get("north-pass", {}).get("pos") == [-60.0, 60.0])

        # --- 3. a posture that NAMES it ----------------------------------
        seq = send([
            {"type": "squad", "units": [u["id"] for u in state["units"]
                                        if u.get("kind") != "Worker"][:3], "id": 2},
            {"type": "posture", "id": 2,
             "posture": {"type": "defend", "region": "north-pass"}},
        ])
        state = wait_applied(seq)
        squads = {s["id"]: s for s in state.get("squads", [])}
        posture = parse_posture(squads.get(2, {}).get("posture"))
        check("a posture may name a region", posture.get("type") == "defend",
              json.dumps(posture))
        check("...and the engine resolved it to the region's centre",
              abs(float(posture.get("x", 0)) + 60.0) < 0.51
              and abs(float(posture.get("z", 0)) - 60.0) < 0.51,
              f"({posture.get('x')}, {posture.get('z')})")
        check("...and took the region's own radius as the ring",
              abs(float(posture.get("radius", 0)) - 20.0) < 0.51,
              str(posture.get("radius")))

        # --- 4. the teaching refusal --------------------------------------
        seq = send([{"type": "move", "units": [state["units"][0]["id"]],
                     "region": "the-perimiter"}])
        state = wait_applied(seq)
        errs = state.get("errors", [])
        check("a misspelled place is refused", any("no region named" in e for e in errs),
              errs[0] if errs else "(no error at all)")
        check("...with the list of names this seat may speak",
              any("north-pass" in e and "center ford" in e for e in errs),
              errs[0] if errs else "")

        # --- 5. an enemy_in trigger, armed and fired ----------------------
        seq = send([
            {"type": "trigger_set", "name": "pass-watch", "repeat": 30,
             "when": {"type": "enemy_in", "region": "north-pass", "count": 3},
             "then": {"type": "posture", "id": 2,
                      "posture": {"type": "defend", "region": "north-pass"}}},
        ])
        state = wait_applied(seq)
        triggers = {t["name"]: t for t in state.get("triggers", [])}
        check("the enemy_in rule armed", "pass-watch" in triggers,
              json.dumps(state.get("errors")))
        check("...and reads as English with the place named",
              "north-pass" in triggers.get("pass-watch", {}).get("sentence", ""),
              triggers.get("pass-watch", {}).get("sentence", ""))

        # Arming a rule on a name this seat does not have must be refused
        # immediately, not at fire time.
        seq = send([
            {"type": "trigger_set", "name": "bad-watch",
             "when": {"type": "enemy_in", "region": "nowhere", "count": 3},
             "then": {"type": "stop", "units": []}},
        ])
        state = wait_applied(seq)
        errs = state.get("errors", [])
        check("an unknown place is refused AT ARM TIME",
              any("no region named 'nowhere'" in e for e in errs),
              errs[0] if errs else "(no error)")
        check("...and the rule was not armed",
              "bad-watch" not in {t["name"] for t in state.get("triggers", [])})

        # Now walk the enemy in. The scripted AI owns blue, so rather than wait
        # for it to attack a ford, the rule is re-aimed onto ground the enemy
        # is standing on already: their base. One `region_set` moving the
        # circle re-aims the ARMED RULE — which is the late-binding claim, made
        # live.
        their_base = places["their base"]
        seq = send([{"type": "region_set", "name": "north-pass",
                     "x": their_base["pos"][0], "z": their_base["pos"][1],
                     "radius": 30.0}])
        wait_applied(seq)
        state, fired = wait_for(
            lambda s: any("trigger pass-watch fired" in e[1]
                          for e in s.get("events", [])), timeout=60.0)
        lines = [e[1] for e in state.get("events", [])
                 if "pass-watch" in e[1]]
        check("moving the region re-aimed the armed rule, and it fired",
              fired, lines[0] if lines else "(never fired)")
        check("...and the fired line names the PLACE, not two floats",
              any("defends north-pass" in l for l in lines),
              lines[0] if lines else "")

        # --- 6. forgetting ------------------------------------------------
        seq = send([{"type": "region_clear", "name": "north-pass"}])
        state = wait_applied(seq)
        check("region_clear forgets it",
              "north-pass" not in {r["name"] for r in state.get("regions", [])})
        seq = send([{"type": "region_clear", "name": "our base"}])
        state = wait_applied(seq)
        check("...and a built-in cannot be forgotten",
              any("cannot be cleared" in e for e in state.get("errors", [])),
              (state.get("errors") or ["(no error)"])[0])
    finally:
        game.terminate()
        try:
            game.wait(timeout=10)
        except Exception:
            game.kill()

    print()
    if FAILURES:
        print(f"FAILED: {len(FAILURES)} check(s): {', '.join(FAILURES)}")
        return 1
    print("all territory checks passed over the live bridge")
    return 0


if __name__ == "__main__":
    sys.exit(main())
