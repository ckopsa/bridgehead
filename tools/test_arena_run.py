#!/usr/bin/env python3
"""Tests for the arena runner's argument handling and log reading.

    python3 tools/test_arena_run.py

The match itself is not testable here — it needs the engine — so what is tested
is everything that decides what the match WILL be: the seat grammar, the
environment those seats imply, and the reading of the verdict back out of the
engine's log. Those are the parts a hand-typed launch line got wrong.
"""

from __future__ import annotations

import json
import os
import re
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
# The scaffold and the tuning it depends on (AFFORDANCES.md constraint 3)
# ---------------------------------------------------------------------------


def scaffolded(*specs, sides=("red",)):
    seats = [arena_run.parse_seat(s) for s in specs]
    arena_run.mark_scaffolds(seats, list(sides), arena_run.scaffold_version())
    return seats


def test_the_scaffold_version_is_the_documents_own():
    """Not a copy: a second spelling of the version is a ledger that can
    disagree with the tool it is recording."""
    import affordances  # noqa: PLC0415

    assert arena_run.scaffold_version() == affordances.DOC_VERSION


def test_only_the_scaffolded_seat_carries_the_document():
    """The A/B round is the reason this is per seat and not per round: the same
    model in both chairs, the document in one of them."""
    red, blue = scaffolded("red=commander:haiku", "blue=commander:haiku", sides=("red",))
    assert red["scaffold"] == arena_run.scaffold_version()
    assert "scaffold" not in blue, "the bare seat was stamped too"


def test_both_scaffolds_every_commander_seat():
    seats = scaffolded("red=commander:a", "blue=commander:b", sides=("both",))
    assert all(s.get("scaffold") for s in seats)
    # ...and a comma list is the same thing said the long way.
    seats = scaffolded("red=commander:a", "blue=commander:b", sides=("red,blue",))
    assert all(s.get("scaffold") for s in seats)


def test_a_scripted_seat_cannot_be_scaffolded():
    """The document is a rendering of a bridge seat's own snapshot. Stamping a
    scripted seat would put a scaffold in the ruleset of a round that had
    none — the exact dishonesty the field exists to prevent."""
    for spec, sides in (("blue=scripted", ("blue",)), ("blue=scripted", ("both",))):
        try:
            scaffolded("red=commander:rusher", spec, sides=sides)
        except ValueError as err:
            assert "scripted" in str(err), err
        else:
            raise AssertionError(f"--scaffold {sides} stamped a scripted seat")


def test_the_scaffold_flag_rejects_a_side_that_is_not_playing():
    for sides in (("green",), ("blue",)):
        try:
            scaffolded("red=commander:rusher", sides=sides)
        except ValueError:
            continue
        raise AssertionError(f"--scaffold {sides} was accepted")


def test_an_unscaffolded_round_says_nothing_about_the_document():
    """The stamp is conditional on purpose: an unconditional one would make
    every round look scaffolded, and the comparison would be worthless."""
    seats = [arena_run.parse_seat("red=scripted"), arena_run.parse_seat("blue=scripted")]
    consts = arena_run.ruleset_constants(seats, {})
    assert "affordance_doc" not in consts
    # ...but the tuning the scaffold reads is still recorded, because it moved
    # this round whether or not anybody read a document about it.
    assert set(consts) == set(arena.TUNING_FILES)


def test_a_scaffolded_round_names_the_document_version_in_the_ruleset():
    seats = scaffolded("red=commander:haiku", "blue=commander:haiku")
    consts = arena_run.ruleset_constants(seats, {})
    assert consts["affordance_doc"] == arena_run.scaffold_version()


# ---------------------------------------------------------------------------
# The other half of model+scaffold
# ---------------------------------------------------------------------------


def modelled(*specs, models=("red=opus",)):
    seats = [arena_run.parse_seat(s) for s in specs]
    arena_run.mark_models(seats, list(models))
    return seats


