#!/usr/bin/env python3
"""Compact tactical summary of bridge/state.json for the Red commander.

Two views over the same file, and nothing but views: this script never writes
`state.json`, never sends a command, and never needs the wire to grow a key.

  * the default full readout — every unit id, every building, mines and trees.
    What a commander wants when it is editing a rule or hunting an id.
  * ``--digest`` — the ~15-line commander digest of docs/AFFORDANCES.md
    ("Snapshot diet"): resources, army by squad, production queues, the enemy
    production buildings this seat has SEEN (the win-condition line), the last
    five events, any active alarms, and the running default — what silence does
    now. Drops the per-unit id rosters, the tree ids, the per-farm HP and the
    full plans echo, which is where a small commander's attention was going.

  * ``--doc`` — the whole hypermedia affordance document, of which the digest
    is one section. See ``tools/affordances.py``.

The digest is deliberately split in two halves. ``digest()`` returns structured
data and ``render_digest()`` turns that into lines. AFFORDANCES.md's "One
document" section makes the digest the PROPERTIES section of a single
hypermedia document whose other half is actions/forms; that renderer is
``tools/affordances.py`` and it embeds ``digest()``'s dict verbatim beside its
own ``actions`` key, rather than re-deriving any of this from the snapshot a
second time.

Everything here degrades on old snapshots. Every read is a ``.get`` with a
default, because the keys this reads were added over a dozen releases and a
digest that raises ``KeyError`` on a seat that never armed a trigger is worse
than no digest at all.
"""
import argparse
import json
import math
import os
import re
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from markers import marker_path  # noqa: E402

BASES = {"Claude": (70.0, 70.0), "Human": (-70.0, -70.0)}

# --- digest constants -------------------------------------------------------

# How many lines the digest may occupy. "~15 lines" is the design number; this
# is the hard ceiling that keeps a fifteen-squad late game from quietly turning
# the digest back into the thing it replaced. Sections trim themselves against
# it in `render_digest`, cheapest information first.
MAX_LINES = 18
#: Events shown, newest last. Five is AFFORDANCES.md's number.
EVENT_LINES = 5
#: Squad lines before the rest collapse into a "+N more" tail.
SQUAD_LINES = 4

# A building is PRODUCTION — the thing the win condition counts — when it
# trains something (`shared::check_game_over` asks `!trainable(kind).is_empty()`).
# The seat's own `catalog.json` carries `trains` per building and is consulted
# first; this is the fallback for a digest rendered from a bare `state.json`
# with no catalog beside it. Both hall ladders and both races are here because
# a kind missing from this set would silently under-count the win condition.
PRODUCTION_KINDS = frozenset(
    {
        # Kingdom
        "TownHall", "Keep", "Castle", "Barracks", "Workshop", "Sanctum",
        # Horde
        "Stronghold", "Fortress", "Hold", "WarCamp", "SpiritLodge",
    }
)

#: How near a named choke a spot must be to be called by its name — the same
#: 30.0 `shared::place_name` uses, so the digest names ground the way the event
#: feed does.
PLACE_CHOKE_RADIUS = 30.0

_COORDS = re.compile(r"\(\s*(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*\)")


def dist(a, b):
    return math.hypot(a[0] - b[0], a[1] - b[1])


def centroid(points):
    if not points:
        return None
    return (
        round(sum(p[0] for p in points) / len(points), 1),
        round(sum(p[1] for p in points) / len(points), 1),
    )


# ---------------------------------------------------------------------------
# The digest — docs/AFFORDANCES.md, "Snapshot diet"
#
# `digest()` is a pure function of the snapshot dict (plus, optionally, the
# seat's catalog). It reads nothing off disk, writes nothing, and has no
# opinions: every number in it is either copied out of the snapshot or summed
# from it. Where it reports the enemy, it reports only what the snapshot has
# already decided this seat may know — `buildings` is fog-gated by the engine
# (live sightings plus remembered ghosts carrying `last_seen`), so the
# win-condition line inherits fog-honesty instead of re-deriving it.
# ---------------------------------------------------------------------------


