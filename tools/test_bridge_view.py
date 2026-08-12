#!/usr/bin/env python3
"""Tests for the commander digest — `bridge_view.py --digest`.

    python3 tools/test_bridge_view.py        # or: python3 -m pytest tools/

The digest is docs/AFFORDANCES.md's "snapshot diet": the ~15 lines a small
commander steers from, rendered from a snapshot nobody changed to make it
possible. Three properties are worth a test each, and they are the three that
would be expensive to discover in an arena round:

* **It is a view.** `state.json` is byte-identical with the digest on and off,
  and `digest()` does not mutate the dict it is handed.
* **It degrades.** Every key it reads was added at some point, so a snapshot
  from before that point must still render. The repo's oldest fixture
  (`legacy_crossings.json`, no `intel`, no `my_race`) is the real specimen and
  the empty dict is the paranoid one. That fixture is named for the job: it is
  a specimen of an OLD wire, never a sample of the current one, and the rename
  from `state_crossings.json` was because the old name invited exactly that
  misreading.
* **It stays short.** Fifteen lines that grow to fifty in a late game is the
  problem it was written to fix, so the ceiling is asserted against a snapshot
  built to blow it.

The live fixtures were captured from real headless matches on both maps
(`BH_BRIDGE=red` + `autopilot`, seed 42), not written by hand: a digest tested
only against snapshots its author invented is a test of the author's
imagination.
"""

from __future__ import annotations

import copy
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import bridge_view  # noqa: E402

HERE = Path(__file__).resolve().parent
TOOL = HERE / "bridge_view.py"
FIX = HERE / "fixtures"

#: A mid-match snapshot from each map: two squads, real intel, real queues.
LIVE = [FIX / "digest_open_mid.json", FIX / "digest_crossings_mid.json"]
#: t=8s: five workers, no army, nothing scouted.
EARLY = FIX / "digest_open_early.json"
#: t=236s: squad 1 stanced `stage` with a `defend@(0,0)` anchor underneath it,
#: two named regions, two armed triggers, a running plan.
ARMED = FIX / "doc_open_armed.json"
#: t=388s with an `income_collapse` alarm ringing — the shipped `AlarmOut`
#: shape, whose `running_default` names every standing squad by number.
ALARM = FIX / "doc_open_alarm.json"
#: The pre-`intel`, pre-`my_race` snapshot this repo has always carried.
LEGACY = FIX / "legacy_crossings.json"


def load(path):
    with open(path) as f:
        return json.load(f)


def run(*args, cwd=None):
    out = subprocess.run(
        [sys.executable, str(TOOL), *args], capture_output=True, text=True, timeout=60, cwd=cwd
    )
    assert out.returncode == 0, out.stderr
    return out.stdout


def prefixes(lines):
    return [ln.split(" ", 1)[0] for ln in lines]


# -- it renders, on both maps ------------------------------------------------


def test_the_digest_renders_from_a_live_snapshot_on_both_maps():
    """Every section the design names is on the page, for open and crossings."""
    for path in LIVE:
        lines = bridge_view.render_digest(bridge_view.digest(load(path)))
        got = prefixes(lines)
        for want in ("DIGEST", "RESOURCES", "ARMY", "SQUAD", "PRODUCTION", "WIN", "DEFAULT"):
            assert want in got, f"{path.name}: no {want} line in {got}"


def test_the_cli_prints_the_same_lines_the_function_returns():
    """The CLI is a `print` around the renderer and nothing else."""
    path = LIVE[0]
    out = run("--digest", str(path)).splitlines()
    assert out == bridge_view.render_digest(bridge_view.digest(load(path)))


def test_the_json_mode_hands_back_the_properties_section():
    """`--digest --json` is what the hypermedia document embeds (0uu.3)."""
    path = LIVE[0]
    props = json.loads(run("--digest", "--json", str(path)))
    for key in ("resources", "squads", "production", "win_condition", "events", "default"):
        assert key in props, f"properties section is missing {key}"
    assert props["default"], "the running default is never empty"


