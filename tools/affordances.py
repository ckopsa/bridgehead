#!/usr/bin/env python3
"""The hypermedia affordance document — the ACTIONS half of the commander view.

    python3 tools/bridge_view.py --doc  bridge/red/state.json
    python3 tools/bridge_view.py --doc --all  bridge/red/state.json
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
    why it is a file rather than a wire key. Since 2.0 it also carries an
    optional `focus`, which chooses what the TEXT render expands, and since 2.1
    an optional `playbook` — both declared by the commander, never inferred by
    the engine.

PLAYBOOKS (2.1). A third thing on the page, between the alarms and the actions:
a declared game-plan from `catalog.playbooks`, served as the ONE step this
snapshot says you are on, rendered as a FORK of 2-4 live options rather than as
an instruction. See the "Playbooks" section below for the anchoring constraint
that shapes every line of it, and docs/AFFORDANCES.md § Playbooks for why
authored strategy is allowed in the scaffold at all (it is versioned in the
round's ruleset, and the engine executes none of it).

FACT-COLLAPSED RENDERING (2.0). The arena's model ladder (arena/LADDER.md,
Findings 2 and 5) measured the cost of the full render: ~600 lines mid-game,
every tier abandoning the document for the digest at loop cadence, and the
readiness annotations that would have prevented the mid-tier losing moves
served every cycle and read never. So the default TEXT render folds each action
onto ONE line that still carries its complete command, groups them, and puts
the blocking fact on the line of every action that is not ready. Nothing is
deleted: `--all` restores the full render, a declared `focus` expands a section,
and `--json` is untouched — a machine reader pays no line cost and therefore
gets no collapse. `collapsed` on each JSON action is the hint that says which
way the text render went.

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
#: `2.0` — the SHAPE moved, which is what the major half is for. The text
#: render is fact-collapsed by default: one line per action carrying its
#: complete command, grouped by section, with the blocking fact on the line of
#: everything that is not ready, and the full render behind `--all`. The
#: preference channel gained a commander-DECLARED `focus`, which expands one
#: section. No action was added, removed or reworded and the JSON grew only two
#: additive per-action hints (`sections`, `collapsed`), so a machine reader sees
#: 1.3 plus two keys — but a commander reads a different page, and the ledger
#: has to be able to tell the two apart. Evidence: arena/LADDER.md Findings 2
#: and 5 (every tier used the document as an orientation page and the digest as
#: the loop page; the annotations that addressed the mid-tier losing moves were
#: served every cycle and read never).
#: With 2.0 the push gates and the staleness threshold also moved to
#: `catalog.gates` (wc3clone-b9m): the readiness annotations now speak the
#: engine's own thresholds, and the engine echoes an advisory note on any
#: accepted command that contradicts them. Neither 1.4 nor a bare 2.0 ever
#: played a round, so the ledger sees one scaffold.
#: `2.1` — a PLAYBOOK section, between the alarms and the actions. Additive: the
#: rows are `catalog.playbooks` (`assets/data/playbooks.ron`), nothing was
#: removed or reworded, and a seat that declares no playbook gets one
#: advertisement line. It is a version bump rather than a free change because
#: the section changes what a commander reads at the decision moment — a
#: sequenced plan with a "you are here" pointer — and arena/LADDER.md Finding 4
#: says the tiers differ in judgment, which is exactly what a playbook supplies.
#: The section is a FORK and never an instruction (docs/AFFORDANCES.md
#: § Playbooks), so it cannot be compared against 2.0 as "the same page".
#: `2.2` — the forms stop lying about their own holes (wc3clone-2su4). Every
#: field now carries `required`, and the render says which of the three things a
#: hole is: engine-filled, yours-and-REQUIRED, or optional-and-null-means-
#: omitted. The old annotation — "leave null — this one is yours" — read as
#: permission on both of the last two, and r34 blue took it: it sent
#: `recipe:home-guard` back with `"squad": null` and got serde's *invalid type:
#: null, expected u8*, then armed `recipe:expand` with `then.region` null and
#: spent the rule's one fire, 322 seconds later, on a refusal. `recipe:expand`
#: now ships that hole pre-filled from `mines[]` (`expansion_place`), so the
#: recipe a trusting commander arms verbatim actually takes a second base. A
#: fixed recipe changes what a trusting commander arms — the `1.3` argument
#: exactly — so the rounds either side of it must not claim one scaffold.
DOC_VERSION = "affordance-doc/2.2"

# ---------------------------------------------------------------------------
# Sections: what a declared focus can expand, and how the collapsed render
# groups
#
# Every action carries a `sections` list. The assignment is MECHANICAL — read
# off the verb, off the catalog's `trains`, off `unlocked` — and never a
# judgment about what belongs to a strategy: the engine does not have opinions
# about phases either (this bead's rejected half was engine-INFERRED phase
# filtering, docs/AFFORDANCES.md constraint 1 and arena/LADDER.md Finding 5).
#
# `standing` is not a focus word. The trigger/region/plan CRUD family belongs to
# no phase — it is the machinery every phase is written in — so it groups
# together and is expanded only by `--all`.
# ---------------------------------------------------------------------------

#: The words a commander may declare in its prefs file. Anything else is
#: ignored with a note, because a focus the render silently dropped is a
#: commander that thinks it is reading a filtered page and is not.
FOCUS_WORDS = ("economy", "tech", "army", "harass")

#: Group order in the collapsed render. A declared focus jumps to the front of
#: it; otherwise this is the order, and it is fixed so the same snapshot always
#: renders the same page.
SECTION_ORDER = ("army", "harass", "economy", "tech", "standing", "other")
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
MINE_HOME_RADIUS = 40.0  # shared::MINE_HOME_RADIUS — what "our mine" means

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
#: How old this seat's picture of the enemy may be before committing to it is
#: worth saying out loud, in game-seconds. Half the sighting TTL — past the
#: horizon the ledger drops the record entirely and the EMPTY reading takes over.
COMMIT_INTEL_STALE_S = 45.0
#: The rumour horizon (`shared::SIGHTING_TTL_S`), stated so the empty-ledger
#: sentence can name the window it is empty over.
SIGHTING_TTL_S = 90.0


def gates(catalog):
    """`(min_units, hero_frac, stale_s, ttl_s)` — the four thresholds, from the
    engine if it published them.

    Since wc3clone-b9m the engine measures the SAME three numbers when it writes
    an acceptance note into `state.json`'s `notes` array, so a commander that
    over-commits is told at the decision moment as well as in this document. Two
    renderings of one rule that can disagree is the failure docs/FOG.md is
    written against — nothing errors, the two channels simply say different
    things about the same squad — so the engine publishes them as
    `catalog.gates` and this reads them from there.

    The module constants above survive as the fallback for a document rendered
    beside a `catalog.json` written before that block existed, on exactly the
    reasoning `STANCE_FALLBACK` gives. They are the same numbers today; the
    point is that when one of them moves, it moves once.
    """
    g = (catalog or {}).get("gates") or {}
    return (
        g.get("push_min_units", PUSH_MIN_UNITS),
        g.get("push_hero_frac", PUSH_HERO_FRAC),
        g.get("intel_stale_s", COMMIT_INTEL_STALE_S),
        g.get("sighting_ttl_s", SIGHTING_TTL_S),
    )


def freshest_enemy_age(state):
    """Game-seconds since this seat last laid eyes on anything of theirs, or
    `None` if it never has.

    The mirror of `shared::FogGrid::freshest_enemy_age`, read off the same two
    ledgers the engine reads and computed the same way: the freshest of the unit
    sightings and the hero beliefs. Both halves are needed and for the same
    reason — `sightings` is dropped after the TTL, so on its own it can never
    report an age past ninety, and `heroes` keeps its `t_seen` forever, so it is
    the half that can say four hundred seconds.

    Fog-legal by construction: `intel` is this seat's own memory and the only
    place either number comes from.
    """
    intel = state.get("intel") or {}
    ages = [s["age"] for s in (intel.get("sightings") or []) if s.get("age") is not None]
    ages += [
        h["age"]
        for h in (intel.get("heroes") or {}).values()
        if h.get("age") is not None
    ]
    return min(ages) if ages else None

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


def expansion_place(state, catalog):
    """The mine this seat would expand TO, as a name a `region` field takes.

    A FACT, in the sense docs/AFFORDANCES.md guard 1 means: of the gold mines
    the snapshot publishes (`mines[]` is unfiltered map geography — bridge.rs's
    module docstring), keep the ones with gold left, drop the ones a finished
    hall of ours already works, and take the nearest to our own base. Every
    clause is read off the snapshot; none of them is an opinion about when to
    expand or whether to.

    "Already works" is `MINE_HOME_RADIUS` — the same radius `mine_dry` and the
    income alarm use, so the mine that FIRES the `expand` rule is by
    construction not the mine the rule is sent to. That distinction is the
    whole reason this function exists rather than "nearest legal site to the
    mine that triggered": the trigger fires on a hole in the ground with no
    gold in it, and a second TownHall beside it would be 385 gold spent on
    nothing. A default that fires and does the wrong thing is worse than a hole
    that refuses in words.

    Returns `None` when nothing qualifies — every mine dry, or all of them
    already ours. Then the field goes back to being the commander's, which is
    the honest answer: this document does not invent a default it cannot read.

    A NAME and never a coordinate: `intent::resolve_places` looks it up at FIRE
    time, so a rule armed at t=0 aims at the same hole in the ground at t=322
    even though everything else about the match has moved.
    """
    mines = [m for m in state.get("mines") or [] if (m.get("remaining") or 0) > 0
             and m.get("pos")]
    if not mines:
        return None
    home = None
    for p in (state.get("map") or {}).get("places") or []:
        if p.get("name") == "our base" and p.get("pos"):
            home = p["pos"]
    if not home:
        return None
    # A hall is a building that turns out Workers — read off the catalog rather
    # than a kind list, so a race whose hall is called something else, and a
    # tier-up that renames it, are both covered by the same sentence.
    hall_kinds = {
        b.get("id")
        for b in (catalog or {}).get("buildings") or []
        if "Worker" in (b.get("trains") or [])
    }
    halls = [
        b.get("pos")
        for b in own_buildings(state)
        if b.get("done") and b.get("kind") in hall_kinds and b.get("pos")
    ]
    free = [
        m for m in mines
        if not any(dist(m["pos"], h) <= MINE_HOME_RADIUS for h in halls)
    ]
    if not free:
        return None
    # `(distance, name)` so a symmetric map — where the two neutral expansions
    # are exactly equidistant from either base — still renders the same page
    # twice. A document that picked a different mine on each render would be a
    # worse fact than no fact.
    named = [(dist(m["pos"], home), place_name_at(m["pos"], state)) for m in free]
    named = sorted((d, n) for d, n in named if n)
    return named[0][1] if named else None


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


def push_gate_facts(state, props, sid, catalog=None):
    """The three push gates, each as a comparison with both numbers on it.

    Returns `(ready, reason)`. `reason` is written whether or not the gates
    hold: "precondition truth + reason" is one channel, and a link that only
    explains itself when it is refusing teaches nothing on the cycle you needed
    it. Read every clause as a fact — the thresholds are the ENGINE's since
    wc3clone-b9m (`catalog.gates`, see `gates`), the same three it measures an
    acceptance note against, and the commander is free to disagree and send it
    anyway.
    """
    min_units, hero_frac, _, _ = gates(catalog)
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
    elif len(members) < min_units:
        bad.append("size {}/{}".format(len(members), min_units))
    else:
        good.append("size {}/{}".format(len(members), min_units))

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
            h.get("kind", "hero"), 100.0 * frac, 100.0 * hero_frac
        )
        (bad if frac < hero_frac else good).append(clause)
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


def intel_note(state, catalog=None):
    """The staleness line — red's loss at t=490, written down.

    Reads the `intel` ledger and nothing else, so it can only report what this
    seat watched with its own eyes and has not yet forgotten. An EMPTY ledger
    is the loudest reading of the three and gets the loudest sentence: red read
    current sight as ground truth and walked into seventeen troops.

    Since wc3clone-b9m each reading also carries the ENGINE's staleness verdict
    when the picture is past `catalog.gates.intel_stale_s` — the identical
    sentence the acceptance note appends to a commitment sent on that picture.
    The two channels say it in the same words on purpose: a commander that sees
    "past the 45s threshold" here and then reads it again in the echo of its own
    `stance push` is being told one thing twice, not two things once.
    """
    intel = state.get("intel")
    if intel is None:
        return None
    _, _, stale_s, _ = gates(catalog)
    age = freshest_enemy_age(state)
    # One clause, appended to whichever of the three readings applies. The
    # comparison carries both numbers, like every other readiness fact here.
    verdict = (
        "  [{:.0f}s old, past the {:.0f}s threshold the engine notes a commitment "
        "against]".format(age, stale_s)
        if age is not None and age > stale_s
        else ""
    )
    ttl = intel.get("ttl_s")
    groups = intel.get("groups") or []
    if groups:
        g = max(groups, key=lambda x: x.get("size", 0))
        return "last seen: {} troops ({}) {}, {:.0f}s ago — not since{}".format(
            g.get("size", "?"),
            g.get("composition", "composition unknown"),
            g.get("place", "somewhere"),
            g.get("age", 0.0),
            verdict,
        )
    sightings = intel.get("sightings") or []
    if sightings:
        freshest = min(s.get("age", 0.0) for s in sightings)
        return (
            "no enemy FORCE in your ledger — {} loose sighting{}, freshest {:.0f}s old. "
            "A body of troops you have not seen is not a body of troops that is not there{}".format(
                len(sightings), "" if len(sightings) == 1 else "s", freshest, verdict
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


def link(rel, title, ready, reason, command, intel=None, cost=None, note=None,
         sections=()):
    """One LINK: a complete command and the facts about sending it now."""
    a = {
        "kind": "link",
        "rel": rel,
        "sections": list(sections),
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


def field(path, ftype, note, domain=None, rng=None, default=None, required=True):
    """One FORM field.

    `default` is present on every field and is `null` wherever the answer is a
    judgment. AFFORDANCES.md guard 1: a form default may come only from an
    engine fact or from the commander's own earlier declaration, because a
    default that encodes strategy makes the arena measure the form's author.
    Everything else ships empty.

    `required` says which kind of empty it is, and it exists because r34 blue
    could not tell. The document printed `then.squad` and `repeat` with the same
    `null` and the same annotation — "leave null — this one is yours" — but the
    wire takes one of them empty and refuses the other, so the seat sent a
    `home-guard` rule back exactly as printed and got serde's *invalid type:
    null, expected u8* for its trouble. A hole a commander must fill and a hole
    it may leave are different facts, and a form that renders them identically
    is a form whose own convention is a trap.

    THE RULE THIS FIELD PINS (`test_a_printed_template_is_sendable_once_only_
    your_own_fields_are_filled`): fill every `required` hole, leave every other
    one exactly as printed, and the result must be a command the wire takes.
    """
    f = {
        "path": path,
        "type": ftype,
        "note": note,
        "default": default,
        "required": bool(required),
    }
    if domain is not None:
        f["domain"] = domain
    if rng is not None:
        f["range"] = list(rng)
    return f


def form(rel, title, template, fields, ready=True, reason="", slots=None, note=None,
         cost=None, sections=()):
    """One FORM: a template with the judgment-shaped holes left `null`."""
    a = {
        "kind": "form",
        "rel": rel,
        "sections": list(sections),
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
    intel = intel_note(state, catalog)
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
                ready, reason = push_gate_facts(state, props, sid, catalog)
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
                    # `harass` is the one stance word that is also a focus word,
                    # so it belongs to both sections. That is the word matching
                    # the word, not the engine deciding what harassment is.
                    sections=["army", "harass"] if word == "harass" else ["army"],
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
                "the anchor. Leave it null for your own base — on this wire a null key "
                "is an omitted key. The stance's ring is its own: a named region's "
                "radius is ignored here.",
                domain=place_domain(state),
                required=False,
            ),
        ],
        note="`x`/`z` are accepted instead of `target` if you would rather give numbers.",
        sections=["army"],
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
        sections=["army"],
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
                field("repeat", "number",
                      "cooldown in game seconds. Leave it null and the rule fires once — a "
                      "null key is an omitted key on this wire.",
                      required=False),
            ],
            ready=room,
            reason=slots
            + ("" if room else " — re-use one of those names to replace a rule in place, "
                                "or `trigger_clear` one first"),
            slots=slots,
            sections=["standing"],
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
                    field("repeat", "number", "as armed; null means it fires once.",
                          default=t.get("repeat"), required=False),
                ],
                reason="status {}{}".format(
                    t.get("status", "?"),
                    ", last fired {:.0f}s".format(t["last_fired"])
                    if t.get("last_fired") is not None
                    else ", never fired",
                ),
                slots=slots,
                note="Re-sending under the same name replaces it in place and costs no slot.",
                sections=["standing"],
            )
        )
        out.append(
            link(
                "trigger_clear:{}".format(name),
                "disarm the trigger '{}'".format(name),
                True,
                "frees one of {} trigger slots".format(MAX_TRIGGERS),
                {"type": "trigger_clear", "name": name},
                sections=["standing"],
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
            sections=["standing"],
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
                sections=["standing"],
            )
        )
        out.append(
            link(
                "region_clear:{}".format(name),
                "forget your region '{}'".format(name),
                True,
                "any rule naming '{}' goes quiet rather than firing on the whole map".format(name),
                {"type": "region_clear", "name": name},
                sections=["standing"],
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
            sections=["standing"],
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
                sections=["standing"],
            )
        )
        out.append(
            link(
                "plan_clear:{}".format(name),
                "drop the plan '{}'".format(name),
                True,
                "frees one of {} plan slots".format(MAX_PLANS),
                {"type": "plan_clear", "name": name},
                sections=["standing"],
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
        # Both, and mechanically so: every farm and every expansion is bought
        # here and so is every tech building, and the `kind` domain is the one
        # place a `requires Keep` row is printed. Splitting one form across two
        # sections would be the engine deciding which of your buildings are
        # "economy".
        sections=["economy", "tech"],
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


def producer_sections(state, trains, with_tech=False):
    """Which sections a producer's forms belong to, read off what it trains.

    A building that turns out Workers is economy; one that turns out anything
    else is army; a hall that does both is both. `with_tech` is set only for
    `train:` — a producer with a row its own `unlocked` currently forbids is
    exactly the tech question ("Knight — NOT AVAILABLE at your tech"), and that
    is a fact about this seat's tech, not an opinion about its plan.
    """
    unlocked = state.get("unlocked") or {}
    out = []
    if any(t == "Worker" for t in trains):
        out.append("economy")
    if any(t != "Worker" for t in trains):
        out.append("army")
    if with_tech and any(unlocked.get(t) is False for t in trains):
        out.append("tech")
    return out or ["other"]


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
                sections=producer_sections(state, trains, with_tech=True),
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
    for kind, held, _idle, trains in producer_kinds(state, catalog):
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
                sections=producer_sections(state, trains),
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
    for kind, held, _idle, trains in producer_kinds(state, catalog):
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
                        "stance the moment it walks out. Left null it is unset — which, "
                        "with no other piece sent, REMOVES the template.",
                        domain=squads or None,
                        rng=(0, 255),
                        required=False,
                    ),
                ],
                reason="{} finished {}, {} already carrying a template".format(
                    held, kind, set_on
                ),
                note="`retreat`, `priority` and `autocast` are the other pieces. WHATEVER YOU "
                     "SEND REPLACES THE WHOLE TEMPLATE — a piece you omit is unset, not kept — "
                     "and a `template` with no pieces at all removes it.",
                sections=producer_sections(state, trains),
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
    for kind, held, _idle, trains in producer_kinds(state, catalog):
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
                sections=producer_sections(state, trains),
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
    #: The one hole in `recipe:expand` that r34 blue left in it. See
    #: `expansion_place` for why this is a fact and not a strategy.
    expand_to = expansion_place(state, catalog)
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
            sections=["army"],
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
            sections=["army"],
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
                    "region": expand_to,
                    "site": "nearest legal site",
                },
            },
            [field("then.region", "place",
                   "which mine to go to." if expand_to is None else
                   "which mine to go to. Pre-filled with the nearest mine that still has "
                   "gold and that no hall of yours already works — a fact off `mines[]`, "
                   "not a plan. Name another and it obeys.",
                   domain=places, default=expand_to)],
            reason="fires once, which is right: you only need telling the first time. "
                   "`nearest legal site` is why this cannot loop on 'site blocked'."
                   + ("" if expand_to else
                      " NOTHING IS PRE-FILLED: every mine is either dry or already yours, "
                      "so `then.region` is a hole you must fill or the rule will spend its "
                      "one fire on a refusal."),
            note="a TownHall costs {}{}. The trigger is free to arm; the gold is charged when "
                 "it fires.".format(
                     hall_price,
                     "" if unlocked.get("TownHall", True) else " and is NOT currently available",
                 ),
            cost=hall_price,
            sections=["economy"],
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
                field("then.target", "place",
                      "what it commits to. Null anchors the push on your own base, which "
                      "is a legal sentence and almost never the one you mean here.",
                      domain=places, required=False),
            ],
            reason="`enemy_hero_down` is what you WATCHED, not what is true — a hero that died "
                   "out of your sight is not in this predicate. Once, because you only get to "
                   "spend that window once.",
            sections=["army", "harass"],
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
            sections=producer_sections(state, producers[0][3]) if producers else ["army"],
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
# Playbooks — the fork, and the pointer that says which fork you face
#
# `catalog.playbooks` (assets/data/playbooks.ron) is a library of declarative
# game-plans: ordered steps, each with a fog-legal `entry`, a command to send, a
# `gate` that says you may move on, ONE authored `why`, a `fail_when` that says
# the step is dead, and two or three authored `exits`. The engine publishes them
# and executes none of them — a commander enacts a step by sending its command
# through the ordinary intent path.
#
# THE ANCHORING CONSTRAINT (docs/AFFORDANCES.md § Playbooks) is what this code
# is shaped by: a step is rendered as a FORK, never as an instruction. Told to
# trust the document, arena/LADDER.md's r28 Haiku did exactly and only what the
# document said, and a plan a model cannot abandon is r21's one-long-wrong-
# continue one level up. So the render always carries the step's own action
# BESIDE its authored alternatives and any ringing alarm, each with its complete
# command and the numbers under it, and the instruction the commander is
# anchored to becomes "choose".
#
# "YOU ARE HERE" is computed here, from this seat's own snapshot, by evaluating
# the same `TriggerWhen` predicates `src/trigger.rs` evaluates. It is a FACT
# about the snapshot and not a bookmark: the pointer is the first step whose
# gate does not hold, so losing an army walks the pointer back to the step that
# builds one. Nothing is remembered between renders, which is also why no wire
# key was needed.
# ---------------------------------------------------------------------------


def playbook_table(catalog):
    """The library, straight from the catalog.

    No fallback list, on `predicate_schemas`' reasoning: a hand copy of authored
    strategy is a second source of truth for the one thing the data file exists
    to keep single. Rendered beside a catalog written before playbooks landed,
    the section simply has nothing to advertise.
    """
    return list((catalog or {}).get("playbooks") or [])


def _fold(name):
    return " ".join(str(name or "").split()).casefold()


def _my_units(state):
    me = state.get("my_team")
    return [u for u in state.get("units") or [] if not me or u.get("team") == me]


def _enemy_units(state):
    """The enemy bodies THIS SEAT CAN SEE, which is the only list there is.

    Fog-honesty by inheritance, not by a check here: `state.json`'s `units[]` is
    already filtered to what this seat's own `FogGrid` admits, so counting it is
    counting exactly what `trigger.rs` counts through `fog.sees`. The actions
    half of this module never reads the enemy at all; the playbook half must,
    because `enemy_sighted` is a predicate a commander armed — and it reads the
    same array the digest's own win-condition line does.
    """
    me = state.get("my_team")
    return [u for u in state.get("units") or [] if me and u.get("team") != me]


def _class_of(kind, catalog):
    """A unit kind's `TargetClass`, from the catalog the engine published."""
    for u in (catalog or {}).get("units") or []:
        if u.get("id") == kind:
            return u.get("class")
    return None