def test_each_seat_carries_the_model_that_sat_in_it():
    """AFFORDANCES.md constraint 3 says an arena result measures
    model+scaffold. The ledger recorded the scaffold and left the model to a
    commit message, so half of every result was unrecorded — and the ladder is
    nothing but a comparison of that half."""
    red, blue = modelled("red=commander:rusher", "blue=commander:boomer",
                         models=("red=opus,blue=haiku",))
    assert red["model"] == "opus"
    assert blue["model"] == "haiku"


def test_both_puts_one_model_in_every_commander_chair():
    """The A/B round the scaffold field exists for: the same model in both
    chairs, the document in one of them."""
    seats = modelled("red=commander:a", "blue=commander:b", models=("both=haiku",))
    assert [s["model"] for s in seats] == ["haiku", "haiku"]


def test_a_seat_nobody_named_a_model_for_has_no_model_key():
    """Absence, not a null — the same rule `scaffold` and `ready_wait_s`
    follow. A round run before anybody typed `--model` is not a round with an
    unknown model in the sense `unknown[]` means."""
    red, blue = modelled("red=commander:a", "blue=commander:b", models=("red=opus",))
    assert red["model"] == "opus"
    assert "model" not in blue


def test_a_scripted_seat_cannot_be_given_a_model():
    """The scripted AI is ai.rs. Calling it opus would put a round with no
    model in it into a model-vs-model comparison."""
    for models in (("blue=opus",), ("both=opus",)):
        try:
            modelled("red=commander:rusher", "blue=scripted", models=models)
        except ValueError as err:
            assert "scripted" in str(err), err
        else:
            raise AssertionError(f"--model {models} named a model for ai.rs")


def test_the_model_flag_refuses_a_shape_it_cannot_read():
    for models in (("opus",), ("green=opus",), ("red=",), ("blue=opus",)):
        try:
            modelled("red=commander:rusher", models=models)
        except ValueError:
            continue
        raise AssertionError(f"--model {models} was accepted")


def test_a_recorded_round_carries_the_model_and_validates():
    seats = modelled("red=commander:rusher", "blue=commander:boomer",
                     models=("red=opus,blue=haiku",))
    args = Args(notes="", commit="abc1234", hypothesis="which model?", id="r99")
    rec = arena_run.build_record(args, seats, arena_run.derive_env(seats, args),
                                 arena_run.read_log(DECISIVE_LOG))
    assert arena.validate(rec) == [], arena.validate(rec)
    assert [s["model"] for s in rec["seats"]] == ["opus", "haiku"]
    assert not any("model" in u for u in rec["unknown"])


def test_the_commit_defaults_to_the_head_this_round_was_played_at():
    """`ruleset.commit` was null on every recorded round because it was a flag
    nobody remembered — and it is the only record of which stat tables the
    binary was compiled with, since the engine normally runs the `include_str!`
    copy."""
    head = arena_run.head_commit()
    assert head and re.match(r"^[0-9a-f]{4,40}$", head), head
    out = subprocess.run(
        [sys.executable, str(Path(arena_run.__file__)),
         "--hypothesis", "whose commit?", "--id", "r999",
         "--seat", "red=scripted", "--seat", "blue=scripted", "--dry-run"],
        capture_output=True, text=True,
    )
    assert out.returncode == 0, out.stderr
    assert f"commit: {head}" in out.stdout
    # ...and an explicit --commit still wins, for a round replayed from a tree.
    out = subprocess.run(
        [sys.executable, str(Path(arena_run.__file__)),
         "--hypothesis", "whose commit?", "--id", "r999",
         "--seat", "red=scripted", "--seat", "blue=scripted",
         "--commit", "deadbee", "--dry-run"],
        capture_output=True, text=True,
    )
    assert "commit: deadbee" in out.stdout