def test_the_full_readout_still_works():
    """The digest is a mode, not a replacement — the old invocation is intact."""
    out = run(str(LEGACY))
    assert "WORKERS" in out and "MINES" in out


# -- ~15 lines ---------------------------------------------------------------


def test_the_digest_stays_about_fifteen_lines():
    for path in LIVE + [EARLY, LEGACY]:
        lines = bridge_view.render_digest(bridge_view.digest(load(path)))
        assert len(lines) <= bridge_view.MAX_LINES, f"{path.name}: {len(lines)} lines"
    mid = bridge_view.render_digest(bridge_view.digest(load(LIVE[1])))
    assert 10 <= len(mid) <= 16, f"a mid-game digest should read ~15 lines, got {len(mid)}"


def test_a_late_game_snapshot_cannot_blow_the_ceiling():
    """Twelve squads and forty events still fit — that is what the cap is for."""
    s = load(LIVE[1])
    s["squads"] = [{"id": i, "posture": "push@(0.0,0.0)", "members": 0} for i in range(12)]
    s["events"] = [[float(i), f"event {i}"] for i in range(40)]
    s["alarms"] = [f"alarm {i}" for i in range(6)]
    # ...with every conditional head line showing at once, which cannot really
    # happen (a held match has not started and a finished one has no queues)
    # but is the worst case the renderer must survive.
    s["errors"] = ["cmd 1: cannot afford Footman", "cmd 2: no region named 'x'"]
    # ...and a batch in which every commitment contradicted something, which is
    # the worst case the NOTE section must survive.
    s["notes"] = [
        "cmd {}: accepted; note: push gates not met (squad 2/6); your intel ledger is empty".format(i)
        for i in range(6)
    ]
    s["waiting_for"] = ["blue"]
    s["game_over"] = "Human"
    s["game_over_reason"] = "razed"
    lines = bridge_view.render_digest(bridge_view.digest(s))
    assert len(lines) <= bridge_view.MAX_LINES, f"{len(lines)} lines"
    # The sections that survive the trim are the ones a commander cannot
    # reconstruct from the snapshot for free.
    assert "DEFAULT" in prefixes(lines) and "WIN" in prefixes(lines)
    # The notes COLLAPSE rather than disappear: the text is verbatim in
    # `state.notes`, but the knowledge that there is any is not recoverable
    # from anywhere else the commander is looking.
    note_lines = [ln for ln in lines if ln.startswith("NOTE ")]
    assert len(note_lines) == 1, note_lines
    assert "6 accepted commands contradict" in note_lines[0], note_lines[0]


def test_the_ending_is_named_the_way_it_happened():
    """`game_over` is a team name — or `"draw"`, the one value that is not one
    (wc3clone-j84: a capped match can end dead even and still has to end).
    "draw wins" is the sentence this pins shut."""
    def head_for(game_over, reason):
        s = load(LIVE[1])
        s["game_over"] = game_over
        s["game_over_reason"] = reason
        return " ".join(bridge_view.render_digest(bridge_view.digest(s)))

    won = head_for("Human", "razed")
    assert "GAME OVER: Human wins (razed)" in won, won

    scored = head_for("Claude", "score")
    assert "GAME OVER: Claude wins (score)" in scored, scored

    drawn = head_for("draw", "score")
    assert "GAME OVER: a draw (score)" in drawn, drawn
    assert "draw wins" not in drawn, drawn


def test_no_line_runs_away():
    for path in LIVE + [EARLY, LEGACY]:
        for line in bridge_view.render_digest(bridge_view.digest(load(path))):
            assert len(line) <= 240, line


# -- it degrades -------------------------------------------------------------


def test_a_snapshot_that_predates_every_optional_key_still_renders():
    """`legacy_crossings.json` has no `intel`, no `my_race`, no `alarms`."""
    s = load(LEGACY)
    assert "intel" not in s and "my_race" not in s, "fixture is no longer the old shape"
    props = bridge_view.digest(s)
    assert props["race"] is None
    assert props["alarms"] is None
    lines = bridge_view.render_digest(props)
    assert prefixes(lines).count("ALARM") == 0
    assert "DIGEST t=215s" in lines[0] and "seat=Claude" in lines[0]


