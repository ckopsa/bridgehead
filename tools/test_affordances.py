#!/usr/bin/env python3
"""Tests for the hypermedia affordance document — `bridge_view.py --doc`.

    python3 tools/test_affordances.py        # standalone; pytest optional

The properties half is `tools/test_bridge_view.py`'s problem. This suite is
about the actions half, and the properties it checks are the ones that would be
expensive to discover in an arena round:

* **Every transition is listed.** All five stance words for every squad,
  whether or not they are ready. A menu that hides the option a commander
  needed is AFFORDANCES.md constraint 1 failing quietly.
* **A refusal teaches.** A NOT-READY link carries both sides of every
  comparison it failed, in numbers.
* **Nothing is frozen.** No template and no command contains an entity id.
  Freezing an id into a rendered link would automate the r21/r23 staleness
  failure class this whole design exists to kill.
* **It is fog-legal by construction.** The actions half never reads the enemy's
  `units[]` or `buildings[]`, so an omniscient snapshot renders the same
  actions as a blind one.
* **It is a view.** `state.json` is byte-identical before and after.
* **It degrades.** The empty dict, and the repo's pre-everything fixture.

The live fixtures were captured from real headless matches (`BH_BRIDGE=red` +
`autopilot`, tools/BUILDER_BRIEF.md A.5b) with real regions, real armed
triggers, a running plan, a stanced squad and — in one of them — a ringing
alarm. A document tested only against snapshots its author invented is a test
of the author's imagination.
"""

from __future__ import annotations

import atexit
import copy
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import affordances  # noqa: E402
import bridge_view  # noqa: E402

HERE = Path(__file__).resolve().parent
TOOL = HERE / "bridge_view.py"
FIX = HERE / "fixtures"

#: t=236s: two squads with living members, squad 1 stanced `stage` at mid, two
#: named regions, two armed triggers, a running plan, a real intel ledger.
ARMED = FIX / "doc_open_armed.json"
#: t=388s: the same seat with an `income_collapse` alarm ringing.
ALARM = FIX / "doc_open_alarm.json"
#: t=8s: five workers, no army, nothing scouted.
EARLY = FIX / "digest_open_early.json"
#: The pre-`intel`, pre-`my_race`, pre-everything snapshot this repo has always
#: carried.
LEGACY = FIX / "legacy_crossings.json"
#: `catalog.json` exactly as the engine writes it, `stances` and `selectors`
#: included.
CATALOG = FIX / "catalog_full.json"

LIVE = [ARMED, ALARM]


def load(path):
    with open(path) as f:
        return json.load(f)


def catalog():
    return load(CATALOG)


def doc(path=ARMED, prefs=None, cat=True):
    return affordances.document(load(path), catalog() if cat else None, prefs)


def run(*args):
    out = subprocess.run(
        [sys.executable, str(TOOL), *args], capture_output=True, text=True, timeout=60
    )
    assert out.returncode == 0, out.stderr
    return out.stdout


def rels(d):
    return [a["rel"] for a in d["actions"]]


def by_rel(d, rel):
    return next(a for a in d["actions"] if a["rel"] == rel)


def walk(node):
    """Every scalar in a nested structure, so a test can look for ids."""
    if isinstance(node, dict):
        for k, v in node.items():
            yield k
            yield from walk(v)
    elif isinstance(node, list):
        for v in node:
            yield from walk(v)
    else:
        yield node


# -- the document's shape ----------------------------------------------------


def test_the_document_has_the_five_top_level_sections():
    d = doc()
    for key in ("doc_version", "seq", "properties", "default", "alarms", "actions"):
        assert key in d, "no {} section".format(key)
    assert d["seq"] == load(ARMED)["seq_applied"]
    assert d["properties"] == bridge_view.digest(load(ARMED), catalog()), \
        "the properties section IS digest(), embedded rather than re-derived"


def test_the_document_carries_its_own_version():
    """AFFORDANCES.md constraint 3: the scaffold's version travels with the
    result, or the ledger cannot tell model from model+scaffold."""
    # `2.0`: the SHAPE moved (fact-collapsed render, declared focus), and the
    # gates moved to `catalog.gates` with the acceptance-notes channel beside
    # them (wc3clone-b9m). Neither half ever played a round alone, so the
    # ledger sees one scaffold. A ledger row that could not tell the ~600-line
    # page from the ~76-line one would be comparing two different experiments.
    assert affordances.DOC_VERSION == "affordance-doc/2.0"
    assert doc()["doc_version"] == affordances.DOC_VERSION
    assert run("--doc-version").strip() == affordances.DOC_VERSION
    assert subprocess.run(
        [sys.executable, str(HERE / "affordances.py"), "--version"],
        capture_output=True, text=True, timeout=60,
    ).stdout.strip() == affordances.DOC_VERSION


def test_the_running_default_is_a_first_class_element_with_a_null_command():
    """Rung 1 of the ladder. Silence is a move, so it gets an element."""
    d = doc()
    assert d["default"]["command"] is None
    assert d["default"]["title"] == d["properties"]["default"]
    assert d["default"]["title"], "the document must always say what silence does"


def test_the_document_says_the_full_vocabulary_is_still_open():
    """Constraint 1, in the document itself: a floor, never a ceiling."""
    assert "floor, never a ceiling" in doc()["raw"]
    assert "floor, never a ceiling" in "\n".join(
        affordances.render_document(doc())
    )


# -- every transition, always ------------------------------------------------


def test_every_stance_transition_is_listed_for_every_squad():
    for path in LIVE:
        state = load(path)
        d = affordances.document(state, catalog())
        words = [r["id"] for r in affordances.stance_table(catalog())]
        assert len(words) == 5
        for sq in state["squads"]:
            for word in words:
                rel = "stance:squad-{}:{}".format(sq["id"], word)
                assert rel in rels(d), "{}: {} is missing".format(path.name, rel)


def test_a_not_ready_transition_is_still_listed_and_still_sendable():
    """Advisory, never blocking (AFFORDANCES.md constraint 1)."""
    d = doc(ARMED)
    push = by_rel(d, "stance:squad-0:push")
    assert push["ready"] is False
    assert push["command"] == {
        "type": "stance", "squad": 0, "stance": "push", "target": "home"
    }, "a refused option still carries a complete, valid command"


def test_the_stance_a_squad_already_holds_is_listed_as_a_re_apply():
    """Re-sending is how a stance's leash and focus reach the units that joined
    since (COMMANDER_BRIEF, Stances note 3) — a real command, not a no-op."""
    d = doc(ARMED)
    assert "re-apply" in by_rel(d, "stance:squad-1:stage")["title"]
    assert "re-apply" not in by_rel(d, "stance:squad-1:push")["title"]


# -- readiness is a fact with numbers on it ----------------------------------


def test_a_not_ready_link_names_both_sides_of_every_comparison():
    d = doc(ARMED)
    why = by_rel(d, "stance:squad-1:push")["reason"]
    assert "push gates" in why
    # squad 1 holds six of the seat's nine army units: size passes, the
    # consolidation gate does not, and BOTH halves are printed.
    assert "6/6" in why, why
    assert re.search(r"3 of your 9 army units are outside squad 1", why), why
    assert "92%, gate is 80%" in why, why