def _supply_of(kind, catalog):
    for u in (catalog or {}).get("units") or []:
        if u.get("id") == kind:
            return u.get("supply") or 0
    return 0


def _circle(name, state):
    """`(pos, radius)` for a place name, or None.

    `map.places` is the engine's `builtin_places` and `regions` is this seat's
    own vocabulary — together they are exactly what `Regions::find` resolves, so
    a name this cannot place is a name the engine could not place either.
    """
    for r in list((state.get("map") or {}).get("places") or []) + own_regions(state):
        if _fold(r.get("name")) == _fold(name) and r.get("pos"):
            return r["pos"], r.get("radius", 0.0)
    return None


def _hall_kinds(catalog):
    """Which building kinds are halls, read off the catalog's own upgrade ladder.

    `shared::is_hall` is derived code, not data, so there is no `is_hall` key to
    read. What there IS is the fact that a hall is the thing that trains Workers
    — true for TownHall/Keep/Castle and for the horde ladder, and false for
    everything else on the board.
    """
    return {
        b.get("id")
        for b in (catalog or {}).get("buildings") or []
        if "Worker" in (b.get("trains") or [])
    }


def _pred_game_time(w, state, catalog):
    now, at = state.get("t", 0.0), float(w.get("at", 0.0))
    return now >= at, "clock {:.0f}s, this asks for {:.0f}s".format(now, at)


