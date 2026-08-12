#!/usr/bin/env python3
"""The hypermedia affordance document — the ACTIONS half of the commander view.

    python3 tools/bridge_view.py --doc  bridge/red/state.json
    python3 tools/bridge_view.py --doc --json  bridge/red/state.json
    python3 tools/affordances.py --version        # the media-type version

docs/AFFORDANCES.md makes the commander digest and the affordance menu ONE
media type: a per-seat, per-cycle document with a properties section, a running
default, the ringing alarms, and a list of actions. `bridge_view.digest()` is
the properties section and this module is everything else. The document is
rendered from `state.json` and `catalog.json` and writes nothing: no wire key
was added for it, and every command it hands back is a command the protocol
already had.

THE LADDER, which is what lets one document serve a Haiku and a Fable:

  1. `default`  — zero decisions. Silence follows it, and it is always printed.
  2. link       — zero fields. `command` is complete; send it back verbatim.
  3. form       — the engine filled every fact-shaped field; you fill the
                  judgment-shaped ones, which arrive as `null`.
  4. raw intent — the URI bar. Anything in tools/COMMANDER_BRIEF.md is legal
                  whether or not it appears here. **The document is a floor,
                  never a ceiling** (AFFORDANCES.md constraint 1): an engine
                  that refused an off-document command would break the fairness
                  invariant in reverse, since the human seat has no such menu.

TWO ANNOTATION CHANNELS, STRICTLY SEPARATED (AFFORDANCES.md):

  * **readiness** — engine-computed FACTS, fog-legal. `ready`, `reason`,
    `intel`, `cost`. Every number in a reason comes from this seat's own
    snapshot. There is no "best move" here and there must never be one: a
    recommendation is an opinion, and the engine does not have opinions.
  * **preference** — commander-declared doctrine, engine-SORTED and never
    engine-generated. See `load_prefs` for the mechanism and the argument for
    why it is a file rather than a wire key.

FOG-LEGALITY, STRUCTURALLY. The actions half reads the seat's own resources,
its own units, its own standing state, the public map, the static catalog, and
the `intel` ledger — and **nothing else**. It never looks at `units[]` or
`buildings[]` of the enemy team, so no annotation can leak a fact the seat did
not earn, by construction rather than by a visibility check at each call site
(tools/BUILDER_BRIEF.md §6.10, "the one insert guard"). The properties section
inherits the snapshot's own fog-honesty separately.

Everything degrades. Every read is a `.get` with a default, exactly as in
`bridge_view.digest`: a document that raises `KeyError` on a seat that never
armed a trigger is worse than no document at all.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bridge_view  # noqa: E402
from bridge_view import dist, load_catalog  # noqa: E402

# ---------------------------------------------------------------------------
# The media-type version
# ---------------------------------------------------------------------------

#: AFFORDANCES.md constraint 3: "once the scaffold encodes any judgment, an
#: arena result measures model+scaffold... the scaffold version must appear in
#: the round's `ruleset` so ledger comparisons stay honest." This document
#: encodes several judgments — the push gates below are the loudest — so it
#: carries a version, the document prints it, and `--version` hands it to the
#: arena runner without parsing anything.
#:
#: Bump the minor half when the action SET changes (a new form, a new recipe, a
#: changed template) and the major half when the document's shape does. A
#: renumbered scaffold and an unchanged one must never be confusable in the
#: ledger, which is the whole point of the field.
#: `1.1` — the action SET grew a production section (`train:<kind>` forms and
#: the `recipe:steady-production` rule) once the building selector family made
#: `train` sayable without an entity id. The document's SHAPE did not move, so
#: the major half did not either.
#: `1.3` — r25: `recipe:steady-production`'s `when` becomes a compilable
#: repeating `game_time` pulse (the served `unit_count`+`below` shape never
#: compiled). A fixed recipe changes what a trusting commander arms, so the
#: rounds either side of it must not claim one scaffold.
#: `1.2` — the `when` field serves the real predicate schema out of
#: `catalog.predicates` (`enemy_in(region, [class], [count=1])`) instead of
#: fourteen bare type names, and the rally/template/cancel forms arrived beside
#: the `train` ones. Both are capability changes for a small commander — a model
#: that no longer has to leave the document to find out what an arm takes writes
#: different commands — so the ledger has to be able to tell the two scaffolds
#: apart.
DOC_VERSION = "affordance-doc/1.3"

# ---------------------------------------------------------------------------
# Engine constants this view mirrors
#
# Each is a number the engine enforces; the document states it so that
# validation-as-teaching happens at AUTHORING time (the domain arrives with the
# form) instead of only at fire time (a refusal naming the list). The refusal
# path stays intact underneath — these are not a second enforcement, they are a
# second *rendering*, which is the only thing a view is allowed to be.
# ---------------------------------------------------------------------------

MAX_TRIGGERS = 8  # shared::MAX_TRIGGERS_PER_TEAM
MAX_REGIONS = 8  # shared::MAX_REGIONS_PER_TEAM
MAX_PLANS = 2  # shared::MAX_PLANS_PER_TEAM
MAX_PLAN_STEPS = 8  # shared::MAX_PLAN_STEPS
REGION_RADIUS_MIN = 4.0  # shared::REGION_RADIUS_MIN
REGION_RADIUS_MAX = 60.0  # shared::REGION_RADIUS_MAX

# ---------------------------------------------------------------------------
# The push gates — the one place this document encodes a judgment
#
# docs/AFFORDANCES.md, readiness channel: "one consolidated squad, size >= N,
# heroes >= 80% — the exact three conditions blue violated in its failed t=697
# trickle-push and satisfied in the winning t=787 one." They are FACTS about
# the seat's own army measured against a threshold, never advice: a `push` link
# that fails them is still listed, still sendable, and still does exactly what
# it says. The reason line prints both halves of every comparison so the
# commander can disagree with the threshold rather than with the engine.
#
# These numbers are why DOC_VERSION exists.
# ---------------------------------------------------------------------------

#: Blue's failed push went out at four; the winning one at eight.
PUSH_MIN_UNITS = 6
#: A hero below this is the most expensive casualty in the game walking into a
#: fight (COMMANDER_BRIEF, "Hero save").
PUSH_HERO_FRAC = 0.80

# ---------------------------------------------------------------------------
# Vocabularies, served from the catalog when there is one
#
# `catalog.stances` and `catalog.selectors` are published by the engine
# (`shared::game_catalog`) exactly so a form's domain and the refusal a bad
# value earns are the same words. These fallbacks stand in for a document
# rendered beside an older `catalog.json`, on the same reasoning as
# `bridge_view.PRODUCTION_KINDS`.
# ---------------------------------------------------------------------------

STANCE_FALLBACK = [
    {"id": "turtle", "description": "Hold home tight; break off early."},
    {"id": "stage", "description": "Gather at a forward point and wait."},
    {"id": "push", "description": "Commit to the objective."},
    {"id": "secure", "description": "Hold ground away from home."},
    {"id": "harass", "description": "Hit the soft targets; leave before the trade."},
]

SELECTOR_FALLBACK = {
    "units": ["my hero", "all army", "all units", "workers", "idle workers",
              "nearest worker", "squad <n>"],
    "nodes": ["nearest tree", "nearest mine"],
    "sites": ["nearest legal site"],
    "buildings": ["my <building>", "idle <building>", "my hall"],
}

_COORDS = re.compile(r"\(\s*(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*\)")
_SQUAD_IN_TEXT = re.compile(r"squad (\d+)")


def stance_table(catalog):
    """The five stance words with their numbers."""
    rows = (catalog or {}).get("stances")
    return list(rows) if rows else list(STANCE_FALLBACK)


def predicate_schemas(catalog):
    """Every `when` predicate with the fields its arm carries.

    Served straight from `catalog.predicates`, which the engine publishes from
    `shared::catalog_predicates()`. There is deliberately NO fallback list here,
    unlike `STANCE_FALLBACK` and `SELECTOR_FALLBACK`: this module used to keep a
    hand copy of the fourteen names, kept honest only by a test that parsed the
    table out of tools/COMMANDER_BRIEF.md, and a second copy of a vocabulary is
    the thing the catalog exists to delete. Rendered beside a catalog written
    before `predicates` landed, the `when` field simply serves no domain — which
    is the honest answer ("this document does not know") rather than a
    fourteen-name guess that could be a predicate short.
    """
    return list((catalog or {}).get("predicates") or [])


def predicate_signature(row):
    """One predicate as a form domain reads it: `enemy_in(region, [class], [count=1])`.

    Square brackets are optional keys and `=` is the value the engine fills in,
    which is the whole reason the schema was worth exporting: the domain used to
    be fourteen bare type names and a commander had to go read the brief to find
    out that `enemy_in` wants a place at all.
    """
    parts = []
    for f in row.get("fields") or []:
        name = f.get("name", "?")
        if f.get("default") is not None:
            name = "{}={}".format(name, f["default"])
        parts.append(name if f.get("required") else "[{}]".format(name))
    return "{}({})".format(row.get("id", "?"), ", ".join(parts))


def selector_vocabulary(catalog):
    """The selector phrases, by channel.

    Per-key fallback, not all-or-nothing: `catalog.selectors.buildings` arrived
    after the other three, so a document rendered beside a catalog written
    before it still serves the building phrases the engine has since learned.
    """
    sel = (catalog or {}).get("selectors")
    if not sel:
        return dict(SELECTOR_FALLBACK)
    return {k: list(sel.get(k) or SELECTOR_FALLBACK[k]) for k in SELECTOR_FALLBACK}


# ---------------------------------------------------------------------------
# Places: the domain of every field that names ground
# ---------------------------------------------------------------------------


def own_regions(state):
    """The circles THIS seat named. Doctrine, not information — they appear in
    no other seat's snapshot, so serving them as a domain leaks nothing."""
    return list(state.get("regions") or [])