def test_an_empty_snapshot_renders_rather_than_raising():
    """A digest that throws KeyError mid-match is worse than no digest."""
    for s in ({}, {"t": 5.0}, {"me": {}, "units": [], "buildings": [], "squads": []}):
        lines = bridge_view.render_digest(bridge_view.digest(copy.deepcopy(s)))
        assert prefixes(lines)[:2] == ["DIGEST", "RESOURCES"]


def test_a_squad_survives_a_snapshot_with_no_unit_rosters():
    """Pre-`units[].squad` snapshots still get a count off the squad record."""
    s = load(LIVE[0])
    for u in s["units"]:
        u.pop("squad", None)
    props = bridge_view.digest(s)
    assert props["squads"][0]["units"] == 5, props["squads"][0]
    assert props["squads"][0]["strength"] == 0, "strength is summed from rosters, honestly"


# -- the early game ----------------------------------------------------------


def test_the_early_game_has_no_army_and_says_so():
    props = bridge_view.digest(load(EARLY))
    assert props["army"]["units"] == 0
    assert props["squads"][0]["units"] == 0
    lines = bridge_view.render_digest(props)
    assert any(ln.startswith("ARMY 0 ") for ln in lines), lines
    assert any("EMPTY" in ln for ln in lines), "an empty squad 0 is the r21 failure"
    assert any(ln.startswith("WIN") and "none seen yet" in ln for ln in lines), lines


# -- the win-condition line is fog-honest ------------------------------------


def test_the_win_line_counts_only_production_this_seat_has_seen():
    s = load(LIVE[1])
    enemy = "Human" if s["my_team"] == "Claude" else "Claude"
    expected = [
        b for b in s["buildings"]
        if b["team"] == enemy and b["kind"] in bridge_view.PRODUCTION_KINDS
    ]
    assert expected, "fixture should have scouted something"
    win = bridge_view.digest(s)["win_condition"]
    assert win["seen"] == len(expected)
    # Farms, towers and walls are not war-making capacity and never count.
    s["buildings"].append(
        {"team": enemy, "kind": "Farm", "pos": [0, 0], "hp": 1, "max_hp": 1, "done": True,
         "queue": [], "tier": 1}
    )
    assert bridge_view.digest(s)["win_condition"]["seen"] == len(expected)


def test_an_unscouted_enemy_is_reported_as_unscouted_not_as_absent():
    """Nothing seen is "none seen yet — scout", never "they have none"."""
    s = load(LIVE[1])
    s["buildings"] = [b for b in s["buildings"] if b["team"] == s["my_team"]]
    line = [ln for ln in bridge_view.render_digest(bridge_view.digest(s)) if ln.startswith("WIN")][0]
    assert "none seen yet" in line and "scout" in line
    assert "explored" in line, "the fog block is the honest denominator"


def test_a_remembered_building_reports_its_age():
    s = load(LIVE[1])
    enemy = "Human" if s["my_team"] == "Claude" else "Claude"
    for b in s["buildings"]:
        if b["team"] == enemy:
            b["last_seen"] = s["t"] - 90.0
    win = bridge_view.digest(s)["win_condition"]
    assert win["remembered"] == win["seen"] and win["oldest_age"] == 90.0
    line = [ln for ln in bridge_view.render_digest(bridge_view.digest(s)) if ln.startswith("WIN")][0]
    assert "remembered" in line and "90s ago" in line


# -- alarms ------------------------------------------------------------------


def test_the_alarm_section_is_absent_until_the_key_exists():
    """0uu.4 adds `alarms[]`; until then the section must not appear at all."""
    s = load(LIVE[0])
    assert "alarms" not in s
    assert bridge_view.digest(s)["alarms"] is None
    assert "ALARM" not in prefixes(bridge_view.render_digest(bridge_view.digest(s)))


def test_an_empty_alarm_list_is_not_the_same_claim_as_no_key():
    s = load(LIVE[0])
    s["alarms"] = []
    assert bridge_view.digest(s)["alarms"] == [], "an empty list means 'nothing is ringing'"


