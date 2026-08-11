#!/usr/bin/env python3
"""Tests for the arena runner's argument handling and log reading.

    python3 tools/test_arena_run.py

The match itself is not testable here — it needs the engine — so what is tested
is everything that decides what the match WILL be: the seat grammar, the
environment those seats imply, and the reading of the verdict back out of the
engine's log. Those are the parts a hand-typed launch line got wrong.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import arena  # noqa: E402
import arena_run  # noqa: E402


class Args:
    """The handful of parsed flags `derive_env` reads."""

    def __init__(self, **kw):
        self.map = "open"
        self.speed = 1.0
        self.cap = 1800.0
        self.windowed = False
        self.env = []
        self.out = "arena"
        self.id = "r99"
        self.__dict__.update(kw)


# ---------------------------------------------------------------------------
# The seat grammar
# ---------------------------------------------------------------------------


def test_a_scripted_seat_parses_to_its_team_and_directory():
    seat = arena_run.parse_seat("red=scripted")
    assert seat["team"] == "Claude"
    assert seat["seat"] == "bridge/red"
    assert seat["kind"] == "scripted"
    # Not null: the scripted AI has no creed, and saying so is different from
    # not knowing which creed it had.
    assert seat["persona"] == "scripted"


def test_a_commander_seat_carries_its_persona_and_brief():
    seat = arena_run.parse_seat("blue=commander:boomer:tools/COMMANDER_BRIEF.md")
    assert seat["team"] == "Human"
    assert seat["seat"] == "bridge/blue"
    assert seat["persona"] == "boomer"
    assert seat["prompt"] == "tools/COMMANDER_BRIEF.md"


def test_a_commander_without_a_named_persona_is_unnamed_not_scripted():
    assert arena_run.parse_seat("red=commander")["persona"] is None


def test_the_seat_grammar_rejects_nonsense():
    for spec in ("red", "green=scripted", "red=lurker", "=scripted"):
        try:
            arena_run.parse_seat(spec)
        except ValueError:
            continue
        raise AssertionError(f"{spec!r} was accepted")


# ---------------------------------------------------------------------------
# The environment the seats imply
# ---------------------------------------------------------------------------


def env_for(*specs, **kw):
    seats = [arena_run.parse_seat(s) for s in specs]
    return arena_run.derive_env(seats, Args(**kw))


def test_two_scripted_seats_turn_the_bridge_off_and_the_ai_on():
    env = env_for("red=scripted", "blue=scripted")
    assert env["BH_BRIDGE"] == "0"
    # Claude is machine-driven anyway; the Human side is the one that needs it.
    assert env["BH_AI_BOTH"] == "1"


def test_two_commander_seats_open_both_sides_of_the_bridge():
    env = env_for("red=commander:rusher", "blue=commander:boomer")
    assert env["BH_BRIDGE"] == "both"
    assert env["BH_AI_BOTH"] == "0"


def test_one_commander_against_the_script():
    """The asymmetric cases are the ones a hand-typed launch line gets wrong."""
    red = env_for("red=commander:rusher", "blue=scripted")
    assert red["BH_BRIDGE"] == "red"
    assert red["BH_AI_BOTH"] == "1", "the scripted blue seat still needs driving"

    blue = env_for("red=scripted", "blue=commander:boomer")
    assert blue["BH_BRIDGE"] == "blue"
    assert blue["BH_AI_BOTH"] == "0", "Claude's side is scripted by default"


def test_headless_is_the_default_and_windowed_removes_it():
    assert env_for("red=scripted", "blue=scripted")["BH_HEADLESS"] == "1"
    assert "BH_HEADLESS" not in env_for("red=commander:a", "blue=commander:b", windowed=True)


def test_the_cap_is_optional():
    assert env_for("red=scripted", "blue=scripted", cap=600)["BH_MAX_GAME_SECS"] == "600"
    assert "BH_MAX_GAME_SECS" not in env_for("red=scripted", "blue=scripted", cap=0)


def test_an_explicit_override_beats_the_derived_value():
    """A probe knob has to be settable — and has to show up in the record, so a
    probe run can never be mistaken for a baseline."""
    env = env_for("red=scripted", "blue=scripted", env=["BH_FOG=0", "BH_AI_BOTH=0"])
    assert env["BH_FOG"] == "0"
    assert env["BH_AI_BOTH"] == "0"


def test_screenshots_are_filed_with_the_round():
    env = env_for("red=scripted", "blue=scripted", id="r42", out="arena")
    assert env["BH_SHOT_DIR"].endswith(os.path.join("arena", "r42", "shots"))


# ---------------------------------------------------------------------------
# Reading the verdict back out
# ---------------------------------------------------------------------------

DECISIVE_LOG = """
INFO [ 300.0s] Human: gold 812 lumber 240 supply 30/100 | 24 units, 9 buildings | 12 Footman
INFO [ 300.0s] Claude: gold 1539 lumber 370 supply 40/100 | 33 units, 12 buildings | 14 Footman
INFO Human surrenders at t=324s — Claude wins
INFO headless: game over — Claude wins (surrender) at t=324.0s — exiting
"""

CAP_LOG = """
INFO [1800.0s] Human: gold 4900 lumber 4200 supply 100/100 | 44 units, 25 buildings | 20 Archer
INFO [1800.0s] Claude: gold 300 lumber 100 supply 90/100 | 40 units, 11 buildings | 18 Footman
INFO headless: time cap 1800s — timeout verdict: Human wins on score (Human 812 vs Claude 640)
"""


def test_a_decisive_ending_yields_winner_reason_and_game_clock():
    v = arena_run.read_log(DECISIVE_LOG)
    assert v["winner"] == "Claude"
    assert v["reason"] == "surrender"
    assert v["duration_s"] == 324.0
    assert v["decisive"] is True


def test_the_closing_position_comes_from_the_last_status_line():
    m = arena_run.read_log(DECISIVE_LOG)["metrics"]
    assert m["claude_gold_final"] == 1539
    assert m["human_units_final"] == 24
    assert m["claude_buildings_final"] == 12


def test_a_time_cap_is_recorded_as_a_score_verdict_not_a_win():
    v = arena_run.read_log(CAP_LOG)
    assert v["winner"] == "Human"
    # The engine recognises exactly two endings and this is neither: a cap is a
    # referee's opinion, and the ledger must not be able to spell it "razed".
    assert v["reason"] == "score"
    assert v["decisive"] is False
    assert v["duration_s"] == 1800.0


def test_an_old_log_without_the_reason_or_clock_still_parses():
    """Rounds 1-8 were logged before either was printed. The reader widened
    rather than moved, so old sweep logs keep working."""
    v = arena_run.read_log("INFO headless: game over — Human wins — exiting\n")
    assert v["winner"] == "Human"
    assert v["reason"] is None
    assert v["duration_s"] is None
    assert v["decisive"] is True


def test_a_crashed_run_has_no_verdict_and_says_so():
    v = arena_run.read_log("thread 'main' panicked at src/main.rs:1:1\n")
    assert v["winner"] is None
    assert v["decisive"] is False


# ---------------------------------------------------------------------------
# Safety
# ---------------------------------------------------------------------------


def test_the_process_check_never_finds_itself():
    """`pgrep -f target/debug/bridgehead` would match this script's own command
    line, which carries that path. The check matches the executable instead."""
    assert os.getpid() not in [pid for pid, _ in arena_run.running_engines(Path(sys.executable))]
    assert arena_run.running_engines(Path("/nonexistent/wc3clone-not-a-real-binary")) == []


def test_the_process_check_does_find_a_live_engine():
    with tempfile.TemporaryDirectory() as tmp:
        fake = Path(tmp) / "bridgehead"
        shutil.copy(shutil.which("sleep"), fake)
        proc = subprocess.Popen([str(fake), "30"])
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                found = [pid for pid, _ in arena_run.running_engines(fake)]
                if proc.pid in found:
                    break
                time.sleep(0.05)
            assert proc.pid in found, "a running engine went undetected"
        finally:
            # Our own child, by the pid we hold — never a pattern sweep.
            proc.terminate()
            proc.wait(timeout=10)
        assert proc.pid not in [pid for pid, _ in arena_run.running_engines(fake)]


def test_an_existing_seat_directory_is_never_deleted():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "red").mkdir()
        (root / "red" / "state.json").write_text('{"game_over": "Claude", "t": 99}')
        seats = [arena_run.parse_seat("red=commander:rusher")]

        # Without --reuse-seat it refuses outright.
        try:
            arena_run.prepare_seats(seats, root, reuse=False)
        except SystemExit as err:
            assert "will not delete" in str(err)
        else:
            raise AssertionError("it started on top of an existing seat")

        # With it, the stale snapshot is moved aside rather than removed — a
        # leftover `game_over` would otherwise read as this match ending at once.
        created = arena_run.prepare_seats(seats, root, reuse=True)
        assert created == [], "an existing directory was reported as created"
        assert not (root / "red" / "state.json").exists()
        assert list((root / "red").glob("state.prev-*.json")), "the snapshot was deleted"


def test_a_seat_directory_it_makes_is_reported_as_its_own():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "bridge"
        seats = [arena_run.parse_seat("blue=commander:boomer")]
        created = arena_run.prepare_seats(seats, root, reuse=False)
        assert created == [root / "blue"]
        assert seats[0]["dir"] == str(root / "blue")


# ---------------------------------------------------------------------------
# The record it builds
# ---------------------------------------------------------------------------


def test_the_recorded_round_validates_and_names_the_winning_creed():
    seats = [arena_run.parse_seat("red=commander:rusher"), arena_run.parse_seat("blue=scripted")]
    args = Args(notes="", commit=None, hypothesis="does the rush still win?", id="r99")
    env = arena_run.derive_env(seats, args)
    rec = arena_run.build_record(args, seats, env, arena_run.read_log(DECISIVE_LOG))
    assert arena.validate(rec) == [], arena.validate(rec)
    assert rec["result"]["winner"] == "Claude"
    assert rec["result"]["winner_persona"] == "rusher"
    assert rec["kind"] == "mixed"
    assert rec["provenance"] == "recorded"
    # The environment the match actually ran under, not the one someone meant.
    assert rec["ruleset"]["env"]["BH_BRIDGE"] == "red"


def test_a_round_with_no_verdict_still_produces_a_valid_record():
    """A crashed or interrupted match must still be recordable — an experiment
    that produced nothing is a result, and losing it is how a series starts
    lying about its own denominator."""
    seats = [arena_run.parse_seat("red=scripted"), arena_run.parse_seat("blue=scripted")]
    args = Args(notes="", commit=None, hypothesis="did it crash?", id="r99")
    env = arena_run.derive_env(seats, args)
    rec = arena_run.build_record(args, seats, env, arena_run.read_log(""))
    assert arena.validate(rec) == [], arena.validate(rec)
    assert rec["result"]["winner"] is None
    assert "result.winner" in rec["unknown"]


def test_the_handshake_credits_each_seat_when_it_leaves_the_waiting_list():
    """docs/INTENT.md, "The ready handshake". `record_readies` is the whole of
    what reaches the ledger, so it is the whole of what needs pinning."""
    red = arena_run.parse_seat("red=commander:rusher")
    blue = arena_run.parse_seat("blue=commander:boomer")
    by_side = {"red": red, "blue": blue}

    # First observation: both still owed. Nobody is credited, because `last`
    # is None and we have not seen anyone leave.
    arena_run.record_readies(["red", "blue"], None, by_side, 3.0)
    assert "ready_wait_s" not in red and "ready_wait_s" not in blue

    # Red speaks at wall 12.4s.
    arena_run.record_readies(["blue"], ["red", "blue"], by_side, 12.44)
    assert red["ready_wait_s"] == 12.4
    assert "ready_wait_s" not in blue

    # Blue speaks at 41s and the hold lifts.
    arena_run.record_readies([], ["blue"], by_side, 41.0)
    assert blue["ready_wait_s"] == 41.0
    # Red is not re-credited by a later observation.
    assert red["ready_wait_s"] == 12.4


def test_a_seat_that_never_readied_carries_no_ready_wait_into_the_ledger():
    """A timeout start leaves the silent seat with no key at all — absence, not
    a null, so the round's `unknown` list stays a claim about things there were
    to know."""
    seats = [
        arena_run.parse_seat("red=commander:rusher"),
        arena_run.parse_seat("blue=commander:boomer"),
    ]
    by_side = {s["side"]: s for s in seats}
    arena_run.record_readies(["red", "blue"], None, by_side, 1.0)
    arena_run.record_readies(["blue"], ["red", "blue"], by_side, 9.0)
    # ...and then the timeout fires; blue is never observed leaving the list.

    args = Args(notes="", commit=None, hypothesis="does a dead seat sink it?", id="r99")
    env = arena_run.derive_env(seats, args)
    rec = arena_run.build_record(args, seats, env, arena_run.read_log(DECISIVE_LOG))
    assert arena.validate(rec) == [], arena.validate(rec)
    assert rec["seats"][0]["ready_wait_s"] == 9.0
    assert "ready_wait_s" not in rec["seats"][1]
    assert not any("ready_wait_s" in u for u in rec["unknown"])


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