def place_domain(state):
    """Every name legal in a `region` / `target` field, annotated.

    `map.places` is public geography — the opponent reads the same list — and
    the seat's own `regions` are its own words for its own ground. Nothing else
    is a place, so nothing else is in the domain.
    """
    out = []
    for p in (state.get("map") or {}).get("places") or []:
        if not p.get("name"):
            continue
        pos = p.get("pos") or [0.0, 0.0]
        out.append(
            "{} — map place at ({:.0f}, {:.0f}), r{:.0f}".format(
                p["name"], pos[0], pos[1], p.get("radius", 0.0)
            )
        )
    for r in own_regions(state):
        if not r.get("name"):
            continue
        pos = r.get("pos") or [0.0, 0.0]
        out.append(
            "{} — YOUR region at ({:.0f}, {:.0f}), r{:.0f}".format(
                r["name"], pos[0], pos[1], r.get("radius", 0.0)
            )
        )
    return out


def place_name_at(pos, state):
    """The exact name a `region` field would accept for this spot, or None.

    Distinct from `bridge_view.place_of`, which produces English for a sentence
    ("near the center ford"). This produces the *token* ("center ford"), because
    it goes into a command rather than into a line of prose. The tightest circle
    covering the spot wins, so an expansion inside a wide region is still named
    by the expansion.
    """
    if not pos or len(pos) < 2:
        return None
    best = None
    circles = [(r.get("name"), r.get("pos"), r.get("radius", 0.0)) for r in own_regions(state)]
    circles += [
        (p.get("name"), p.get("pos"), p.get("radius", 0.0))
        for p in (state.get("map") or {}).get("places") or []
    ]
    for name, cpos, radius in circles:
        if not name or not cpos:
            continue
        if dist(pos, cpos) <= radius and (best is None or radius < best[0]):
            best = (radius, name)
    return best[1] if best else None


def posture_anchor(posture):
    """The `(x, z)` a squad's posture string is anchored on, or None.

    Postures arrive as `"defend@(70.0,70.0)r=18"` / `"push@(x,z)"` /
    `"forage@(x,z)"` / `"escort:<unitid>"`. Only the ground-anchored three have
    an anchor a stance could carry over; an escort names a unit, which a stance
    deliberately cannot (`shared::StancePosture` has no `Escort` arm).
    """
    if not posture or "@" not in posture:
        return None
    m = _COORDS.search(posture.partition("@")[2])
    return [float(m.group(1)), float(m.group(2))] if m else None


# ---------------------------------------------------------------------------
# Readiness — facts about this seat's own army
# ---------------------------------------------------------------------------


def squad_members(state, sid):
    """This seat's non-worker units currently enrolled in squad `sid`."""
    me = state.get("my_team") or "Claude"
    return [
        u
        for u in state.get("units") or []
        if u.get("team") == me and u.get("kind") != "Worker" and u.get("squad") == sid
    ]


def squad_headcount(state, sid):
    """What the squad RECORD says it holds, which is not always what the unit
    rosters say: a snapshot from before `units[].squad` existed carries the
    count and no roster."""
    record = next((sq for sq in state.get("squads") or [] if sq.get("id") == sid), {})
    return record.get("members", 0)


def push_gate_facts(state, props, sid):
    """The three push gates, each as a comparison with both numbers on it.

    Returns `(ready, reason)`. `reason` is written whether or not the gates
    hold: "precondition truth + reason" is one channel, and a link that only
    explains itself when it is refusing teaches nothing on the cycle you needed
    it. Read every clause as a fact — the thresholds are this document's
    (DOC_VERSION), and the commander is free to disagree and send it anyway.
    """
    members = squad_members(state, sid)
    if not members and squad_headcount(state, sid):
        # A snapshot old enough to predate `units[].squad` still reports the
        # count on the squad record, and reporting that as "no members" would
        # be the one wrong answer: it is the r21 fact, said about a squad that
        # is not empty. Defer to the reason that says so.
        return stance_facts(state, props, sid)
    army = props["army"]["units"]
    outside = army - len(members)
    heroes = [u for u in members if u.get("hero")]

    bad, good = [], []
    if not members:
        bad.append("squad {} has no members — a stance on an empty squad installs "
                   "doctrine on nobody".format(sid))
    elif len(members) < PUSH_MIN_UNITS:
        bad.append("size {}/{}".format(len(members), PUSH_MIN_UNITS))
    else:
        good.append("size {}/{}".format(len(members), PUSH_MIN_UNITS))

    if outside > 0:
        bad.append(
            "not consolidated: {} of your {} army units are outside squad {}".format(
                outside, army, sid
            )
        )
    elif members:
        good.append("consolidated ({}/{} of your army)".format(len(members), army))

    for h in heroes:
        frac = h.get("hp", 0.0) / h["max_hp"] if h.get("max_hp") else None
        if frac is None:
            continue
        clause = "{} at {:.0f}%, gate is {:.0f}%".format(
            h.get("kind", "hero"), 100.0 * frac, 100.0 * PUSH_HERO_FRAC
        )
        (bad if frac < PUSH_HERO_FRAC else good).append(clause)
    if not heroes and members:
        good.append("no hero in the squad, so no hero gate")

    if bad:
        return False, "push gates: " + "; ".join(bad) + (
            " (met: " + "; ".join(good) + ")" if good else ""
        )
    return True, "push gates met: " + "; ".join(good)


def stance_facts(state, props, sid):
    """Readiness for the four stances that have no push gate.

    The one gate is r21's: a stance on an empty squad is the `"units":[]`
    corpse wearing a new word, and it is caught HERE, at authoring time, rather
    than at fire time.
    """
    members = squad_members(state, sid)
    if not members:
        counted = squad_headcount(state, sid)
        if counted:
            return True, (
                "squad {} reports {} members but this snapshot carries no roster for "
                "them — the stance lands on whoever is enrolled when it arrives".format(sid, counted)
            )
        return False, (
            "squad {} has no members — a stance on an empty squad installs doctrine "
            "on nobody (enrol units first: see the `squad` form)".format(sid)
        )
    strength = round(sum(u.get("hp", 0.0) for u in members))
    return True, "squad {} holds {} units, pooled strength {}".format(sid, len(members), strength)


