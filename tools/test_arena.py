#!/usr/bin/env python3
"""Tests for the arena ledger's schema, its honesty rule, and its queries.

    python3 tools/test_arena.py

The schema is the point of the ledger, so most of what is asserted here is what
the validator REFUSES: a round with no question, a null nobody declared, a
declared unknown that isn't missing, a decisive result with no winner. A schema
that only accepts good records is a suggestion.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import arena  # noqa: E402

LEDGER = Path(__file__).resolve().parent.parent / "arena" / "ledger.jsonl"


def good() -> dict:
    """A minimal round that validates — the base every test below deviates from."""
    rec = arena.skeleton("r99", "does the thing still hold?")
    rec.update(
        date="2026-08-10",
        kind="commander",
        provenance="recorded",
        seats=[
            {"seat": "bridge/red", "team": "Claude", "kind": "commander", "persona": "rusher"},
            {"seat": "bridge/blue", "team": "Human", "kind": "commander", "persona": "boomer"},
        ],
    )
    rec["ruleset"].update(map="crossings", commit="abc1234")
    rec["result"].update(
        winner="Claude", winner_persona="rusher", duration_s=561,
        game_over_reason="surrender", decisive=True,
    )
    rec["unknown"] = arena.null_paths(rec)
    return rec


# ---------------------------------------------------------------------------
# Schema
# ---------------------------------------------------------------------------


def test_a_complete_round_validates():
    assert arena.validate(good()) == [], arena.validate(good())


def test_a_round_without_a_question_is_not_a_round():
    rec = good()
    rec["hypothesis"] = "   "
    assert any("hypothesis" in m for m in arena.validate(rec))


def test_missing_and_unexpected_fields_are_both_caught():
    rec = good()
    del rec["verdicts"]
    assert any("missing fields: verdicts" in m for m in arena.validate(rec))

    rec = good()
    rec["winner"] = "Claude"  # right idea, wrong level — it belongs under result
    assert any("unknown fields: winner" in m for m in arena.validate(rec))


def test_enums_are_closed():
    rec = good()
    rec["ruleset"]["map"] = "moonbase"
    assert any("moonbase" in m for m in arena.validate(rec))

    rec = good()
    rec["result"]["game_over_reason"] = "razzed"
    assert any("razzed" in m for m in arena.validate(rec))

    rec = good()
    rec["seats"][0]["team"] = "Red"  # the seat is red; the team is Claude
    assert any("seats.0.team" in m for m in arena.validate(rec))


def test_a_draw_is_a_winner_that_is_absent():
    """Round 2 really had no winner, and the schema has to be able to say so."""
    rec = good()
    rec["result"].update(winner=None, winner_persona=None, decisive=False,
                         game_over_reason="none", duration_s=None)
    rec["unknown"] = arena.null_paths(rec)
    assert arena.validate(rec) == []
    # ...but not while also claiming the round was decisive.
    rec["result"]["decisive"] = True
    assert any("decisive but names no winner" in m for m in arena.validate(rec))


def test_a_round_needs_a_seat():
    rec = good()
    rec["seats"] = []
    assert any("at least one seat" in m for m in arena.validate(rec))


# ---------------------------------------------------------------------------
# The honesty rule
# ---------------------------------------------------------------------------


def test_an_undeclared_null_is_an_error():
    rec = good()
    rec["result"]["duration_s"] = None  # we no longer know how long it ran...
    # ...and did not say so.
    problems = arena.validate(rec)
    assert any("result.duration_s is null but is not listed" in m for m in problems), problems


def test_declaring_something_that_is_not_missing_is_also_an_error():
    rec = good()
    rec["unknown"].append("result.duration_s")
    assert any("which is not missing" in m for m in arena.validate(rec))


def test_null_paths_names_the_exact_seat():
    rec = good()
    rec["seats"][1]["persona"] = None
    assert "seats.1.persona" in arena.null_paths(rec)
    # And a record is valid again once it admits it.
    rec["unknown"] = arena.null_paths(rec)
    assert arena.validate(rec) == []


def test_the_unknown_list_cannot_itself_be_unknown():
    """`unknown` is excluded from the scan, or a null inside it would demand
    that it list itself."""
    rec = good()
    rec["unknown"] = ["result.duration_s"]
    rec["result"]["duration_s"] = None
    assert arena.validate(rec) == []


# ---------------------------------------------------------------------------
# Storage
# ---------------------------------------------------------------------------


def test_append_round_trips_and_refuses_duplicates():
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "ledger.jsonl"
        arena.append(good(), path)
        assert [r["id"] for r in arena.load(path)] == ["r99"]
        try:
            arena.append(good(), path)
        except ValueError as err:
            assert "already in the ledger" in str(err)
        else:
            raise AssertionError("a duplicate round id was accepted")


def test_an_invalid_record_never_reaches_the_file():
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "ledger.jsonl"
        rec = good()
        rec["hypothesis"] = ""
        try:
            arena.append(rec, path)
        except ValueError:
            pass
        else:
            raise AssertionError("an invalid record was appended")
        assert arena.load(path) == []


def test_records_are_written_one_per_line_in_schema_order():
    rec = good()
    line = arena.dumps(rec)
    assert "\n" not in line
    assert list(json.loads(line)) == [k for k in arena.TOP_LEVEL]


def test_next_id_counts_past_the_highest():
    assert arena.next_id([]) == "r1"
    assert arena.next_id([{"id": "r1"}, {"id": "r10"}, {"id": "r2"}]) == "r11"


# ---------------------------------------------------------------------------
# Queries
# ---------------------------------------------------------------------------


def test_series_counts_creeds_and_draws():
    a, b, c = good(), good(), good()
    b["result"].update(winner="Human", winner_persona="boomer")
    c["result"].update(winner=None, winner_persona=None, decisive=False,
                       game_over_reason="none", duration_s=None)
    tally = arena.series([a, b, c])
    assert tally == {"rusher": 1, "boomer": 1, "draws": 1}, tally


def test_the_winning_creed_can_be_inferred_from_the_seat():
    rec = good()
    rec["result"]["winner_persona"] = None
    rec["unknown"] = arena.null_paths(rec)
    assert arena.winning_persona(rec) == "rusher"


# ---------------------------------------------------------------------------
# The committed ledger itself
# ---------------------------------------------------------------------------


def test_the_committed_ledger_validates():
    rounds = arena.load(LEDGER)
    assert rounds, "the ledger is empty"
    for rec in rounds:
        assert arena.validate(rec) == [], f"{rec['id']}: {arena.validate(rec)}"


def test_the_backfilled_series_matches_the_history_it_came_from():
    """The one number the whole prose history agrees on: after round 10 the
    standings were Rusher 6, Boomer 3, one draw. If the backfill reproduces it,
    the ten records are at least mutually consistent with the story.

    Pinned to the BACKFILLED rounds only — the live series keeps growing, and a
    test that froze the whole ledger would fail the moment round 12 was played
    (it did, twice, in two worktrees). The claim under test is about the
    reconstruction, not the future.
    """
    backfilled = [r for r in arena.load(LEDGER) if r["provenance"] == "backfilled"]
    assert len(backfilled) == 10, [r["id"] for r in backfilled]
    tally = arena.series(backfilled)
    assert tally["rusher"] == 6, tally
    assert tally["boomer"] == 3, tally
    assert tally["draws"] == 1, tally


def test_every_backfilled_round_says_where_it_came_from():
    for rec in arena.load(LEDGER):
        if rec["provenance"] == "backfilled":
            assert rec["evidence"]["sources"], f"{rec['id']} cites no source"


def test_cited_evidence_exists_on_disk():
    repo = LEDGER.parent.parent
    for rec in arena.load(LEDGER):
        for aar in rec["evidence"]["aars"]:
            assert (repo / aar["path"]).exists(), f"{rec['id']} cites a missing AAR {aar['path']}"


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