def _pred_unit_count(w, state, catalog):
    kind, want = w.get("kind"), int(w.get("count", 1))
    have = sum(1 for u in _my_units(state) if u.get("kind") == kind)
    return have >= want, "{} {}/{}".format(kind, have, want)


def _pred_tier_reached(w, state, catalog):
    tier = (state.get("me") or {}).get("tier", 1)
    want = int(w.get("tier", 1))
    return tier >= want, "tier {}/{}".format(tier, want)


def _heroes(state):
    return [u for u in _my_units(state) if u.get("hero") and u.get("max_hp")]


def _pred_hero_below(w, state, catalog):
    frac = float(w.get("frac", 0.0))
    heroes = _heroes(state)
    if not heroes:
        return False, "you field no living hero, and a dead hero is not a hurt one"
    worst = min(h["hp"] / h["max_hp"] for h in heroes)
    return worst < frac, "your worst hero is at {:.0f}%, this asks below {:.0f}%".format(
        100.0 * worst, 100.0 * frac
    )


def _pred_hero_above(w, state, catalog):
    """NOT the negation of `hero_below`, and the difference decides matches.

    `shared::TriggerWhen::HeroAbove`: with no living hero this is FALSE, exactly
    as `hero_below` is, so a plan that waits for a healed hero does not advance
    over the corpse. And it is ALL heroes, not any.
    """
    frac = float(w.get("frac", 0.0))
    heroes = _heroes(state)
    if not heroes:
        return False, "you field no living hero, and a dead hero is not a healed one"
    worst = min(h["hp"] / h["max_hp"] for h in heroes)
    return worst >= frac, "your worst hero is at {:.0f}%, this asks {:.0f}% or better".format(
        100.0 * worst, 100.0 * frac
    )