def test_alarms_render_as_strings_or_as_records():
    s = load(LIVE[0])
    s["alarms"] = [
        "income collapse: every mine near your base is dry",
        {"kind": "base_under_attack", "text": "the Keep is taking fire",
         "default": "home-guard recalls squad 1 (ETA 22s)"},
    ]
    lines = bridge_view.render_digest(bridge_view.digest(s))
    alarms = [ln for ln in lines if ln.startswith("ALARM")]
    assert len(alarms) == 2, lines
    assert "income collapse" in alarms[0]
    assert "base_under_attack" in alarms[1] and "default:" in alarms[1]


def test_the_shipped_alarm_shape_renders_every_field_that_matters():
    """The real `AlarmOut` (src/bridge.rs): id / fact / running_default /
    since_t / severity / eta_s. `fact` is the noun, `running_default` is what
    happens if you say nothing, and the ETA is the number that makes
    "recall or sacrifice?" answerable at LLM latency."""
    s = load(LIVE[0])
    s["alarms"] = [
        {
            "id": "places_under_attack",
            "fact": "2 places under attack at once: near our base (TownHall); at (60, 60) (Farm)",
            "running_default": "your trigger home-guard fired at t=203 — squad 1 is closing on near our base (ETA 22s)",
            "since_t": 209.0,
            "severity": "critical",
            "eta_s": 22.0,
        },
        {
            "id": "income_collapse",
            "fact": "income collapse: the one gold mine your hall works is dry",
            "running_default": "nothing recovers this automatically",
            "since_t": 180.0,
            "severity": "warning",
        },
    ]
    props = bridge_view.digest(s)
    assert props["alarms"][0]["since"] == 209.0
    lines = [ln for ln in bridge_view.render_digest(props) if ln.startswith("ALARM")]
    assert len(lines) == 2, lines
    assert "places_under_attack" in lines[0] and "2 places under attack" in lines[0]
    assert "ETA 22s" in lines[0], "the recall ETA has to survive the truncation"
    assert props["alarms"][0]["default"].startswith("your trigger home-guard"), \
        "an alarm names its running default, whether or not the ALARM line fits it"
    assert "income collapse" in lines[1]
    # And the DEFAULT line leads with the reflex, as it does for every alarm.
    assert props["default"].startswith("your trigger home-guard fired")


def test_an_alarms_running_default_leads_the_default_line():
    """AFFORDANCES.md: every alarm names its running default, and silence takes it.

    It leads rather than replaces: the reflex is the urgent half of what
    silence does, and the standing stances go on being true underneath it.
    """
    s = load(LIVE[0])
    s["alarms"] = [{"text": "base under attack", "default": "home-guard recalls squad 1"}]
    props = bridge_view.digest(s)
    assert props["default"].startswith("home-guard recalls squad 1")
    assert "squad 0 keeps defend" in props["default"]
    line = [ln for ln in bridge_view.render_digest(props) if ln.startswith("DEFAULT")][0]
    assert "home-guard recalls squad 1" in line


def test_a_squad_the_alarm_already_named_is_not_named_twice():
    """`income_collapse`'s running default is a full sentence about what every
    squad is doing, so the DEFAULT line used to say "squad 0 (15 units) holds
    defend near our base; …; squad 0 keeps defend near our base" — the same
    squad twice, in two vocabularies, inside one line meant to be read at a
    glance.

    The ALARM's clause is the one that survives: it is the engine's account of
    what the reflex left that squad doing, and it is the half a commander
    cannot reconstruct.
    """
    props = bridge_view.digest(load(ALARM))
    default = props["default"]
    assert [sq["id"] for sq in props["squads"]] == [0, 1], "fixture has both squads"
    assert "squad 0 (15 units) holds" in default, "the alarm's own clause stays"
    assert "squad 0 keeps" not in default and "squad 1 keeps" not in default
    # Not a blanket suppression: an alarm that names ONE squad leaves the
    # others to be reported the ordinary way.
    s = load(ALARM)
    s["alarms"] = [{"text": "base under attack", "default": "home-guard recalls squad 1"}]
    default = bridge_view.digest(s)["default"]
    assert "squad 1 keeps" not in default
    assert "squad 0 keeps" in default