def production_kinds(catalog):
    """Which building kinds count toward the win condition.

    The catalog is authoritative — a building is production when it trains
    something — and `PRODUCTION_KINDS` stands in when no catalog is at hand.
    """
    if not catalog:
        return PRODUCTION_KINDS
    kinds = {
        b.get("id")
        for b in catalog.get("buildings") or []
        if b.get("trains") and b.get("id")
    }
    return kinds or PRODUCTION_KINDS


def load_catalog(state_path):
    """The `catalog.json` sitting beside a seat's `state.json`, if there is one."""
    path = os.path.join(os.path.dirname(os.path.abspath(state_path)), "catalog.json")
    try:
        with open(path) as f:
            return json.load(f)
    except Exception:
        return None


def place_of(pos, smap):
    """Name a spot the way `shared::place_name` does, from public geography.

    Nearest named choke inside 30 wins, then the named place whose radius
    covers the spot, then the bare coordinate. `map.places` is already written
    from THIS seat's point of view ("our base" / "their base"), so no team
    argument is needed here.
    """
    if not pos or len(pos) < 2:
        return "position unknown"
    best = None
    for c in smap.get("chokes") or []:
        cp = c.get("pos")
        if not cp or not c.get("name"):
            continue
        d = dist(pos, cp)
        if d <= PLACE_CHOKE_RADIUS and (best is None or d < best[0]):
            best = (d, "the " + c["name"])
    if best is None:
        for p in smap.get("places") or []:
            pp = p.get("pos")
            if not pp or not p.get("name"):
                continue
            d = dist(pos, pp)
            if d <= p.get("radius", 0.0) and (best is None or d < best[0]):
                best = (d, p["name"])
    if best is not None:
        return "near " + best[1]
    return "at ({:.0f}, {:.0f})".format(pos[0], pos[1])


def stance_phrase(stance, smap):
    """Turn a squad's posture string into something a sentence can hold.

    Postures arrive as `"defend@(70.0,70.0)r=18"`, `"push@(x,z)"`,
    `"forage@(x,z)"` or `"escort:<unitid>"`. A named stance (turtle / stage /
    push / secure / harass, AFFORDANCES.md's five) is a bare word and passes
    through untouched, so this keeps working when `squads[].stance` lands.
    """
    if not stance:
        return "no standing posture"
    if "@" in stance:
        verb, _, rest = stance.partition("@")
        m = _COORDS.search(rest)
        if m:
            return "{} {}".format(verb, place_of([float(m.group(1)), float(m.group(2))], smap))
        return verb
    if ":" in stance:
        verb, _, arg = stance.partition(":")
        return "{} {}".format(verb, arg)
    return stance


def _squad_props(sid, sq, members, smap):
    """One squad as the digest sees it: who, how strong, and where."""
    hp = sum(u.get("hp", 0.0) for u in members)
    max_hp = sum(u.get("max_hp", 0.0) for u in members)
    pos = centroid([u["pos"] for u in members if u.get("pos")])
    # `stance` is AFFORDANCES.md item 2 and is not on the wire yet; `posture`
    # is what every shipped snapshot carries. Prefer the newer key when it
    # appears and never require it.
    stance = sq.get("stance") or sq.get("posture")
    return {
        "id": sid,
        "stance": stance,
        "stance_phrase": stance_phrase(stance, smap),
        # A snapshot always carries the count on the squad record; the unit
        # rosters are what we sum strength from. They agree, except on a
        # snapshot old enough to predate `units[].squad`, where the count is
        # still right and the roster is empty.
        "units": len(members) or sq.get("members", 0),
        "strength": round(hp),
        "hp_frac": round(hp / max_hp, 2) if max_hp else None,
        "pos": pos,
        "place": place_of(pos, smap) if pos else "position unknown",
        "hurt": sum(1 for u in members if u.get("hp", 0) < 0.55 * u.get("max_hp", 1)),
    }