def _pred_squad_below(w, state, catalog):
    """Pooled health, and EVERY enrolled body — including a worker.

    Deliberately not `squad_members`, which drops workers because a squad's
    *army* is what the readiness channel is about. `trigger.rs` filters on team
    and squad id only, so a Call-to-Arms worker enrolled into the line counts
    toward the pool the engine measures, and a second reading of one predicate
    would be a second language.
    """
    sid, frac = w.get("id"), float(w.get("frac", 0.0))
    members = [u for u in _my_units(state) if u.get("squad") == sid]
    cur = sum(u.get("hp", 0.0) for u in members)
    mx = sum(u.get("max_hp", 0.0) for u in members)
    if mx <= 0.0:
        return False, "squad {} has no living members — a squad that is gone cannot be hurt".format(sid)
    return cur / mx < frac, "squad {} is pooled at {:.0f}%, this asks below {:.0f}%".format(
        sid, 100.0 * cur / mx, 100.0 * frac
    )


def _sighted(state, catalog, cls, inside=None):
    seen = []
    for u in _enemy_units(state):
        if cls and _fold(_class_of(u.get("kind"), catalog)) != _fold(cls):
            continue
        if inside:
            pos, radius = inside
            if not u.get("pos") or dist(u["pos"], pos) > radius:
                continue
        seen.append(u)
    return seen


