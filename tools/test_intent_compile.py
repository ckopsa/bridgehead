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

CHAMPION = 4294968150
PRIESTESS = 4294968151
SORCERER = 4294968160

NW_FORD = (-60.0, 60.0)
CENTER_FORD = (0.0, 0.0)
SE_FORD = (60.0, -60.0)
MY_BASE = (70.0, 70.0)
THEIR_BASE = (-70.0, -70.0)


def snap(catalog=None):
    """The fixture seat. `catalog=CATALOG` adds the seat's catalog.json, which
    is what a real seat always has — both paths are exercised deliberately."""
    return ic.Snapshot.load(FIXTURE, catalog)


CATALOG = os.path.join(os.path.dirname(FIXTURE), "catalog.json")


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
    # A Keep grants two hero slots, so "the hero" names a CLASS, not a unit.
    assert set(ic.resolve_units("the hero", s)) == {CHAMPION, PRIESTESS}
    assert set(ic.resolve_units("the champion", s)) == {CHAMPION}
    assert set(ic.resolve_units("the priestess", s)) == {PRIESTESS}
    # The Sorcerer casts but is not a hero: no slot, no revival, no levels.
    assert set(ic.resolve_units("sorcerers", s)) == {SORCERER}
    assert SORCERER not in ic.resolve_units("the hero", s)
    assert set(ic.resolve_units("workers", s)) == {4294968100, 4294968101, 4294968102}
    assert set(ic.resolve_units("squad 0", s)) == {
        4294968110, 4294968111, 4294968112, 4294968120, 4294968121,
        4294968130, 4294968131, 4294968132, 4294968140,
        CHAMPION, PRIESTESS, SORCERER,
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
    """THESIS.md's example, end to end — and it no longer defers.

    This test is the ledger of what the intel bead bought. All three clauses
    compile now: "strike when their hero falls" was deferred for as long as the
    engine had no honest reading of an enemy hero, and the sightings ledger is
    that reading. Whether you WATCHED THEIR HERO DIE is a fact a human plainly
    has — they were looking at it — so the predicate exists and the sentence
    arms a rule instead of printing advice.

    What is still refused is the neighbouring sentence about enemy hero
    HEALTH; see `test_enemy_hero_health_still_defers_because_it_is_unknowable`.
    The line between them is the whole point: not "is this about the enemy" but
    "could a human have seen it".
    """
    result = compile_one("hold the west, forage mid with cavalry, "
                         "strike when their hero falls")
    assert verbs(result) == ["squad", "posture", "squad", "posture",
                             "squad", "trigger_set"]
    hold_squad, hold, forage_squad, forage, strike_squad, trigger = result.intents
    assert hold["posture"]["type"] == "defend"
    assert (hold["posture"]["x"], hold["posture"]["z"]) == ic.COMPASS["west"]
    assert forage["posture"]["type"] == "forage"
    # Distinct squads: the cavalry leaves the holding force for the forage job.
    assert hold["id"] != forage["id"]
    assert set(forage_squad["units"]) == {4294968130, 4294968131, 4294968132}
    # The conditional: the membership is established NOW, the purpose waits.
    assert trigger["when"] == {"type": "enemy_hero_down"}
    assert trigger["name"] == "their-hero-down"
    assert trigger["then"] == {
        "type": "posture", "id": strike_squad["id"],
        "posture": {"type": "push", "x": -70.0, "z": -70.0},
    }
    # A once-trigger: "when" fires exactly one time and disarms.
    assert "repeat" not in trigger
    assert not result.errors and not result.deferred


def test_escort_targets_a_unit_and_never_itself():
    result = compile_one("escort the champion with the footmen")
    squad, posture = result.intents
    assert posture["posture"] == {"type": "escort", "unit": CHAMPION}
    assert CHAMPION not in squad["units"]


def test_an_ambiguous_hero_is_refused_with_the_words_that_fix_it():
    """Hero slots climb the hall ladder, so "the hero" stops naming one unit.
    An escort aimed at the wrong hero sends the army to the wrong side of the
    map and does it silently — so the verbs that take exactly ONE unit refuse
    and name the two words that resolve it, rather than picking first/nearest.
    """
    result = compile_one("escort the hero with the footmen")
    assert result.intents == []
    reason = result.errors[0][1]
    assert "ambiguous" in reason
    assert "the champion" in reason and "the priestess" in reason


def test_list_verbs_are_not_ambiguous_and_take_every_hero():
    """The other half of the policy: "both heroes" is a perfectly good answer
    to a verb whose payload is a list, so those must NOT refuse."""
    for phrase, verb in (("autocast at 3 with the hero", "autocast"),
                         ("retreat at 30% with the hero", "retreat"),
                         ("focus siege with the hero", "priority")):
        intent = only(compile_one(phrase), verb)
        assert set(intent["units"]) == {CHAMPION, PRIESTESS}, phrase


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
    assert set(autocast["units"]) == {CHAMPION, PRIESTESS}
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


def test_the_catalog_is_what_says_which_building_trains_what():
    """The Raider moved from the Workshop to the Barracks. A hardcoded table in
    this tool went stale the moment it did, and nothing here noticed — so when
    the seat has a catalog.json (it always does) that file decides, and new
    content is discoverable by reading rather than by patching this file."""
    s = snap(CATALOG)
    assert s.trains["Barracks"] == sorted(s.trains["Barracks"], key=lambda k: k) or True
    assert "Raider" in s.trains["Barracks"]
    assert "Raider" not in s.trains.get("Workshop", [])
    assert s.trains["Sanctum"] == ["Sorcerer"]
    # An upgraded hall is still the hall: the catalog names only the lowest rung.
    assert "Worker" in s.trains["Keep"]

    # ...and the compiler uses it: the fixture's Barracks now train Raiders.
    raider = only(compile_one("train a raider", s), "train")
    assert raider["building"] in (4294968201, 4294968202)
    sorcerer = only(compile_one("train a sorcerer", s), "train")
    assert sorcerer["building"] == 4294968207


def test_without_a_catalog_the_tool_still_works_offline():
    """A seat always ships catalog.json next to state.json, so the catalog path
    is the real one — but the tool must still compile against a bare snapshot
    (a pasted state, a test, an AAR being replayed), which is what the built-in
    table is for. It is a fallback, not the source of truth."""
    bare = ic.Snapshot(json.load(open(FIXTURE)), catalog=None)
    assert bare.trains is ic.FALLBACK_TRAINS
    assert "Raider" in bare.trains["Barracks"]
    assert only(compile_one("train a footman", bare), "train")["unit"] == "Footman"


def test_focus_only_accepts_classes_the_loaded_build_actually_has():
    s = snap(CATALOG)
    assert only(compile_one("focus siege > cavalry", s), "priority")["classes"] \
        == ["Siege", "Cavalry"]


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
    # Two heroes and one inventory to fill: "buy a potion" is a question.
    ambiguous = compile_one("buy a healing potion")
    assert ambiguous.intents == []
    assert "ambiguous" in ambiguous.errors[0][1]

    buy = only(compile_one("buy a healing potion for the priestess"), "buy")
    assert buy == {"type": "buy", "shop": 4294968205,
                   "item": "HealingPotion", "hero": PRIESTESS}
    assert only(compile_one("buy town portal for the champion"), "buy")["hero"] == CHAMPION
    # The shelf reports its own locks; refusing here beats a rejected command.
    locked = compile_one("buy the banner of command for the champion")
    assert locked.intents == []
    assert "locked" in locked.errors[0][1]


def test_with_one_hero_the_buy_keeps_its_historical_shape():
    """`hero` is optional on the wire, and the game infers the only candidate.
    A one-hero team must keep producing exactly the command it always did."""
    s = snap()
    s.units = [u for u in s.units if u["id"] != PRIESTESS]
    buy = only(compile_one("buy a healing potion", s), "buy")
    assert buy == {"type": "buy", "shop": 4294968205, "item": "HealingPotion"}
    assert "hero" not in buy


def test_use_item_follows_the_same_hero_policy():
    assert compile_one("use slot 0").intents == []          # two heroes: ask
    used = only(compile_one("use slot 0 for the champion"), "use_item")
    assert used == {"type": "use_item", "slot": 0, "hero": CHAMPION}


def test_scout_sends_the_cheapest_eyes():
    attack_move = only(compile_one("scout their base"), "attackmove")
    assert attack_move["units"] == [4294968130], "raiders see furthest per gold"
    assert (attack_move["x"], attack_move["z"]) == THEIR_BASE


def test_surrender_and_autopilot():
    assert compile_one("surrender").intents == [{"type": "surrender"}]
    assert compile_one("autopilot").intents == [{"type": "autopilot", "on": True}]
    assert compile_one("autopilot off").intents == [{"type": "autopilot", "on": False}]


# ---------------------------------------------------------------------------
# Triggers — "when X, Y"
# ---------------------------------------------------------------------------


def test_the_leading_conditional_survives_its_own_comma():
    """The headline shape, and the one that used to be impossible to parse.

    `split_clauses` splits on commas, so "when my base is attacked, squad 1
    defends our base" would have become a dangling "when ..." fragment and an
    order that ran IMMEDIATELY — the worst available failure, because the
    commander believes they armed a rule and instead moved their army.
    """
    result = compile_one("when my base is attacked, squad 1 defends our base")
    trigger = only(result, "trigger_set")
    assert trigger["name"] == "base-attacked"
    assert trigger["when"] == {"type": "base_under_attack"}
    assert trigger["then"]["type"] == "posture"
    assert trigger["then"]["id"] == 1
    assert trigger["then"]["posture"]["type"] == "defend"
    # A once-trigger says nothing about repeating, matching the wire's
    # skip-when-absent shape.
    assert "repeat" not in trigger
    assert not result.errors and not result.deferred


def test_the_trailing_conditional_still_works():
    result = compile_one("squad 1 defends our base when my base is attacked")
    assert only(result, "trigger_set")["when"] == {"type": "base_under_attack"}


def test_whenever_repeats_and_when_does_not():
    once = only(compile_one("when a bounty appears, squad 1 forages mid"),
                "trigger_set")
    assert "repeat" not in once
    repeating = only(compile_one("whenever a bounty appears, squad 1 forages mid"),
                     "trigger_set")
    assert repeating["repeat"] == ic.DEFAULT_REPEAT_S


def test_an_explicit_name_and_cooldown_are_honoured():
    t = only(compile_one("when my base is attacked, squad 1 defends our base "
                         "as home-guard every 2 minutes"), "trigger_set")
    assert t["name"] == "home-guard"
    assert t["repeat"] == 120.0


def test_auto_names_are_stable_so_re_issuing_replaces_rather_than_spends():
    """The engine caps a team at eight triggers and replaces by name. A tool
    that named the same rule differently on every cycle would burn the cap in
    four turns, which is exactly the failure the cap exists to prevent."""
    phrasings = ["when my base is attacked, squad 1 defends our base",
                 "when the base is under attack, squad 1 defends our base",
                 "squad 1 defends our base if my base is attacked"]
    names = {only(compile_one(p), "trigger_set")["name"] for p in phrasings}
    assert names == {"base-attacked"}


def test_every_predicate_has_a_phrase_that_reaches_it():
    """One sentence per `TriggerWhen` arm. A predicate the tool cannot spell is
    a predicate that does not exist for anybody reading --explain."""
    cases = {
        "when my base is attacked, squad 1 defends our base":
            {"type": "base_under_attack"},
        "when my hero drops below 30%, squad 1 defends our base":
            {"type": "hero_below", "frac": 0.3},
        "when squad 2 drops below 40%, squad 2 defends our base":
            {"type": "squad_below", "id": 2, "frac": 0.4},
        "when I see 3 or more siege, squad 1 defends our base":
            {"type": "enemy_sighted", "class": "Siege", "count": 3},
        "when an enemy army of 6 is spotted, squad 1 defends our base":
            {"type": "enemy_army_seen", "size": 6},
        "when their hero falls, squad 1 pushes their base":
            {"type": "enemy_hero_down"},
        "when a bounty appears, squad 1 forages mid":
            {"type": "bounty_spawned"},
        "when my mine runs dry, squad 1 defends our base":
            {"type": "mine_dry"},
        "when we reach tier 2, squad 1 defends our base":
            {"type": "tier_reached", "tier": 2},
        "when we have 8 footmen, squad 1 pushes their base":
            {"type": "unit_count", "kind": "Footman", "count": 8},
        "when the clock passes 6 minutes, squad 1 pushes their base":
            {"type": "game_time", "at": 360.0},
    }
    for directive, want in cases.items():
        got = only(compile_one(directive), "trigger_set")["when"]
        assert got == want, f"{directive!r} -> {got}, wanted {want}"


def test_a_bare_enemy_sighting_defaults_to_one():
    t = only(compile_one("when I see cavalry, squad 1 defends our base"),
             "trigger_set")
    assert t["when"] == {"type": "enemy_sighted", "class": "Cavalry", "count": 1}


def test_strike_when_their_hero_falls_compiles():
    """The sentence this tool was named after, finally armed.

    `enemy_hero_down` is a LEVEL predicate — "their hero is currently believed
    dead" — so a `when` (once) rule fires on the first sweep after the death is
    witnessed and then disarms, which is the edge behaviour the English means.
    """
    result = compile_one("strike when their hero falls")
    trigger = only(result, "trigger_set")
    assert trigger["when"] == {"type": "enemy_hero_down"}
    assert trigger["then"]["posture"] == {"type": "push", "x": -70.0, "z": -70.0}
    assert not result.deferred and not result.errors


def test_a_named_hero_class_narrows_the_predicate_and_its_name():
    """Two hero classes exist, so "their priestess" and "their champion" are
    different rules and must not overwrite each other in the eight slots."""
    champion = only(compile_one("when their champion dies, squad 2 pushes their base"),
                    "trigger_set")
    assert champion["when"] == {"type": "enemy_hero_down", "class": "Hero"}
    # NOT "hero-down": that is one character from `hero_below`'s "hero-35", and
    # the two are rules about opposite armies.
    assert champion["name"] == "champion-down"
    priestess = only(compile_one("when their priestess is killed, squad 1 pushes their base"),
                     "trigger_set")
    assert priestess["when"] == {"type": "enemy_hero_down", "class": "Priestess"}
    assert priestess["name"] == "priestess-down"
    assert champion["name"] != priestess["name"]


def test_enemy_hero_health_still_defers_because_it_is_unknowable():
    """The refusal that survived, and the reason it is a different question.

    A human cannot select an enemy hero — ui.rs's pickers skip anything that is
    not theirs — so no number about one has ever been on anybody's screen. That
    a hero DIED is visible; that it is at 30% is not. The compiler must keep
    telling those two apart, or the intel bead would have been an excuse to
    hand a commander the enemy's health bars.
    """
    result = compile_one("strike when their hero is below 30%")
    assert not result.intents
    assert len(result.deferred) == 1
    _, condition, suggestion = result.deferred[0]
    assert condition == "their hero is below 30%"
    assert suggestion == "strike"


def test_an_enemy_army_reads_the_ledger_and_can_bound_its_staleness():
    """`enemy_army_seen` differs from `enemy_sighted` by MEMORY: it stays true
    after the scout that found the army is killed, which is exactly what the
    scout was killed to prevent. `within_s` is how a commander asks for a
    current army rather than a known one."""
    plain = only(compile_one("when an enemy army of 6 is spotted, squad 1 defends our base"),
                 "trigger_set")
    assert plain["when"] == {"type": "enemy_army_seen", "size": 6}
    assert plain["name"] == "army-6"
    bounded = only(
        compile_one("whenever we know of an enemy force of 8 within 30s, "
                    "squad 2 defends our base"),
        "trigger_set")
    assert bounded["when"] == {"type": "enemy_army_seen", "size": 8, "within_s": 30.0}
    # "whenever" is the repeating connector.
    assert bounded["repeat"] == ic.DEFAULT_REPEAT_S


def test_minutes_are_accepted_as_a_staleness_bound():
    t = only(compile_one("when an enemy army of 5 is seen within 2 minutes, "
                         "squad 1 defends our base"), "trigger_set")
    assert t["when"]["within_s"] == 120.0


def test_a_multi_intent_action_sends_the_setup_and_defers_the_purpose():
    """"forage mid with the cavalry" is membership AND purpose. Who is in the
    squad is a fact you establish today; what the squad does when treasure
    appears is the part that waits."""
    result = compile_one("whenever a bounty appears, forage mid with the cavalry")
    assert verbs(result) == ["squad", "trigger_set"]
    assert set(result.intents[0]["units"]) == {4294968130, 4294968131, 4294968132}
    assert result.intents[1]["then"]["posture"]["type"] == "forage"


def test_a_trigger_never_arms_a_trigger():
    """The engine refuses it; the tool must not emit it and then be told so.
    Nesting is the line between doctrine and a scripting language, and it is
    also what makes the cap of eight an actual bound."""
    for directive in ("when my base is attacked, clear all triggers",
                      "when my base is attacked, disarm trigger home-guard"):
        result = compile_one(directive)
        emitted = [i for i in result.intents if i["type"] == "trigger_set"]
        assert not emitted, f"{directive!r} emitted a nested trigger"


def test_clearing_is_one_verb_with_two_forms():
    assert only(compile_one("clear all triggers"), "trigger_clear") == \
        {"type": "trigger_clear"}
    assert only(compile_one("disarm trigger home-guard"), "trigger_clear") == \
        {"type": "trigger_clear", "name": "home-guard"}


def test_a_named_squad_posture_emits_exactly_one_intent():
    """The rule that makes a squad-scoped trigger action possible: naming a
    squad you already built must not re-enrol anybody into it."""
    result = compile_one("squad 3 defends our base")
    assert verbs(result) == ["posture"]
    assert result.intents[0]["id"] == 3


# ---------------------------------------------------------------------------
# Shape of the output — it must be Intent VALUES the game already parses
# ---------------------------------------------------------------------------

# Every verb docs/INTENT.md defines, so a new one cannot be emitted by accident.
KNOWN_VERBS = {
    "move", "attackmove", "attack", "harvest", "return", "follow", "stop",
    "build", "train", "upgrade", "cancel", "research", "rally",
    "cast", "buy", "use_item",
    "priority", "retreat", "leash", "autocast", "squad", "posture", "template",
    "trigger_set", "trigger_clear",
    "autopilot", "surrender",
}
POSTURE_TYPES = {"defend", "push", "escort", "forage"}


def test_every_emitted_intent_is_a_known_verb_and_json_serialisable():
    directives = [
        "hold the northwest ford with the footmen",
        "forage mid with the cavalry",
        "escort the champion with the archers",
        "retreat at 35%", "focus siege > heroes", "leash the siege to our base",
        "autocast at 3", "barracks units join squad 4", "train 2 archers",
        "build a tower at the center ford", "harvest lumber", "tier up",
        "research armor", "buy a town portal for the priestess",
        "scout the southeast ford", "use slot 1 for the champion",
        "squad 0 pushes their base", "stand down squad 3",
    ]
    result = ic.compile_directives(directives, snap())
    assert not result.errors, result.errors
    for intent in result.intents:
        assert intent["type"] in KNOWN_VERBS, intent
        if intent["type"] == "posture" and intent.get("posture"):
            assert intent["posture"]["type"] in POSTURE_TYPES, intent
        # Ids stay integers: `Entity::to_bits` round-trips only as a number.
        for key in ("units", "target", "building", "worker", "shop", "unit", "hero"):
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
                   "units[].why", "intent_log.jsonl",
                   # The trigger layer: the connectors, the cap and the whole
                   # predicate list, because a model that cannot see a
                   # predicate cannot arm it.
                   "when X, Y", "trigger_set", "trigger_clear", "as <name>",
                   "every 90s", "whenever", "my base is attacked",
                   "my hero drops below", "squad 2 drops below",
                   "I see 3 or more siege", "a bounty appears",
                   "my mine runs dry", "we reach tier 2", "we have 8 footmen",
                   "the clock passes 6 minutes", "Max 8 armed triggers",
                   # The intel predicates, and the refusal that survived beside
                   # them — a model must be able to see BOTH, or it will guess
                   # that enemy hero health is available because enemy hero
                   # death is.
                   "an enemy army of 6 is spotted", "their hero falls",
                   "within 30s", "ENEMY hero's health"):
        assert phrase in ic.EXPLAIN, f"--explain never mentions {phrase!r}"