def test_a_met_gate_is_reported_as_a_fact_too():
    """"Precondition truth + reason" is one channel — a link that only explains
    itself when refusing teaches nothing on the cycle you needed it."""
    s = load(ARMED)
    me = s["my_team"]
    for u in s["units"]:
        if u["team"] == me and u["kind"] != "Worker":
            u["squad"] = 1
            u["hp"] = u["max_hp"]
    s["squads"] = [q for q in s["squads"] if q["id"] == 1]
    d = affordances.document(s, catalog())
    push = by_rel(d, "stance:squad-1:push")
    assert push["ready"] is True
    assert push["reason"].startswith("push gates met:")
    assert "consolidated (9/9 of your army)" in push["reason"]


def test_an_empty_squad_is_caught_at_authoring_time():
    """r21's acceptance criterion. `"units":[]` fired as "move 0 units" and the
    hero died three seconds later; here the emptiness is a NOT READY reason on
    the option itself, before anything is armed."""
    s = load(ARMED)
    for u in s["units"]:
        u["squad"] = None
    for q in s["squads"]:
        q["members"] = 0
    d = affordances.document(s, catalog())
    for word in ("turtle", "stage", "push", "secure", "harass"):
        a = by_rel(d, "stance:squad-0:{}".format(word))
        assert a["ready"] is False, word
        assert "no members" in a["reason"], a["reason"]
    assert "squad" in rels(d), "the document offers the enrolment that fixes it"


def test_the_engine_never_recommends():
    """Constraint 4: ready / cost / staleness / ETA are facts; "best move" is
    an opinion and is forbidden. This is a crude guard and it is meant to be —
    the words it bans are the ones an author reaches for by accident."""
    text = "\n".join(affordances.render_document(doc(ALARM))).lower()
    for banned in ("best move", "you should", "recommend", "optimal", "we suggest"):
        assert banned not in text, banned


# -- intel staleness ---------------------------------------------------------


def test_an_offensive_transition_carries_the_intel_ledger_not_current_sight():
    """Red's r23 loss: it read current sight as ground truth at t=490."""
    d = doc(ARMED)
    note = by_rel(d, "stance:squad-1:push")["intel"]
    groups = load(ARMED)["intel"]["groups"]
    biggest = max(groups, key=lambda g: g["size"])
    assert "{} troops".format(biggest["size"]) in note
    assert "not since" in note


def test_an_empty_ledger_is_the_loudest_reading_of_the_three():
    s = load(ARMED)
    s["intel"] = {"sightings": [], "groups": [], "heroes": {}, "ttl_s": 90.0}
    note = affordances.intel_note(s)
    assert "EMPTY" in note and "90s" in note
    assert "not the same as their having none" in note


def test_a_snapshot_with_no_ledger_at_all_gets_no_intel_line():
    s = load(LEGACY)
    assert "intel" not in s
    assert affordances.intel_note(s) is None
    d = affordances.document(s, catalog())
    assert all("intel" not in a for a in d["actions"])


# -- one set of numbers, two renderings (wc3clone-b9m) -----------------------


def test_the_gates_come_from_the_catalog_when_the_engine_publishes_them():
    """The engine writes an acceptance note against `catalog.gates`; this
    document annotates its `push` links against the same block. Two renderings
    of one rule that can disagree is the failure docs/FOG.md is written
    against — nothing errors, they just say different things about one squad.

    Both directions are asserted: a published block WINS, and a catalog written
    before the block existed falls back to the module constants rather than
    raising, exactly as `STANCE_FALLBACK` does.
    """
    cat = catalog()
    assert "gates" not in cat, "the fixture predates the block — that is the fallback case"
    assert affordances.gates(cat) == (
        affordances.PUSH_MIN_UNITS,
        affordances.PUSH_HERO_FRAC,
        affordances.COMMIT_INTEL_STALE_S,
        affordances.SIGHTING_TTL_S,
    )
    assert affordances.gates(None) == affordances.gates({})

    # An engine that moved the size gate moves this document with it.
    cat["gates"] = {
        "push_min_units": 3,
        "push_hero_frac": 0.5,
        "intel_stale_s": 20.0,
        "sighting_ttl_s": 90.0,
    }
    s = load(ARMED)
    props = bridge_view.digest(s, cat)
    sid = s["squads"][0]["id"]
    _, reason = affordances.push_gate_facts(s, props, sid, cat)
    assert "/3" in reason, "the served gate must be the engine's: {}".format(reason)
    assert "/6" not in reason, "the module constant must not survive a published one"


def test_a_stale_picture_names_the_threshold_the_engine_notes_against():
    """The document and the echo say it in the same words.

    A commander that reads "past the 45s threshold" here and then reads it
    again in the echo of its own `stance push` is being told one thing twice,
    which is the point: the annotation was already right at r26/r27 and it was
    the READING that failed, not the fact.
    """
    s = load(ARMED)
    s["intel"] = {
        "sightings": [{"id": 1, "age": 190.0}],
        "groups": [{"size": 11, "composition": "8 Footman, 3 Archer",
                    "place": "near the center ford", "age": 190.0}],
        "heroes": {},
        "ttl_s": 90.0,
    }
    note = affordances.intel_note(s, catalog())
    assert "190s ago" in note, "the reading itself is unchanged: {}".format(note)
    assert "190s old, past the 45s threshold" in note, note

    # ...and a fresh picture names no threshold at all. A line that fires on
    # every cycle is a line a commander learns to skip.
    s["intel"]["sightings"][0]["age"] = 12.0
    s["intel"]["groups"][0]["age"] = 12.0
    fresh = affordances.intel_note(s, catalog())
    assert "threshold" not in fresh, fresh


def test_the_freshest_age_reads_both_ledgers_the_way_the_engine_does():
    """`shared::FogGrid::freshest_enemy_age`, mirrored.

    Both halves are load-bearing. `sightings` is dropped after the TTL, so on
    its own it can never report an age past ninety; `heroes` keeps its `t_seen`
    forever, so it is the only half that can say four hundred seconds.
    """
    assert affordances.freshest_enemy_age({}) is None
    assert affordances.freshest_enemy_age(
        {"intel": {"sightings": [], "groups": [], "heroes": {}}}
    ) is None
    # Hero-only memory: an age the sightings ledger could never express.
    assert affordances.freshest_enemy_age(
        {"intel": {"sightings": [], "heroes": {"Hero": {"status": "alive", "age": 400.0}}}}
    ) == 400.0
    # The freshest of the two wins, whichever ledger it is in.
    both = {"intel": {
        "sightings": [{"id": 1, "age": 70.0}, {"id": 2, "age": 30.0}],
        "heroes": {"Hero": {"status": "alive", "age": 400.0}},
    }}
    assert affordances.freshest_enemy_age(both) == 30.0
    # A hero never seen carries no `age` key and must not be read as age zero.
    assert affordances.freshest_enemy_age(
        {"intel": {"sightings": [{"id": 1, "age": 55.0}],
                   "heroes": {"Hero": {"status": "unknown"}}}}
    ) == 55.0


# -- fog-legality ------------------------------------------------------------


def test_the_actions_half_never_reads_the_enemy():
    """The adversarial test. Hand the renderer an omniscient snapshot — the
    enemy's whole army and whole base, none of it in the intel ledger — and the
    actions and the alarms must come out byte-identical. Not a visibility check
    at each call site: the enemy's rosters are simply not an input here
    (tools/BUILDER_BRIEF.md §6.10).
    """
    for path in LIVE:
        honest = load(path)
        omniscient = copy.deepcopy(honest)
        enemy = "Human" if honest["my_team"] == "Claude" else "Claude"
        omniscient["units"] += [
            {"id": 900000 + i, "team": enemy, "kind": "Knight", "pos": [i, i],
             "hp": 100.0, "max_hp": 100.0, "order": "Idle", "moving": False,
             "carrying": False, "hero": None, "squad": None}
            for i in range(40)
        ]
        omniscient["buildings"] += [
            {"id": 800000 + i, "team": enemy, "kind": "Barracks", "pos": [i, -i],
             "hp": 700.0, "max_hp": 700.0, "done": True, "queue": [], "tier": 1}
            for i in range(9)
        ]
        a = affordances.document(honest, catalog())
        b = affordances.document(omniscient, catalog())
        assert json.dumps(a["actions"]) == json.dumps(b["actions"]), path.name
        assert json.dumps(a["alarms"]) == json.dumps(b["alarms"]), path.name