def intel_note(state):
    """The staleness line — red's loss at t=490, written down.

    Reads the `intel` ledger and nothing else, so it can only report what this
    seat watched with its own eyes and has not yet forgotten. An EMPTY ledger
    is the loudest reading of the three and gets the loudest sentence: red read
    current sight as ground truth and walked into seventeen troops.
    """
    intel = state.get("intel")
    if intel is None:
        return None
    ttl = intel.get("ttl_s")
    groups = intel.get("groups") or []
    if groups:
        g = max(groups, key=lambda x: x.get("size", 0))
        return "last seen: {} troops ({}) {}, {:.0f}s ago — not since".format(
            g.get("size", "?"),
            g.get("composition", "composition unknown"),
            g.get("place", "somewhere"),
            g.get("age", 0.0),
        )
    sightings = intel.get("sightings") or []
    if sightings:
        freshest = min(s.get("age", 0.0) for s in sightings)
        return (
            "no enemy FORCE in your ledger — {} loose sighting{}, freshest {:.0f}s old. "
            "A body of troops you have not seen is not a body of troops that is not there".format(
                len(sightings), "" if len(sightings) == 1 else "s", freshest
            )
        )
    horizon = " (nothing seen in the last {:.0f}s)".format(ttl) if ttl else ""
    return (
        "your intel ledger is EMPTY{} — you have no picture of their army at all, "
        "which is not the same as their having none".format(horizon)
    )


def affordable(state, gold, lumber):
    """`(ok, price, shortfall)` for a price against this seat's own bank.

    The bank is stated only when it is the news: repeating "you hold 964g/145l"
    on fifteen rows of a domain list is fifteen lines a reader has to skip to
    find the two rows where it matters.
    """
    me = state.get("me") or {}
    have_g, have_l = me.get("gold", 0), me.get("lumber", 0)
    short = []
    if have_g < gold:
        short.append("{}g short".format(gold - have_g))
    if have_l < lumber:
        short.append("{}l short".format(lumber - have_l))
    return not short, "{}g/{}l".format(gold, lumber), ", ".join(short)


# ---------------------------------------------------------------------------
# Action constructors
# ---------------------------------------------------------------------------


def link(rel, title, ready, reason, command, intel=None, cost=None, note=None):
    """One LINK: a complete command and the facts about sending it now."""
    a = {
        "kind": "link",
        "rel": rel,
        "title": title,
        "ready": bool(ready),
        "reason": reason,
        "command": command,
    }
    if intel is not None:
        a["intel"] = intel
    if cost is not None:
        a["cost"] = cost
    if note is not None:
        a["note"] = note
    return a


def field(path, ftype, note, domain=None, rng=None, default=None):
    """One FORM field.

    `default` is present on every field and is `null` wherever the answer is a
    judgment. AFFORDANCES.md guard 1: a form default may come only from an
    engine fact or from the commander's own earlier declaration, because a
    default that encodes strategy makes the arena measure the form's author.
    Everything else ships empty.
    """
    f = {"path": path, "type": ftype, "note": note, "default": default}
    if domain is not None:
        f["domain"] = domain
    if rng is not None:
        f["range"] = list(rng)
    return f


def form(rel, title, template, fields, ready=True, reason="", slots=None, note=None, cost=None):
    """One FORM: a template with the judgment-shaped holes left `null`."""
    a = {
        "kind": "form",
        "rel": rel,
        "title": title,
        "ready": bool(ready),
        "reason": reason,
        "template": template,
        "fields": fields,
    }
    if cost is not None:
        a["cost"] = cost
    if slots is not None:
        a["slots"] = slots
    if note is not None:
        a["note"] = note
    return a


# ---------------------------------------------------------------------------
# Stance transitions: every one of them, from wherever the squad stands
# ---------------------------------------------------------------------------


def stance_actions(state, props, catalog):
    """All five stance words for every squad this seat has.

    ALL of them, including the one the squad is already in: re-sending a stance
    is how you land its leash, threshold and focus list on the units that
    joined since (COMMANDER_BRIEF, "Stances", note 3), which is a real and
    frequently-wanted command rather than a no-op. Listing everything is
    AFFORDANCES.md constraint 1 in miniature — the menu is a floor, and a
    transition left off it is one a small commander stops believing in.

    The command is written in the stance word plus a PLACE NAME, never an
    entity id, so a link rendered at t=200 is still a valid sentence at t=260.
    That is the whole reason 0uu.1 was a hard dependency: a frozen id is the
    r21/r23 staleness failure class, automated.
    """
    table = stance_table(catalog)
    intel = intel_note(state)
    out = []
    for sq in state.get("squads") or []:
        sid = sq.get("id")
        if sid is None:
            continue
        current = sq.get("stance")
        anchor_pos = posture_anchor(sq.get("posture"))
        anchor_name = place_name_at(anchor_pos, state) if anchor_pos else None
        for row in table:
            word = row.get("id")
            if not word:
                continue
            command = {"type": "stance", "squad": sid, "stance": word}
            note = None
            if word == "turtle":
                # The engine's own default anchor IS home, which is what turtle
                # means. Omitting the field is the honest spelling.
                note = "no target: `turtle` anchors on your own base, which is the engine default"
            elif anchor_name:
                command["target"] = anchor_name
                note = (
                    "target carried over from squad {}'s current anchor; name any place "
                    "from the domain in the `stance` form to move it".format(sid)
                )
            elif anchor_pos:
                command["x"], command["z"] = anchor_pos
                note = (
                    "target carried over from squad {}'s current anchor, which sits on no "
                    "named ground".format(sid)
                )
            else:
                note = (
                    "no anchor to carry over — sent as written this anchors on your own "
                    "base (engine default); use the `stance` form to name somewhere else"
                )

            if word == "push":
                ready, reason = push_gate_facts(state, props, sid)
            else:
                ready, reason = stance_facts(state, props, sid)

            title = "squad {} → {}{} · {}".format(
                sid,
                word,
                " (re-apply; lands the bundle on units that joined since)"
                if word == current
                else "",
                row.get("description", ""),
            )
            out.append(
                link(
                    "stance:squad-{}:{}".format(sid, word),
                    title.rstrip(" ·"),
                    ready,
                    reason,
                    command,
                    intel=intel if word in ("push", "harass") else None,
                    note=note,
                )
            )
    return out


def stance_form(state, catalog):
    """The stance with its anchor left open — the form under the links.

    The links carry an anchor over; this is how you put one somewhere new
    without writing a `posture`, a `leash`, a `retreat` and a `priority`.
    """
    words = [
        "{} — {}".format(r.get("id"), r.get("description", ""))
        + (
            " [ring r{:.0f}, leash {}, falls back at {:.0f}%]".format(
                r.get("radius", 0.0),
                "none" if not r.get("leash") else "{:.0f}".format(r["leash"]),
                100.0 * r.get("retreat_below", 0.0),
            )
            if "leash" in r
            else ""
        )
        for r in stance_table(catalog)
    ]
    squads = [sq.get("id") for sq in state.get("squads") or [] if sq.get("id") is not None]
    return form(
        "stance",
        "set a squad's whole doctrine in one word, anchored where you choose",
        {"type": "stance", "squad": None, "stance": None, "target": None},
        [
            field("squad", "integer", "which squad. Squad 0 exists automatically.",
                  domain=[str(s) for s in squads] or None),
            field("stance", "stance", "one of the five fixed words.", domain=words),
            field(
                "target",
                "place",
                "the anchor. Omit for your own base. The stance's ring is its own — a "
                "named region's radius is ignored here.",
                domain=place_domain(state),
            ),
        ],
        note="`x`/`z` are accepted instead of `target` if you would rather give numbers.",
    )


def squad_form(state, catalog=None):
    """Enrolment — the prerequisite a stance has no way to state for itself."""
    return form(
        "squad",
        "enrol units into a squad (a squad survives its members; a unit id does not)",
        {"type": "squad", "select": "all army", "id": None},
        [
            field(
                "select",
                "selector",
                "resolved when the command runs. `all army` is pre-filled because it is "
                "a fact about what the phrase means, never a claim about where they belong.",
                domain=selector_vocabulary(catalog)["units"],
                default="all army",
            ),
            field("id", "integer", "which squad number to enrol them into.", rng=(0, 255)),
        ],
        note="A `squad` and a `\"select\":\"squad N\"` in the SAME batch do not see each "
             "other — enrolment lands after the batch compiles. A `stance` in the same "
             "batch does.",
    )


