#!/usr/bin/env python3
"""End-to-end check of the round-9/10 legibility fixes over a live bridge seat.

Drives a `WC3_BRIDGE=1` seat and asserts, against the real protocol rather than
against unit-test doubles, that four AAR findings are actually fixed:

  * `wc3clone-pbd` — `catalog.json` shows a trainer's gate ON the trainer
    (`buildings[].trains_gated`), and a refused `train` names the building that
    will accept the order once the gate is met;
  * `wc3clone-vjy` — a blocked `build` names the clearance rule and the nearest
    legal site, and that site is legal (asserted by BUILDING there);
  * `wc3clone-azo` — `game_over_reason` reports which win it was, while
    `game_over` keeps its historical string-or-null shape;
  * `wc3clone-d4y` — a `cast` at a dead id says "not found", not "is not a hero
    or an own ability building".

Run from the repo root. Starts and stops the game itself.
"""
import json
import os
import subprocess
import sys
import time

SEAT = "bridge/red"
STATE = os.path.join(SEAT, "state.json")
CATALOG = os.path.join(SEAT, "catalog.json")
SEND = [sys.executable, "tools/bridge_send.py", "--seat", SEAT]

failures = []


def check(cond, msg):
    print(f"  {'ok  ' if cond else 'FAIL'} {msg}")
    if not cond:
        failures.append(msg)


def read_state():
    for _ in range(300):
        try:
            with open(STATE) as f:
                return json.load(f)
        except Exception:
            time.sleep(0.1)
    raise SystemExit("FAIL: no readable state.json")


def send(cmds):
    subprocess.run(SEND + [json.dumps(cmds)], check=True, stdout=subprocess.DEVNULL)


def wait_for(pred, what, timeout=120.0):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = read_state()
        if pred(last):
            return last
        time.sleep(0.25)
    raise SystemExit(f"FAIL: timed out waiting for {what}")


def errors_after(cmds, what, timeout=30.0):
    """Send a batch and return the errors the seat reports for it."""
    before = read_state().get("seq_applied", 0)
    send(cmds)
    st = wait_for(lambda s: s.get("seq_applied", 0) > before, what, timeout)
    return st.get("errors", [])


def start_match():
    """Clear the ready handshake if the engine is holding the match at t=0.

    A bridged seat now gates the start (docs/INTENT.md, "The ready handshake"):
    the world is spawned and photographed, but the clock does not move until
    every seat says the word. This script waits on a real verdict, so it needs
    a clock that runs. Harmless against an engine built before the handshake —
    `ready` is then an unrecognized verb that lands in `errors` and is cleared
    by the next batch, and `waiting_for` is never there to begin with.
    """
    st = read_state()
    if "waiting_for" not in st:
        return st
    print(f"[0] held at t=0, waiting for {st['waiting_for']} — sending ready")
    send([{"type": "ready"}])
    return wait_for(lambda s: "waiting_for" not in s, "the match to start")


