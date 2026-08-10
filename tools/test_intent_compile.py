#!/usr/bin/env python3
"""Tests for tools/intent_compile.py.

    python3 -m pytest tools/test_intent_compile.py     # or just: python3 tools/test_intent_compile.py

Plain asserts, no pytest features, so the file runs either way — an agent with
no venv should still be able to check the compiler before sending an army
somewhere on its say-so.

The fixture (`fixtures/state_crossings.json`) is a real-shaped `state.json` for
the Claude seat on the `crossings` map at ~3:34, with three named fords, two
barracks, a shop with one locked rung, five mines and one visible bounty. Every
place-name test below is therefore a test of the SNAPSHOT vocabulary, not of a
hardcoded table: change the map and these tests change with it.
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import intent_compile as ic  # noqa: E402

FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "fixtures", "state_crossings.json")

NW_FORD = (-60.0, 60.0)
CENTER_FORD = (0.0, 0.0)
SE_FORD = (60.0, -60.0)
MY_BASE = (70.0, 70.0)
THEIR_BASE = (-70.0, -70.0)


def snap():
    return ic.Snapshot.load(FIXTURE)


def compile_one(text, s=None):
    return ic.compile_directives([text], s or snap())


def verbs(result):
    return [i["type"] for i in result.intents]


def only(result, verb):
    matches = [i for i in result.intents if i["type"] == verb]
    assert len(matches) == 1, f"expected one {verb}, got {verbs(result)}"
    return matches[0]


# ---------------------------------------------------------------------------
# Places — the named geography, resolved from the snapshot
# ---------------------------------------------------------------------------


def test_named_ford_resolves_from_the_snapshot_chokes():
    s = snap()
    assert ic.resolve_place("the northwest ford", s) == NW_FORD
    assert ic.resolve_place("the center ford", s) == CENTER_FORD
    assert ic.resolve_place("southeast ford", s) == SE_FORD
    # A partial name still lands: "west" is inside "northwest", and the choke
    # noun says the commander meant a gap.
    assert ic.resolve_place("the west ford", s) == NW_FORD


def test_a_bare_direction_is_a_direction_not_a_ford():
    # The distinction matters: "hold the west" is a side of the map, "hold the
    # west ford" is a 30-unit gap. Silently conflating them would put an army
    # somewhere the commander never named.
    s = snap()
    assert ic.resolve_place("the west", s) == ic.COMPASS["west"]
    assert ic.resolve_place("the west", s) != NW_FORD


def test_bases_and_middle():
    s = snap()
    assert ic.resolve_place("mid", s) == (0.0, 0.0)
    assert ic.resolve_place("the middle", s) == (0.0, 0.0)
    # Our base is our real hall, not the fixed corner constant.
    assert ic.resolve_place("our base", s) == MY_BASE
    # Theirs is a remembered ghost — a scouted building we cannot see now.
    assert ic.resolve_place("their base", s) == THEIR_BASE
    assert ic.resolve_place("the enemy base", s) == THEIR_BASE


def test_mines_are_named_by_direction_and_not_stolen_by_chokes():
    s = snap()
    # "northwest" appears in "northwest ford"; the mine noun must win, and
    # pick the mine nearest that direction rather than the ford at it.
    assert ic.resolve_place("the northwest mine", s) == (-58.0, 58.0)
    assert ic.resolve_place("the southeast mine", s) == (58.0, -58.0)
    assert ic.resolve_place("the contested mine", s) == (22.0, 52.0)
    assert ic.resolve_place("the nearest bounty", s) == (6.0, -12.0)


def test_explicit_coordinates():
    s = snap()
    assert ic.resolve_place("(-40, 20)", s) == (-40.0, 20.0)
    assert ic.resolve_place("-40,20", s) == (-40.0, 20.0)
    assert ic.resolve_place("at 12 -8", s) == (12.0, -8.0)


def test_unresolvable_place_is_an_error_not_a_guess():
    s = snap()
    assert ic.resolve_place("the mushroom kingdom", s) is None
    result = compile_one("hold the mushroom kingdom")
    assert result.intents == []
    assert result.errors and "cannot resolve place" in result.errors[0][1]


# ---------------------------------------------------------------------------
# Units
# ---------------------------------------------------------------------------


def test_unit_selectors():
    s = snap()
    assert set(ic.resolve_units("cavalry", s)) == {4294968130, 4294968131, 4294968132}
    assert set(ic.resolve_units("the siege", s)) == {4294968140}
    assert set(ic.resolve_units("the hero", s)) == {4294968150}
    assert set(ic.resolve_units("workers", s)) == {4294968100, 4294968101, 4294968102}
    assert set(ic.resolve_units("squad 0", s)) == {
        4294968110, 4294968111, 4294968112, 4294968120, 4294968121,
        4294968130, 4294968131, 4294968132, 4294968140, 4294968150,
    }


def test_default_selection_is_the_army_and_excludes_workers():
    s = snap()
    army = ic.resolve_units(None, s)
    assert 4294968100 not in army, "workers must not be dragged into a push"
    assert 4294968110 in army and 4294968150 in army


def test_enemy_units_are_never_selectable():
    s = snap()
    # 4294967400 is a visible Human footman. No selector may name it.
    for phrase in ("the army", "footmen", "everything", "squad 0"):
        assert 4294967400 not in (ic.resolve_units(phrase, s) or [])


def test_an_unrecognised_noun_is_not_silently_the_whole_army():
    s = snap()
    assert ic.resolve_units("the wizards", s) is None
    result = compile_one("push their base with the wizards")
    assert result.intents == []
    assert result.errors


# ---------------------------------------------------------------------------
# Doctrine verbs — the ones that win matches
# ---------------------------------------------------------------------------


def test_hold_compiles_to_squad_then_defend_posture():
    result = compile_one("hold the northwest ford")
    assert verbs(result) == ["squad", "posture"]
    squad, posture = result.intents
    assert squad["id"] == posture["id"] == 1, "squad 0 is the engine's pool; allocate above it"
    assert posture["posture"] == {"type": "defend", "x": -60.0, "z": 60.0, "radius": 18.0}
    assert 4294968100 not in squad["units"]


def test_hold_takes_an_explicit_radius():
    posture = only(compile_one("hold the center ford within 30"), "posture")
    assert posture["posture"]["radius"] == 30.0


def test_push_and_its_synonyms():
    for phrase in ("push their base", "attack their base", "strike their base",
                   "press their base", "assault their base"):
        posture = only(compile_one(phrase), "posture")
        assert posture["posture"] == {"type": "push", "x": -70.0, "z": -70.0}, phrase


def test_forage_mid_with_cavalry_names_only_the_cavalry():
    result = compile_one("forage mid with the cavalry")
    squad, posture = result.intents
    assert set(squad["units"]) == {4294968130, 4294968131, 4294968132}
    assert posture["posture"] == {"type": "forage", "x": 0.0, "z": 0.0}


def test_the_headline_directive():
    """THESIS.md's example, end to end.

    Two clauses compile; the conditional one is deferred with the exact
    follow-up command, because the engine has no trigger verb to compile it to.
    """
    result = compile_one("hold the west, forage mid with cavalry, "
                         "strike when their hero falls")
    assert verbs(result) == ["squad", "posture", "squad", "posture"]
    hold_squad, hold, forage_squad, forage = result.intents
    assert hold["posture"]["type"] == "defend"
    assert (hold["posture"]["x"], hold["posture"]["z"]) == ic.COMPASS["west"]
    assert forage["posture"]["type"] == "forage"
    # Distinct squads: the cavalry leaves the holding force for the forage job.
    assert hold["id"] != forage["id"]
    assert set(forage_squad["units"]) == {4294968130, 4294968131, 4294968132}
    assert len(result.deferred) == 1
    clause, condition, suggestion = result.deferred[0]
    assert condition == "their hero falls"
    # The action, ready to re-run the moment the event feed shows the trigger.
    assert suggestion == "strike"
    assert only(compile_one(suggestion), "posture")["posture"] == {
        "type": "push", "x": -70.0, "z": -70.0
    }


def test_escort_targets_a_unit_and_never_itself():
    result = compile_one("escort the hero with the footmen")
    squad, posture = result.intents
    assert posture["posture"] == {"type": "escort", "unit": 4294968150}
    assert 4294968150 not in squad["units"]


def test_squad_retask_keeps_the_squads_existing_job():
    # Squad 0 is defending. Re-pointing it without naming a verb must not
    # quietly turn a defensive squad into an attacking one.
    result = compile_one("squad 0 the center ford")
    posture = only(result, "posture")
    assert posture["id"] == 0
    assert posture["posture"]["type"] == "defend"
    assert (posture["posture"]["x"], posture["posture"]["z"]) == CENTER_FORD


def test_squad_retask_can_change_the_job_explicitly():
    posture = only(compile_one("squad 0 pushes their base"), "posture")
    assert posture["posture"]["type"] == "push"


def test_a_reissued_directive_reuses_the_squad_it_made():
    """The property that makes this usable in a 4 Hz command loop.

    A commander that repeats a standing directive every cycle must keep one
    squad, not shred its army into a new squad per turn.
    """
    s = snap()
    first = compile_one("hold the center ford", s)
    sid = only(first, "posture")["id"]
    # Feed the resulting posture back into the snapshot, as the game would.
    s.squads = s.squads + [{"id": sid, "posture": "defend@(0.0,0.0)r=18", "members": 9}]
    second = compile_one("hold the center ford", s)
    assert only(second, "posture")["id"] == sid


def test_two_clauses_get_two_squads():
    result = compile_one("hold the northwest ford with the footmen, "
                         "hold the southeast ford with the archers")
    ids = [i["id"] for i in result.intents if i["type"] == "posture"]
    assert ids == [1, 2]


def test_stand_down_clears_a_posture():
    result = compile_one("stand down squad 0")
    assert result.intents == [{"type": "posture", "id": 0}]


def test_retreat_and_focus():
    retreat = only(compile_one("retreat at 35%"), "retreat")
    assert retreat["below"] == 0.35
    assert (retreat["x"], retreat["z"]) == MY_BASE
    assert 4294968100 not in retreat["units"]

    retreat = only(compile_one("fall back at 40% to the center ford"), "retreat")
    assert retreat["below"] == 0.4
    assert (retreat["x"], retreat["z"]) == CENTER_FORD

    priority = only(compile_one("focus siege > heroes"), "priority")
    assert priority["classes"] == ["Siege", "Hero"]
    priority = only(compile_one("focus the catapults"), "priority")
    assert priority["classes"] == ["Siege"]


def test_focus_rejects_a_class_the_engine_does_not_have():
    result = compile_one("focus wizards")
    assert result.intents == []
    assert "no valid target class" in result.errors[0][1]


def test_leash_and_autocast():
    leash = only(compile_one("leash the siege to our base within 25"), "leash")
    assert leash["units"] == [4294968140]
    assert (leash["x"], leash["z"], leash["radius"]) == (70.0, 70.0, 25.0)

    autocast = only(compile_one("autocast at 3"), "autocast")
    assert autocast["units"] == [4294968150]
    assert autocast["min_enemies"] == 3


def test_template_stamps_every_matching_building():
    result = compile_one("barracks units join squad 2")
    assert verbs(result) == ["template", "template"], "both barracks, not just one"
    assert {i["building"] for i in result.intents} == {4294968201, 4294968202}
    assert all(i["squad"] == 2 for i in result.intents)


# ---------------------------------------------------------------------------
# Economy and production
# ---------------------------------------------------------------------------


def test_rally_points_every_matching_building():
    result = compile_one("rally the barracks to the center ford")
    assert verbs(result) == ["rally", "rally"]
    assert {i["building"] for i in result.intents} == {4294968201, 4294968202}
    assert all((i["x"], i["z"]) == CENTER_FORD for i in result.intents)


def test_train_spreads_across_producers():
    result = compile_one("train 3 footmen")
    assert verbs(result) == ["train"] * 3
    # The empty barracks gets two, the one already building a footman gets one.
    counts = {}
    for i in result.intents:
        counts[i["building"]] = counts.get(i["building"], 0) + 1
    assert counts == {4294968202: 2, 4294968201: 1}
    assert all(i["unit"] == "Footman" for i in result.intents)


def test_train_picks_the_right_kind_of_building():
    assert only(compile_one("train a catapult"), "train")["building"] == 4294968203
    assert only(compile_one("train 1 worker"), "train")["building"] in (4294968200, 4294968210)


def test_build_falls_through_from_train_and_picks_the_nearest_worker():
    s = snap()
    build = only(compile_one("build a farm at our base", s), "build")
    assert build["kind"] == "Farm"
    assert build["worker"] in (4294968100, 4294968101, 4294968102)
    # "our base" is an anchor, so the site is near the hall but not ON it.
    assert ic.dist((build["x"], build["z"]), MY_BASE) <= 40.0
    for standing in s.own_buildings():
        assert ic.dist((build["x"], build["z"]), tuple(standing["pos"])) >= 8.0


def test_explicit_coordinates_are_never_nudged():
    """A landmark is an anchor the tool may improve on; a coordinate is a
    decision, and relocating it would be the tool overruling the commander."""
    for phrase in ("build a tower at (-40, 20)", "build a tower at -40, 20"):
        build = only(compile_one(phrase), "build")
        assert (build["x"], build["z"]) == (-40.0, 20.0), phrase


def test_two_builds_in_one_directive_do_not_collide():
    """Found live: a batch is applied against a world that has not moved yet,
    so two `build` clauses picked the same default site AND the same nearest
    worker — the second silently replaced the first and one building appeared
    where the commander asked for two, with no error anywhere."""
    result = ic.compile_directives(["build a farm, build a barracks"], snap())
    assert verbs(result) == ["build", "build"]
    first, second = result.intents
    assert first["worker"] != second["worker"], "both builds took the same worker"
    assert (first["x"], first["z"]) != (second["x"], second["z"]), "same site twice"


def test_a_default_build_site_avoids_buildings_that_already_stand():
    """Also found live: a default site that lands on your own farm comes back
    from intent.rs as "site is blocked" — a correct error and a useless one,
    because the commander said "build a farm", not "build it exactly there"."""
    s = snap()
    build = only(compile_one("build a farm", s), "build")
    site = (build["x"], build["z"])
    for standing in s.own_buildings():
        assert ic.dist(site, tuple(standing["pos"])) >= 8.0, \
            f"default site {site} sits on the {standing['kind']}"


def test_harvest_only_ever_names_workers():
    harvest = only(compile_one("harvest gold"), "harvest")
    assert set(harvest["units"]) == {4294968100, 4294968101, 4294968102}
    assert harvest["target"] == 4294968300  # the mine nearest our hall
    lumber = only(compile_one("harvest lumber"), "harvest")
    assert lumber["target"] == 4294968400


def test_tier_up_and_research():
    assert only(compile_one("tier up"), "upgrade")["building"] == 4294968200
    research = only(compile_one("research attack"), "research")
    assert research == {"type": "research", "building": 4294968204, "upgrade": "attack"}
    assert compile_one("research charisma").errors


def test_buy_reads_the_shelf_and_respects_the_tier_lock():
    buy = only(compile_one("buy a healing potion"), "buy")
    assert buy == {"type": "buy", "shop": 4294968205, "item": "HealingPotion"}
    assert only(compile_one("buy town portal"), "buy")["item"] == "TownPortal"
    # The shelf reports its own locks; refusing here beats a rejected command.
    locked = compile_one("buy the banner of command")
    assert locked.intents == []
    assert "locked" in locked.errors[0][1]


def test_scout_sends_the_cheapest_eyes():
    attack_move = only(compile_one("scout their base"), "attackmove")
    assert attack_move["units"] == [4294968130], "raiders see furthest per gold"
    assert (attack_move["x"], attack_move["z"]) == THEIR_BASE


def test_surrender_and_autopilot():
    assert compile_one("surrender").intents == [{"type": "surrender"}]
    assert compile_one("autopilot").intents == [{"type": "autopilot", "on": True}]
    assert compile_one("autopilot off").intents == [{"type": "autopilot", "on": False}]


# ---------------------------------------------------------------------------
# Shape of the output — it must be Intent VALUES the game already parses
# ---------------------------------------------------------------------------

# Every verb docs/INTENT.md defines, so a new one cannot be emitted by accident.
KNOWN_VERBS = {
    "move", "attackmove", "attack", "harvest", "return", "follow", "stop",
    "build", "train", "upgrade", "cancel", "research", "rally",
    "cast", "buy", "use_item",
    "priority", "retreat", "leash", "autocast", "squad", "posture", "template",
    "autopilot", "surrender",
}
POSTURE_TYPES = {"defend", "push", "escort", "forage"}


def test_every_emitted_intent_is_a_known_verb_and_json_serialisable():
    directives = [
        "hold the northwest ford with the footmen",
        "forage mid with the cavalry",
        "escort the hero with the archers",
        "retreat at 35%", "focus siege > heroes", "leash the siege to our base",
        "autocast at 3", "barracks units join squad 4", "train 2 archers",
        "build a tower at the center ford", "harvest lumber", "tier up",
        "research armor", "buy a town portal", "scout the southeast ford",
        "squad 0 pushes their base", "stand down squad 3",
    ]
    result = ic.compile_directives(directives, snap())
    assert not result.errors, result.errors
    for intent in result.intents:
        assert intent["type"] in KNOWN_VERBS, intent
        if intent["type"] == "posture" and intent.get("posture"):
            assert intent["posture"]["type"] in POSTURE_TYPES, intent
        # Ids stay integers: `Entity::to_bits` round-trips only as a number.
        for key in ("units", "target", "building", "worker", "shop", "unit"):
            value = intent.get(key)
            if isinstance(value, list):
                assert all(isinstance(v, int) for v in value), intent
            elif key != "unit" and value is not None:
                assert isinstance(value, int), intent
    json.loads(json.dumps(result.intents))


def test_compiling_is_deterministic():
    text = "hold the west, forage mid with cavalry, focus siege > heroes"
    first = ic.compile_directives([text], snap()).intents
    second = ic.compile_directives([text], snap()).intents
    assert first == second


def test_clauses_split_on_commas_and_not_on_and():
    assert ic.split_clauses("hold mid, push their base") == \
        ["hold mid", "push their base"]
    assert ic.split_clauses("hold the ford and the mine") == \
        ["hold the ford and the mine"]


def test_a_comma_inside_a_coordinate_is_not_a_clause_break():
    """Found while driving a live seat: the comma in `(-40, 20)` split the
    clause, producing a build order somewhere else entirely plus an
    unparseable fragment — a wrong order that still read as a clean compile."""
    assert ic.split_clauses("build a tower at (-40, 20)") == \
        ["build a tower at (-40, 20)"]
    assert ic.split_clauses("hold -40, 20") == ["hold -40, 20"]
    # ...but a comma between two real clauses still separates them.
    assert ic.split_clauses("build a tower at (-40, 20), hold mid") == \
        ["build a tower at (-40, 20)", "hold mid"]


def test_explain_lists_the_whole_vocabulary():
    """--explain is the LLM escape hatch, so it has to be complete enough to
    act as the prompt. If a verb stops being mentioned there, a model reading
    it will not know the verb exists."""
    for phrase in ("hold <place>", "push <place>", "forage <place>", "escort",
                   "retreat at", "focus", "leash", "autocast", "template",
                   "harvest", "build", "train", "tier up", "research", "buy",
                   "scout", "surrender", "rally", "bridge_send.py", "docs/INTENT.md",
                   "units[].why", "intent_log.jsonl"):
        assert phrase in ic.EXPLAIN, f"--explain never mentions {phrase!r}"


def test_cli_end_to_end(tmp_path=None):
    import subprocess
    out = subprocess.run(
        [sys.executable, os.path.join(os.path.dirname(FIXTURE), "..", "intent_compile.py"),
         "--state", FIXTURE, "--json", "hold the northwest ford"],
        capture_output=True, text=True, check=True)
    intents = json.loads(out.stdout)
    assert [i["type"] for i in intents] == ["squad", "posture"]


# ---------------------------------------------------------------------------


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