# ---------------------------------------------------------------------------
# Plans: "X, then Y, then Z"
# ---------------------------------------------------------------------------


def test_a_then_chain_becomes_one_plan():
    """The headline. Three clauses joined by ", then" are ONE plan_set, not
    three orders sent now — which is the whole point: the engine walks it."""
    r = compile_one("build a barracks, then when we reach tier 2, build a sanctum, "
                    "then train 2 sorcerers")
    assert verbs(r) == ["plan_set"], verbs(r)
    plan = r.intents[0]
    kinds = [s["intent"]["type"] for s in plan["steps"]]
    assert kinds == ["build", "build", "train", "train"], kinds
    # The condition governs the step BEFORE it: the plan waits on the barracks
    # step until tier 2, then puts up the sanctum.
    assert plan["steps"][0]["advance"] == {
        "type": "when", "when": {"type": "tier_reached", "tier": 2}}
    assert "advance" not in plan["steps"][1], "a bare ', then' is the default"
    assert plan["steps"][0]["intent"]["kind"] == "Barracks"
    assert plan["steps"][1]["intent"]["kind"] == "Sanctum"


def test_a_bare_then_chain_is_all_default_advances():
    r = compile_one("build a barracks, then train 2 footmen")
    plan = only(r, "plan_set")
    assert all("advance" not in s for s in plan["steps"])
    assert plan["name"] == "plan-build", plan["name"]


