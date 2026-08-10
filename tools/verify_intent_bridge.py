#!/usr/bin/env python3
"""End-to-end check that bridge commands still flow, now via the intent path.

Drives a live WC3_BRIDGE=1 seat with tools/bridge_send.py, then asserts:
  * the state.json snapshot keeps its exact historical key set,
  * valid commands take effect (orders/queues/policies change),
  * invalid commands come back in `errors` with the historical `cmd <i>:` prefix,
  * every submitted intent appears in bridge/intent_log.jsonl as a sentence
    plus its serialized form.
"""
import json
import os
import subprocess
import sys
import time

SEAT = "bridge/red"
STATE = os.path.join(SEAT, "state.json")
LOG = "bridge/intent_log.jsonl"

# The snapshot contract as it stood before the intent refactor.
EXPECTED_TOP_KEYS = {
    "t", "my_team", "seq_applied", "errors", "game_over", "me", "map",
    "unlocked", "units", "buildings", "squads", "bounties", "mines",
    "trees_near", "events", "fog",
}


def upgrade_price(st):
    """What the catalog says the hall's next tier costs."""
    with open(os.path.join(SEAT, "catalog.json")) as f:
        cat = json.load(f)
    for b in cat["buildings"]:
        up = b.get("upgrades_to")
        if b["id"] == "TownHall" and up:
            return up["cost_gold"], up["cost_lumber"]
    raise SystemExit("FAIL: catalog has no TownHall upgrade")


def read_state():
    for _ in range(120):
        try:
            with open(STATE) as f:
                return json.load(f)
        except Exception:
            time.sleep(0.5)
    raise SystemExit("FAIL: no state.json appeared")