def run_checks():
    start_match()
    st = wait_for(lambda s: s.get("units"), "the opening snapshot")
    print(f"[0] seat live at t={st['t']}s, my_team={st['my_team']}")

    # --- wc3clone-pbd: the roster carries its own gates ---------------------
    print("\n[1] catalog: the gate is where the roster is read")
    with open(CATALOG) as f:
        cat = json.load(f)
    by_id = {b["id"]: b for b in cat["buildings"]}
    barracks = by_id["Barracks"]
    check("trains_gated" in barracks, "buildings[] carries trains_gated")
    gated = {t["unit"]: t for t in barracks.get("trains_gated", [])}
    check(gated.get("Raider", {}).get("requires") == ["Workshop"],
          f"Barracks/Raider gate is visible on the Barracks entry: "
          f"{gated.get('Raider')}")
    check(gated.get("Knight", {}).get("requires") == ["Castle"],
          f"Barracks/Knight gate: {gated.get('Knight')}")
    check(gated.get("Footman", {}).get("requires") == [],
          "an ungated unit says so with an empty list")
    check([t["unit"] for t in barracks["trains_gated"]] == barracks["trains"],
          "trains and trains_gated are parallel (historical key untouched)")
    sanctum = by_id["Sanctum"]
    check(sanctum["requires"] == ["Keep"],
          "the Sorcerer's gate rides on its trainer, as designed")

    # --- wc3clone-pbd: the rejection teaches --------------------------------
    print("\n[2] a refused train names the building that will take the order")
    hall = next(b for b in st["buildings"]
                if b["team"] == st["my_team"] and b["kind"] == "TownHall")
    barracks_live = [b for b in st["buildings"]
                     if b["team"] == st["my_team"] and b["kind"] == "Barracks"]
    # No Barracks in the opening, so use the hall for the wrong-trainer string
    # and let the gate string come from wherever a Raider is legal to ask for.
    errs = errors_after([{"type": "train", "building": hall["id"], "unit": "Raider"}],
                        "the Raider rejection")
    msg = next((e for e in errs if "Raider" in e), "")
    print(f"    -> {msg}")
    check("trains at the Barracks" in msg,
          "the rejection names the Barracks as the trainer")
    check("is not a hero" not in msg, "no leftover generic phrasing")

    errs = errors_after([{"type": "train", "building": hall["id"], "unit": "Sorcerer"}],
                        "the Sorcerer rejection")
    msg = next((e for e in errs if "Sorcerer" in e), "")
    print(f"    -> {msg}")
    check("trains at the Sanctum" in msg, "the Sorcerer's trainer is named")
    check("Keep" in msg, "the Sanctum's own gate is named too")

    # --- wc3clone-vjy: a blocked site names a legal one ---------------------
    print("\n[3] a blocked placement names the rule and a legal alternative")
    worker = next(u for u in st["units"]
                  if u["team"] == st["my_team"] and u["kind"] == "Worker")
    mine = min(st["mines"], key=lambda m: (m["pos"][0] - hall["pos"][0]) ** 2
                                        + (m["pos"][1] - hall["pos"][1]) ** 2)
    # Straight on top of a gold mine: its 6x6 block plus a TownHall's 8x8 makes
    # this the exact site a commander's eye picks and the compiler refuses.
    errs = errors_after([{"type": "build", "worker": worker["id"],
                          "kind": "TownHall", "x": mine["pos"][0],
                          "z": mine["pos"][1]}], "the blocked-site rejection")
    msg = next((e for e in errs if "blocked" in e), "")
    print(f"    -> {msg}")
    check("8x8 clear" in msg, "the clearance rule is stated")
    check("mines block 6x6" in msg, "the mine footprint is stated")
    check("nearest legal:" in msg, "an alternative is offered")

    hint = msg.split("nearest legal: (")[1].split(")")[0]
    hx, hz = (float(v) for v in hint.split(", "))
    print(f"    hint = ({hx}, {hz})")
    # The real assertion: the hint is legal. Send the same build there and
    # demand the compiler take it.
    errs = errors_after([{"type": "build", "worker": worker["id"],
                          "kind": "TownHall", "x": hx, "z": hz}],
                        "the hinted placement")
    blocked = [e for e in errs if "blocked" in e]
    check(not blocked, f"the hinted site is accepted by the same validator: {blocked}")

    # --- wc3clone-d4y: a cast at a dead id says what is wrong ---------------
    print("\n[4] a cast at an id nothing owns says so plainly")
    # A WELL-FORMED id that nothing owns — not a garbage integer, which the
    # `intent_entity` guard catches earlier with a different string. Shifting a
    # real hall's index leaves the generation bits intact, so this reaches the
    # caster resolution proper, which is the code round 10 was about.
    dead_id = hall["id"] + 100000
    errs = errors_after([{"type": "cast", "hero": dead_id,
                          "ability": "CallToArms"}], "the bad-caster rejection")
    msg = next((e for e in errs if "caster" in e), "")
    print(f"    -> {msg}")
    check("not found" in msg, "the real cause is named")
    check("is not a hero or an own ability building" not in msg,
          "the string that sent round 10 to the catalog is gone")
    # ...and a real hall IS a caster, from the same seat.
    errs = errors_after([{"type": "cast", "hero": hall["id"],
                          "ability": "CallToArms"}], "the hall cast")
    check(not [e for e in errs if "caster" in e],
          f"an own TownHall casts Call to Arms: {errs}")

    # --- wc3clone-azo: which win was it -------------------------------------
    print("\n[5] game_over_reason, and game_over keeping its shape")
    live = read_state()
    check("game_over_reason" not in live,
          "absent for the whole live match (historical key set untouched)")
    check(live["game_over"] is None, "game_over is still null-or-string")

    send([{"type": "surrender"}])
    try:
        over = wait_for(lambda s: s.get("game_over"), "the match to end", timeout=90)
    except SystemExit:
        last = read_state()
        print(f"    last errors: {last.get('errors')}")
        print(f"    last t={last.get('t')} game_over={last.get('game_over')!r}")
        raise
    print(f"    -> game_over={over['game_over']!r} "
          f"game_over_reason={over.get('game_over_reason')!r}")
    check(isinstance(over["game_over"], str),
          "game_over is STILL a bare team name (bridge_view.py/bridge_wait.py)")
    check(over.get("game_over_reason") == "surrender",
          "the reason says which win it was")


def main():
    os.makedirs(SEAT, exist_ok=True)
    for stale in (STATE, os.path.join(SEAT, "commands.json")):
        if os.path.exists(stale):
            os.remove(stale)

    env = dict(os.environ)
    env.update(
        WC3_BRIDGE="1",
        WC3_HEADLESS="1",
        # Deliberately slow. `headless_exit` quits ~5 GAME seconds after a
        # verdict, and at high speed that window can close before another
        # snapshot is written — so the final `game_over_reason` this script
        # exists to read would never reach disk.
        WC3_SPEED="2",
        WC3_MAX_GAME_SECS="4000",
        WC3_MAP="open",
        RUST_LOG="wc3clone=info",
    )
    game = subprocess.Popen(
        ["cargo", "run", "--quiet"], env=env,
        stdout=open("/tmp/r9_legibility_game.log", "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        run_checks()
    finally:
        game.terminate()
        try:
            game.wait(timeout=30)
        except subprocess.TimeoutExpired:
            game.kill()

    print()
    if failures:
        print(f"FAILED ({len(failures)}):")
        for f in failures:
            print(f"  - {f}")
        sys.exit(1)
    print("ALL CHECKS PASSED")


if __name__ == "__main__":
    main()