# ---------------------------------------------------------------------------
# Standing state as CRUD-by-name resources
#
# The verbs were always CRUD: a fresh name creates, the snapshot echo reads, the
# same name updates in place (free — the cap counts names), `*_clear` deletes.
# The document only makes it explicit, and adds the one thing the wire never
# said out loud: how much of each collection is spoken for.
# ---------------------------------------------------------------------------


def slots_line(used, cap, noun):
    return "{} of {} {} in use".format(used, cap, noun)


def trigger_forms(state, catalog):
    triggers = list(state.get("triggers") or [])
    schemas = predicate_schemas(catalog)
    #: `enemy_in(region, [class], [count=1])`, not `enemy_in` — the domain now
    #: says what each arm TAKES, which is what a form is for. Empty (and so
    #: absent from the field) beside a catalog written before `predicates`.
    predicates = [predicate_signature(p) for p in schemas] or None
    slots = slots_line(len(triggers), MAX_TRIGGERS, "trigger names")
    room = len(triggers) < MAX_TRIGGERS
    out = [
        form(
            "trigger_set",
            "arm a new contingent order — the engine watches at 4 Hz and submits it for you",
            {"type": "trigger_set", "name": None, "when": None, "then": None, "repeat": None},
            [
                field("name", "string", "a fresh name creates; an existing one replaces that "
                                        "rule in place, free.",
                      domain=[t.get("name") for t in triggers] or None),
                field("when", "predicate",
                      "a `{\"type\":\"<id>\", ...}` object. The domain lists every arm with "
                      "its fields — `[square]` is optional, `=` is the value the engine "
                      "fills in. tools/COMMANDER_BRIEF.md says what each one MEANS.",
                      domain=predicates),
                field("then", "intent", "any intent. Prefer a `stance`/`posture` on a SQUAD, or a "
                                        "`\"select\"` phrase over a list of unit ids — a frozen "
                                        "id becomes a corpse. Legal phrases: "
                        + ", ".join(selector_vocabulary(catalog)["units"]) + "."),
                field("repeat", "number", "cooldown in game seconds. Omit and the rule fires once."),
            ],
            ready=room,
            reason=slots
            + ("" if room else " — re-use one of those names to replace a rule in place, "
                                "or `trigger_clear` one first"),
            slots=slots,
        )
    ]
    for t in triggers:
        name = t.get("name")
        out.append(
            form(
                "trigger_set:{}".format(name),
                "edit the armed trigger '{}' — {}".format(name, t.get("sentence", "")),
                {
                    "type": "trigger_set",
                    "name": name,
                    "when": t.get("when"),
                    "then": t.get("then"),
                    **({"repeat": t["repeat"]} if t.get("repeat") is not None else {}),
                },
                [
                    field("when", "predicate", "as armed; change and re-send.",
                          domain=predicates, default=t.get("when")),
                    field("then", "intent", "as armed; change and re-send.", default=t.get("then")),
                    field("repeat", "number", "as armed.", default=t.get("repeat")),
                ],
                reason="status {}{}".format(
                    t.get("status", "?"),
                    ", last fired {:.0f}s".format(t["last_fired"])
                    if t.get("last_fired") is not None
                    else ", never fired",
                ),
                slots=slots,
                note="Re-sending under the same name replaces it in place and costs no slot.",
            )
        )
        out.append(
            link(
                "trigger_clear:{}".format(name),
                "disarm the trigger '{}'".format(name),
                True,
                "frees one of {} trigger slots".format(MAX_TRIGGERS),
                {"type": "trigger_clear", "name": name},
            )
        )
    return out


def region_forms(state):
    regions = own_regions(state)
    slots = slots_line(len(regions), MAX_REGIONS, "region names")
    room = len(regions) < MAX_REGIONS
    taken = [p.get("name") for p in (state.get("map") or {}).get("places") or [] if p.get("name")]
    out = [
        form(
            "region_set",
            "name a circle of ground — then speak in the name, and move the circle once to "
            "re-aim everything that mentions it",
            {"type": "region_set", "name": None, "x": None, "z": None, "radius": None},
            [
                field("name", "string",
                      "yours alone; your opponent never sees it. You may not take a name "
                      "`map.places` already owns: " + ", ".join(taken),
                      domain=[r.get("name") for r in regions] or None),
                field("x", "number", "centre, world coordinates."),
                field("z", "number", "centre, world coordinates."),
                field("radius", "number", "how big the circle is.",
                      rng=(REGION_RADIUS_MIN, REGION_RADIUS_MAX)),
            ],
            ready=room,
            reason=slots + ("" if room else " — re-use a name to MOVE that circle rather than "
                                            "spending a slot"),
            slots=slots,
        )
    ]
    for r in regions:
        name = r.get("name")
        pos = r.get("pos") or [None, None]
        out.append(
            form(
                "region_set:{}".format(name),
                "move or resize your region '{}'".format(name),
                {
                    "type": "region_set",
                    "name": name,
                    "x": pos[0],
                    "z": pos[1],
                    "radius": r.get("radius"),
                },
                [
                    field("x", "number", "as you drew it.", default=pos[0]),
                    field("z", "number", "as you drew it.", default=pos[1]),
                    field("radius", "number", "as you drew it.",
                          rng=(REGION_RADIUS_MIN, REGION_RADIUS_MAX), default=r.get("radius")),
                ],
                reason="re-using the name MOVES the circle and spends no slot; every posture "
                       "and every rule that says '{}' re-aims with it".format(name),
                slots=slots,
            )
        )
        out.append(
            link(
                "region_clear:{}".format(name),
                "forget your region '{}'".format(name),
                True,
                "any rule naming '{}' goes quiet rather than firing on the whole map".format(name),
                {"type": "region_clear", "name": name},
            )
        )
    return out


def plan_forms(state):
    plans = [p for p in (state.get("plans") or [])]
    #: `held` is the third live word (docs/INTENT.md, "Arm time and late
    #: binding"): a plan whose current step waits on a place that is not named
    #: yet is stopped, not finished, and it is still holding one of the two
    #: slots. Reading it as dead would tell a commander it has room it does not
    #: have — which is the one number this form exists to state.
    live = [p for p in plans
            if str(p.get("status", "")).startswith(("running", "blocked", "held"))]
    slots = slots_line(len(live), MAX_PLANS, "plan slots")
    room = len(live) < MAX_PLANS
    out = [
        form(
            "plan_set",
            "hand the engine a sequence and stop transcribing it one poll at a time",
            {"type": "plan_set", "name": None, "steps": None},
            [
                field("name", "string", "a fresh name creates; an existing one replaces it.",
                      domain=[p.get("name") for p in plans] or None),
                field(
                    "steps",
                    "array",
                    "up to {} steps, each `{{\"intent\": <any intent>, \"advance\": …}}`. "
                    "`advance` omitted means 'as soon as this one is accepted'; "
                    "`{{\"type\":\"after\",\"secs\":30}}` waits; "
                    "`{{\"type\":\"when\",\"when\":<predicate>}}` waits for a condition.".format(
                        MAX_PLAN_STEPS
                    ),
                    rng=(1, MAX_PLAN_STEPS),
                ),
            ],
            ready=room,
            reason=slots + ("" if room else " — clear one first, or re-use its name"),
            slots=slots,
        )
    ]
    for p in plans:
        name = p.get("name")
        out.append(
            form(
                "plan_set:{}".format(name),
                "edit the plan '{}' — step {}/{}, {}".format(
                    name, p.get("step", "?"), p.get("of", "?"), p.get("status", "?")
                ),
                {"type": "plan_set", "name": name, "steps": p.get("steps")},
                [field("steps", "array", "as set; change a step and re-send.",
                       rng=(1, MAX_PLAN_STEPS), default=p.get("steps"))],
                reason="{}{}".format(
                    p.get("status", "?"),
                    " — current: " + p["current"] if p.get("current") else "",
                ),
                slots=slots,
            )
        )
        out.append(
            link(
                "plan_clear:{}".format(name),
                "drop the plan '{}'".format(name),
                True,
                "frees one of {} plan slots".format(MAX_PLANS),
                {"type": "plan_clear", "name": name},
            )
        )
    return out


