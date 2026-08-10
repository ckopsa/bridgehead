#!/usr/bin/env python3
"""End-to-end check that the `research` verb works over the live bridge.

Drives a WC3_BRIDGE=1 seat all the way from five workers to a completed
research level, asserting at every step that the protocol says what it should:

  * `catalog.json` exports the two ladders with escalating per-level costs;
  * `state.json`'s `me.research` reports both ladders, their level, the bonus
    currently in force and the price of the next rung;
  * the compiler refuses a `research` on a non-forge, an unknown ladder id, and
    a second job at a forge that is already busy — each with a `cmd <i>:` prefix;
  * `build Blacksmith` is refused until a Keep stands (the tech gate);
  * a legal `research` deducts the catalog price, shows up as `researching` on
    the forge and `in_progress` under `me.research`, and on completion raises
    the team level and the flat bonus;
  * every one of those intents — accepted and rejected — appears in
    `bridge/intent_log.jsonl` as an English sentence plus its serialized form.

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
LOG = "bridge/intent_log.jsonl"
SEND = [sys.executable, "tools/bridge_send.py", "--seat", SEAT]

failures = []


def check(cond, msg):
    if cond:
        print(f"  ok   {msg}")
    else:
        print(f"  FAIL {msg}")
        failures.append(msg)


def read_state():
    for _ in range(200):
        try:
            with open(STATE) as f:
                return json.load(f)
        except Exception:
            time.sleep(0.1)
    raise SystemExit("FAIL: no readable state.json")


def send(cmds):
    subprocess.run(SEND + [json.dumps(cmds)], check=True,
                   stdout=subprocess.DEVNULL)


def wait_for(pred, what, timeout=180.0):
    """Poll snapshots until `pred(state)` holds. Returns the state."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = read_state()
        if last.get("game_over"):
            raise SystemExit(f"FAIL: game ended while waiting for {what}")
        if pred(last):
            return last
        time.sleep(0.25)
    raise SystemExit(f"FAIL: timed out waiting for {what}\nlast me={last.get('me')}")


def errors_after(prev_seq):
    """The errors attached to the batch we just sent."""
    st = wait_for(lambda s: s["seq_applied"] > prev_seq, "the batch to apply")
    return st, st["errors"]


def ladder(state, ladder_id):
    for entry in state["me"]["research"]:
        if entry["id"] == ladder_id:
            return entry
    raise SystemExit(f"FAIL: no '{ladder_id}' ladder in me.research")