def _alarm_props(raw):
    """Normalize one alarm.

    The shipped shape (src/bridge.rs `AlarmOut`) is
    `{id, fact, running_default, since_t, severity, eta_s?}`, and `fact` /
    `running_default` are the two fields the design makes mandatory: what
    happened, and what is already being done about it. This renderer was
    written before that bead landed and stays tolerant of the other obvious
    spellings — a string, or a dict under any of them, all render — because a
    digest that goes blank on an unfamiliar key is worse than one that prints
    the wrong noun.

    `eta_s` rides along where it exists: the recall ETA is the whole reason
    "multiple places under attack" is answerable at LLM latency at all.
    """
    if isinstance(raw, str):
        return {"text": raw, "default": None}
    if not isinstance(raw, dict):
        return {"text": str(raw), "default": None}
    text = (
        raw.get("fact")
        or raw.get("text")
        or raw.get("message")
        or raw.get("title")
        or raw.get("why")
        or raw.get("reason")
        or raw.get("kind")
        or raw.get("name")
        or raw.get("id")
        or "alarm"
    )
    # The kind is a prefix, not a replacement — except when it IS the whole
    # text, which is what an alarm carrying nothing but an id comes down to.
    # The ETA rides in the prefix rather than the tail on purpose: this line
    # is truncated to the digest's width, and "when does the recall land" is
    # the half of a two-front alarm a commander cannot reconstruct.
    kind = raw.get("kind") or raw.get("name") or raw.get("id")
    eta = raw.get("eta_s")
    head = kind if kind and kind != text else None
    if eta is not None:
        head = "{} [ETA {:.0f}s]".format(head, eta) if head else "[ETA {:.0f}s]".format(eta)
    if head:
        text = "{}: {}".format(head, text)
    return {
        "text": text,
        "default": raw.get("default") or raw.get("running_default") or raw.get("doing"),
        "since": raw.get("since", raw.get("since_t", raw.get("t"))),
    }


def digest(state, catalog=None):
    """The ~15-line view as structured data — the document's PROPERTIES half.

    Pure: no disk, no marker files, no mutation of `state`.
    """
    me = state.get("me") or {}
    my_team = state.get("my_team") or "Claude"
    enemy_team = "Human" if my_team == "Claude" else "Claude"
    smap = state.get("map") or {}
    units = state.get("units") or []
    buildings = state.get("buildings") or []
    prod_kinds = production_kinds(catalog)
    now = state.get("t", 0.0)

    mine_u = [u for u in units if u.get("team") == my_team]
    workers = [u for u in mine_u if u.get("kind") == "Worker"]
    army = [u for u in mine_u if u.get("kind") != "Worker"]

    # --- army by squad ---
    by_squad = defaultdict(list)
    for u in army:
        by_squad[u.get("squad")].append(u)
    squads = []
    for sq in state.get("squads") or []:
        sid = sq.get("id")
        squads.append(_squad_props(sid, sq, by_squad.get(sid, []), smap))
    loose = by_squad.get(None, [])
    if loose:
        squads.append(_squad_props(None, {}, loose, smap))

    # --- my production ---
    queues, idle, building, jobs = [], [], [], []
    for b in buildings:
        if b.get("team") != my_team:
            continue
        kind = b.get("kind", "?")
        if not b.get("done", True):
            building.append("{}({:.0f}%)".format(kind, 100.0 * b.get("progress", 0.0)))
            continue
        if kind in prod_kinds:
            q = b.get("queue") or []
            if q:
                queues.append({"kind": kind, "id": b.get("id"), "queue": list(q)})
            elif not b.get("upgrading"):
                idle.append(kind)
        job = b.get("researching")
        if job:
            jobs.append(
                "{} L{} {:.0f}s".format(
                    job.get("upgrade", "?"), job.get("level", 0), job.get("remaining", 0.0)
                )
            )
        up = b.get("upgrading")
        if up:
            jobs.append("{}→{} {:.0f}s".format(kind, up.get("into", "next"), up.get("remaining", 0.0)))

    # --- the win-condition line ---
    #
    # Fog-honest by inheritance: `buildings` holds enemy structures this seat
    # can see plus the ghosts it remembers, and a ghost carries `last_seen`.
    # Nothing here counts a building nobody looked at, and nothing here guesses
    # at one — "2 seen" is a floor on what they have, never a total, which is
    # why the line says `seen` and reports how stale the memory is.
    enemy_prod = [
        b for b in buildings if b.get("team") == enemy_team and b.get("kind") in prod_kinds
    ]
    ages = [now - b["last_seen"] for b in enemy_prod if b.get("last_seen") is not None]
    win = {
        "seen": len(enemy_prod),
        "by_kind": dict(sorted(Counter(b.get("kind", "?") for b in enemy_prod).items())),
        "remembered": len(ages),
        "oldest_age": round(max(ages), 0) if ages else None,
        "explored": (state.get("fog") or {}).get("explored"),
    }

    # --- alarms: absent key and empty list are different claims ---
    alarms = None
    if state.get("alarms") is not None:
        alarms = [_alarm_props(a) for a in state.get("alarms") or []]

    props = {
        "t": now,
        "seq_applied": state.get("seq_applied", 0),
        "team": my_team,
        "race": state.get("my_race"),
        "map": smap.get("name", "?"),
        "resources": {
            "gold": me.get("gold", 0),
            "lumber": me.get("lumber", 0),
            "supply_used": me.get("supply_used", 0),
            "supply_cap": me.get("supply_cap", 0),
            "upkeep_rate": me.get("upkeep_rate", 1.0),
            "tier": me.get("tier", 1),
            "workers": len(workers),
            "idle_workers": sum(1 for w in workers if w.get("order") == "Idle"),
        },
        "army": {
            "units": len(army),
            "strength": round(sum(u.get("hp", 0.0) for u in army)),
            "by_kind": dict(sorted(Counter(u.get("kind", "?") for u in army).items())),
            "heroes": [
                {
                    "kind": u.get("kind"),
                    "level": (u.get("hero") or {}).get("level"),
                    "hp_frac": round(u.get("hp", 0.0) / u["max_hp"], 2) if u.get("max_hp") else None,
                }
                for u in mine_u
                if u.get("hero")
            ],
        },
        "squads": squads,
        "production": {
            "queues": queues,
            "queued": sum(len(q["queue"]) for q in queues),
            "idle": sorted(idle),
            "building": building,
            "jobs": jobs,
        },
        "win_condition": win,
        "events": [list(e) for e in (state.get("events") or [])[-EVENT_LINES:]],
        "alarms": alarms,
        "status": {
            "game_over": state.get("game_over"),
            "game_over_reason": state.get("game_over_reason"),
            "waiting_for": state.get("waiting_for"),
            "errors": list(state.get("errors") or []),
        },
    }
    props["default"] = running_default(props)
    return props