def _pred_enemy_sighted(w, state, catalog):
    want = max(int(w.get("count", 1)), 1)
    cls = w.get("class")
    seen = _sighted(state, catalog, cls)
    return len(seen) >= want, "{} enemy {} in sight, this asks for {}".format(
        len(seen), cls or "units", want
    )


def _pred_enemy_in(w, state, catalog):
    want = max(int(w.get("count", 1)), 1)
    cls, region = w.get("class"), w.get("region")
    circle = _circle(region, state)
    if circle is None:
        # The engine goes QUIET on a name it cannot resolve rather than falling
        # back to the whole map, and so does this: an unresolvable name is not a
        # bigger question, it is no question.
        return False, "no place called {!r} on this map, so this asks about nowhere".format(region)
    seen = _sighted(state, catalog, cls, inside=circle)
    return len(seen) >= want, "{} enemy {} inside {}, this asks for {}".format(
        len(seen), cls or "units", region, want
    )


def _pred_enemy_army_seen(w, state, catalog):
    want = max(int(w.get("size", 1)), 1)
    within = w.get("within_s")
    groups = ((state.get("intel") or {}).get("groups")) or []
    live = [g for g in groups if within is None or g.get("age", 0.0) <= float(within)]
    biggest = max((g.get("size", 0) for g in live), default=0)
    return biggest >= want, "your ledger's largest force is {} troops{}, this asks for {}".format(
        biggest, "" if within is None else " seen inside {:.0f}s".format(float(within)), want
    )


def _pred_enemy_hero_down(w, state, catalog):
    cls = w.get("class")
    heroes = ((state.get("intel") or {}).get("heroes")) or {}
    rows = heroes.items() if not cls else [(cls, heroes.get(cls) or {})]
    down = [k for k, v in rows if (v or {}).get("status") == "seen-dying"]
    return bool(down), "believed down: {} (belief, not truth — you have to have watched it)".format(
        ", ".join(down) or "nobody"
    )


def _pred_bounty_spawned(w, state, catalog):
    n = len(state.get("bounties") or [])
    return n > 0, "{} bounty cache{} visible to you".format(n, "" if n == 1 else "s")


def _pred_mine_dry(w, state, catalog):
    halls = [
        b for b in own_buildings(state)
        if b.get("done") and b.get("kind") in _hall_kinds(catalog) and b.get("pos")
    ]
    if not halls:
        return False, "you hold no completed hall, so no mine is yours to lose"
    # `== 0`, not falsiness: a snapshot with no `remaining` key at all is a
    # snapshot that has not told us, and "we were not told" is not "it is dry".
    dry = [
        m for m in state.get("mines") or []
        if m.get("remaining") == 0
        and m.get("pos")
        and any(dist(m["pos"], h["pos"]) <= MINE_HOME_RADIUS for h in halls)
    ]
    return bool(dry), "{} of your halls' mines {} dry".format(
        len(dry), "is" if len(dry) == 1 else "are"
    )


def _pred_supply_capped(w, state, catalog):
    me = state.get("me") or {}
    cap, used = me.get("supply_cap", 0), me.get("supply_used", 0)
    queued = sum(
        _supply_of(k, catalog)
        for b in own_buildings(state)
        for k in (b.get("queue") or [])
    )
    if not cap:
        # A cap of zero is "no completed supply building yet", which is where
        # every team stands on frame one — not "supply blocked". The engine
        # draws the line in the same place and two readings would be two
        # languages.
        return False, "your supply cap is 0, which is 'no base yet' and not 'blocked'"
    return cap - (used + queued) <= 0, "{}/{} supply used with {} more queued".format(
        used, cap, queued
    )


#: Every `when` arm this view can answer from the seat's OWN snapshot, and
#: therefore every arm a playbook may use. The set is pinned in Rust too
#: (`data::PLAYBOOK_PREDICATES`), and the loader refuses a playbook step that
#: reaches outside it — so the two halves cannot drift without the engine
#: refusing to start.
PREDICATE_EVALUATORS = {
    "game_time": _pred_game_time,
    "unit_count": _pred_unit_count,
    "tier_reached": _pred_tier_reached,
    "hero_below": _pred_hero_below,
    "hero_above": _pred_hero_above,
    "squad_below": _pred_squad_below,
    "enemy_sighted": _pred_enemy_sighted,
    "enemy_in": _pred_enemy_in,
    "enemy_army_seen": _pred_enemy_army_seen,
    "enemy_hero_down": _pred_enemy_hero_down,
    "bounty_spawned": _pred_bounty_spawned,
    "mine_dry": _pred_mine_dry,
    "supply_capped": _pred_supply_capped,
}

#: The arms this view deliberately cannot answer, each with the reason, so a
#: predicate that arrives in the engine and lands in neither table fails the
#: cross-check test rather than being silently treated as "unknown". A pointer
#: that moves on a guess is worse than a pointer that refuses to exist.
UNANSWERABLE_PREDICATES = {
    "base_under_attack": (
        "no snapshot key carries when your buildings were last hit, so this view "
        "would have to guess; say it with `enemy_in` on `our base`, which the "
        "snapshot can answer and which names the place as well"
    ),
}


def predicate_truth(when, state, catalog):
    """`(truth, fact)` for one predicate against this seat's own snapshot.

    `truth` is `True`, `False`, or `None` for an arm this view cannot answer —
    and `None` is treated everywhere downstream as "does not hold", said out
    loud. Every read is a `.get` with a default, so a snapshot missing a key
    answers "no" rather than raising.
    """
    if not isinstance(when, dict):
        return None, "no predicate"
    pid = when.get("type")
    fn = PREDICATE_EVALUATORS.get(pid)
    if fn is None:
        return None, UNANSWERABLE_PREDICATES.get(
            pid, "this document has no reading for `{}`".format(pid)
        )
    try:
        return fn(when, state, catalog)
    except Exception as err:  # a document that raises is worse than none at all
        return None, "could not read `{}` from this snapshot ({})".format(pid, err)


def playbook_position(book, state, catalog):
    """Where this seat stands in a plan, as a fact about the snapshot.

    The pointer is **the first step whose gate does not hold**. Every step
    before it has its gate satisfied, so the plan has got that far; if every
    gate holds, the plan is COMPLETE and says so rather than inventing an
    eleventh step. Nothing is remembered between renders, which is what lets the
    pointer walk BACKWARDS — lose the army and the plan says the army step is
    where you are, because it is.
    """
    steps = book.get("steps") or []
    rows = []
    for s in steps:
        ok, fact = predicate_truth(s.get("gate"), state, catalog)
        rows.append({"id": s.get("id"), "title": s.get("title"),
                     "gate_met": bool(ok), "gate_fact": fact})
    index = next((i for i, r in enumerate(rows) if not r["gate_met"]), None)
    return {"steps": rows, "index": index, "of": len(steps)}