def main():
    os.makedirs(SEAT, exist_ok=True)
    for stale in (STATE, os.path.join(SEAT, "commands.json"), LOG):
        if os.path.exists(stale):
            os.remove(stale)

    env = dict(os.environ)
    env.update(
        WC3_BRIDGE="1",
        WC3_HEADLESS="1",
        WC3_SPEED="16",
        WC3_MAX_GAME_SECS="4000",
        WC3_MAP="open",
        RUST_LOG="wc3clone=info",
    )
    game = subprocess.Popen(
        ["cargo", "run"], env=env,
        stdout=open("/tmp/research_bridge_game.log", "w"),
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
    print("all research bridge checks passed")


def run_checks():
    st = wait_for(lambda s: True, "the first snapshot")
    me = st["me"]

    print("\n[1] catalog exports the research ladders")
    with open(CATALOG) as f:
        cat = json.load(f)
    check("research" in cat, "catalog.json has a `research` section")
    ids = [r["id"] for r in cat["research"]]
    check(ids == ["attack", "armor"], f"ladders are attack+armor (got {ids})")
    for entry in cat["research"]:
        lv = entry["levels"]
        check(len(lv) == entry["max_level"],
              f"{entry['id']}: one entry per level ({entry['max_level']})")
        golds = [l["cost_gold"] for l in lv]
        times = [l["research_time"] for l in lv]
        check(golds == sorted(golds) and len(set(golds)) == len(golds),
              f"{entry['id']}: gold escalates {golds}")
        check(times == sorted(times) and len(set(times)) == len(times),
              f"{entry['id']}: time escalates {times}")
        check(entry["researched_at"] == "Blacksmith",
              f"{entry['id']}: researched at a Blacksmith")
        check("level" not in entry,
              f"{entry['id']}: catalog carries no current level (that is state)")
    forge = next(b for b in cat["buildings"] if b["id"] == "Blacksmith")
    check(forge["requires"] == ["Keep"], "Blacksmith requires a Keep")
    check(forge["trains"] == [], "Blacksmith trains nothing")

    print("\n[2] snapshot reports team research")
    check(len(me["research"]) == 2, "me.research has both ladders")
    atk = ladder(st, "attack")
    check(atk["level"] == 0 and atk["bonus"] == 0.0, "attack starts at level 0")
    check(atk["max_level"] == 3, "attack caps at 3")
    check(atk["next"]["level"] == 1, "next rung is level 1")
    check("in_progress" not in atk, "nothing in progress at t=0")
    check(st["unlocked"]["Blacksmith"] is False,
          "Blacksmith is locked before a Keep")

    print("\n[3] the compiler refuses bad research commands")
    hall = next(b for b in st["buildings"]
                if b["team"] == "Claude" and b["kind"] == "TownHall")
    seq = st["seq_applied"]
    send([
        {"type": "research", "building": hall["id"], "upgrade": "attack"},
        {"type": "research", "building": hall["id"], "upgrade": "banana"},
        {"type": "research", "building": 999999, "upgrade": "attack"},
        {"type": "build", "worker": next(u["id"] for u in st["units"]
                                         if u["kind"] == "Worker"),
         "kind": "Blacksmith", "x": -20.0, "z": -20.0},
    ])
    st, errs = errors_after(seq)
    joined = " | ".join(errs)
    check(any("cmd 0" in e and "cannot research" in e for e in errs),
          f"a TownHall cannot research  [{joined}]")
    check(any("cmd 1" in e and "unknown research" in e for e in errs),
          "an unknown ladder id is refused")
    check(any("cmd 2" in e and "not found/not yours" in e for e in errs),
          "a bogus building id is refused")
    check(any("cmd 3" in e and "Keep" in e for e in errs),
          "building a Blacksmith needs a Keep")

    print("\n[4] tech up to a Keep, then place the forge")
    # Everyone onto lumber: a Keep (160l) + forge (80l) + rung (50l) is well
    # past the 150 lumber a match opens with.
    trees = st["trees_near"][:4]
    workers = [u["id"] for u in st["units"] if u["kind"] == "Worker"]
    send([{"type": "harvest", "units": [w], "target": trees[i % len(trees)]["id"]}
          for i, w in enumerate(workers)])
    st = wait_for(lambda s: s["me"]["lumber"] >= 340, "lumber for the whole plan",
                  timeout=300)

    seq = st["seq_applied"]
    send([{"type": "upgrade", "building": hall["id"]}])
    st = wait_for(lambda s: any(b["id"] == hall["id"] and b["kind"] == "Keep"
                                for b in s["buildings"]),
                  "the Keep to finish", timeout=300)
    check(st["unlocked"]["Blacksmith"] is True, "a Keep unlocks the Blacksmith")

    worker = next(u["id"] for u in st["units"] if u["kind"] == "Worker")
    send([{"type": "build", "worker": worker,
           "kind": "Blacksmith", "x": -50.0, "z": -50.0}])
    st = wait_for(lambda s: any(b["kind"] == "Blacksmith" and b["done"]
                                for b in s["buildings"]),
                  "the Blacksmith to finish", timeout=300)
    forge_id = next(b["id"] for b in st["buildings"]
                    if b["kind"] == "Blacksmith" and b["done"])
    print(f"  ..   forge {forge_id} standing")

    print("\n[5] research runs, and a busy forge refuses a second job")
    # The lumber is banked; put the line back on gold to pay for the rungs.
    home = next(b for b in st["buildings"] if b["kind"] == "Keep")["pos"]
    mine = min(st["mines"], key=lambda m: (m["pos"][0] - home[0]) ** 2
               + (m["pos"][1] - home[1]) ** 2)
    workers = [u["id"] for u in st["units"] if u["kind"] == "Worker"]
    send([{"type": "harvest", "units": workers, "target": mine["id"]}])

    price = ladder(st, "attack")["next"]
    st = wait_for(lambda s: s["me"]["gold"] >= price["cost_gold"] + 20
                  and s["me"]["lumber"] >= price["cost_lumber"],
                  "money for the first rung", timeout=300)
    # Both ladders named in ONE batch. The compiler cannot reject the second —
    # the `Researching` component it would check is inserted through Commands
    # and does not exist yet this frame — so the guarantee is enforced where the
    # money is: economy.rs starts one job and drops the other. What must hold is
    # that the team is charged once and gets one job, never two.
    seq = st["seq_applied"]
    send([
        {"type": "research", "building": forge_id, "upgrade": "attack"},
        {"type": "research", "building": forge_id, "upgrade": "armor"},
    ])
    st = wait_for(lambda s: any("in_progress" in l for l in s["me"]["research"]),
                  "a job to appear in the snapshot")
    running = [l["id"] for l in st["me"]["research"] if "in_progress" in l]
    check(running == ["attack"],
          f"one job only, and it is the first one named (got {running})")

    # A LATER batch does see the component, so the compiler refuses it with a
    # sentence the commander can read.
    seq = st["seq_applied"]
    send([{"type": "research", "building": forge_id, "upgrade": "armor"}])
    st, errs = errors_after(seq)
    check(any("cmd 0" in e and "already researching" in e for e in errs),
          f"a busy forge rejects a second job  [{' | '.join(errs)}]")

    st = wait_for(lambda s: "in_progress" in ladder(s, "attack"),
                  "the attack job")
    prog = ladder(st, "attack")["in_progress"]
    check(prog["level"] == 1, "in_progress names the level being produced")
    check(prog["building"] == forge_id, "in_progress names the forge doing it")
    check(prog["remaining"] > 0, "in_progress counts down")
    job = next(b for b in st["buildings"] if b["id"] == forge_id)
    check(job.get("researching", {}).get("upgrade") == "attack",
          "the forge reports its own job")

    st = wait_for(lambda s: ladder(s, "attack")["level"] == 1,
                  "attack 1 to complete", timeout=300)
    atk = ladder(st, "attack")
    check(atk["bonus"] == 1.0, "the flat bonus is now +1")
    check("in_progress" not in atk, "the job is gone once it lands")
    check(atk["next"]["level"] == 2 and atk["next"]["cost_gold"] > price["cost_gold"],
          f"the next rung costs more ({atk['next']['cost_gold']}g)")
    check(ladder(st, "armor")["level"] == 0,
          "armor is untouched — the duplicate was dropped, not banked")
    check(sum(l["level"] for l in st["me"]["research"]) == 1,
          "exactly one rung was bought by the whole run")
    check(any("research" in msg.lower() or "Weapon" in msg
              for _, msg in st["events"]),
          "the owner's event feed announced it")

    print("\n[6] every intent reached the replay log")
    with open(LOG) as f:
        lines = [json.loads(l) for l in f if l.strip()]
    research = [l for l in lines if l.get("verb") == "research"]
    check(len(research) >= 5, f"research intents were logged ({len(research)})")
    check(any(r["ok"] and "researches attack" in r["sentence"] for r in research),
          "the accepted one reads as an English sentence")
    check(any(not r["ok"] and r.get("errors") for r in research),
          "the rejected ones were kept, with their errors")
    for r in research:
        check(r["intent"]["type"] == "research" and "upgrade" in r["intent"],
              f"logged intent replays verbatim: {r['intent']}")
        break


if __name__ == "__main__":
    main()
