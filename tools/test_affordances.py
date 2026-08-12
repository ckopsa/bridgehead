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
LEGACY = FIX / "state_crossings.json"
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
    assert affordances.DOC_VERSION == "affordance-doc/1"
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
    """The thirteen `when` predicates are not in the catalog, so this module
    keeps a copy — and a copy that can rot quietly is worse than no copy. The
    brief's own table is the referee."""
    brief = (HERE / "COMMANDER_BRIEF.md").read_text()
    section = brief.split("### The thirteen predicates", 1)[1].split("\n###", 1)[0]
    listed = re.findall(r'\{"type":"(\w+)"', section)
    assert len(listed) == 13, listed
    assert listed == affordances.TRIGGER_PREDICATES


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
        assert lines[0].startswith("DOC affordance-doc/1")
        assert any(ln.startswith("ACTIONS") for ln in lines)
        assert any(ln.startswith("DEFAULT") for ln in lines)
        assert any(ln.startswith("RAW") for ln in lines)
        for ln in lines:
            assert len(ln) <= 400, ln


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