def test_the_place_domain_is_public_geography_plus_this_seats_own_circles():
    """A place field's domain must be knowable. `map.places` is public — the
    opponent reads the same list — and `regions` are this seat's own words,
    which appear in no other snapshot."""
    s = load(ARMED)
    domain = affordances.place_domain(s)
    names = [d.split(" — ")[0] for d in domain]
    assert names[: len(s["map"]["places"])] == [p["name"] for p in s["map"]["places"]]
    for r in s["regions"]:
        assert r["name"] in names
        assert any(d.startswith(r["name"] + " — YOUR region") for d in domain)
    assert len(names) == len(s["map"]["places"]) + len(s["regions"])


def test_a_kind_domain_serves_this_seats_own_roster_and_its_own_tech():
    """The catalog describes both races; a seat may only build one of them.
    Availability comes from `unlocked`, which is this team's own tech."""
    d = doc(ARMED)
    kinds = next(f for f in by_rel(d, "build")["fields"] if f["path"] == "kind")
    served = [row.split(" — ")[0] for row in kinds["domain"]]
    assert "Barracks" in served and "Farm" in served
    assert "WarCamp" not in served, "the horde roster is not this kingdom seat's"
    assert "Keep" not in served, "an upgrade-only kind is not something a worker places"
    blacksmith = next(r for r in kinds["domain"] if r.startswith("Blacksmith"))
    assert "NOT AVAILABLE: requires Keep" in blacksmith
    assert "140g/80l" in blacksmith, "the price is a fact and rides along with the refusal"


def test_production_is_reachable_without_an_entity_id():
    """The gap 3ji closed. `train` took a building id and no selector channel
    covered buildings, so the verb a commander sends every cycle was the one it
    had to hand-write from `buildings[]`. One form per producer kind now, and
    the producer is a ROLE."""
    d = doc(ARMED)
    rels = [a["rel"] for a in d["actions"] if a["rel"].startswith("train:")]
    assert "train:Barracks" in rels and "train:TownHall" in rels
    assert not any(r == "train:Blacksmith" for r in rels), \
        "a building that trains nothing is not a producer"
    assert not any(r == "train:Sanctum" for r in rels), \
        "only kinds this seat actually has standing"
    hall = by_rel(d, "train:TownHall")
    assert hall["template"] == {
        "type": "train", "select": "idle TownHall", "unit": None,
    }
    assert "building" not in hall["template"], "no id, ever"


def test_the_producer_default_is_a_phrase_that_would_not_refuse():
    """`idle <kind>` when one is free and `my <kind>` when none is. A default
    that refuses as written is not a default, it is a trap — and both readings
    are facts about the seat's own buildings, so neither is advice.

    Both halves off ONE fixture, whose barracks are all busy and whose halls are
    all free, so the two branches are read from real match state."""
    d = doc(ARMED)
    assert by_rel(d, "train:Barracks")["template"]["select"] == "my Barracks"
    assert ", 0 idle" in by_rel(d, "train:Barracks")["reason"]
    assert by_rel(d, "train:TownHall")["template"]["select"] == "idle TownHall"
    assert ", 2 idle" in by_rel(d, "train:TownHall")["reason"]

    s = load(ARMED)
    for b in s["buildings"]:
        if b["team"] == s["my_team"] and b.get("kind") == "Barracks":
            b["queue"] = []
    freed = affordances.document(s, catalog())
    assert by_rel(freed, "train:Barracks")["template"]["select"] == "idle Barracks"


def test_the_other_three_building_verbs_are_served_too():
    """3w9 item 3. The building selector made `rally`, `template` and `cancel`
    sayable without an entity id in round 1, and the document went on serving
    only `train` — so the three verbs a commander could now speak by role were
    the three it had no way to discover."""
    d = doc(ARMED)
    for rel, verb in (("rally:Barracks", "rally"),
                      ("template:Barracks", "template"),
                      ("cancel:Barracks", "cancel")):
        a = by_rel(d, rel)
        assert a["template"]["type"] == verb
        assert a["template"]["select"] == "my Barracks"
        assert "building" not in a["template"], "no id, ever"
    # A producer kind this seat does not hold is not offered, exactly as with
    # `train`.
    assert not any(a["rel"].startswith("rally:Sanctum") for a in d["actions"])


def test_a_cancel_with_nothing_queued_is_listed_and_says_why():
    """AFFORDANCES.md constraint 1: the menu never hides the option. An empty
    queue makes `cancel` refuse with `queue index 0 out of range`, so the form
    is listed NOT-READY with every queue of that kind in the reason."""
    d = doc(ARMED)
    busy, idle = by_rel(d, "cancel:Barracks"), by_rel(d, "cancel:TownHall")
    assert busy["ready"] and "Footman" in busy["reason"]
    assert not idle["ready"], "a hall with an empty queue has nothing to cancel"
    index = next(f for f in busy["fields"] if f["path"] == "index")
    assert index["range"][0] == 0 and index["range"][1] >= 1


def test_a_rally_form_reads_back_the_rally_point_it_would_replace():
    """The `buildings[].rally` key exists so this question has an answer without
    re-sending the verb. `unset` is stated rather than omitted: 'no rally point'
    and 'I could not tell you' are different facts, and the older fixtures say
    the first because they predate the key."""
    d = doc(ARMED)
    assert "rally now: unset, unset" in by_rel(d, "rally:Barracks")["reason"], \
        "two barracks, neither rallied — and it says so twice rather than staying quiet"

    s = load(ARMED)
    for b in s["buildings"]:
        if b["team"] == s["my_team"] and b.get("kind") == "Barracks":
            b["rally"] = {"pos": [55.0, -12.0]}
            break
    told = affordances.document(s, catalog())
    assert "(55, -12)" in by_rel(told, "rally:Barracks")["reason"]


def test_the_unit_domain_prices_every_row_against_this_seats_own_bank():
    """Same three annotations `build`'s `kind` domain carries, from the same two
    sources — and the hero row prices off `me.hero_costs`, which is a match fact
    the catalog cannot know."""
    d = doc(ARMED)
    hall = by_rel(d, "train:TownHall")
    unit = next(f for f in hall["fields"] if f["path"] == "unit")
    served = [row.split(" — ")[0] for row in unit["domain"]]
    assert "Worker" in served and "Hero" in served
    assert "Footman" not in served, "a hall does not train footmen"
    hero = next(r for r in unit["domain"] if r.startswith("Hero"))
    assert "400g/100l" in hero, "the live price, not the catalog's"
    assert "hero slots full (1/1)" in hero, \
        "the gate a commander forgets, stated with both halves"
    assert unit["default"] is None, "what to build is the commander's call"


