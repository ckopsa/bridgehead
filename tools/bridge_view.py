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

    hero = next((u for u in mine_units if u["kind"] == "Hero"), None)
    if hero and hero.get("hero"):
        h = hero["hero"]
        print(
            f"HERO id={hero['id']} Lv{h['level']} hp={hero['hp']:.0f}/{hero['max_hp']:.0f} "
            f"mana={h['mana']:.0f}/{h['max_mana']:.0f} cd={h['cd']:.0f} "
            f"@{tuple(hero['pos'])} order={hero['order']}"
        )
    else:
        hr = me.get("hero_record")
        hc = me["hero_cost"]
        print(
            "HERO none"
            + (f" (record Lv{hr['level']})" if hr else "")
            + f" cost={hc['gold']}g/{hc['lumber']}l {hc['time']:.0f}s"
        )

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
    for sq in s.get("squads", []):
        print(f"SQUAD {sq['id']}: {sq['posture']} members={sq['members']}")

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