def running_default(props):
    """What silence does now.

    AFFORDANCES.md makes persistence the default — "no command means continue
    current stance" — so the digest has to be able to say what continuing IS.
    An alarm's own named default comes FIRST, because an alarm fires only after
    the reflex it is reporting has already acted and that reflex is the part of
    silence a commander most needs to know about; the standing stances follow,
    because they go on being true underneath it.
    """
    parts = [a["default"] for a in props.get("alarms") or [] if a.get("default")]
    stances = [
        "{} keeps {}".format(
            "squad {}".format(sq["id"]) if sq["id"] is not None else "loose army",
            sq["stance_phrase"],
        )
        for sq in props["squads"]
    ]
    parts += stances or ["no squad has a standing posture"]
    queued = props["production"]["queued"]
    idle = props["production"]["idle"]
    if queued:
        parts.append(
            "{} queued item{} finish{}".format(queued, "" if queued == 1 else "s", "es" if queued == 1 else "")
        )
    if idle:
        # The names are already on the PRODUCTION line; here the count is the
        # news, and r21's lesson is that an idle production building is news.
        parts.append(
            "{} production building{} stay{} idle".format(
                len(idle), "" if len(idle) == 1 else "s", "s" if len(idle) == 1 else ""
            )
        )
    return "; ".join(parts)


def _trunc(line, width=110):
    return line if len(line) <= width else line[: width - 1] + "…"


def _game_over_phrase(game_over):
    """`game_over` is a team name, or `"draw"` — the one value that is not a
    team (wc3clone-j84: a capped match ends dead even and still has to end).
    "draw wins" is the sentence this exists to not print."""
    return "a draw" if game_over == "draw" else f"{game_over} wins"