# ---------------------------------------------------------------------------
# The build form — the one place a `kind` domain is served
# ---------------------------------------------------------------------------


def build_form(state, catalog):
    """`build`, written entirely in selectors and place names.

    Zero ids: the worker is a `select`, the ground is a place name, and the
    footprint is `nearest legal site` — which is the fix for blue-r23's
    fixed-coordinate farm trigger that reported "site blocked" for the whole
    match. The `kind` domain carries each building's price and whether this
    seat may put it down right now, so a refusal that would have cost a poll
    arrives in the document instead.
    """
    unlocked = state.get("unlocked") or {}
    me = state.get("me") or {}
    # The catalog is ONE document for the whole session and describes both
    # rosters (`CatalogBuilding::race`); a commander finds its own by matching
    # against the race its snapshot carries. Serving the other side's buildings
    # as a domain would be a menu of refusals.
    race = state.get("my_race")
    rows = []
    for b in (catalog or {}).get("buildings") or []:
        kid = b.get("id")
        if not kid or b.get("built_by") not in (None, "Worker"):
            continue
        if race and b.get("race") and race not in b["race"]:
            continue
        g, l = b.get("cost_gold"), b.get("cost_lumber")
        if g is None:
            rows.append("{} — price not in this catalog".format(kid))
            continue
        ok, price, short = affordable(state, g, l)
        if unlocked.get(kid) is False:
            req = ", ".join(b.get("requires") or []) or "higher tech"
            rows.append("{} — {} — NOT AVAILABLE: requires {}".format(kid, price, req))
        elif not ok:
            rows.append("{} — {} — cannot afford ({})".format(kid, price, short))
        else:
            rows.append("{} — {} — available".format(kid, price))
    return form(
        "build",
        "put a building down without naming a worker, a coordinate or a free tile",
        {
            "type": "build",
            "select": "workers",
            "kind": None,
            "region": None,
            "site": "nearest legal site",
        },
        [
            field("select", "selector", "the lowest-id match builds it. A role, not an id.",
                  domain=selector_vocabulary(catalog)["units"], default="workers"),
            field("kind", "kind", "what to put down. Availability is your OWN tech, read off "
                                  "`unlocked`.", domain=rows or None),
            field("region", "place", "roughly where.", domain=place_domain(state)),
            field("site", "selector", "`nearest legal site` moves the footprint to the nearest "
                                      "legal one within 15 instead of refusing.",
                  domain=selector_vocabulary(catalog)["sites"],
                  default="nearest legal site"),
        ],
        reason="you hold {}g/{}l at tier {}".format(
            me.get("gold", 0), me.get("lumber", 0), me.get("tier", 1)
        ),
    )


# ---------------------------------------------------------------------------
# Production — the one thing a small commander does every cycle
#
# `train` used to be unreachable from this document for one reason: it took a
# building ENTITY ID and no selector channel covered buildings, so the verb a
# commander sends more often than any other was the one verb it had to
# hand-write with a number read out of `buildings[]`. The building selector
# family (`"select":"idle barracks"`) closes that, and this section is what it
# was for.
#
# One form per producer KIND the seat actually owns, on exactly the pattern
# `build_form` set: the judgment-shaped hole is `unit`, and its domain carries
# the price and the availability of every row so a refusal that would have cost
# a poll cycle arrives with the menu instead. The `select` default is
# `idle <kind>` because "a producer with nothing queued" is a fact about the
# phrase, never a claim about what to build.
# ---------------------------------------------------------------------------


def own_buildings(state):
    """This seat's own buildings. Never the enemy's — the fog-legality rule in
    this module's docstring is kept by not asking, not by filtering later."""
    me = state.get("my_team")
    return [b for b in state.get("buildings") or [] if not me or b.get("team") == me]


def producer_kinds(state, catalog):
    """`[(kind, finished, idle, trains)]` for every producer kind this seat has
    standing, in the catalog's own order.

    Finished only, because an unfinished Barracks trains nothing and a form
    offering one is a menu row that refuses. `idle` is the count with an empty
    queue — the number that decides whether `idle <kind>` resolves, so the
    reason line can state it rather than let the commander discover it.
    """
    mine = [b for b in own_buildings(state) if b.get("done")]
    rows = []
    for b in (catalog or {}).get("buildings") or []:
        kid, trains = b.get("id"), b.get("trains") or []
        if not kid or not trains:
            continue
        held = [x for x in mine if x.get("kind") == kid]
        if not held:
            continue
        idle = [x for x in held if not (x.get("queue") or [])]
        rows.append((kid, len(held), len(idle), list(trains)))
    return rows


def unit_domain(state, catalog, trains):
    """The `unit` field's domain: every unit this producer makes, priced, with
    this seat's OWN tech and OWN bank against it.

    The same three annotations `build_form`'s `kind` domain carries, from the
    same two sources (`catalog.units` for the price, `unlocked` for the gate),
    so the two production verbs read alike.
    """
    unlocked = state.get("unlocked") or {}
    me = state.get("me") or {}
    by_id = {u.get("id"): u for u in (catalog or {}).get("units") or []}
    # A hero's price is a MATCH FACT and not a catalog one: the first hero a
    # team fields is free and every one after it is not, so the catalog row is
    # the wrong number for exactly the decision being made. `me.hero_costs` is
    # what the engine will actually charge (bridge.rs `hero_costs`), per class.
    hero_costs = {h.get("kind"): h for h in me.get("hero_costs") or []}
    slots, used = me.get("hero_slots"), me.get("hero_slots_used")
    rows = []
    for uid in trains:
        u = by_id.get(uid) or {}
        g, l = u.get("cost_gold"), u.get("cost_lumber")
        hero = hero_costs.get(uid)
        if hero:
            g, l = hero.get("gold", g), hero.get("lumber", l)
        if g is None:
            rows.append("{} — price not in this catalog".format(uid))
            continue
        ok, price, short = affordable(state, g, l or 0)
        supply = " {}supply".format(u["supply"]) if u.get("supply") else ""
        # Hero slots are a separate gate from tech and from money, and the one a
        # commander forgets: with the slot full the queue refuses whatever the
        # bank says. Stating both halves is the readiness rule, not advice.
        if hero and slots is not None and used is not None and used >= slots:
            rows.append(
                "{} — {}{} — NOT AVAILABLE: hero slots full ({}/{}); upgrade a hall".format(
                    uid, price, supply, used, slots
                )
            )
        elif unlocked.get(uid) is False:
            rows.append("{} — {}{} — NOT AVAILABLE at your tech".format(uid, price, supply))
        elif not ok:
            rows.append("{} — {}{} — cannot afford ({})".format(uid, price, supply, short))
        else:
            rows.append("{} — {}{} — available".format(uid, price, supply))
    return rows


def production_forms(state, catalog):
    """`train`, written as a role — one form per producer kind the seat holds."""
    me = state.get("me") or {}
    domain = selector_vocabulary(catalog)["buildings"]
    out = []
    for kind, held, idle, trains in producer_kinds(state, catalog):
        # `idle <kind>` when one is free, `my <kind>` when none is: a default
        # that would refuse if sent as written is not a default, it is a trap.
        # Both are facts about the seat's own buildings, so neither is advice.
        phrase = "idle {}".format(kind) if idle else "my {}".format(kind)
        supply = "{}/{} supply".format(me.get("supply_used", 0), me.get("supply_cap", 0))
        out.append(
            form(
                "train:{}".format(kind),
                "train at your {} — no building id, the role resolves when it runs".format(kind),
                {"type": "train", "select": phrase, "unit": None},
                [
                    field(
                        "select",
                        "selector",
                        "which producer. `idle {k}` takes one with an empty queue and refuses "
                        "in words if they are all busy; `my {k}` takes the lowest-id one and "
                        "queues behind whatever it is doing.".format(k=kind),
                        domain=domain,
                        default=phrase,
                    ),
                    field(
                        "unit",
                        "kind",
                        "what to queue. Availability is your OWN tech, read off `unlocked`.",
                        domain=unit_domain(state, catalog, trains) or None,
                    ),
                ],
                reason="{} finished {}, {} idle; you hold {}g/{}l at {}".format(
                    held, kind, idle, me.get("gold", 0), me.get("lumber", 0), supply
                ),
                note="the same phrase is legal in a trigger's or a plan step's `then`, and it "
                     "resolves when the rule FIRES — which is how a repeating `train` rule "
                     "survives the building it names being razed and rebuilt.",
            )
        )
    out.extend(rally_forms(state, catalog))
    out.extend(template_forms(state, catalog))
    out.extend(cancel_forms(state, catalog))
    return out


