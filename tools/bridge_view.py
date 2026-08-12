#!/usr/bin/env python3
"""Compact tactical summary of bridge/state.json for the Red commander."""
import json
import math
import sys
from collections import Counter, defaultdict

BASES = {"Claude": (70.0, 70.0), "Human": (-70.0, -70.0)}


def dist(a, b):
    return math.hypot(a[0] - b[0], a[1] - b[1])


def centroid(points):
    if not points:
        return None
    return (
        round(sum(p[0] for p in points) / len(points), 1),
        round(sum(p[1] for p in points) / len(points), 1),
    )


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "bridge/red/state.json"
    with open(path) as f:
        s = json.load(f)

    # Events newer than my previous read; one marker per seat so parallel
    # commanders don't clobber each other's read position.
    marker = "/tmp/claude-1000/bridge_last_t_" + path.replace("/", "_")
    last_t = 0.0
    try:
        with open(marker) as f:
            last_t = float(f.read().strip())
    except Exception:
        pass
    try:
        with open(marker, "w") as f:
            f.write(str(s["t"]))
    except Exception:
        pass
    fresh = [e for e in s.get("events", []) if e[0] > last_t]
    for t, msg in fresh[-12:]:
        print(f"EVT [{t:.0f}s] {msg}")

    me = s["me"]
    my_team = s.get("my_team", "Claude")
    enemy_team = "Human" if my_team == "Claude" else "Claude"
    global BASE
    BASE = BASES[my_team]
    print(
        f"[{my_team}] t={s['t']:.0f}s seq={s['seq_applied']} "
        f"gold={me['gold']} lumber={me['lumber']} "
        f"supply={me['supply_used']}/{me['supply_cap']}"
    )
    # The two ends of a match's life, side by side. The hold is the louder of
    # the two because it is the one a commander can do something about.
    if s.get("waiting_for") is not None:
        waiting = s["waiting_for"]
        print(
            f"MATCH NOT STARTED — held at t=0, waiting for: {' '.join(waiting) or '(nobody)'}"
        )
        print("  send '[{\"type\":\"ready\"}]' once you have read the map and set your opening")
    if s.get("game_over"):
        print(f"GAME OVER: {s['game_over']} wins")
    for e in s.get("errors", []):
        print(f"ERR: {e}")

    mine_units = [u for u in s["units"] if u["team"] == my_team]
    enemy_units = [u for u in s["units"] if u["team"] == enemy_team]
    mine_b = [b for b in s["buildings"] if b["team"] == my_team]
    enemy_b = [b for b in s["buildings"] if b["team"] == enemy_team]

    # --- my workers ---
    workers = [u for u in mine_units if u["kind"] == "Worker"]
    by_order = Counter(w["order"] for w in workers)
    idle = [w for w in workers if w["order"] == "Idle"]
    carrying = sum(1 for w in workers if w["carrying"])
    print(
        f"WORKERS {len(workers)}: "
        + " ".join(f"{k}:{v}" for k, v in sorted(by_order.items()))
        + (f" carrying:{carrying}" if carrying else "")
    )
    if idle:
        print("  idle ids: " + " ".join(str(w["id"]) for w in idle[:8]))

    # --- my army ---
    army = [u for u in mine_units if u["kind"] not in ("Worker",)]
    if army:
        kinds = Counter(a["kind"] for a in army)
        hurt = sum(1 for a in army if a["hp"] < 0.55 * a["max_hp"])
        print(
            f"ARMY {len(army)}: "
            + " ".join(f"{k}:{v}" for k, v in sorted(kinds.items()))
            + f" @ {centroid([a['pos'] for a in army])}"
            + (f" hurt:{hurt}" if hurt else "")
        )
        by_o = defaultdict(list)
        for a in army:
            by_o[a["order"]].append(str(a["id"]))
        for o, ids in sorted(by_o.items()):
            print(f"  {o}({len(ids)}): {' '.join(ids[:14])}")
        by_k = defaultdict(list)
        for a in army:
            by_k[a["kind"]].append(str(a["id"]))
        for k, ids in sorted(by_k.items()):
            print(f"  ids/{k}({len(ids)}): {' '.join(ids[:14])}")
    else:
        print("ARMY 0")

    # Hero slots scale with the hall tier (1/2/3) and classes must be distinct,
    # so this is a list now rather than "the" hero.
    living = [u for u in mine_units if u.get("hero")]
    for u in living:
        h = u["hero"]
        print(
            f"HERO {u['kind']} id={u['id']} Lv{h['level']} "
            f"hp={u['hp']:.0f}/{u['max_hp']:.0f} "
            f"mana={h['mana']:.0f}/{h['max_mana']:.0f} cd={h['cd']:.0f} "
            f"@{tuple(u['pos'])} order={u['order']}"
        )
    slots = me.get("hero_slots", 1)
    used = me.get("hero_slots_used", len(living))
    dead = [r for r in me.get("hero_records", []) if not r["alive"]]
    costs = {c["kind"]: c for c in me.get("hero_costs", [])}
    held = {u["kind"] for u in living}
    buyable = [
        "{}={}g/{}l{}".format(
            k, costs[k]["gold"], costs[k]["lumber"], "(revive)" if costs[k]["revive"] else ""
        )
        for k in costs
        if k not in held
    ]
    line = "HERO SLOTS {}/{}".format(used, slots)
    if dead:
        line += " dead=[{}]".format(
            ",".join("{} Lv{}".format(r["kind"], r["level"]) for r in dead)
        )
    if used < slots and buyable:
        line += " can train: " + " ".join(buyable)
    print(line)

    # --- my buildings ---
    for b in mine_b:
        q = ",".join(b["queue"]) if b["queue"] else "-"
        state = "" if b["done"] else " BUILDING"
        print(
            f"B {b['kind']} id={b['id']} hp={b['hp']:.0f}/{b['max_hp']:.0f}"
            f"{state} q=[{q}] @{tuple(b['pos'])}"
        )

    # --- enemy picture ---
    ek = Counter(u["kind"] for u in enemy_units)
    print(
        f"ENEMY units {len(enemy_units)}: "
        + " ".join(f"{k}:{v}" for k, v in sorted(ek.items()))
    )
    combat = [u for u in enemy_units if u["kind"] != "Worker"]
    if combat:
        print(f"  army centroid {centroid([u['pos'] for u in combat])}")
    threats = [u for u in enemy_units if dist(u["pos"], BASE) < 45 and u["kind"] != "Worker"]
    if threats:
        print(
            f"  !! {len(threats)} enemy near MY base @ "
            f"{centroid([u['pos'] for u in threats])}"
        )
    eh = next((u for u in enemy_units if u["kind"] == "Hero"), None)
    if eh:
        print(
            f"  enemy hero id={eh['id']} hp={eh['hp']:.0f}/{eh['max_hp']:.0f} @{tuple(eh['pos'])}"
        )
    print(
        f"ENEMY buildings {len(enemy_b)}: "
        + " ".join(
            f"{b['kind']}[{b['hp']:.0f}]id={b['id']}@{tuple(b['pos'])}" for b in enemy_b
        )
    )

    # --- squads ---
    # The stance word, when one put the posture there, printed FIRST and in
    # brackets: it is the thing a commander decides and the posture is the thing
    # the engine derived from it. Absent for a hand-tasked squad, and absent
    # entirely from a snapshot written before stances existed, so `.get` rather
    # than `[...]` — this readout must survive an older state.json.
    for sq in s.get("squads", []):
        stance = sq.get("stance")
        tag = f"[{stance}] " if stance else ""
        print(f"SQUAD {sq['id']}: {tag}{sq['posture']} members={sq['members']}")

    # --- triggers ---
    # Absent until this seat has armed one, so a v1 snapshot prints nothing
    # extra. Shown as the English sentence rather than the JSON: this readout is
    # for deciding, and the JSON is one `state.json` away when you want to edit
    # a rule and re-send it.
    for t in s.get("triggers", []):
        fired = f" last={t['last_fired']:.0f}s" if t.get("last_fired") is not None else ""
        print(f"TRIGGER [{t['status']}]{fired} {t['sentence']}")

    # --- mines & trees ---
    print(
        "MINES: "
        + " ".join(
            f"id={m['id']}@{tuple(m['pos'])}:{m['remaining']}"
            + ("(near me)" if dist(m["pos"], BASE) < 40 else "")
            for m in s["mines"]
        )
    )
    trees = s.get("trees_near", [])
    if trees and isinstance(trees[0], dict):
        print(
            "TREES: "
            + " ".join(f"id={t['id']}@{tuple(t['pos'])}" for t in trees[:6])
        )


if __name__ == "__main__":
    main()
