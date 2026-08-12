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
  (`state_crossings.json`, no `intel`, no `my_race`) is the real specimen and
  the empty dict is the paranoid one.
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
#: The pre-`intel`, pre-`my_race` snapshot this repo has always carried.
LEGACY = FIX / "state_crossings.json"


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
    s["waiting_for"] = ["blue"]
    s["game_over"] = "Human"
    s["game_over_reason"] = "razed"
    lines = bridge_view.render_digest(bridge_view.digest(s))
    assert len(lines) <= bridge_view.MAX_LINES, f"{len(lines)} lines"
    # The sections that survive the trim are the ones a commander cannot
    # reconstruct from the snapshot for free.
    assert "DEFAULT" in prefixes(lines) and "WIN" in prefixes(lines)


def test_no_line_runs_away():
    for path in LIVE + [EARLY, LEGACY]:
        for line in bridge_view.render_digest(bridge_view.digest(load(path))):
            assert len(line) <= 240, line


# -- it degrades -------------------------------------------------------------


def test_a_snapshot_that_predates_every_optional_key_still_renders():
    """`state_crossings.json` has no `intel`, no `my_race`, no `alarms`."""
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


def test_a_named_stance_passes_through_untouched():
    """When 0uu.2 lands, `squads[].stance` is a bare word and outranks posture."""
    s = load(LIVE[0])
    s["squads"][0]["stance"] = "turtle"
    props = bridge_view.digest(s)
    assert props["squads"][0]["stance"] == "turtle"
    assert props["squads"][0]["stance_phrase"] == "turtle"
    assert "squad 0 keeps turtle" in props["default"]


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