def test_a_squad_number_is_matched_whole_when_deduping():
    """"squad 1" must not swallow "squad 10" — a ten-squad late game is exactly
    when the DEFAULT line matters most."""
    s = load(ALARM)
    s["squads"][1]["id"] = 10
    for u in s["units"]:
        if u.get("squad") == 1:
            u["squad"] = 10
    s["alarms"] = [{"text": "base under attack", "default": "home-guard recalls squad 1"}]
    default = bridge_view.digest(s)["default"]
    assert "squad 10 keeps" in default


# -- the running default -----------------------------------------------------


def test_the_running_default_says_what_silence_does():
    props = bridge_view.digest(load(LIVE[0]))
    line = [ln for ln in bridge_view.render_digest(props) if ln.startswith("DEFAULT")][0]
    assert "squad 0 keeps defend" in line
    assert "squad 1 keeps push" in line
    assert line.endswith(props["default"]), "the DEFAULT line is never truncated"


def test_a_squad_with_no_posture_says_so_rather_than_inventing_one():
    s = load(LIVE[0])
    for sq in s["squads"]:
        sq["posture"] = None
    props = bridge_view.digest(s)
    assert "no standing posture" in props["default"]


def test_a_named_stance_keeps_the_anchor_underneath_it():
    """`squads[].stance` is a bare doctrine word and it OUTRANKS posture — but
    it does not replace it. A stanced squad carries both: the stance names the
    doctrine, the posture underneath carries the ground the stance was
    installed at.

    WAS `test_a_named_stance_passes_through_untouched`, which asserted the
    phrase was the bare word. That threw the anchor away, so a squad staging at
    mid and a squad staging on its own hall both read "stage" — in a document
    whose entire job is to say where things are.

    `stance` stays the bare word, because it is the FACT and the phrase is the
    sentence: a caller matching on the doctrine must not have to strip a
    parenthesis off it.
    """
    s = load(LIVE[0])
    s["squads"][0]["stance"] = "turtle"
    props = bridge_view.digest(s)
    assert props["squads"][0]["stance"] == "turtle"
    assert props["squads"][0]["stance_phrase"] == "turtle (near our base)"
    assert "squad 0 keeps turtle (near our base)" in props["default"]


def test_a_real_stanced_squad_says_where_it_is_staging():
    """The shipped fixtures, not a hand-edited one: squad 1 is `stage` with a
    `defend@(0,0)` underneath it, and read "stage" for as long as the anchor
    was being dropped."""
    props = bridge_view.digest(load(ARMED))
    stanced = [sq for sq in props["squads"] if sq["id"] == 1][0]
    assert stanced["stance"] == "stage"
    assert stanced["stance_phrase"] == "stage (near mid)"
    line = [ln for ln in bridge_view.render_digest(props)
            if ln.startswith("SQUAD 1")][0]
    assert "stage (near mid)" in line


def test_the_squad_line_says_whether_the_push_is_actually_advancing():
    """`squads[].status` (wc3clone-6wa), on the line a commander actually reads.

    r22 set `posture:push` and its army oscillated in front of the crossings
    fords for four hundred game seconds. Every digest of that match rendered
    "SQUAD 1 push (near mid) · 12 units · str …" — the same line it would have
    rendered for a squad walking straight at the objective. The status is the
    only thing that separates them, so it belongs on the line and directly
    behind the posture it qualifies.
    """
    s = load(ARMED)
    sq = [x for x in s["squads"] if x["id"] == 1][0]
    sq["status"] = "gathering"
    props = bridge_view.digest(s)
    stanced = [x for x in props["squads"] if x["id"] == 1][0]
    assert stanced["status"] == "gathering"
    line = [ln for ln in bridge_view.render_digest(props)
            if ln.startswith("SQUAD 1")][0]
    assert "stage (near mid), gathering" in line, line

    sq["status"] = "pressing on"
    line = [ln for ln in bridge_view.render_digest(bridge_view.digest(s))
            if ln.startswith("SQUAD 1")][0]
    assert "stage (near mid), pressing on" in line, line