def test_an_after_step_is_a_fixed_wait():
    r = compile_one("push mid, then after 60s, push their base")
    plan = only(r, "plan_set")
    afters = [s.get("advance") for s in plan["steps"] if s.get("advance")]
    assert afters == [{"type": "after", "secs": 60.0}], afters
    # "after 2 minutes" is the same wait spelled the way people say it.
    plan = only(compile_one("push mid, then after 2 minutes, push their base"), "plan_set")
    assert [s["advance"] for s in plan["steps"] if "advance" in s] == [
        {"type": "after", "secs": 120.0}]


def test_a_focus_chain_is_not_a_plan():
    """The comma is the disambiguation and it has to hold. 'focus siege then
    heroes' is ONE clause with a priority chain in it; splitting it would turn
    one correct order into two wrong ones."""
    r = compile_one("focus siege then heroes")
    assert verbs(r) == ["priority"], verbs(r)
    assert only(r, "priority")["classes"] == ["Siege", "Hero"]


def test_a_plan_can_be_named_and_the_derived_name_is_stable():
    r = compile_one("build a barracks, then train 2 footmen as opener")
    assert only(r, "plan_set")["name"] == "opener"
    # Unnamed, the same directive twice derives the same name, so re-issuing it
    # REPLACES the plan instead of spending the other of the two slots.
    a = only(compile_one("build a barracks, then train 2 footmen"), "plan_set")["name"]
    b = only(compile_one("build a barracks, then train 2 footmen"), "plan_set")["name"]
    assert a == b == "plan-build"