def test_every_served_when_matches_the_predicate_schema_the_document_serves():
    """r25's lesson: the steady-production recipe shipped a `when` with a
    `below` field no predicate has, and the first commander to trust the
    printed template got 'missing field count' back at fire time. A printed
    recipe must compile, and the document itself serves the schema to check
    against — so every template's `when` is validated here against
    `catalog.predicates`, required fields covered, no invented fields."""
    cat = catalog()
    schema = {p["id"]: p["fields"] for p in cat["predicates"]}
    for fixture in (ARMED, ALARM):
        d = affordances.document(load(fixture), cat)
        for a in d["actions"]:
            when = (a.get("template") or {}).get("when")
            if not isinstance(when, dict):
                continue
            pid = when.get("type")
            assert pid in schema, "{}: unknown predicate {!r}".format(a["rel"], pid)
            legal = {f["name"] for f in schema[pid]}
            sent = set(when) - {"type"}
            assert sent <= legal, "{}: `when` invents fields {} — predicate {} takes {}".format(
                a["rel"], sorted(sent - legal), pid, sorted(legal))
            required = {f["name"] for f in schema[pid] if f.get("required")}
            # A None is a judgment hole the commander fills; the KEY must exist.
            assert required <= sent, "{}: `when` omits required {}".format(
                a["rel"], sorted(required - sent))


def test_the_steady_production_recipe_names_a_role_and_not_a_barracks():
    """The r23-class win: a repeating train rule that finds a live, idle
    producer every time it fires instead of a barracks that may be rubble."""
    d = doc(ARMED)
    a = by_rel(d, "recipe:steady-production")
    then = a["template"]["then"]
    assert then["type"] == "train"
    assert then["select"].startswith("idle ")
    assert "building" not in then
    assert a["template"]["repeat"] == 20, "production is a level, not an event"


def test_a_seat_with_no_producers_still_gets_the_recipe():
    """Degradation: the empty snapshot has no producer kind to default with, so
    the field ships empty and the note says why rather than raising."""
    d = affordances.document({}, catalog())
    a = by_rel(d, "recipe:steady-production")
    sel = next(f for f in a["fields"] if f["path"] == "then.select")
    assert sel["default"] is None
    assert "no finished producer" in a["note"]
    assert not [x for x in d["actions"] if x["rel"].startswith("train:")]


# -- forms: slot pressure, templates, defaults -------------------------------


def test_the_collections_report_real_slot_pressure():
    d = doc(ARMED)
    s = load(ARMED)
    assert by_rel(d, "trigger_set")["slots"] == "{} of 8 trigger names in use".format(
        len(s["triggers"])
    )
    assert by_rel(d, "region_set")["slots"] == "{} of 8 region names in use".format(
        len(s["regions"])
    )
    assert by_rel(d, "plan_set")["slots"] == "1 of 2 plan slots in use"


def test_a_full_collection_is_not_ready_and_says_how_to_proceed_anyway():
    s = load(ARMED)
    s["triggers"] = [
        {"name": "t{}".format(i), "when": {"type": "mine_dry"},
         "then": {"type": "stop", "select": "all army"}, "status": "armed",
         "sentence": "rule {}".format(i)}
        for i in range(affordances.MAX_TRIGGERS)
    ]
    a = by_rel(affordances.document(s, catalog()), "trigger_set")
    assert a["ready"] is False
    assert "8 of 8" in a["reason"] and "replace a rule in place" in a["reason"]


def test_every_existing_resource_gets_an_edit_form_and_a_delete_link():
    d = doc(ARMED)
    s = load(ARMED)
    for t in s["triggers"]:
        edit = by_rel(d, "trigger_set:{}".format(t["name"]))
        assert edit["template"]["when"] == t["when"], "pre-filled with the armed values"
        assert edit["template"]["then"] == t["then"]
        assert by_rel(d, "trigger_clear:{}".format(t["name"]))["command"] == {
            "type": "trigger_clear", "name": t["name"]
        }
    for r in s["regions"]:
        edit = by_rel(d, "region_set:{}".format(r["name"]))
        assert edit["template"]["radius"] == r["radius"]
        assert edit["template"]["x"] == r["pos"][0]
        assert by_rel(d, "region_clear:{}".format(r["name"]))["command"]["name"] == r["name"]
    for p in s["plans"]:
        assert by_rel(d, "plan_set:{}".format(p["name"]))["template"]["steps"] == p["steps"]
        assert by_rel(d, "plan_clear:{}".format(p["name"]))["command"]["name"] == p["name"]


def test_numeric_fields_carry_the_engines_real_ranges():
    d = doc(ARMED)
    radius = next(f for f in by_rel(d, "region_set")["fields"] if f["path"] == "radius")
    assert radius["range"] == [affordances.REGION_RADIUS_MIN, affordances.REGION_RADIUS_MAX]
    assert radius["range"] == [4.0, 60.0], "shared::REGION_RADIUS_MIN/MAX"
    steps = next(f for f in by_rel(d, "plan_set")["fields"] if f["path"] == "steps")
    assert steps["range"] == [1, affordances.MAX_PLAN_STEPS]


def test_judgment_fields_ship_empty_and_fact_fields_ship_filled():
    """AFFORDANCES.md guard 1. A default that encodes strategy makes the arena
    measure the form's author, so a threshold, an anchor and a squad choice all
    arrive `null`; a selector phrase whose meaning IS the fact arrives filled.
    """
    d = doc(ARMED)
    hero_save = by_rel(d, "recipe:hero-save")
    assert hero_save["template"]["when"]["frac"] is None, "a threshold is a judgment"
    assert hero_save["template"]["then"]["region"] is None, "so is where it runs to"
    assert hero_save["template"]["then"]["select"] == "my hero"
    for f in hero_save["fields"]:
        assert f["default"] is None, f
    build = by_rel(d, "build")
    assert build["template"]["kind"] is None and build["template"]["region"] is None
    assert build["template"]["select"] == "workers"
    assert build["template"]["site"] == "nearest legal site"


def test_the_field_domains_come_from_the_engines_own_vocabulary():
    cat = catalog()
    assert [r["id"] for r in affordances.stance_table(cat)] == [
        "turtle", "stage", "push", "secure", "harass"
    ]
    assert affordances.selector_vocabulary(cat)["units"][0] == "my hero"
    # ...and there is a fallback for a document rendered beside an older
    # catalog, on the reasoning `bridge_view.PRODUCTION_KINDS` is there for.
    assert [r["id"] for r in affordances.stance_table(None)] == [
        "turtle", "stage", "push", "secure", "harass"
    ]
    assert affordances.selector_vocabulary(None) == affordances.SELECTOR_FALLBACK


def test_a_unit_field_serves_the_selector_vocabulary_not_a_roster_of_ids():
    """The point of 0uu.1: a commander names a ROLE and stops plumbing ids, so
    the domain of a unit-shaped field is the phrase list, never `units[]`."""
    d = doc(ARMED)
    phrases = affordances.selector_vocabulary(catalog())
    select = next(f for f in by_rel(d, "squad")["fields"] if f["path"] == "select")
    assert select["domain"] == phrases["units"]
    assert "my hero" in select["domain"] and "squad <n>" in select["domain"]
    site = next(f for f in by_rel(d, "build")["fields"] if f["path"] == "site")
    assert site["domain"] == phrases["sites"] == ["nearest legal site"]
    then = next(f for f in by_rel(d, "trigger_set")["fields"] if f["path"] == "then")
    assert "all army" in then["note"], "the phrase list rides with the free-form field too"


def test_a_form_that_spends_says_what_it_costs():
    expand = by_rel(doc(ARMED), "recipe:expand")
    assert expand["cost"], "arming is free; the TownHall it will place is not"
    assert "g/" in expand["cost"]
    assert "charged when it fires" in expand["note"]