def test_a_squad_with_no_status_renders_exactly_as_it_always_did():
    """The key is absent on a defend ring, an escort, and every snapshot older
    than the feature. None of those may grow a stray comma."""
    props = bridge_view.digest(load(ARMED))
    for sq in props["squads"]:
        assert sq["status"] is None
    for line in bridge_view.render_digest(props):
        if line.startswith("SQUAD ") or line.startswith("LOOSE "):
            assert ", None" not in line and ", " not in line.split(" · ")[0], line


def test_a_stance_with_no_ground_under_it_is_still_a_bare_word():
    """A squad whose only record is the stance has no anchor to name, and an
    empty parenthesis would be the renderer inventing punctuation."""
    s = load(LIVE[0])
    s["squads"][0].pop("posture", None)
    s["squads"][0]["stance"] = "turtle"
    props = bridge_view.digest(s)
    assert props["squads"][0]["stance_phrase"] == "turtle"


# -- places ------------------------------------------------------------------


def test_ground_is_named_the_way_the_event_feed_names_it():
    smap = load(LIVE[1])["map"]
    assert bridge_view.place_of([0.0, 0.0], smap) == "near the center ford"
    assert bridge_view.place_of([70.0, 70.0], smap) == "near our base"
    assert bridge_view.place_of([-70.0, -70.0], smap) == "near their base"
    # Ground that is near nothing named keeps its coordinates — the digest
    # never rounds a spot to the nearest name it can reach.
    assert bridge_view.place_of([30.0, 30.0], smap) == "at (30, 30)"
    assert bridge_view.place_of(None, smap) == "position unknown"
    assert bridge_view.place_of([1.0, 2.0], {}) == "at (1, 2)"


def test_a_posture_string_becomes_a_phrase_a_sentence_can_hold():
    smap = load(LIVE[1])["map"]
    assert bridge_view.stance_phrase("defend@(70.0,70.0)r=22", smap) == "defend near our base"
    assert bridge_view.stance_phrase("push@(0.0,0.0)", smap) == "push near the center ford"
    assert bridge_view.stance_phrase("escort:12345", smap) == "escort 12345"
    assert bridge_view.stance_phrase(None, smap) == "no standing posture"


# -- strength ----------------------------------------------------------------


def test_a_squad_pools_the_health_of_its_members():
    s = load(LIVE[0])
    props = bridge_view.digest(s)
    for sq in props["squads"]:
        members = [
            u for u in s["units"]
            if u["team"] == s["my_team"] and u.get("squad") == sq["id"] and u["kind"] != "Worker"
        ]
        assert sq["units"] == len(members)
        assert sq["strength"] == round(sum(u["hp"] for u in members))


def test_loose_army_units_get_their_own_line():
    s = load(LIVE[0])
    for u in s["units"]:
        if u["team"] == s["my_team"] and u["kind"] != "Worker":
            u["squad"] = None
    props = bridge_view.digest(s)
    assert props["squads"][-1]["id"] is None
    assert props["squads"][-1]["units"] == props["army"]["units"]
    assert "LOOSE" in prefixes(bridge_view.render_digest(props))


# -- the construction ledger (wc3clone-phc, from arena r26) ------------------


def _with_construction(state, **kw):
    """`state` plus one of MY buildings under construction."""
    s = copy.deepcopy(state)
    b = {
        "id": 999,
        "team": s.get("my_team", "Claude"),
        "kind": "TownHall",
        "pos": [30.0, 30.0],
        "hp": 100.0,
        "max_hp": 1500.0,
        "done": False,
        "queue": [],
        "progress": 0.0,
    }
    b.update(kw)
    s.setdefault("buildings", []).append(b)
    return s