def wait_for_seq(seq, timeout=30.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        st = read_state()
        if st.get("seq_applied", 0) >= seq:
            return st
        time.sleep(0.3)
    raise SystemExit(f"FAIL: seq {seq} never applied")


def send(cmds):
    out = subprocess.run(
        [sys.executable, "tools/bridge_send.py", "--seat", SEAT, json.dumps(cmds)],
        capture_output=True, text=True, check=True,
    )
    print("   ", out.stdout.strip())
    return int(out.stdout.split("seq=")[1].split()[0])


def main():
    st = read_state()
    print(f"[1] snapshot present at t={st['t']}s, my_team={st['my_team']}")

    # --- contract: the snapshot key set is unchanged ------------------------
    keys = set(st.keys())
    missing, extra = EXPECTED_TOP_KEYS - keys, keys - EXPECTED_TOP_KEYS
    assert not missing, f"FAIL: snapshot lost keys {missing}"
    assert not extra, f"FAIL: snapshot gained keys {extra}"
    unit_keys = set(st["units"][0].keys()) if st["units"] else set()
    print(f"[2] snapshot key set unchanged ({len(keys)} top-level keys)")
    print(f"    unit keys: {sorted(unit_keys)}")

    mine = [u for u in st["units"] if u["team"] == "Claude"]
    workers = [u for u in mine if u["kind"] == "Worker"]
    halls = [b for b in st["buildings"]
             if b["team"] == "Claude" and b["kind"] == "TownHall"]
    assert workers and halls, "FAIL: expected workers and a town hall"

    movers = [u["id"] for u in workers[:2]]
    harvesters = [u["id"] for u in workers[2:5]]
    hall = halls[0]["id"]
    assert st["trees_near"], "FAIL: no trees in the snapshot"
    tree = st["trees_near"][0]["id"]

    # A batch mixing every intent family, valid and invalid, so both the
    # success path and the error channel are exercised in one go.
    seq = send([
        {"type": "move", "units": movers, "x": 40.0, "z": 40.0},
        {"type": "rally", "building": hall, "x": 55.0, "z": 55.0},
        {"type": "train", "building": hall, "unit": "Worker"},
        {"type": "retreat", "units": movers, "below": 0.4, "x": 70.0, "z": 70.0},
        {"type": "priority", "units": movers, "classes": ["Hero", "Siege"]},
        {"type": "squad", "units": movers, "id": 2},
        {"type": "posture", "id": 2,
         "posture": {"type": "defend", "x": 60.0, "z": 60.0, "radius": 15.0}},
        # --- verbs the master merge added -----------------------------------
        # Lumber for the tier-up below: this seat has no AI earning it.
        {"type": "harvest", "units": harvesters, "target": tree},
        # `cast` with an ability selector, in both spellings. The TownHall's
        # Call to Arms is slot 0, so the index form is legal and the id form
        # names the same slot.
        {"type": "cast", "caster": hall, "ability": 0},
        {"type": "cast", "caster": hall, "ability": "calltoarms"},
        # --- deliberately invalid, to prove the error channel is unchanged --
        {"type": "attack", "units": movers, "target": 999999},
        {"type": "build", "worker": movers[0], "kind": "Nonsense", "x": 0, "z": 0},
        {"type": "priority", "units": movers, "classes": ["Wizard"]},
        {"type": "cast", "caster": hall, "ability": "Fireball"},
        {"type": "nonsense_verb", "units": movers},
    ])
    st = wait_for_seq(seq)
    print(f"[3] batch seq={seq} applied")

    # --- errors: same strings, same `cmd <i>:` prefix -----------------------
    errs = st["errors"]
    print("    errors returned:")
    for e in errs:
        print(f"      {e}")
    assert any("target 999999 not found" in e for e in errs), "FAIL: missing attack error"
    assert any("unknown building kind 'Nonsense'" in e for e in errs), "FAIL: missing build error"
    assert any("unknown target class 'Wizard'" in e for e in errs), "FAIL: missing class error"
    assert any("has no ability" in e and "Fireball" in e for e in errs), \
        "FAIL: missing ability-selector error"
    assert any("unrecognized command" in e for e in errs), "FAIL: missing parse error"
    assert all(e.startswith("cmd ") for e in errs), f"FAIL: error prefix changed: {errs}"
    print(f"[4] {len(errs)} validation errors, all with historical `cmd <i>:` prefix")

    # --- effects: the valid half actually changed the world -----------------
    by_id = {u["id"]: u for u in st["units"]}
    moved = [by_id[i] for i in movers if i in by_id]
    # Move, or AttackMove if squad 2's Defend posture has already re-tasked
    # them — doctrine.rs owns them the moment they join a squad with a posture.
    assert any(u["order"] in ("Move", "AttackMove") for u in moved), \
        f"FAIL: move never took: {[u['order'] for u in moved]}"
    assert any(u.get("squad") == 2 for u in moved), "FAIL: squad assignment never took"
    assert any(u.get("policies", {}).get("retreat") for u in moved), \
        "FAIL: retreat policy never took"
    assert any(u.get("policies", {}).get("prio") for u in moved), \
        "FAIL: priority policy never took"
    assert any(s["id"] == 2 for s in st["squads"]), "FAIL: squad 2 posture never took"
    hall_out = next(b for b in st["buildings"] if b["id"] == hall)
    assert hall_out.get("queue"), "FAIL: train never queued"
    # `rally` is deliberately not in the snapshot (it never was), so it is
    # verified below by its intent log record carrying no errors.
    print(f"[5] effects confirmed: order={[u['order'] for u in moved]}, squad=2, "
          f"retreat+prio policies set, queue={hall_out['queue']}")

    # --- second batch: the tier-up, once the lumber is actually in ----------
    gold, lumber = upgrade_price(st)
    print(f"[6] waiting on {gold}g {lumber}l for the tier-up...")
    deadline = time.time() + 120
    while time.time() < deadline:
        st = read_state()
        if st["me"]["gold"] >= gold and st["me"]["lumber"] >= lumber:
            break
        time.sleep(1.0)
    else:
        raise SystemExit("FAIL: never accumulated enough for the upgrade")
    seq = send([{"type": "upgrade", "building": hall}])
    st = wait_for_seq(seq)
    assert not [e for e in st["errors"] if "upgrade" in e or "afford" in e], \
        f"FAIL: upgrade rejected: {st['errors']}"
    hall_out = next(b for b in st["buildings"] if b["id"] == hall)
    print(f"[7] upgrade accepted at {st['me']['gold']}g {st['me']['lumber']}l; "
          f"hall now {hall_out['kind']}")

    # --- the replay spine ---------------------------------------------------
    with open(LOG) as f:
        lines = [json.loads(l) for l in f if l.strip()]
    header, records = lines[0], lines[1:]
    assert header.get("session") == "wc3clone-intent-log-v1", "FAIL: no session header"
    assert records, "FAIL: intent log is empty"
    for r in records:
        for key in ("wall_ms", "t", "team", "source", "tag", "verb", "sentence",
                    "ok", "intent"):
            assert key in r, f"FAIL: log record missing {key}: {r}"
        assert r["intent"]["type"] == r["verb"], "FAIL: verb/type disagree"
    ok = [r for r in records if r["ok"]]
    bad = [r for r in records if not r["ok"]]
    verbs_ok = {r["verb"] for r in ok}
    for verb in ("move", "rally", "train", "retreat", "priority", "squad",
                 "posture", "upgrade", "cast"):
        assert verb in verbs_ok, f"FAIL: '{verb}' intent was never applied cleanly"
    assert {r["verb"] for r in bad} == {"attack", "build", "priority", "cast"}, \
        f"FAIL: unexpected rejected verbs {[r['verb'] for r in bad]}"
    # The untagged ability selector must survive the round trip into the log.
    casts = [r for r in ok if r["verb"] == "cast"]
    assert any(r["intent"].get("ability") == 0 for r in casts), \
        "FAIL: index selector lost in the log"
    assert any(r["intent"].get("ability") == "calltoarms" for r in casts), \
        "FAIL: id selector lost in the log"
    assert all(r["source"] == "bridge" for r in records), "FAIL: source mislabelled"
    print(f"[8] intent log: {len(records)} records ({len(ok)} applied, {len(bad)} rejected)")
    print("    sentences:")
    for r in records:
        mark = " " if r["ok"] else "!"
        print(f"     {mark} [{r['t']:>6.1f}s] {r['team']}/{r['source']}: {r['sentence']}")

    print("\nPASS: bridge protocol unchanged, commands flow through intents, "
          "log is a readable replay spine")


if __name__ == "__main__":
    main()
