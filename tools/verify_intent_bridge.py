#!/usr/bin/env python3
"""End-to-end check that bridge commands still flow, now via the intent path.

Drives a live BH_BRIDGE=1 seat with tools/bridge_send.py, then asserts:
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
    # `intel` is ALWAYS present, on the same reasoning as `fog` and unlike the
    # optional keys below: an absent block and an empty one are different
    # claims ("this build has no ledger" vs "you have seen nothing"), and a
    # commander that cannot tell them apart will read silence as safety.
    "intel",
    # `my_race` is always present too — it is the key that turns the shared
    # two-race catalog into THIS seat's build tree, and it reads "kingdom" for
    # both seats in a match nobody opted a second race into. It shipped with
    # the race work and this set was never updated, so this script has been
    # failing on `snapshot gained keys {'my_race'}` since then, independently
    # of the ready handshake. Listed here rather than left broken: a contract
    # check that nobody can run is not a contract check.
    "my_race",
}

# Keys that appear only in states this check does not run in, so the assertion
# above stays an EXACT-set check for the live match it actually inspects.
#   * game_over_reason ("razed"/"surrender") exists only once a match has ended
#     — see docs/INTENT.md, "Which win was it": `game_over` itself keeps its
#     historical string-or-null shape precisely so this tooling never breaks.
#   * triggers is present only once this seat has armed one (`trigger_set`).
#     Like `command_nodes` and `applied` it is `skip_serializing_if` empty, so
#     a seat that never speaks the word sends exactly the historical key set.
#   * plans, identically, appears only once this seat has set one (`plan_set`).
#     Same `skip_serializing_if` rule and the same reason: the v3 vocabulary is
#     additive, and a commander that never says the word must not be able to
#     tell it exists from the shape of its snapshot.
#   * alarms is present only while this seat has a standing alarm
#     (docs/AFFORDANCES.md, "Alarms"). Same `skip_serializing_if` empty rule as
#     triggers and plans: a quiet seat's snapshot is byte-shape identical to a
#     pre-alarm one, and this script's own short scripted run raises nothing —
#     it never sees an enemy army, never loses a squad, never runs its mine
#     dry. Listed here rather than in the exact set for the honest reason that
#     a slow or unlucky run could raise one, not because the alarm layer is
#     optional.
#   * waiting_for / match_started exist ONLY while the ready handshake is
#     holding the match at t=0 (docs/INTENT.md, "The ready handshake"). This
#     script sends `ready` below and then waits for the hold to lift, so by the
#     time the exact-set assertion runs they are gone again — they are listed
#     here for the honest reason that a slow engine could still be writing the
#     held snapshot when we first look, not because the check tolerates them
#     mid-match. `the_handshake_keys_are_gone_once_the_match_starts` (step 2b)
#     is the assertion that actually pins their disappearance.
#   * notes (wc3clone-b9m) is the advisory half of the acknowledgement: one
#     line per ACCEPTED command that contradicts a readiness fact the scaffold
#     serves — push gates unmet, an intel ledger too stale to be committing
#     against. Same `skip_serializing_if` empty rule as `applied` beside it, and
#     the same reason it is listed here rather than in the exact set: this
#     script's run never stances a squad into a push, so it must not require the
#     key — but a seat that did would grow one, and the assertion has to tolerate
#     that without being loosened to a subset check.
OPTIONAL_TOP_KEYS = {
    "applied", "game_over_reason", "triggers", "plans", "alarms",
    "waiting_for", "match_started", "notes",
}

# The same contract, one level down, for the array this run always populates.
# `squads[]` shipped as `{id, posture, members}` and has gained two optional
# keys since, both `skip_serializing_if`:
#   * stance (wc3clone-0uu.2) — the word a `stance` command put there, absent
#     for a squad tasked by hand with `posture`.
#   * status (wc3clone-6wa) — "gathering" or "pressing on", which is what
#     doctrine is doing about the posture as opposed to what the posture says.
#     Absent for a squad that is not walking anywhere: no posture, a defend
#     ring, an escort, or a forager with no treasure in sight. r22's push
#     oscillated in front of a ford for four hundred seconds and no snapshot
#     could say so, which is why the key exists.
# Both are absent-not-null, so a seat that never stances and never pushes
# writes a `squads[]` byte-identical to the pre-feature one — and that is the
# half worth pinning, because an accidental always-present key is exactly the
# regression this whole file is here to catch.
EXPECTED_SQUAD_KEYS = {"id", "posture", "members"}
OPTIONAL_SQUAD_KEYS = {"stance", "status"}


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


def start_match(st):
    """Clear the ready handshake, if this engine is holding at t=0.

    A bridged seat now gates the start (docs/INTENT.md, "The ready handshake"),
    so a script that attaches to one and never says the word would sit in front
    of a frozen world until the timeout rescued it — every assertion below
    presumes a clock that runs. Sending `ready` unconditionally is correct on
    both sides of the change: against an engine without the handshake it is an
    unrecognized verb, which lands in `errors` and is cleared by the next batch
    rather than failing anything.
    """
    if "waiting_for" not in st:
        return st
    print(f"[0] match held at t=0, waiting for {st['waiting_for']} — sending ready")
    send([{"type": "ready"}])
    deadline = time.time() + 90.0
    while time.time() < deadline:
        st = read_state()
        if "waiting_for" not in st:
            print("[0] match started")
            return st
        time.sleep(0.5)
    raise SystemExit(f"FAIL: match never started, still waiting for {st.get('waiting_for')}")


def main():
    st = start_match(read_state())
    print(f"[1] snapshot present at t={st['t']}s, my_team={st['my_team']}")

    # --- contract: the snapshot key set is unchanged ------------------------
    # 2b, and the reason the two handshake keys can be optional without
    # weakening this: once the match is running they must be ABSENT, not merely
    # tolerated. An always-present `match_started: true` would be a permanent
    # addition to every snapshot of every run, which is exactly what the
    # skip_serializing_if convention exists to prevent.
    assert "waiting_for" not in st, "FAIL: waiting_for outlived the hold"
    assert "match_started" not in st, "FAIL: match_started outlived the hold"
    keys = set(st.keys())
    missing, extra = EXPECTED_TOP_KEYS - keys, keys - EXPECTED_TOP_KEYS - OPTIONAL_TOP_KEYS
    assert not missing, f"FAIL: snapshot lost keys {missing}"
    assert not extra, f"FAIL: snapshot gained keys {extra}"
    for sq in st["squads"]:
        sk = set(sq.keys())
        sq_missing = EXPECTED_SQUAD_KEYS - sk
        sq_extra = sk - EXPECTED_SQUAD_KEYS - OPTIONAL_SQUAD_KEYS
        assert not sq_missing, f"FAIL: squad {sq.get('id')} lost keys {sq_missing}"
        assert not sq_extra, f"FAIL: squad {sq.get('id')} gained keys {sq_extra}"
    unit_keys = set(st["units"][0].keys()) if st["units"] else set()
    print(f"[2] snapshot key set unchanged ({len(keys)} top-level keys)")
    print(f"    unit keys: {sorted(unit_keys)}")
    print(f"    squad keys: {sorted({k for sq in st['squads'] for k in sq})}")

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
    # `rally` reads back now (`wc3clone-3w9`). It is an ADDITIVE, optional key
    # inside `buildings[]` — `skip_serializing_if` keeps it off a building that
    # has none, so the top-level EXPECTED/OPTIONAL sets above are untouched and
    # a snapshot from a match where nobody says the word is shape-identical to
    # the pre-feature one. What is asserted here is the positive case: the seat
    # sent one above, so the readback must carry it.
    assert hall_out.get("rally", {}).get("pos") == [55.0, 55.0], \
        f"FAIL: rally never read back: {hall_out.get('rally')}"
    assert all("rally" not in b for b in st["buildings"] if b["team"] != "Claude"), \
        "FAIL: a rally point is command structure and must never leave its own seat"
    print(f"[5] effects confirmed: order={[u['order'] for u in moved]}, squad=2, "
          f"retreat+prio policies set, queue={hall_out['queue']}, "
          f"rally={hall_out['rally']}")

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