def test_a_site_going_up_reports_its_real_progress_and_not_the_queues():
    """r26-blue's "phantom": `building: TownHall(0%)` on a hall that was most of
    the way up.

    The digest was reading `progress`, which is the TRAINING QUEUE's progress
    and is `0.0` on anything still under scaffolding — so every site in every
    match read `0%` at every stage, and a commander reasonably concluded the
    line was describing a building that did not exist. `build_progress` is the
    fraction that actually means construction.
    """
    s = _with_construction(load(LIVE[0]), build_progress=0.73)
    props = bridge_view.digest(s)
    assert "TownHall(73%)" in props["production"]["building"], props["production"]
    line = [ln for ln in bridge_view.render_digest(props) if ln.startswith("PRODUCTION")]
    assert line and "TownHall(73%)" in line[0], line


def test_a_snapshot_without_the_new_field_says_building_rather_than_zero():
    """Degrade honestly. An older engine cannot tell us how far along the site
    is, and `0%` is the guess that caused the trouble in the first place."""
    props = bridge_view.digest(_with_construction(load(LIVE[0])))
    assert "TownHall(building)" in props["production"]["building"], props["production"]
    assert not any("(0%)" in x for x in props["production"]["building"]), props["production"]


def test_an_accepted_build_is_visible_before_it_breaks_ground():
    """The window `buildings[]` cannot cover. Between "accepted" and "ground
    broken" the build lived in no array the digest read, which is how three of
    r26-blue's expansions went from ordered to never-mentioned in silence."""
    s = copy.deepcopy(load(LIVE[0]))
    mine = s.get("my_team", "Claude")
    s["units"].append(
        {
            "id": 4242,
            "team": mine,
            "kind": "Worker",
            "pos": [10.0, -4.0],
            "hp": 50.0,
            "max_hp": 50.0,
            "order": "Build",
            "moving": True,
            "carrying": False,
            "build_site": {"kind": "Barracks", "pos": [12.0, -4.0]},
        }
    )
    props = bridge_view.digest(s)
    assert props["production"]["walking"] == ["Barracks@(12,-4)"], props["production"]
    line = [ln for ln in bridge_view.render_digest(props) if ln.startswith("PRODUCTION")]
    assert line and "walking: Barracks@(12,-4)" in line[0], line


def test_nothing_walking_adds_no_line():
    props = bridge_view.digest(load(LIVE[0]))
    assert props["production"]["walking"] == []
    line = [ln for ln in bridge_view.render_digest(props) if ln.startswith("PRODUCTION")]
    assert line and "walking:" not in line[0], line


# -- the catalog decides what production means -------------------------------


def test_the_catalog_names_the_production_buildings_when_it_is_there():
    catalog = load(FIX / "catalog.json")
    kinds = bridge_view.production_kinds(catalog)
    assert "Barracks" in kinds and "TownHall" in kinds and "Workshop" in kinds
    assert "Farm" not in kinds and "Shop" not in kinds
    # No catalog, or a catalog with nothing in it, falls back to the table.
    assert bridge_view.production_kinds(None) is bridge_view.PRODUCTION_KINDS
    assert bridge_view.production_kinds({"buildings": []}) is bridge_view.PRODUCTION_KINDS


def test_both_hall_ladders_and_both_races_count_as_production():
    """A kind missing here silently under-counts the win condition."""
    for kind in ("Keep", "Castle", "Stronghold", "Fortress", "Hold", "WarCamp", "SpiritLodge"):
        assert kind in bridge_view.PRODUCTION_KINDS, kind
    for kind in ("Farm", "Tower", "Wall", "Shop", "Blacksmith", "Burrow", "Watchtower", "WarMill"):
        assert kind not in bridge_view.PRODUCTION_KINDS, kind


# -- it is a view ------------------------------------------------------------


def test_digest_does_not_mutate_the_snapshot_it_is_handed():
    s = load(LIVE[1])
    before = json.dumps(s, sort_keys=True)
    bridge_view.digest(s)
    assert json.dumps(s, sort_keys=True) == before