def rally_readback(held):
    """What these buildings' rally points currently are, as one phrase.

    Reads `buildings[].rally`, the key the snapshot gained so this question had
    an answer at all. Before it, the only way to be sure where a building sent
    its output was to send `rally` again — a poll, and the thing the document
    exists to delete. `unset` is stated rather than omitted: "no rally point"
    and "I could not tell you" are different facts.
    """
    seen = []
    for b in held:
        r = b.get("rally")
        if not r:
            seen.append("unset")
        elif r.get("pos"):
            seen.append("({:.0f}, {:.0f})".format(r["pos"][0], r["pos"][1]))
        else:
            seen.append("onto {}".format(r.get("target")))
    return ", ".join(seen) or "none standing"


def rally_forms(state, catalog):
    """`rally`, written as a role — where a producer sends what it trains."""
    out = []
    for kind, held, _idle, _trains in producer_kinds(state, catalog):
        mine = [b for b in own_buildings(state)
                if b.get("done") and b.get("kind") == kind]
        out.append(
            form(
                "rally:{}".format(kind),
                "send what your {} trains somewhere — no building id".format(kind),
                {"type": "rally", "select": "my {}".format(kind), "region": None},
                [
                    field(
                        "select",
                        "selector",
                        "which producer. `my {k}` is the lowest-id one; name the kind you "
                        "mean if you hold several sorts.".format(k=kind),
                        domain=selector_vocabulary(catalog)["buildings"],
                        default="my {}".format(kind),
                    ),
                    field(
                        "region",
                        "place",
                        "where they walk. A named place or a built-in; `x`/`z` take numbers "
                        "instead, and `target` takes a resource node (new workers harvest it) "
                        "or one of your own units (new units follow it).",
                        domain=place_domain(state),
                    ),
                ],
                reason="{} finished {}; rally now: {}".format(
                    held, kind, rally_readback(mine)
                ),
                note="the snapshot reads it back as `buildings[].rally`, so you never have to "
                     "re-send one to find out what it is.",
            )
        )
    return out


def template_forms(state, catalog):
    """`template`, written as a role — standing doctrine for everything a
    producer trains from here on.

    The one verb in the family that is *policy* rather than an order, so its
    reason line says whether one is already installed (`buildings[].template`
    has been a flag since the verb landed) — replacing a template replaces the
    WHOLE of it, and a commander that did not know one was there would silently
    drop the half it did not restate.
    """
    out = []
    squads = [str(sq.get("id")) for sq in state.get("squads") or [] if sq.get("id") is not None]
    for kind, held, _idle, _trains in producer_kinds(state, catalog):
        mine = [b for b in own_buildings(state)
                if b.get("done") and b.get("kind") == kind]
        set_on = sum(1 for b in mine if b.get("template"))
        out.append(
            form(
                "template:{}".format(kind),
                "stamp standing doctrine on everything your {} trains".format(kind),
                {"type": "template", "select": "my {}".format(kind), "squad": None},
                [
                    field(
                        "select",
                        "selector",
                        "which producer's output the doctrine applies to.",
                        domain=selector_vocabulary(catalog)["buildings"],
                        default="my {}".format(kind),
                    ),
                    field(
                        "squad",
                        "integer",
                        "enrol every new unit into this squad, so it inherits the squad's "
                        "stance the moment it walks out.",
                        domain=squads or None,
                        rng=(0, 255),
                    ),
                ],
                reason="{} finished {}, {} already carrying a template".format(
                    held, kind, set_on
                ),
                note="`retreat`, `priority` and `autocast` are the other pieces. WHATEVER YOU "
                     "SEND REPLACES THE WHOLE TEMPLATE — a piece you omit is unset, not kept — "
                     "and a `template` with no pieces at all removes it.",
            )
        )
    return out


def cancel_forms(state, catalog):
    """`cancel`, written as a role — and offered only where there is a queue.

    The one form in the family whose readiness is not about the building: a
    cancel with nothing queued is `queue index 0 out of range`, so the form is
    listed with `ready: false` and the reason states every queue this seat holds
    of that kind. Listing it anyway is AFFORDANCES.md constraint 1 — a menu that
    hides the option is worse than one that explains why it would refuse.
    """
    out = []
    for kind, held, _idle, _trains in producer_kinds(state, catalog):
        mine = [b for b in own_buildings(state)
                if b.get("done") and b.get("kind") == kind]
        queues = [list(b.get("queue") or []) for b in mine]
        longest = max((len(q) for q in queues), default=0)
        out.append(
            form(
                "cancel:{}".format(kind),
                "drop one entry from a {} training queue".format(kind),
                {"type": "cancel", "select": "my {}".format(kind), "index": None},
                [
                    field(
                        "select",
                        "selector",
                        # Never `idle` here, and that is a fact rather than a
                        # preference: an idle producer is exactly the one with
                        # nothing to cancel.
                        "which producer's queue. `idle {k}` is never right here — an idle "
                        "one has nothing queued.".format(k=kind),
                        domain=selector_vocabulary(catalog)["buildings"],
                        default="my {}".format(kind),
                    ),
                    field(
                        "index",
                        "integer",
                        "which slot, 0-based. 0 is the one being built right now and "
                        "cancelling it restarts the timer for whatever is behind it.",
                        rng=(0, max(longest - 1, 0)),
                    ),
                ],
                ready=longest > 0,
                reason="{} finished {}; queues: {}".format(
                    held, kind, " | ".join(str(q) for q in queues) or "none"
                ),
                note="`select` resolves to the LOWEST-id match, which may not be the one whose "
                     "queue you are reading. Send `building: <id>` off `buildings[]` when you "
                     "mean a particular one.",
            )
        )
    return out


# ---------------------------------------------------------------------------
# The recipes, as served forms
#
# tools/COMMANDER_BRIEF.md's recipes were JSON blocks with `<hero id>` and
# `<worker id>` in them — a commander had to look an id up, and the rule then
# went stale the moment that unit died (r21 armed hero-save on `"units":[]` and
# the hero died three seconds later). Here they are templates written in
# selectors and place names, with one or two judgment-shaped holes and nothing
# else. Every pre-filled value below is either an engine fact or a phrase whose
# meaning IS the fact; the thresholds and the ground are left null on purpose,
# because a default threshold is a strategy this document is not allowed to have.
# ---------------------------------------------------------------------------