def test_the_predicate_domain_matches_the_brief_a_commander_reads():
    """The `when` vocabulary now ships in `catalog.predicates`, so this test
    changed what it is refereeing.

    It used to compare the brief's table against a hand copy in this module —
    the only thing keeping that copy honest, and it earned its keep when
    `hero_above` arrived with stance chains (0uu.6). The copy is gone; the
    catalog is the source. So the two documents a commander might read are what
    is compared: the machine-readable one the engine writes and the prose one it
    is told to consult. A predicate in one and not the other is a commander sent
    to the wrong place, whichever direction it is missing in.

    The heading is matched loosely because it carries the count in words ("the
    fourteen predicates") and that count is exactly the thing that moves.
    """
    brief = (HERE / "COMMANDER_BRIEF.md").read_text()
    m = re.search(r"^### The \w+ predicates$", brief, re.M)
    assert m, "the brief no longer has a predicate table under a '### The N predicates' heading"
    section = brief[m.end():].split("\n###", 1)[0]
    listed = re.findall(r'\{"type":"(\w+)"', section)
    exported = [p["id"] for p in affordances.predicate_schemas(catalog())]
    assert listed == exported, (
        "the brief lists {} and catalog.predicates carries {}".format(listed, exported))
    # And the heading's own word has to agree with the table under it.
    words = {"twelve": 12, "thirteen": 13, "fourteen": 14, "fifteen": 15, "sixteen": 16}
    count = words.get(m.group(0).split()[2])
    assert count == len(listed), "the heading says {} and the table has {}".format(
        m.group(0), len(listed))


def test_the_predicate_domain_serves_fields_and_not_bare_names():
    """The point of exporting the schema: the `when` field's domain used to be
    fourteen bare type names, so a commander that wanted to know `enemy_in`
    takes a `region` had to leave the document to find out."""
    d = doc(ARMED)
    when = next(f for f in by_rel(d, "trigger_set")["fields"] if f["path"] == "when")
    assert "enemy_in(region, [class], [count=1])" in when["domain"]
    assert "base_under_attack()" in when["domain"], "an arm with no fields says so"
    assert "unit_count(kind, count)" in when["domain"], "both required, neither defaulted"


def test_the_predicate_domain_is_absent_beside_a_catalog_that_has_none():
    """No fallback list, deliberately: a second copy of a vocabulary is the
    thing the catalog exists to delete, and 'this document does not know' beats
    a fourteen-name guess that could be a predicate short."""
    old = copy.deepcopy(catalog())
    old.pop("predicates", None)
    d = affordances.document(load(ARMED), old, None)
    when = next(f for f in by_rel(d, "trigger_set")["fields"] if f["path"] == "when")
    assert "domain" not in when
    assert affordances.predicate_schemas(None) == []


# -- the recipes -------------------------------------------------------------


def test_the_core_recipes_are_served_as_forms():
    d = doc(ARMED)
    for name in ("home-guard", "hero-save", "expand", "counter-punch"):
        a = by_rel(d, "recipe:{}".format(name))
        assert a["kind"] == "form"
        assert 1 <= len(a["fields"]) <= 2, "one or two open judgment fields, no more"


def test_a_recipe_template_is_written_in_selectors_and_place_names():
    """The brief's recipes had `<hero id>` and `<worker id>` in them. These do
    not: `select` resolves at FIRE time, so the rule outlives its units."""
    d = doc(ARMED)
    hero_save = by_rel(d, "recipe:hero-save")["template"]
    assert hero_save["then"]["select"] == "my hero"
    assert "units" not in hero_save["then"]
    expand = by_rel(d, "recipe:expand")["template"]
    assert expand["then"]["select"] == "workers"
    assert expand["then"]["site"] == "nearest legal site", \
        "the fix for blue-r23's site-blocked loop"
    assert "worker" not in expand["then"]


def test_no_command_and_no_template_contains_an_entity_id():
    """The hard one, and the reason 0uu.1 was a hard dependency. An entity id
    frozen into a rendered link is the r21/r23 staleness failure class,
    automated. Ids in this engine are large integers; nothing legitimate in a
    template is."""
    for path in LIVE:
        d = affordances.document(load(path), catalog())
        for a in d["actions"] + [x for al in d["alarms"] or [] for x in al["actions"]]:
            blob = a.get("command") if a["kind"] == "link" else a["template"]
            for value in walk(blob):
                assert not (isinstance(value, int) and not isinstance(value, bool)
                            and value > 4096), \
                    "{} carries what looks like an entity id: {}".format(a["rel"], value)


# -- alarms ------------------------------------------------------------------


def test_an_alarm_leads_with_the_reflex_that_already_fired():
    """"An alarm is never the first responder" — its payoff is attention."""
    d = doc(ALARM)
    assert len(d["alarms"]) == 1
    a = d["alarms"][0]
    raw = load(ALARM)["alarms"][0]
    assert a["fact"] == raw["fact"]
    assert a["running_default"] == raw["running_default"]
    first = a["actions"][0]
    assert first["rel"] == "alarm:confirm"
    assert first["command"] is None, "confirming the default sends nothing"
    assert raw["running_default"] in first["title"]


def test_an_alarms_overrides_follow_its_own_subject():
    d = doc(ALARM)
    over = [x["rel"] for x in d["alarms"][0]["actions"]]
    assert "recipe:expand" in over and "build" in over, \
        "an income collapse points at the two things that make income"
    assert not any(r.startswith("stance:") for r in over), \
        "the running default mentions squads; the FACT does not, so they are not the subject"
    # A squad named in the fact does pull that squad's transitions in.
    s = load(ALARM)
    s["alarms"] = [{"id": "squad_below_half", "fact": "squad 1 is under half strength",
                    "running_default": "nothing pulls them out (no retreat threshold set)",
                    "since_t": 100.0, "severity": "warning"}]
    over = [x["rel"] for x in affordances.document(s, catalog())["alarms"][0]["actions"]]
    assert "stance:squad-1:turtle" in over and "stance:squad-1:push" in over
    assert not any(r.startswith("stance:squad-0:") for r in over)


def test_an_alarm_sorts_the_actions_it_points_at_to_the_top():
    s = load(ALARM)
    s["alarms"] = [{"id": "squad_below_half", "fact": "squad 1 is under half strength",
                    "running_default": "nothing pulls them out", "severity": "critical"}]
    d = affordances.document(s, catalog())
    lead = rels(d)[:5]
    assert all(r.startswith("stance:squad-1:") for r in lead), lead


def test_an_absent_alarms_key_is_not_an_empty_one():
    assert affordances.document(load(ARMED), catalog())["alarms"] is None
    s = load(ARMED)
    s["alarms"] = []
    assert affordances.document(s, catalog())["alarms"] == []
    assert "ALARMS none ringing" in "\n".join(
        affordances.render_document(affordances.document(s, catalog()))
    )


def test_a_bare_string_alarm_still_renders():
    s = load(ARMED)
    s["alarms"] = ["income collapse: every mine near your base is dry"]
    a = affordances.document(s, catalog())["alarms"][0]
    assert a["fact"].startswith("income collapse")
    assert a["actions"][0]["rel"] == "alarm:confirm"


# -- ordering: fact, then the commander's own doctrine -----------------------


def test_with_no_declared_doctrine_the_order_is_facts_only():
    d = doc(ARMED)
    assert d["preference"]["doctrine"] is None
    assert d["preference"]["source"].startswith("none")
    kinds = [a["kind"] for a in d["actions"]]
    assert kinds.index("form") > max(i for i, k in enumerate(kinds) if k == "link"), \
        "links (rung 2) before forms (rung 3) — the ladder is structural"
    links = [a for a in d["actions"] if a["kind"] == "link"]
    ready = [i for i, a in enumerate(links) if a["ready"]]
    assert max(ready) < min(i for i, a in enumerate(links) if not a["ready"]), \
        "ready before not-ready, within a rung"