def test_a_dry_run_names_the_model_in_each_chair():
    out = subprocess.run(
        [sys.executable, str(Path(arena_run.__file__)),
         "--hypothesis", "does the plan print?", "--id", "r999",
         "--seat", "red=commander:haiku", "--seat", "blue=commander:boomer",
         "--model", "red=haiku,blue=opus", "--dry-run"],
        capture_output=True, text=True,
    )
    assert out.returncode == 0, out.stderr
    assert "red=commander:haiku (haiku)" in out.stdout
    assert "blue=commander:boomer (opus)" in out.stdout


def test_the_tuning_digests_are_stable_and_content_addressed():
    """Same bytes, same digest; one byte different, different digest. This is
    the whole claim the ledger makes when it compares two rounds' constants."""
    with tempfile.TemporaryDirectory() as tmp:
        data = Path(tmp)
        (data / "alarms.ron").write_text("([(id: \"push\", secs: 30.0)])\n")
        (data / "stances.ron").write_text("([(id: \"turtle\")])\n")
        env = {"BH_DATA_DIR": str(data)}
        first = arena_run.ruleset_constants([], env)
        assert first == arena_run.ruleset_constants([], env), "the digest is not stable"
        assert len(first["alarms_ron"]) == arena_run.DIGEST_CHARS
        assert first["alarms_ron"] != first["stances_ron"]

        (data / "alarms.ron").write_text("([(id: \"push\", secs: 45.0)])\n")
        after = arena_run.ruleset_constants([], env)
        assert after["alarms_ron"] != first["alarms_ron"], "a retune left no trace"
        assert after["stances_ron"] == first["stances_ron"], "an untouched table moved"


def test_a_missing_tuning_file_is_an_absent_key_not_a_null():
    """A null would land in the round's `unknown` list and claim we failed to
    learn something there was nothing to learn."""
    with tempfile.TemporaryDirectory() as tmp:
        consts = arena_run.ruleset_constants([], {"BH_DATA_DIR": tmp})
    assert consts == {}
    assert arena_run.file_digest(Path(tmp) / "alarms.ron") is None


def test_the_digests_are_read_from_the_data_directory_the_engine_reads():
    """`BH_DATA_DIR` is what decides which copy of the tables the engine loads
    (src/data.rs). Hashing the repo's copy while the engine read another
    directory would record a tuning that was not in force."""
    with tempfile.TemporaryDirectory() as tmp:
        data = Path(tmp)
        (data / "alarms.ron").write_text("(probe)\n")
        probe = arena_run.ruleset_constants([], {"BH_DATA_DIR": str(data)})
        shipped = arena_run.ruleset_constants([], {})
    assert probe["alarms_ron"] != shipped["alarms_ron"]


def test_the_repos_own_tables_hash_and_reach_the_record():
    """The digests the next real round will carry, checked against the
    validator's shape rule rather than against a frozen value — pinning the
    hash here would mean a bead every time somebody tunes an alarm."""
    seats = [arena_run.parse_seat("red=scripted"), arena_run.parse_seat("blue=scripted")]
    args = Args(notes="", commit=None, hypothesis="what is in force?", id="r99")
    rec = arena_run.build_record(args, seats, arena_run.derive_env(seats, args),
                                 arena_run.read_log(DECISIVE_LOG))
    assert arena.validate(rec) == [], arena.validate(rec)
    for key in arena.TUNING_FILES:
        assert arena.DIGEST_RE.match(rec["ruleset"]["constants"][key]), key


def test_the_recorded_ab_round_says_which_seat_had_the_document():
    """The two halves of constraint 3 in one record: the version in the
    ruleset, the chair on the seat."""
    seats = scaffolded("red=commander:haiku", "blue=commander:haiku", sides=("red",))
    args = Args(notes="", commit=None, hypothesis="does the doc carry haiku?", id="r99")
    rec = arena_run.build_record(args, seats, arena_run.derive_env(seats, args),
                                 arena_run.read_log(DECISIVE_LOG))
    assert arena.validate(rec) == [], arena.validate(rec)
    assert rec["ruleset"]["constants"]["affordance_doc"] == arena_run.scaffold_version()
    assert rec["seats"][0]["scaffold"] == arena_run.scaffold_version()
    assert "scaffold" not in rec["seats"][1]
    # Absence, not a null: the honesty rule has nothing to say about the seat
    # that played bare.
    assert not any("scaffold" in u for u in rec["unknown"])