def playbook_fork(book, pos, state, catalog, alarms):
    """The current step as 3-4 live options, each carrying its whole command.

    Order is the news: normally the step's own action leads and its authored
    exits follow; when `fail_when` holds the exits come FIRST and the continue
    option is labelled as being taken on a broken assumption. Any ringing alarm
    adds a final option with a `null` command — confirming the reflex is a move,
    and the alarm's own overrides are in the ALARMS section above, which is
    where they belong and where nothing may hide them.
    """
    step = (book.get("steps") or [])[pos["index"]]
    entry_ok, entry_fact = predicate_truth(step.get("entry"), state, catalog)
    fail_ok, fail_fact = predicate_truth(step.get("fail_when"), state, catalog)
    gate_row = pos["steps"][pos["index"]]

    cont = {
        "kind": "continue",
        "title": step.get("title"),
        "command": step.get("action"),
        "why": step.get("why"),
        "note": (
            "the step's own move, on an assumption that has broken"
            if fail_ok
            else "advance when: " + gate_row["gate_fact"]
            if entry_ok
            else "this step is not open yet — " + entry_fact
        ),
    }
    exits = [
        {
            "kind": "exit",
            "title": x.get("title"),
            "command": x.get("command"),
            "why": x.get("why"),
            "note": None,
        }
        for x in step.get("exits") or []
    ]
    options = (exits + [cont]) if fail_ok else ([cont] + exits)
    for a in alarms or []:
        options.append({
            "kind": "alarm",
            "title": "ALARM {}: {}".format(a.get("id") or "?", a.get("fact")),
            "command": None,
            "why": "running default: {} — its overrides are in ALARMS above, and an alarm "
                   "outranks any plan".format(
                       a.get("running_default") or "nothing is standing to answer this"),
            "note": "an alarm fires only after the reflex has; confirming it is a move",
        })
    return {
        "step": step.get("id"),
        "title": step.get("title"),
        "n": pos["index"] + 1,
        "of": pos["of"],
        "entry_met": bool(entry_ok),
        "entry_fact": entry_fact,
        "gate_met": gate_row["gate_met"],
        "gate_fact": gate_row["gate_fact"],
        "invalidated": bool(fail_ok),
        "broken_assumption": fail_fact if fail_ok else None,
        "why": step.get("why"),
        "options": options,
    }


def playbook_entry(state, catalog, prefs, alarms):
    """The PLAYBOOK section: the library, and — if one is declared — the fork.

    Selection rides the `--prefs` file beside `focus` and for the same reason
    (`load_prefs`): a plan a commander chose to follow is a declaration, the
    engine is forbidden to act on it, and inventing a wire verb for a value the
    engine may not read would be a protocol change inside a view. Declared,
    never inferred — the engine has no opinion about which plan you are on, and
    an inferred one would be exactly the opinion this document may not have.
    """
    library = [
        {"id": b.get("id"), "label": b.get("label"), "race": b.get("race"),
         "pitch": b.get("pitch"), "steps": len(b.get("steps") or [])}
        for b in playbook_table(catalog)
    ]
    if not library:
        return None
    want = (prefs or {}).get("playbook")
    entry = {"library": library, "selected": None, "note": None, "fork": None}
    if not want:
        entry["note"] = (
            "no playbook declared — declare {{\"playbook\":\"{}\"}} in your prefs file to "
            "follow one. Every step renders as a fork with its own exits; going off-book is "
            "legal and unflagged.".format(library[0]["id"])
        )
        return entry
    book = next((b for b in playbook_table(catalog) if b.get("id") == want), None)
    if book is None:
        entry["note"] = "no playbook called {!r} — the library holds: {}".format(
            want, ", ".join(b["id"] for b in library)
        )
        return entry
    entry["selected"] = book.get("id")
    race = state.get("my_race")
    if race and book.get("race") and _fold(race) != _fold(book["race"]):
        entry["note"] = (
            "'{}' is written for the {} roster and you are playing {} — its steps name "
            "buildings you cannot put down. Served anyway: this document is a floor, never "
            "a ceiling.".format(book.get("id"), book.get("race"), race)
        )
    pos = playbook_position(book, state, catalog)
    entry["position"] = pos
    if pos["index"] is None:
        entry["note"] = (
            "every gate in '{}' holds — the plan has run out, and from here the whole "
            "vocabulary below is the plan.".format(book.get("id"))
        )
        return entry
    entry["fork"] = playbook_fork(book, pos, state, catalog, alarms)
    return entry


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
         "avoid":  ["turtle"],
         "focus":  "army",
         "playbook": "standard-kingdom"}

    `prefer` and `avoid` are plain substrings matched, case-folded, against
    each action's `rel` and `title`. Nothing here changes a `ready`, a `reason`
    or a `command`: preference reorders the menu and annotates it, and that is
    the entire extent of its power.

    `focus` (2.0) is the same channel used for the same reason. It is one of
    `economy` / `tech` / `army` / `harass`, it EXPANDS that section of the text
    render and leaves every other action on its one line, and it hides nothing:
    the counts and the one-liners stay, and alarms break through it. The engine
    never infers one — absent means the fact-collapsed default. The owner's
    original proposal was engine-inferred phase *filtering*, and it was rejected
    twice over: base/tech/army are concurrent budgets rather than sequential
    states, so the allocation ratio IS the skill being measured and a phase
    model would bless one allocation; and an inferred phase is an opinion, which
    is the one thing this document may not have. Declared, the same idea is a
    fact — the commander's own judgment rendered back at it, and measurable
    against what it then did.

    `playbook` (2.1) rides the same channel for the same reason, one rung up:
    it names one of `catalog.playbooks` and the PLAYBOOK section then serves
    that plan's current step as a fork. Declared, never inferred — the engine
    has no opinion about which plan you are on, an inferred one would be exactly
    the opinion this document may not have, and a name that matches nothing in
    the library is reported in the section rather than dropped.

    An unrecognised focus word is IGNORED and said so in `source`, never
    silently dropped: a commander that thinks it is reading a filtered page and
    is not has been lied to by a view.
    """
    if not path:
        return None
    with open(path) as f:
        raw = json.load(f)
    focus, source = raw.get("focus"), path
    if focus is not None and str(focus).lower() not in FOCUS_WORDS:
        source = "{} (focus {!r} is not one of {} — ignored)".format(
            path, focus, "/".join(FOCUS_WORDS)
        )
        focus = None
    playbook = raw.get("playbook")
    return {
        "doctrine": raw.get("doctrine"),
        "prefer": [str(x).lower() for x in raw.get("prefer") or []],
        "avoid": [str(x).lower() for x in raw.get("avoid") or []],
        "focus": str(focus).lower() if focus else None,
        # NOT case-folded: a playbook id is a key into a data file, and the
        # section says so out loud when it matches nothing rather than guessing
        # which row was meant.
        "playbook": str(playbook) if playbook else None,
        "source": source,
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


def alarm_pointed(alarms):
    """Every `rel` a ringing alarm points at. `alarm:confirm` is not one — it
    is the running default wearing a link's clothes and carries no command."""
    return {
        x["rel"]
        for a in alarms or []
        for x in a.get("actions") or []
        if x.get("command") is not None or x.get("template") is not None
    }