def test_state_json_is_byte_identical_with_the_digest_on_and_off():
    """The acceptance criterion, checked the only way worth checking it."""
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        state = seat / "state.json"
        shutil.copy(LIVE[1], state)
        original = state.read_bytes()
        run("--digest", str(state))
        assert state.read_bytes() == original
        run("--digest", "--json", str(state))
        assert state.read_bytes() == original
        run(str(state))
        assert state.read_bytes() == original, "even the full readout only reads"


def test_the_digest_leaves_no_marker_behind():
    """The full readout keeps a read position; the digest is stateless."""
    with tempfile.TemporaryDirectory() as tmp:
        state = Path(tmp) / "state.json"
        shutil.copy(EARLY, state)
        marker_dir = Path(tmp) / "markers"
        out = subprocess.run(
            [sys.executable, str(TOOL), "--digest", str(state)],
            capture_output=True, text=True, timeout=60,
            env={**os.environ, "BH_MARKER_DIR": str(marker_dir)},
        )
        assert out.returncode == 0, out.stderr
        leftovers = list(marker_dir.glob("*")) if marker_dir.exists() else []
        assert not leftovers, "the digest must not fight bridge_wait for a read position"


# -- acceptance notes (wc3clone-b9m) -----------------------------------------


def test_an_acceptance_note_reaches_the_digest_under_its_own_prefix():
    """The whole point of the feature at this level.

    The engine's advisory arrives in `state.json`'s `notes`, and the digest is
    the page every tier actually reads at loop cadence (arena/LADDER.md,
    Finding 5). It has to be HERE, and it has to be `NOTE` rather than `ERRORS`:
    a note is the echo of a command that was accepted, and filing it under
    errors would teach a commander that the engine refuses pushes.
    """
    s = load(LIVE[0])
    s["notes"] = [
        "cmd 1: accepted; note: push gates not met (squad 4/6, Hero 61%, "
        "gate is 80%); last enemy sighting 190s stale, threshold is 45s"
    ]
    lines = bridge_view.render_digest(bridge_view.digest(s))
    notes = [ln for ln in lines if ln.startswith("NOTE ")]
    assert len(notes) == 1, "expected one NOTE line, got {}".format(lines)
    assert "accepted" in notes[0]
    assert "squad 4/6" in notes[0], "both halves of the comparison survive: {}".format(notes[0])
    # ...and so does the clause AFTER it. The gates come first in the string, so
    # the digest's 110-column truncation would eat the staleness half — the
    # exact half r26 lost to — which is why this line runs at DEFAULT's width.
    assert "190s stale, threshold is 45s" in notes[0], notes[0]
    assert not any(ln.startswith("ERRORS") for ln in lines), (
        "a note is not an error and must not be counted as one"
    )
    # ...and it is carried structurally too, for `--json` readers.
    assert bridge_view.digest(s)["status"]["notes"] == s["notes"]


def test_a_snapshot_with_no_notes_renders_exactly_as_it_did():
    """`notes` is `skip_serializing_if` empty on the wire, so most snapshots
    never carry the key at all. The digest of one must be byte-identical to
    what it was before the key existed."""
    s = load(LIVE[0])
    assert "notes" not in s, "the fixture predates the key — that is the point"
    lines = bridge_view.render_digest(bridge_view.digest(s))
    assert not [ln for ln in lines if ln.startswith("NOTE ")]
    assert bridge_view.digest(s)["status"]["notes"] == []
    assert bridge_view.digest({})["status"]["notes"] == [], "and the paranoid case"


def test_notes_and_errors_are_two_channels_in_one_readout():
    """Both at once, in the same digest, distinguishable at a glance. The
    contrast IS the contract: `cmd 0` was refused and `cmd 1` was not."""
    s = load(LIVE[0])
    s["errors"] = ["cmd 0: no stance called 'charge' - the five are: turtle, stage, push, secure, harass"]
    s["notes"] = ["cmd 1: accepted; note: push gates not met (squad 2/6)"]
    lines = bridge_view.render_digest(bridge_view.digest(s))
    assert any(ln.startswith("ERRORS 1:") for ln in lines)
    assert any(ln.startswith("NOTE cmd 1: accepted") for ln in lines)


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