def recipe_forms(state, catalog):
    places = place_domain(state)
    squads = [str(sq.get("id")) for sq in state.get("squads") or [] if sq.get("id") is not None]
    unlocked = state.get("unlocked") or {}
    producers = producer_kinds(state, catalog)
    hall = next(
        (
            b
            for b in (catalog or {}).get("buildings") or []
            if b.get("id") == "TownHall"
        ),
        {},
    )
    hall_price = (
        "{}g/{}l".format(hall.get("cost_gold"), hall.get("cost_lumber"))
        if hall.get("cost_gold") is not None
        else "price not in this catalog"
    )
    out = [
        form(
            "recipe:home-guard",
            "HOME GUARD — the army comes home when the base burns",
            {
                "type": "trigger_set",
                "name": "home-guard",
                "repeat": 30,
                "when": {"type": "base_under_attack"},
                "then": {"type": "stance", "squad": None, "stance": "turtle"},
            },
            [
                field("then.squad", "integer", "which squad answers the doorbell.",
                      domain=squads or None)
            ],
            reason="repeating, because a base is raided more than once. `turtle` anchors on "
                   "your own base, so this rule needs no coordinate and cannot go stale.",
        ),
        form(
            "recipe:hero-save",
            "HERO SAVE — the hero walks out before it dies",
            {
                "type": "trigger_set",
                "name": "hero-save",
                "repeat": 45,
                "when": {"type": "hero_below", "frac": None},
                "then": {"type": "move", "select": "my hero", "region": None},
            },
            [
                field("when.frac", "number", "the health fraction that pulls it out.",
                      rng=(0.05, 0.95)),
                field("then.region", "place", "where it runs to.", domain=places),
            ],
            reason="the most expensive single event in a match, and it happens inside one poll "
                   "cycle. `\"select\":\"my hero\"` resolves at FIRE time, so the rule survives "
                   "the hero dying and being revived with a new id — this is the exact command "
                   "r21 armed as `\"units\":[]`.",
        ),
        form(
            "recipe:expand",
            "EXPAND — take the next base the moment this one runs dry",
            {
                "type": "trigger_set",
                "name": "expand",
                "when": {"type": "mine_dry"},
                "then": {
                    "type": "build",
                    "select": "workers",
                    "kind": "TownHall",
                    "region": None,
                    "site": "nearest legal site",
                },
            },
            [field("then.region", "place", "which mine to go to.", domain=places)],
            reason="fires once, which is right: you only need telling the first time. "
                   "`nearest legal site` is why this cannot loop on 'site blocked'.",
            note="a TownHall costs {}{}. The trigger is free to arm; the gold is charged when "
                 "it fires.".format(
                     hall_price,
                     "" if unlocked.get("TownHall", True) else " and is NOT currently available",
                 ),
            cost=hall_price,
        ),
        form(
            "recipe:counter-punch",
            "COUNTER-PUNCH — press while their hero is down",
            {
                "type": "trigger_set",
                "name": "their-hero-down",
                "when": {"type": "enemy_hero_down"},
                "then": {"type": "stance", "squad": None, "stance": "push", "target": None},
            },
            [
                field("then.squad", "integer", "which squad goes.", domain=squads or None),
                field("then.target", "place", "what it commits to.", domain=places),
            ],
            reason="`enemy_hero_down` is what you WATCHED, not what is true — a hero that died "
                   "out of your sight is not in this predicate. Once, because you only get to "
                   "spend that window once.",
        ),
    ]
    # STEADY PRODUCTION — the r23 win the building selector was for. Both
    # commanders spent a poll cycle per unit re-reading a barracks id out of
    # `buildings[]`; this is that whole loop as one armed rule, and it survives
    # the barracks dying because it names a role and not an id.
    steady_select = (
        "idle {}".format(producers[0][0]) if producers else None
    )
    out.append(
        form(
            "recipe:steady-production",
            "STEADY PRODUCTION — keep a producer working without spending a poll on it",
            {
                "type": "trigger_set",
                "name": "steady-production",
                "repeat": 20,
                "when": {"type": "game_time", "at": 0},
                "then": {"type": "train", "select": steady_select, "unit": None},
            },
            [
                field(
                    "repeat",
                    "seconds",
                    "the production pulse. Every this-many seconds the rule tries to queue "
                    "one unit at an idle producer; all-busy is a quiet refusal and the rule "
                    "stays armed.",
                    rng=(10, 120),
                    default=20,
                ),
                field(
                    "then.select",
                    "selector",
                    "which producer answers. `idle <kind>` picks one with an empty queue at "
                    "FIRE time, so the rule cannot stack six deep on one building.",
                    domain=selector_vocabulary(catalog)["buildings"],
                    default=steady_select,
                ),
                field("then.unit", "kind", "what it queues.",
                      domain=[k for _, _, _, trains in producers for k in trains] or None),
            ],
            reason="a repeating pulse (`game_time` at 0 + `repeat`), because the trigger "
                   "vocabulary has no 'fewer than N of kind' predicate — 'maintain N' is a "
                   "policy the wire cannot yet say (it is a filed want). The gold is charged "
                   "when it fires, never at arm time — and if the bank is short the fire is "
                   "refused in words and the rule stays armed.",
            note="`then.unit` and `when.kind` are usually the same word; they do not have to be."
                 if producers else
                 "you hold no finished producer yet, so `then.select` has no fact-shaped "
                 "default — name one from the domain.",
        )
    )
    return out


# ---------------------------------------------------------------------------
# Alarms: the reflex first, the overrides after
# ---------------------------------------------------------------------------


def alarm_subject(alarm):
    """The squad an alarm is about, when its own text names one.

    Mechanical on purpose. Choosing which overrides to show would be the engine
    having an opinion about the answer; parsing the subject out of the fact the
    engine already wrote is just following the alarm's own pointing finger.

    `running_default` is deliberately NOT read here. It names whatever happens
    to be standing — "squad 0 holds defend near our base" appears in an income
    collapse's default — and that is the reflex, not the subject.
    """
    text = "{} {}".format(alarm.get("id") or "", alarm.get("fact") or "")
    m = _SQUAD_IN_TEXT.search(text)
    return int(m.group(1)) if m else None


def alarm_entries(state, actions):
    """`alarms[]` from the wire, each leading with the reflex that already fired.

    AFFORDANCES.md: "an alarm fires only after the reflex has — its payoff is
    attention, not speed." So the first action on every alarm is the one that
    is already happening, with a `null` command, and the overrides come after
    it. A commander that only ever confirms is playing acceptably.
    """
    raw = state.get("alarms")
    if raw is None:
        return None
    out = []
    for a in raw:
        if not isinstance(a, dict):
            a = {"fact": str(a)}
        default = a.get("running_default") or a.get("default")
        entry = {
            "id": a.get("id"),
            "fact": a.get("fact") or a.get("text") or "alarm",
            "running_default": default,
            "severity": a.get("severity"),
            "since_t": a.get("since_t", a.get("since")),
        }
        if a.get("eta_s") is not None:
            entry["eta_s"] = a["eta_s"]
        overrides = [
            link(
                "alarm:confirm",
                "say nothing — {}".format(default or "nothing is standing to answer this"),
                True,
                "this is already happening; the sim never waits for your answer",
                None,
            )
        ]
        sid = alarm_subject(a)
        if sid is not None:
            overrides += [
                x for x in actions if x["rel"].startswith("stance:squad-{}:".format(sid))
            ]
        if a.get("id") == "income_collapse":
            overrides += [x for x in actions if x["rel"] in ("recipe:expand", "build")]
        entry["actions"] = overrides
        out.append(entry)
    return out


# ---------------------------------------------------------------------------
# The preference channel
# ---------------------------------------------------------------------------


def load_prefs(path):
    """The commander's declared doctrine, read from a file it wrote itself.

    THE MECHANISM, and why it is this one. Preference is "commander-declared,
    engine-SORTED, never engine-generated" (AFFORDANCES.md). There is no verb on
    the wire that carries a doctrine statement, and inventing one would mean a
    new `Intent` variant, a new snapshot key and a change to both seats — a
    protocol change inside a view-only bead, for a value the engine is
    forbidden to act on. A file beside the seat is the least invasive thing
    that keeps the channel honest: the commander (or its persona prompt) writes
    it, the view reads it, the engine never sees it, and no round in which
    nobody declares anything renders differently than it does today.

        {"doctrine": "aggression: high, risk: low",
         "prefer": ["push", "harass", "trigger"],
         "avoid":  ["turtle"]}

    `prefer` and `avoid` are plain substrings matched, case-folded, against
    each action's `rel` and `title`. Nothing here changes a `ready`, a `reason`
    or a `command`: preference reorders the menu and annotates it, and that is
    the entire extent of its power.
    """
    if not path:
        return None
    with open(path) as f:
        raw = json.load(f)
    return {
        "doctrine": raw.get("doctrine"),
        "prefer": [str(x).lower() for x in raw.get("prefer") or []],
        "avoid": [str(x).lower() for x in raw.get("avoid") or []],
        "source": path,
    }


def _pref_rank(action, prefs):
    if not prefs:
        return 0
    hay = (action["rel"] + " " + action["title"]).lower()
    for i, token in enumerate(prefs["prefer"]):
        if token in hay:
            return i
    for token in prefs["avoid"]:
        if token in hay:
            return len(prefs["prefer"]) + 1
    return len(prefs["prefer"])