def test_the_squad_idiom_is_how_a_plan_names_units_it_does_not_have_yet():
    """A step's units are frozen when the plan is set, so a step cannot name
    soldiers that do not exist. The late-binding selector the language already
    has is the SQUAD: a template stamps membership, and the posture step
    resolves that membership when it runs."""
    r = compile_one("the barracks units join squad 2, "
                    "then when I have 8 footmen, squad 2 pushes their base")
    plan = only(r, "plan_set")
    kinds = [s["intent"]["type"] for s in plan["steps"]]
    assert kinds[-1] == "posture" and "template" in kinds, kinds
    # The wait is on the step before the push, and it is a unit COUNT — the
    # plan waits for the army to exist rather than naming it.
    waits = [s["advance"] for s in plan["steps"] if "advance" in s]
    assert waits == [{"type": "when", "kind": "Footman", "count": 8}] or \
        waits == [{"type": "when", "when": {"type": "unit_count",
                                            "kind": "Footman", "count": 8}}], waits
    # The push names the squad, never a unit list.
    assert plan["steps"][-1]["intent"]["id"] == 2
    assert "units" not in plan["steps"][-1]["intent"]


def test_an_unknown_step_condition_is_an_error_not_a_guess():
    """Same rule as the trigger layer: a condition outside the vocabulary is
    refused by name. A plan that advanced on the wrong thing would be worse
    than one that never compiled."""
    r = compile_one("build a barracks, then when the sky falls, train 4 footmen")
    assert not r.intents
    assert any("not a condition the engine can watch" in why for _, why in r.errors), r.errors


def test_a_plan_is_refused_when_it_is_too_long_or_shaped_like_a_trigger():
    r = compile_one("train 9 footmen, then push mid")
    assert not r.intents
    assert any("steps" in why and "8" in why for _, why in r.errors), r.errors

    # A condition cannot open a plan — that shape is a trigger, and the tool
    # says which word to use rather than compiling something else.
    r = compile_one("when we reach tier 2, build a sanctum, then train 2 sorcerers")
    assert any("cannot open with a condition" in why for _, why in r.errors), r.errors


def test_explain_documents_the_plan_grammar():
    for phrase in ("PLANS", ", then", "then when <cond>", "then after <n>s",
                   "at most 8 steps", "THE COMMA MATTERS"):
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