def test_a_declared_doctrine_sorts_the_menu_and_changes_nothing_else():
    """Preference is commander-declared and engine-SORTED. It may not touch a
    `ready`, a `reason` or a `command` — the engine never generates it and never
    acts on it."""
    with tempfile.TemporaryDirectory() as tmp:
        p = Path(tmp) / "doctrine.json"
        p.write_text(json.dumps({
            "doctrine": "aggression: high, risk: low",
            "prefer": ["harass", "push"],
            "avoid": ["turtle"],
        }))
        prefs = affordances.load_prefs(str(p))
    plain, sorted_ = doc(ARMED), doc(ARMED, prefs)
    assert sorted_["preference"]["doctrine"] == "aggression: high, risk: low"
    assert set(rels(plain)) == set(rels(sorted_)), "sorting adds and removes nothing"
    assert {json.dumps(a, sort_keys=True) for a in plain["actions"]} == \
           {json.dumps(a, sort_keys=True) for a in sorted_["actions"]}, \
        "not one field of one action differs — only their order"
    order = rels(sorted_)
    assert order.index("stance:squad-0:harass") < order.index("stance:squad-0:secure")
    assert order.index("stance:squad-0:turtle") == max(
        order.index(r) for r in order if r.startswith("stance:squad-0:")
    ), "an avoided word sinks"


def test_the_engine_never_generates_a_preference():
    for path in LIVE + [EARLY, LEGACY]:
        d = affordances.document(load(path), catalog())
        assert d["preference"]["doctrine"] is None
        assert d["preference"]["prefer"] == [] and d["preference"]["avoid"] == []


# -- degradation -------------------------------------------------------------


def test_an_empty_match_renders_rather_than_raising():
    for s in ({}, {"t": 5.0}, {"me": {}, "units": [], "buildings": [], "squads": []}):
        d = affordances.document(copy.deepcopy(s), None)
        assert d["default"]["title"]
        assert not any(r.startswith("stance:squad-") for r in rels(d)), \
            "no squads means no stance transitions — and the enrolment form to fix it"
        assert "squad" in rels(d) and "region_set" in rels(d)
        assert affordances.render_document(d)


def test_the_pre_everything_fixture_still_produces_a_document():
    s = load(LEGACY)
    assert "regions" not in s and "triggers" not in s and "plans" not in s
    d = affordances.document(s, load(FIX / "catalog.json"))
    assert by_rel(d, "trigger_set")["slots"] == "0 of 8 trigger names in use"
    assert by_rel(d, "region_set")["slots"] == "0 of 8 region names in use"
    push = by_rel(d, "stance:squad-0:push")
    assert push["ready"] is True and "push gates met" in push["reason"]
    assert affordances.render_document(d)


def test_a_squad_with_a_headcount_and_no_roster_is_not_called_empty():
    """A snapshot from before `units[].squad` carries the count on the squad
    record and no roster. "Squad 0 has no members" would then be the r21 fact
    said about a squad of ten — the one wrong answer available here."""
    s = load(ARMED)
    for u in s["units"]:
        u.pop("squad", None)
    d = affordances.document(s, catalog())
    for word in ("push", "turtle"):
        a = by_rel(d, "stance:squad-1:{}".format(word))
        assert a["ready"] is True, word
        assert "carries no roster" in a["reason"], a["reason"]


def test_the_early_game_offers_the_things_an_early_game_can_do():
    d = affordances.document(load(EARLY), catalog())
    assert by_rel(d, "stance:squad-0:push")["ready"] is False
    assert "no members" in by_rel(d, "stance:squad-0:push")["reason"]
    assert by_rel(d, "build")["ready"] is True


def test_a_squad_with_no_posture_gets_links_that_say_there_is_no_anchor():
    s = load(ARMED)
    for q in s["squads"]:
        q["posture"] = None
    d = affordances.document(s, catalog())
    a = by_rel(d, "stance:squad-1:secure")
    assert "target" not in a["command"] and "x" not in a["command"]
    assert "no anchor to carry over" in a["note"]


# -- it is a view ------------------------------------------------------------


def test_document_does_not_mutate_the_snapshot_it_is_handed():
    s = load(ALARM)
    before = json.dumps(s, sort_keys=True)
    affordances.document(s, catalog())
    assert json.dumps(s, sort_keys=True) == before


def test_state_json_is_byte_identical_before_and_after_rendering():
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        state = seat / "state.json"
        shutil.copy(ARMED, state)
        shutil.copy(CATALOG, seat / "catalog.json")
        original = state.read_bytes()
        run("--doc", str(state))
        assert state.read_bytes() == original
        run("--doc", "--json", str(state))
        assert state.read_bytes() == original


def test_the_cli_json_is_the_function_and_the_text_is_the_renderer():
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        state = seat / "state.json"
        shutil.copy(ARMED, state)
        shutil.copy(CATALOG, seat / "catalog.json")
        d = json.loads(run("--doc", "--json", str(state)))
        # Compared as JSON rather than as dicts: `digest()` reports a centroid
        # as a tuple, which is a list once it has been through a file, and that
        # is a difference in Python's type system rather than in the document.
        assert json.dumps(d, sort_keys=True) == json.dumps(
            affordances.document(load(ARMED), catalog()), sort_keys=True
        )
        assert run("--doc", str(state)).splitlines() == affordances.render_document(d)


def test_the_digest_and_the_full_readout_are_untouched():
    """This bead is the actions half. The other two modes must be exactly what
    they were."""
    out = run("--digest", str(ARMED)).splitlines()
    assert out == bridge_view.render_digest(bridge_view.digest(load(ARMED)))
    assert "WORKERS" in run(str(LEGACY))


def test_the_text_render_is_readable_and_terminates():
    for path in LIVE + [EARLY, LEGACY]:
        lines = affordances.render_document(affordances.document(load(path), catalog()))
        assert lines[0].startswith("DOC affordance-doc/")
        assert any(ln.startswith("ACTIONS") for ln in lines)
        assert any(ln.startswith("DEFAULT") for ln in lines)
        assert any(ln.startswith("RAW") for ln in lines)
        for ln in lines:
            # 2.0 trades height for width on purpose, and a folded line carries
            # a whole template: `plan_set:<name>` reads back two plan steps,
            # which is ~350 characters of JSON wherever it is printed. Prose is
            # clipped (`_clip`); a command never is, because a clipped command
            # is not a command.
            assert len(ln) <= 500, ln


# -- 2.0: fact-collapsed rendering -------------------------------------------
#
# arena/LADDER.md Finding 2: all four scaffolded rounds' commanders disobeyed
# their own spawn instruction to re-read the document each cycle, because ~600
# lines is uneconomical at a 15-second cadence. Finding 5: the readiness
# annotations that directly addressed the mid-tier losing moves therefore sat in
# a render nobody re-opened — served every cycle, read never. These tests are
# the budget that stops the page growing back.


#: The line counts this bead inherited: `render_document` at 1.3, beside
#: `catalog_full.json`, on these two fixtures. Kept as the thing the collapse is
#: measured against — and as the pin on `--all`, whose whole promise is that it
#: is still that render.
FULL_LINES = {"doc_open_armed.json": 643, "doc_open_alarm.json": 803}