def order_actions(actions, alarms, prefs):
    """Fact order, then the commander's own order inside it.

    The key, outermost first:

      1. **alarm** — an action an active alarm points at leads. The engine is
         not saying it is right, only that something changed under it.
      2. **ladder tier** — links before forms. That is the media type's own
         gradient (zero fields before some fields) and it is structural.
      3. **preference** — the commander's declared doctrine, and the only
         channel here that is not a fact.
      4. **readiness** — ready before not-ready. A fact, and the last word
         before the tie-break.
      5. insertion order, so the same snapshot always renders the same page.
    """
    flagged = set()
    for a in alarms or []:
        for x in a.get("actions") or []:
            if x.get("command") is not None:
                flagged.add(x["rel"])
    ordered = sorted(
        enumerate(actions),
        key=lambda p: (
            0 if p[1]["rel"] in flagged else 1,
            0 if p[1]["kind"] == "link" else 1,
            _pref_rank(p[1], prefs),
            0 if p[1]["ready"] else 1,
            p[0],
        ),
    )
    return [a for _, a in ordered]


# ---------------------------------------------------------------------------
# The document
# ---------------------------------------------------------------------------


def document(state, catalog=None, prefs=None):
    """One seat, one cycle, as a hypermedia document.

    Pure: no disk, no marker file, no mutation of `state`. The properties
    section is `bridge_view.digest()` embedded verbatim rather than re-derived,
    so the two halves of the document cannot disagree about the match.
    """
    props = bridge_view.digest(state, catalog)

    actions = []
    actions += stance_actions(state, props, catalog)
    actions.append(stance_form(state, catalog))
    actions.append(squad_form(state, catalog))
    actions += trigger_forms(state, catalog)
    actions += region_forms(state)
    actions += plan_forms(state)
    actions.append(build_form(state, catalog))
    actions += production_forms(state, catalog)
    actions += recipe_forms(state, catalog)

    alarms = alarm_entries(state, actions)
    actions = order_actions(actions, alarms, prefs)

    return {
        "doc_version": DOC_VERSION,
        "seq": state.get("seq_applied", 0),
        "t": state.get("t", 0.0),
        "properties": props,
        # Persistence as a first-class element. The digest already computes the
        # sentence; the document gives it a `command` slot holding `null`,
        # because "send nothing" is a move and the ladder's first rung.
        "default": {
            "title": props["default"],
            "command": None,
            "note": "silence continues all of this. Nothing below is required.",
        },
        "alarms": alarms,
        "actions": actions,
        "preference": {
            "doctrine": (prefs or {}).get("doctrine"),
            "prefer": (prefs or {}).get("prefer") or [],
            "avoid": (prefs or {}).get("avoid") or [],
            "source": (prefs or {}).get("source") or "none — fact order only",
        },
        "raw": (
            "Every verb in tools/COMMANDER_BRIEF.md is legal whether or not it appears "
            "above. This document is a floor, never a ceiling."
        ),
    }


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------


def _wrap(text, width, indent):
    """Fold prose onto `width`, hanging under `indent`. No stdlib textwrap
    dependency worth the import for four lines, and this keeps long words
    (a JSON template, a place list) from being broken mid-token."""
    words, lines, cur = str(text).split(), [], ""
    for w in words:
        if cur and len(cur) + 1 + len(w) > width:
            lines.append(cur)
            cur = w
        else:
            cur = (cur + " " + w).strip()
    if cur:
        lines.append(cur)
    return [lines[0]] + [indent + x for x in lines[1:]] if lines else [""]


def render_action(a, width=100):
    out = []
    flag = "READY    " if a["ready"] else "NOT READY"
    out.append("  [{}] {}  {}".format(flag, a["rel"], a["title"]))
    pad = " " * 14
    if a.get("reason"):
        out += [pad + x for x in _wrap("why: " + a["reason"], width, "     ")]
    if a.get("intel"):
        out += [pad + x for x in _wrap("intel: " + a["intel"], width, "     ")]
    if a.get("cost"):
        out.append(pad + "cost: " + str(a["cost"]))
    if a.get("slots") and not a.get("reason", "").startswith(a["slots"]):
        out.append(pad + "slots: " + a["slots"])
    if a.get("note"):
        out += [pad + x for x in _wrap("note: " + a["note"], width, "     ")]
    if a["kind"] == "link":
        out.append(pad + "send: " + json.dumps(a["command"]))
    else:
        out.append(pad + "template: " + json.dumps(a["template"]))
        for f in a["fields"]:
            head = "  {} <{}>{}{}".format(
                f["path"],
                f["type"],
                " range {}..{}".format(*f["range"]) if f.get("range") else "",
                " default={}".format(json.dumps(f["default"]))
                if f["default"] is not None
                else " (leave null — this one is yours)",
            )
            out.append(pad + head)
            out += [pad + "    " + x for x in _wrap(f["note"], width, "  ")]
            if f.get("domain"):
                out.append(pad + "    domain:")
                for d in f["domain"]:
                    out += [pad + "      - " + x for x in _wrap(d, width, "  ")]
    return out


def render_document(doc):
    """The whole document as text.

    Unlike the digest there is no line ceiling here, and deliberately: the
    digest answers "what is going on" in fifteen lines and this answers "what
    can I say", which is a menu. A commander that wants the short version reads
    `--digest`; this is the one that lists everything so that nothing has to be
    remembered.
    """
    lines = [
        "DOC {} seq={} t={:.0f}s".format(doc["doc_version"], doc["seq"], doc["t"]),
        "",
        "PROPERTIES",
    ]
    lines += ["  " + x for x in bridge_view.render_digest(doc["properties"])]
    lines += [
        "",
        "DEFAULT (rung 1 — send nothing and this is what happens)",
    ]
    lines += ["  " + x for x in _wrap(doc["default"]["title"], 100, "  ")]
    lines.append("  " + doc["default"]["note"])

    if doc["alarms"]:
        lines.append("")
        lines.append("ALARMS ({} ringing — the reflex has already answered)".format(len(doc["alarms"])))
        for a in doc["alarms"]:
            lines.append(
                "  [{}] {}{}".format(
                    a.get("severity") or "alarm",
                    a["fact"],
                    " [ETA {:.0f}s]".format(a["eta_s"]) if a.get("eta_s") is not None else "",
                )
            )
            lines += ["    " + x for x in _wrap(
                "running default: " + (a["running_default"] or "nothing is standing to answer this"),
                100, "  ")]
            for x in a["actions"]:
                lines.append("      - [{}] {}".format(x["rel"], x["title"]))
    elif doc["alarms"] is not None:
        lines += ["", "ALARMS none ringing"]

    pref = doc["preference"]
    lines += ["", "ACTIONS ({} — rungs 2 and 3; sorted {})".format(
        len(doc["actions"]),
        "by your declared doctrine, then by fact" if pref["doctrine"] or pref["prefer"]
        else "by fact only (no doctrine declared)",
    )]
    if pref["doctrine"]:
        lines.append("  your declared doctrine: {} (from {})".format(pref["doctrine"], pref["source"]))
    for a in doc["actions"]:
        lines += render_action(a)
    lines += ["", "RAW (rung 4)"]
    lines += ["  " + x for x in _wrap(doc["raw"], 100, "  ")]
    return lines


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv=None):
    ap = argparse.ArgumentParser(description="the hypermedia affordance document")
    ap.add_argument("path", nargs="?", default="bridge/red/state.json")
    ap.add_argument("--json", action="store_true", help="the document as JSON")
    ap.add_argument("--prefs", help="a JSON file of commander-declared doctrine (see load_prefs)")
    ap.add_argument(
        "--version",
        action="store_true",
        help="print the media-type version and exit — what the arena ruleset records",
    )
    args = ap.parse_args(argv)
    if args.version:
        print(DOC_VERSION)
        return 0
    with open(args.path) as f:
        state = json.load(f)
    doc = document(state, load_catalog(args.path), load_prefs(args.prefs))
    if args.json:
        print(json.dumps(doc, indent=2))
    else:
        for line in render_document(doc):
            print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