def test_a_dry_run_prints_the_constants_it_would_record():
    """The dry run is the only look anybody gets at a ruleset before it is
    written, and the digests are the part nobody typed."""
    out = subprocess.run(
        [sys.executable, str(Path(arena_run.__file__)),
         "--hypothesis", "does the plan print?", "--id", "r999",
         "--seat", "red=commander:haiku", "--seat", "blue=commander:boomer",
         "--scaffold", "red", "--dry-run"],
        capture_output=True, text=True,
    )
    assert out.returncode == 0, out.stderr
    assert "alarms_ron=" in out.stdout and "stances_ron=" in out.stdout
    assert f"affordance_doc={arena_run.scaffold_version()}" in out.stdout
    assert "red read tools/bridge_view.py --doc --all once at t=0" in out.stdout


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


DECIDED_CAP_LOG = """
INFO [1800.0s] Human: gold 4900 lumber 4200 supply 100/100 | 44 units, 25 buildings | 20 Archer
INFO [1800.0s] Claude: gold 300 lumber 100 supply 90/100 | 40 units, 11 buildings | 18 Footman
INFO headless: time cap 1800s — timeout verdict: Human wins on score (Human 812 vs Claude 640)
INFO headless: game over — Human wins (score) at t=1800.1s — exiting
"""

DRAWN_CAP_LOG = """
INFO [1800.0s] Human: gold 900 lumber 400 supply 30/100 | 20 units, 8 buildings | 10 Archer
INFO [1800.0s] Claude: gold 900 lumber 400 supply 30/100 | 20 units, 8 buildings | 10 Footman
INFO headless: time cap 1800s — timeout verdict: dead even (Human 812 vs Claude 812)
INFO headless: game over — dead even (score) at t=1800.1s — exiting
"""


def test_a_capped_round_the_engine_decided_is_still_not_a_decisive_win():
    """wc3clone-j84 made the cap decide the match, so the engine now prints the
    game-over line as well as the timeout verdict. The ledger must read the
    same round the same way it did before: `score`, and not decisive."""
    v = arena_run.read_log(DECIDED_CAP_LOG)
    assert v["winner"] == "Human"
    assert v["reason"] == "score"
    assert v["decisive"] is False


def test_a_dead_even_cap_is_a_draw_with_no_winner():
    """docs/ARENA.md: a draw is an absent winner, never a sentinel team. The
    engine says `dead even` rather than `X wins` for exactly this reason."""
    v = arena_run.read_log(DRAWN_CAP_LOG)
    assert v["winner"] is None
    assert v["reason"] == "score"
    assert v["decisive"] is False
    assert v["duration_s"] == 1800.0


def test_the_windowed_watcher_reads_a_draw_out_of_the_snapshot():
    """The other reader of a verdict: a windowed round watches `state.json`
    instead of the log, and the wire spells a draw `game_over: "draw"`. Both
    paths have to land on the same ledger row."""
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        (seat / "state.json").write_text(json.dumps(
            {"t": 1800.0, "game_over": "draw", "game_over_reason": "score"}))
        v = arena_run.wait_for_seat_game_over([seat], time.monotonic() + 5)
    assert v["winner"] is None, "a draw names nobody in the record"
    assert v["reason"] == "score"
    assert v["decisive"] is False


def test_the_windowed_watcher_still_reads_a_real_win():
    with tempfile.TemporaryDirectory() as tmp:
        seat = Path(tmp) / "red"
        seat.mkdir()
        (seat / "state.json").write_text(json.dumps(
            {"t": 324.0, "game_over": "Claude", "game_over_reason": "surrender"}))
        v = arena_run.wait_for_seat_game_over([seat], time.monotonic() + 5)
    assert v["winner"] == "Claude"
    assert v["reason"] == "surrender"
    assert v["decisive"] is True


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