def expands(action, focus, alarms):
    """Whether the TEXT render prints this action in full rather than folded.

    THE RULE, in one sentence: **expansion is what a declared focus buys, and an
    alarm breaks through it.**

    The fact-collapsed default expands nothing, and that is deliberate rather
    than a budget accident. Every action is still on the page, still carries its
    complete command, and still says what stops it; what it no longer carries is
    the field-by-field domain listing, which is authoring detail. arena/LADDER.md
    Finding 2: every tier of commander read the full document once, at t=0, and
    never again, because ~600 lines is uneconomical at a 15-second cadence. A
    page nobody re-opens has no annotations, however good they are (Finding 5).

    A declared focus expands its own section — the commander asked for the
    detail there, so it gets today's render for it. Anything a ringing alarm
    points at expands too whenever a focus is declared, so a focus can never be
    the reason the fork the alarm just named arrived folded. `--all` expands
    everything and is exempt from all of this.
    """
    if not focus:
        return False
    return focus in (action.get("sections") or []) or action["rel"] in alarm_pointed(alarms)


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

    focus = (prefs or {}).get("focus")
    for a in actions:
        a["collapsed"] = not expands(a, focus, alarms)

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
        # Between the alarms and the actions, and that order is the fork the
        # seat faces: what silence does, then what changed under it, then which
        # plan step you are on, then the whole vocabulary. Alarms stay on top
        # unconditionally — a plan that could sit above the fact that just broke
        # it would be the soft enforcement this section exists to refuse.
        "playbook": playbook_entry(state, catalog, prefs, alarms),
        "actions": actions,
        "preference": {
            "doctrine": (prefs or {}).get("doctrine"),
            "prefer": (prefs or {}).get("prefer") or [],
            "avoid": (prefs or {}).get("avoid") or [],
            "focus": focus,
            "playbook": (prefs or {}).get("playbook"),
            "source": (prefs or {}).get("source")
            or "none — fact order, fact-collapsed, no declared focus",
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
                # Three annotations, not two. "The engine filled this",
                # "you MUST fill this" and "you MAY fill this" are three
                # different instructions, and the old rendering gave the last
                # two the same words — which is how r34 blue sent `home-guard`
                # back with `"squad": null` still in it. See `field`.
                " default={}".format(json.dumps(f["default"]))
                if f["default"] is not None
                else " (REQUIRED — yours to fill; the null is a hole, not a value)"
                if f.get("required", True)
                else " (optional — leave it null and the key is simply omitted)",
            )
            out.append(pad + head)
            out += [pad + "    " + x for x in _wrap(f["note"], width, "  ")]
            if f.get("domain"):
                out.append(pad + "    domain:")
                for d in f["domain"]:
                    out += [pad + "      - " + x for x in _wrap(d, width, "  ")]
    return out


def _compact(obj):
    """A command as a commander pastes it: no spaces to pay for.

    The full render keeps `json.dumps`'s default spacing because it has room;
    a folded line does not, and forty of them is a paragraph of separators.
    Both spellings parse to the same object, which is the only promise a
    rendered command makes.
    """
    return json.dumps(obj, separators=(",", ":"))


def _clip(text, limit):
    """Prose, cut on a word boundary. Never used on a command or a template —
    a clipped JSON object is not a command, it is a paste that fails."""
    text = str(text or "").strip()
    if len(text) <= limit:
        return text
    return text[: limit - 1].rsplit(" ", 1)[0] + "…"


def _blocking_half(reason, limit=240):
    """The clauses that STOP an action, without the ones that do not.

    `push_gate_facts` deliberately writes both halves — the gates that failed,
    then a trailing `(met: …)` for the ones that passed — because a link that
    only explains itself when refusing teaches nothing on the cycle you needed
    it. On a folded line the failed half is the whole news; the met half is one
    `--all` away and unchanged.
    """
    return _clip(str(reason or "").split(" (met:")[0], limit)


def collapse_action(a):
    """One action, folded onto ONE line that is still enough to act on.

    What survives the fold, and why each one:

    * the `rel`, so `--all` and every test can find the same action again;
    * the title, which is the only prose a scanning reader gets;
    * **the complete command or template**, so rung 2's promise — "send it back
      verbatim" — survives the collapse. A folded menu that made you re-open the
      page to get the JSON would have compressed the wrong half;
    * which fields are yours (`you fill:`), the form's judgment-shaped holes;
    * the collection's slot pressure, because "7 of 8 trigger names in use" is
      the fact that changes what you write, not decoration;
    * `BLOCKED:` and the failing clauses for anything NOT READY. This is the
      line arena/LADDER.md Finding 5 is about: r26 red committed 13 units into
      12 defenders with the push gates and the staleness warning served, on a
      page it had not re-opened since t=0;
    * the intel ledger, where the action carries one. It rides free — same line,
      more characters — and it is the sentence a commander lost a match for not
      having.

    What does not survive: the field-by-field notes, the served domains, and the
    per-action `note`. All authoring detail, all one `--all` away, none of it
    deleted from `--json`.
    """
    bits = [a["rel"]]
    if a.get("title"):
        # An edit form's title is a READBACK of the thing it edits — a whole
        # armed trigger's sentence, sometimes 200 characters of it — and the
        # same content is in the template on this very line, structured. So the
        # prose half is clipped and the machine half is not.
        bits.append(_clip(a["title"], 120))
    if a["kind"] == "link":
        bits.append(_compact(a["command"]))
    else:
        bits.append(_compact(a["template"]))
        # The REQUIRED holes only. An optional one left as printed is a legal
        # command (the wire reads a null key as an omitted key), so folding it
        # onto this line as something you must do would be the fold telling a
        # different story from the form — and the form is right.
        open_fields = [
            f["path"] for f in a.get("fields") or []
            if f.get("default") is None and f.get("required", True)
        ]
        if open_fields:
            bits.append("you fill: " + ", ".join(open_fields))
    if a.get("slots"):
        bits.append(a["slots"])
    if a.get("cost"):
        bits.append("cost " + str(a["cost"]))
    if not a["ready"]:
        bits.append("BLOCKED: " + _blocking_half(a.get("reason")))
    if a.get("intel"):
        bits.append("intel: " + a["intel"])
    return " · ".join(bits)


def group_sections(actions, focus=None):
    """`[(section, [action, …]), …]` — the collapsed render's grouping.

    An action lands in the FIRST of its sections that `SECTION_ORDER` names, so
    the grouping is a partition and no action is printed twice. A declared focus
    jumps its own section to the front; otherwise the order is fixed, because
    the same snapshot must always render the same page.
    """
    order = [s for s in SECTION_ORDER if s != focus]
    if focus:
        order.insert(0, focus)
    out = []
    for sec in order:
        rows = [
            a
            for a in actions
            if next((s for s in SECTION_ORDER if s in (a.get("sections") or [])), "other") == sec
        ]
        if rows:
            out.append((sec, rows))
    return out