def render_digest(props):
    """The properties section as ~15 lines of text."""
    r = props["resources"]
    head = [
        _trunc(
            "DIGEST t={:.0f}s seq={} map={} seat={}{}".format(
                props["t"],
                props["seq_applied"],
                props["map"],
                props["team"],
                "/" + props["race"] if props.get("race") else "",
            )
        ),
        _trunc(
            "RESOURCES gold {} lumber {} supply {}/{} upkeep {:.0f}% tier {} workers {}{}".format(
                r["gold"], r["lumber"], r["supply_used"], r["supply_cap"],
                100.0 * r["upkeep_rate"], r["tier"], r["workers"],
                " (idle {})".format(r["idle_workers"]) if r["idle_workers"] else "",
            )
        ),
    ]

    st = props["status"]
    if st["waiting_for"] is not None:
        head.append(
            _trunc("HELD at t=0, waiting for: {} — send ready".format(
                " ".join(str(x) for x in st["waiting_for"]) or "(nobody)"))
        )
    if st["game_over"]:
        head.append("GAME OVER: {}{}".format(
            _game_over_phrase(st["game_over"]),
            " (" + st["game_over_reason"] + ")" if st["game_over_reason"] else ""))
    if st["errors"]:
        head.append(_trunc("ERRORS {}: {}".format(len(st["errors"]), st["errors"][-1])))

    a = props["army"]
    # Most numerous first, and only the head of the list: a composition is read
    # for its shape, and the tail of ones is what the full readout is for.
    kinds = sorted(a["by_kind"].items(), key=lambda kv: (-kv[1], kv[0]))
    comp = " ".join("{}:{}".format(k, v) for k, v in kinds[:4])
    if len(kinds) > 4:
        comp += " +{}".format(sum(v for _, v in kinds[4:]))
    heroes = " ".join(
        "{} L{} {:.0f}%".format(
            h["kind"], h["level"] if h["level"] is not None else "?", 100.0 * (h["hp_frac"] or 0)
        )
        for h in a["heroes"]
    )
    body = [
        _trunc(
            "ARMY {} str {} · {}{}".format(
                a["units"], a["strength"], comp or "-", " · heroes " + heroes if heroes else ""
            )
        )
    ]
    squads = props["squads"]
    for sq in squads[:SQUAD_LINES]:
        name = "SQUAD {}".format(sq["id"]) if sq["id"] is not None else "LOOSE"
        # An empty squad is a live fact — r21 armed a rule on one and it fired
        # as "move 0 units" — so it keeps its line and says so plainly rather
        # than reporting a strength of zero at an unknown position.
        if not sq["units"]:
            body.append(_trunc("{} {} · EMPTY".format(name, sq["stance_phrase"])))
            continue
        body.append(
            _trunc(
                "{} {} · {} units · str {}{} · {}".format(
                    name,
                    sq["stance_phrase"],
                    sq["units"],
                    sq["strength"],
                    " hp {:.0f}%".format(100.0 * sq["hp_frac"]) if sq["hp_frac"] is not None else "",
                    sq["place"],
                )
            )
        )
    if len(squads) > SQUAD_LINES:
        body.append("SQUADS +{} more".format(len(squads) - SQUAD_LINES))

    p = props["production"]
    q = " ".join("{}[{}]".format(x["kind"], ",".join(x["queue"])) for x in p["queues"])
    tail = []
    if p["idle"]:
        tail.append(
            "idle: "
            + " ".join(
                "{}x{}".format(v, k) if v > 1 else k
                for k, v in sorted(Counter(p["idle"]).items())
            )
        )
    if p["building"]:
        tail.append("building: " + " ".join(p["building"]))
    if p["jobs"]:
        tail.append("jobs: " + " ".join(p["jobs"]))
    body.append(_trunc("PRODUCTION " + " · ".join(x for x in [q or "nothing queued"] + tail if x)))

    w = props["win_condition"]
    if w["seen"]:
        seen = " ".join("{}x{}".format(v, k) for k, v in w["by_kind"].items())
        stale = ""
        if w["remembered"]:
            stale = " · {} remembered, oldest {:.0f}s ago".format(w["remembered"], w["oldest_age"])
        body.append(_trunc("WIN raze their production: {} seen ({}){}".format(w["seen"], seen, stale)))
    else:
        explored = ""
        if w["explored"] is not None:
            explored = " (explored {:.0f}% of the map)".format(100.0 * w["explored"])
        body.append(_trunc("WIN raze their production: none seen yet{} — scout".format(explored)))

    alarms = []
    if props["alarms"]:
        for al in props["alarms"][:3]:
            alarms.append(
                _trunc("ALARM {}{}".format(
                    al["text"], " — default: " + al["default"] if al.get("default") else ""))
            )
        if len(props["alarms"]) > 3:
            alarms.append("ALARMS +{} more".format(len(props["alarms"]) - 3))

    events = [_trunc("EVT [{:.0f}s] {}".format(e[0], e[1])) for e in props["events"] if len(e) >= 2]
    # The one line allowed to run long: it is the whole point of the
    # persistence default that a commander can read what silence will do, and
    # a clause lost off the end is a squad it did not know was still marching.
    default = [_trunc("DEFAULT if you say nothing: " + props["default"], 240)]

    lines = head + body + alarms + events + default
    # Trim to the ceiling, cheapest information first: events are the only
    # section a commander can recover from the full snapshot at no cost.
    while len(lines) > MAX_LINES and events:
        events.pop(0)
        lines = head + body + alarms + events + default
    return lines


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("path", nargs="?", default="bridge/red/state.json")
    ap.add_argument(
        "--digest",
        action="store_true",
        help="the ~15-line commander digest instead of the full readout",
    )
    ap.add_argument(
        "--doc",
        action="store_true",
        help="the whole hypermedia affordance document: this digest as its "
        "properties section, plus the running default, the alarms and every "
        "link and form (tools/affordances.py)",
    )
    ap.add_argument(
        "--prefs",
        help="with --doc: a JSON file of commander-declared doctrine, which "
        "SORTS the actions and nothing else",
    )
    ap.add_argument(
        "--doc-version",
        action="store_true",
        help="print the affordance document's media-type version and exit — "
        "what an arena round records in its ruleset",
    )
    ap.add_argument(
        "--json",
        action="store_true",
        help="with --digest: the digest's structured data, which is what the "
        "hypermedia document embeds as its properties section. With --doc: the "
        "whole document",
    )
    args = ap.parse_args()

    # Imported here rather than at module scope: `affordances` imports THIS
    # module for `digest()`, and a top-level import in both directions is a
    # cycle. By the time main() runs, bridge_view is fully loaded.
    import affordances  # noqa: PLC0415

    if args.doc_version:
        print(affordances.DOC_VERSION)
        return

    path = args.path
    with open(path) as f:
        s = json.load(f)

    if args.doc:
        doc = affordances.document(s, load_catalog(path), affordances.load_prefs(args.prefs))
        if args.json:
            print(json.dumps(doc, indent=2))
        else:
            for line in affordances.render_document(doc):
                print(line)
        return

    if args.digest:
        props = digest(s, load_catalog(path))
        if args.json:
            print(json.dumps(props, indent=2))
        else:
            for line in render_digest(props):
                print(line)
        return
    full_view(s, path)


def full_view(s, path):

    # Events newer than my previous read; one marker per seat so parallel
    # commanders don't clobber each other's read position.
    marker = marker_path("bridge_last_t", path)
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
        print(f"GAME OVER: {_game_over_phrase(s['game_over'])}")
    for e in s.get("errors", []):
        print(f"ERR: {e}")

    # --- alarms ---
    # Above everything else in the readout, and printed with its running
    # default on the same line, because that is the whole design: the alarm is
    # a prompt to re-decide, and the sentence after the dash is what happens if
    # you do not. Absent until something is standing, so a quiet match prints
    # nothing extra (docs/AFFORDANCES.md, "Alarms").
    for a in s.get("alarms", []):
        eta = f" [ETA {a['eta_s']:.0f}s]" if a.get("eta_s") is not None else ""
        print(f"ALARM/{a['severity']} {a['fact']}{eta}")
        print(f"  default (happens if you say nothing): {a['running_default']}")

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