#: What the collapsed render must fit in. The real numbers when this landed were
#: **76** (armed, 43 actions) and **94** (alarm, 51 actions and a ringing
#: alarm) — an 8.5x fold both times. The floor is one line per action, because
#: constraint 1 forbids dropping any, so this budget is really a cap on how many
#: actions the document may grow before somebody has to think about grouping
#: them. It is not a pin: a couple of new actions should not fail a suite.
COLLAPSED_BUDGET = 100


def render(path=ARMED, prefs=None, full=False):
    return affordances.render_document(doc(path, prefs), full=full)


def test_the_collapsed_render_fits_on_a_loop_page():
    """The bead's acceptance criterion, in lines."""
    for path in LIVE:
        collapsed = render(path)
        full = render(path, full=True)
        assert len(collapsed) <= COLLAPSED_BUDGET, "{}: {} lines".format(
            path.name, len(collapsed))
        assert len(full) == FULL_LINES[path.name], (
            "{}: `--all` is meant to BE the old render; it is now {} lines and was {}".format(
                path.name, len(full), FULL_LINES[path.name]))
        assert len(full) >= 6 * len(collapsed), "{}: {} -> {} is not a compression".format(
            path.name, len(full), len(collapsed))


def test_the_collapse_loses_no_action():
    """Folding is not filtering. Every `rel` the document carries has at least
    one line of its own in the collapsed render — AFFORDANCES.md constraint 1:
    invisible is inexpressible for a weak model, so hiding is soft enforcement
    and this document does not enforce."""
    for path in LIVE + [EARLY, LEGACY]:
        d = doc(path)
        lines = affordances.render_document(d)
        for a in d["actions"]:
            owned = [ln for ln in lines if a["rel"] in ln]
            assert owned, "{}: {} vanished from the collapsed render".format(
                path.name, a["rel"])


def test_a_folded_line_still_carries_its_complete_command():
    """Rung 2's promise — "send it back verbatim" — has to survive the fold, or
    the collapse compressed the wrong half and a commander has to re-open the
    page to act."""
    d = doc(ARMED)
    for a in d["actions"]:
        line = affordances.collapse_action(a)
        blob = a["command"] if a["kind"] == "link" else a["template"]
        assert json.dumps(blob, separators=(",", ":")) in line, a["rel"]
        if a["kind"] == "form":
            for f in a["fields"]:
                if f["default"] is None:
                    assert f["path"] in line, "{}: {} is not named as yours".format(
                        a["rel"], f["path"])


def test_the_blocking_fact_rides_on_every_not_ready_line():
    """Finding 5, addressed. r26 red committed 13 units into 12 defenders with
    the push gates and the staleness sentence served — on page eight of a render
    it had not re-opened since t=0. Now they are on the line."""
    lines = render(ARMED)
    blocked = [ln for ln in lines if "BLOCKED:" in ln]
    d = doc(ARMED)
    assert len(blocked) == len([a for a in d["actions"] if not a["ready"]])
    push = next(ln for ln in blocked if "stance:squad-1:push" in ln)
    assert "size 6/6" not in push, "the met half is `--all`'s job; the news is what stops it"
    assert "3 of your 9 army units are outside squad 1" in push
    assert "not since" in push, "the intel ledger rides free — same line, more characters"
    # And the met half really is still there under `--all`.
    assert "(met: size 6/6" in "\n".join(render(ARMED, full=True))


def test_a_collection_form_keeps_its_slot_pressure_when_it_folds():
    """"7 of 8 trigger names in use" changes what a commander writes; the field
    notes under it do not. So the slot line survives the fold and the fields do
    not."""
    line = affordances.collapse_action(by_rel(doc(ARMED), "trigger_set"))
    assert "2 of 8 trigger names in use" in line
    assert "a fresh name creates" not in line, "the field notes are `--all`'s"


def test_the_default_block_still_leads_the_page():
    """Silence is rung 1 and it must be the first option a reader meets. The
    properties above it are facts, not options."""
    for path in LIVE:
        lines = render(path)
        heads = [i for i, ln in enumerate(lines) if ln and not ln.startswith(" ")]
        named = [lines[i].split()[0] for i in heads]
        assert named.index("DEFAULT") < named.index("ACTIONS")
        if "ALARMS" in named:
            assert named.index("DEFAULT") < named.index("ALARMS")


def test_every_action_is_grouped_under_exactly_one_section():
    for path in LIVE + [EARLY, LEGACY]:
        d = doc(path)
        grouped = affordances.group_sections(d["actions"])
        seen = [a["rel"] for _, rows in grouped for a in rows]
        assert sorted(seen) == sorted(a["rel"] for a in d["actions"])
        assert len(seen) == len(set(seen)), "an action printed twice is a fact counted twice"
        for sec, _rows in grouped:
            assert sec in affordances.SECTION_ORDER


def test_the_sections_are_read_off_the_verb_and_the_catalog():
    """Mechanical, never a judgment: the rejected half of this bead was
    engine-INFERRED phase filtering, and an engine that decided which of your
    buildings were "economy" would be having exactly that opinion."""
    d = doc(ARMED)
    assert by_rel(d, "build")["sections"] == ["economy", "tech"]
    assert by_rel(d, "train:TownHall")["sections"][:2] == ["economy", "army"], \
        "a hall trains Workers and heroes, so it is both"
    assert by_rel(d, "train:Barracks")["sections"] == ["army", "tech"], \
        "Knight is NOT AVAILABLE at this seat's tech, and that is the tech question"
    assert by_rel(d, "stance:squad-0:harass")["sections"] == ["army", "harass"]
    assert by_rel(d, "stance:squad-0:push")["sections"] == ["army"]
    assert by_rel(d, "trigger_set")["sections"] == ["standing"]
    assert by_rel(d, "recipe:expand")["sections"] == ["economy"]


# -- 2.0: --all restores the render it replaced ------------------------------


def test_all_restores_the_full_render_exactly():
    """`--all` is the reason the collapse is allowed to be aggressive: no fact
    left the document, only the default page. So it must be 1.3's render — the
    same heading, the same order, the same `render_action` for every action —
    and everything outside the ACTIONS section must be identical in both modes.
    """
    for path in LIVE:
        d = doc(path)
        full, collapsed = render(path, full=True), render(path)
        head = "ACTIONS ({} — rungs 2 and 3; sorted by fact only (no doctrine declared))".format(
            len(d["actions"]))
        i, j = full.index(head), full.index("RAW (rung 4)")
        assert full[: i - 1] == collapsed[: _actions_head(collapsed)], \
            "everything above ACTIONS is the same page in both modes"
        assert full[i + 1: j - 1] == [
            ln for a in d["actions"] for ln in affordances.render_action(a)
        ], "the body is render_action over order_actions, unchanged"
        assert full[j:] == collapsed[collapsed.index("RAW (rung 4)"):]


def _actions_head(lines):
    return next(i for i, ln in enumerate(lines) if ln.startswith("ACTIONS ")) - 1


def test_the_cli_all_flag_is_the_renderer():
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        state = seat / "state.json"
        shutil.copy(ARMED, state)
        shutil.copy(CATALOG, seat / "catalog.json")
        assert run("--doc", "--all", str(state)).splitlines() == render(ARMED, full=True)
        assert run("--doc", str(state)).splitlines() == render(ARMED)
        assert state.read_bytes() == ARMED.read_bytes()