def render_document(doc, full=False):
    """The whole document as text — fact-collapsed by default, all of it under
    `full`.

    The information hierarchy, outermost first, is the fork the seat actually
    faces: what silence does, then what changed under it, then what it can say.
    `DEFAULT` therefore stays ahead of `ALARMS` and `ACTIONS` in both modes —
    silence is rung 1 and it must be the first option a reader meets, not a
    footnote under forty of them.

    `full=True` is `--doc --all` and restores 1.3's render exactly: the same
    ACTIONS heading, the same order (`order_actions`), the same `render_action`
    for every action, nothing folded and nothing grouped. It is the reason the
    collapse is allowed to be aggressive — no fact left the document, only the
    default page.
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

    lines += _render_playbook(doc.get("playbook"))
    lines += _render_actions(doc, full)
    lines += ["", "RAW (rung 4)"]
    lines += ["  " + x for x in _wrap(doc["raw"], 100, "  ")]
    return lines


def _playbook_option_line(i, opt):
    """One option of the fork, folded like every other line on this page: the
    tier word, the title, the COMPLETE command, and the reason.

    The command is never clipped — a clipped command is not a command — and the
    reason is, at a length that still carries a whole sentence. `--all` does not
    expand this: a fork is already the short form of itself, and the point of
    the section is that it is readable at loop cadence.
    """
    tier = {"continue": "CONTINUE", "exit": "EXIT", "alarm": "ALARM"}.get(opt["kind"], "?")
    bits = ["{} {}".format(i, tier)]
    # The continue option's title and reason ARE the step's, printed three lines
    # above; repeating them here is the one thing a folded page cannot afford.
    # `--json` keeps both, because a parser pays no line cost.
    if opt["kind"] != "continue":
        bits.append(_clip(opt.get("title"), 90))
    bits.append("send nothing" if opt.get("command") is None else _compact(opt["command"]))
    if opt.get("note"):
        bits.append(_clip(opt["note"], 110))
    if opt.get("why") and opt["kind"] != "continue":
        bits.append("why: " + _clip(opt["why"], 300))
    return "    " + " · ".join(b for b in bits if b)


def _render_playbook(pb):
    """The PLAYBOOK section — one line of library, or one fork.

    THE RULE, and the only one that matters: what gets printed is always a set
    of live options, never a single next action. A step whose `fail_when` holds
    is re-rendered INVALIDATED with the broken assumption named and the exits
    moved to the top, alarm-style — anchoring is broken by interrupts, never by
    disclaimers (docs/AFFORDANCES.md § Playbooks).
    """
    if not pb:
        return []
    lines = [""]
    fork = pb.get("fork")
    if not fork:
        lines.append("PLAYBOOK {}".format(
            "'{}' selected".format(pb["selected"]) if pb.get("selected") else "none declared"))
        for b in pb["library"]:
            lines.append("  playbooks: {} ({}, {} steps) — {}".format(
                b["id"], b["race"], b["steps"], b["pitch"]))
        if pb.get("note"):
            lines += ["  " + x for x in _wrap(pb["note"], 100, "  ")]
        return lines

    lines.append("PLAYBOOK {} · step {}/{}{} — {}".format(
        pb["selected"], fork["n"], fork["of"],
        " INVALIDATED" if fork["invalidated"] else "", fork["title"]))
    if fork["invalidated"]:
        lines += ["  " + x for x in _wrap(
            "broken assumption: " + (fork["broken_assumption"] or "?"), 100, "  ")]
    else:
        lines.append("  you are here: {}".format(
            "this step is OPEN — " + fork["entry_fact"] if fork["entry_met"]
            else "NOT OPEN YET — " + fork["entry_fact"]))
        lines.append("  advance when: {} — {}".format(
            "MET" if fork["gate_met"] else "NOT YET", fork["gate_fact"]))
    if pb.get("note"):
        lines += ["  " + x for x in _wrap(pb["note"], 100, "  ")]
    lines += ["  " + x for x in _wrap("why: " + (fork["why"] or ""), 100, "  ")]
    lines.append(
        "  the fork ({} options — choose one; the exits are first because the step's own "
        "assumption failed)".format(len(fork["options"]))
        if fork["invalidated"] else
        "  the fork ({} options — choose one. Off-book is legal and unflagged, and silence "
        "runs the DEFAULT above)".format(len(fork["options"])))
    for i, opt in enumerate(fork["options"], 1):
        lines.append(_playbook_option_line(i, opt))
    return lines


def _preference_lines(pref, focus):
    """What the commander declared, said back to it.

    The `source` is reported whenever there IS one, even when nothing usable
    came out of the file — that is where `load_prefs` puts "focus 'macro' is not
    one of …", and a commander that believes it is reading a focused page while
    reading the default one has been lied to by a view.
    """
    lines = []
    if pref.get("doctrine"):
        lines.append("  your declared doctrine: {} (from {})".format(
            pref["doctrine"], pref["source"]))
    if focus:
        lines += ["  " + x for x in _wrap(
            "your declared focus: {} — that section is rendered in full below, everything else "
            "stays folded, and an alarm breaks through it. Declared by you in {}; the engine "
            "never infers one.".format(focus, pref["source"]), 100, "  ")]
    elif not pref.get("doctrine") and not str(pref.get("source", "")).startswith("none"):
        lines += ["  " + x for x in _wrap(
            "no focus declared, so this page is fact-collapsed (from {})".format(pref["source"]),
            100, "  ")]
    return lines


def _render_actions(doc, full):
    pref = doc["preference"]
    focus = pref.get("focus")
    actions = doc["actions"]

    if full:
        # 1.3's section, word for word. `--all` is a promise that nothing about
        # the old page moved, and a reworded heading would be a small lie in the
        # one mode whose whole point is that it is not a new render.
        lines = ["", "ACTIONS ({} — rungs 2 and 3; sorted {})".format(
            len(actions),
            "by your declared doctrine, then by fact" if pref["doctrine"] or pref["prefer"]
            else "by fact only (no doctrine declared)",
        )]
        if pref["doctrine"]:
            lines.append("  your declared doctrine: {} (from {})".format(
                pref["doctrine"], pref["source"]))
        if focus:
            lines.append("  your declared focus: {} — ignored here; `--all` expands "
                         "everything".format(focus))
        for a in actions:
            lines += render_action(a)
        return lines

    ready = [a for a in actions if a["ready"]]
    lines = ["", "ACTIONS ({}: {} ready, {} blocked — folded; `--doc --all` for every field, "
                 "domain and reason)".format(
                     len(actions), len(ready), len(actions) - len(ready))]
    lines += _preference_lines(pref, focus)

    expanded = [a for a in actions if not a.get("collapsed", True)]
    folded = [a for a in actions if a.get("collapsed", True)]

    if expanded:
        lines.append("")
        lines.append("  IN FULL ({}) — your declared focus '{}', plus anything an alarm "
                     "points at".format(len(expanded), focus))
        for a in expanded:
            lines += render_action(a)

    fready = [a for a in folded if a["ready"]]
    if fready:
        lines.append("  READY ({}) — the command as printed is complete".format(len(fready)))
        for sec, rows in group_sections(fready, focus):
            lines.append("    {} ({})".format(sec, len(rows)))
            lines += ["      " + collapse_action(a) for a in rows]

    fblocked = [a for a in folded if not a["ready"]]
    if fblocked:
        lines.append("")
        lines.append("  NOT READY ({}) — listed and still sendable; NOT READY is a fact the "
                     "engine can measure, never a refusal".format(len(fblocked)))
        lines += ["    " + collapse_action(a) for a in fblocked]
    return lines


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv=None):
    ap = argparse.ArgumentParser(description="the hypermedia affordance document")
    ap.add_argument("path", nargs="?", default="bridge/red/state.json")
    ap.add_argument("--json", action="store_true", help="the document as JSON")
    ap.add_argument(
        "--all",
        action="store_true",
        dest="all_actions",
        help="every action in full — the pre-2.0 render. The default folds each "
        "action onto one line that still carries its command",
    )
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
        # `--json` is never collapsed: a machine reader pays no line cost, so
        # the compression it would buy is a fact it would lose.
        print(json.dumps(doc, indent=2))
    else:
        for line in render_document(doc, full=args.all_actions):
            print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