def test_the_json_mode_pays_no_line_cost_and_therefore_takes_no_collapse():
    """A machine reader has no page to run out of, so `--json` serves every
    action complete in both modes and `--all` changes nothing about it."""
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        state = seat / "state.json"
        shutil.copy(ALARM, state)
        shutil.copy(CATALOG, seat / "catalog.json")
        plain = json.loads(run("--doc", "--json", str(state)))
        every = json.loads(run("--doc", "--all", "--json", str(state)))
    assert plain == every
    d = doc(ALARM)
    assert json.dumps(plain, sort_keys=True) == json.dumps(d, sort_keys=True)
    fields = [f for a in plain["actions"] for f in a.get("fields") or []]
    domain_rows = [row for f in fields for row in f.get("domain") or []]
    assert len(fields) > 40 and len(domain_rows) > 100, (len(fields), len(domain_rows))
    for a in plain["actions"]:
        if a["kind"] == "form":
            assert a["fields"], a["rel"]
        assert "collapsed" in a and "sections" in a
    # The folded TEXT render is where the domains went; the JSON kept them.
    # Checked on the long, distinctive rows — a `squad` field's domain is
    # ["0", "1"], and "0" appears in every other command on the page.
    text = "\n".join(affordances.render_document(d))
    long_rows = [row for row in domain_rows if len(row) > 30]
    assert len(long_rows) > 40
    assert not [row for row in long_rows if row in text], \
        "the collapse is the text render only"
    assert all(row in "\n".join(affordances.render_document(d, full=True))
               for row in long_rows), "`--all` serves every one of them"


# -- 2.0: commander-declared focus -------------------------------------------


_PREF_DIR = []


def prefs_file(**raw):
    """A prefs side-file on disk, because `load_prefs` reading a real file is
    half of what the preference channel IS (no wire verb carries doctrine)."""
    if not _PREF_DIR:
        _PREF_DIR.append(tempfile.mkdtemp(prefix="affordance-prefs-"))
        atexit.register(shutil.rmtree, _PREF_DIR[0], True)
    p = Path(_PREF_DIR[0]) / "prefs-{}.json".format(len(list(Path(_PREF_DIR[0]).iterdir())))
    p.write_text(json.dumps(raw))
    return affordances.load_prefs(str(p))


def test_the_engine_never_infers_a_focus():
    """Absent means the fact-collapsed default. The whole reason the owner's
    phase proposal was reshaped: an inferred phase is an opinion."""
    for path in LIVE + [EARLY, LEGACY]:
        d = doc(path)
        assert d["preference"]["focus"] is None
        assert all(a["collapsed"] for a in d["actions"])
    assert affordances.load_prefs(None) is None
    assert prefs_file(doctrine="aggression: high")["focus"] is None


def test_a_declared_focus_expands_its_section_and_folds_the_rest():
    d = doc(ARMED, prefs_file(focus="army"))
    assert d["preference"]["focus"] == "army"
    for a in d["actions"]:
        assert a["collapsed"] is ("army" not in a["sections"]), a["rel"]
    lines = affordances.render_document(d)
    text = "\n".join(lines)
    # The focused section is rendered as it always was, domains and all...
    assert "\n".join(affordances.render_action(by_rel(d, "stance:squad-0:turtle"))) in text
    # ...and the sections it did not name are still every one of them, folded.
    for rel in ("build", "trigger_set", "region_set", "plan_clear:opening"):
        assert affordances.collapse_action(by_rel(d, rel)) in text, rel


def test_a_declared_focus_hides_nothing_and_still_counts_everything():
    plain, focused = doc(ARMED), doc(ARMED, prefs_file(focus="economy"))
    assert rels(plain) == rels(focused), "focus is a render, not a filter"
    for path in LIVE:
        for word in affordances.FOCUS_WORDS:
            d = doc(path, prefs_file(focus=word))
            lines = affordances.render_document(d)
            for a in d["actions"]:
                assert any(a["rel"] in ln for ln in lines), "{}/{}: {}".format(
                    path.name, word, a["rel"])
            head = next(ln for ln in lines if ln.startswith("ACTIONS ("))
            assert "{}:".format(len(d["actions"])) in head, "the counts stay honest"


def test_a_declared_focus_changes_no_fact():
    """Preference sorts and renders. It may not touch a `ready`, a `reason`, a
    `command` or a `cost` — 2.0 adds the `collapsed` hint to that list of things
    it MAY touch, and nothing else."""
    plain, focused = doc(ARMED), doc(ARMED, prefs_file(focus="harass"))
    for a, b in zip(plain["actions"], focused["actions"]):
        assert a["rel"] == b["rel"], "no focus reorders the menu"
        x = {k: v for k, v in a.items() if k != "collapsed"}
        y = {k: v for k, v in b.items() if k != "collapsed"}
        assert json.dumps(x, sort_keys=True) == json.dumps(y, sort_keys=True), a["rel"]


def test_an_alarm_breaks_through_any_focus():
    """By design: an alarm is the phase-transition machinery the r23 commanders
    described, and a focus that could hide the fork it just named would be the
    soft enforcement this document refuses."""
    for word in affordances.FOCUS_WORDS:
        d = doc(ALARM, prefs_file(focus=word))
        for rel in ("build", "recipe:expand"):
            a = by_rel(d, rel)
            assert a["collapsed"] is False, "{}: {} came back folded".format(word, rel)
        text = "\n".join(affordances.render_document(d))
        assert "\n".join(affordances.render_action(by_rel(d, "build"))) in text
        # The alarm block itself is untouched by the focus and still leads.
        lines = affordances.render_document(d)
        assert lines.index(next(ln for ln in lines if ln.startswith("ALARMS "))) < \
            lines.index(next(ln for ln in lines if ln.startswith("ACTIONS ")))


def unwrapped(lines):
    """The render with its hanging indents folded back, so a test can match a
    sentence the renderer wrapped."""
    return " ".join(x.strip() for x in lines)


def test_the_preference_source_line_reports_the_focus():
    prefs = prefs_file(doctrine="hold the line", focus="tech")
    text = unwrapped(affordances.render_document(doc(ARMED, prefs)))
    assert "your declared focus: tech" in text
    assert "the engine never infers one" in text
    assert prefs["source"] in text, "the file it came from is named"


def test_an_unrecognised_focus_is_ignored_out_loud():
    """A commander that thinks it is reading a filtered page and is not has been
    lied to by a view, so the word it wrote comes back with the reason."""
    prefs = prefs_file(focus="macro")
    assert prefs["focus"] is None
    assert "focus 'macro' is not one of economy/tech/army/harass — ignored" in prefs["source"]
    d = doc(ARMED, prefs)
    assert d["preference"]["focus"] is None
    assert all(a["collapsed"] for a in d["actions"])
    text = unwrapped(affordances.render_document(d))
    assert "no focus declared, so this page is fact-collapsed" in text
    assert "is not one of economy/tech/army/harass — ignored" in text


def test_a_focus_and_a_doctrine_are_independent_channels():
    d = doc(ARMED, prefs_file(doctrine="raid", prefer=["harass"], avoid=["turtle"],
                              focus="economy"))
    order = rels(d)
    assert order.index("stance:squad-0:harass") < order.index("stance:squad-0:secure"), \
        "`prefer` still sorts"
    assert by_rel(d, "build")["collapsed"] is False, "`focus` still expands"
    assert by_rel(d, "stance:squad-0:harass")["collapsed"] is True, \
        "a preferred action is sorted, not expanded — they are different channels"


def _run():
    tests = [(n, f) for n, f in sorted(globals().items())
             if n.startswith("test_") and callable(f)]
    failed = 0
    for name, fn in tests:
        try:
            fn()
        except AssertionError as err:
            failed += 1
            print(f"FAIL {name}: {err}")
        except Exception as err:  # noqa: BLE001
            failed += 1
            print(f"ERROR {name}: {type(err).__name__}: {err}")
    print(f"{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(_run())
