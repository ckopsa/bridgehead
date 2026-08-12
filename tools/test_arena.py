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


def test_a_score_round_is_never_decisive():
    """The ledger boundary enforces what both readers already believe
    (wc3clone-j84): a time-cap verdict names a winner on assets and is still
    not a win the game recognises. Recorded rounds get this right by
    construction; a backfill or a hand-written record has no such habit, and
    this is the only place that catches one."""
    rec = good()
    rec["result"].update(game_over_reason="score", decisive=True)
    assert any("decisive on a `score` round" in m for m in arena.validate(rec)), arena.validate(rec)
    # The same round told honestly passes...
    rec["result"]["decisive"] = False
    assert arena.validate(rec) == [], arena.validate(rec)
    # ...and `razed` at the same numbers is a real win, so the check cannot be
    # reading `decisive` alone.
    rec["result"].update(game_over_reason="razed", decisive=True)
    assert arena.validate(rec) == []


def test_the_scaffold_a_seat_played_with_is_recorded_per_seat():
    """docs/AFFORDANCES.md constraint 3. Optional and additive: present on the
    seat that read the affordance document, absent on the one that played bare
    — which is what makes an A/B round legible."""
    rec = good()
    rec["seats"][0]["scaffold"] = "affordance-doc/1"
    rec["ruleset"]["constants"]["affordance_doc"] = "affordance-doc/1"
    assert arena.validate(rec) == [], arena.validate(rec)
    # The unscaffolded seat omits the key; nothing lands in `unknown`, because
    # "played bare" is a fact and not a gap.
    assert "seats.1.scaffold" not in arena.null_paths(rec)
    # An empty or mistyped version is caught rather than compared later.
    rec["seats"][0]["scaffold"] = ""
    assert any("seats.0.scaffold" in m for m in arena.validate(rec))
    rec["seats"][0]["scaffold"] = 1
    assert any("seats.0.scaffold" in m for m in arena.validate(rec))


def test_the_model_that_sat_in_a_seat_is_recorded_on_the_seat():
    """The other half of docs/AFFORDANCES.md constraint 3's "model+scaffold".
    The schema example has shown `model` on a seat since the document was
    written; nothing wrote one and nothing checked one, so an arena result
    recorded half its own independent variable.

    Free-form, because model ids are somebody else's vocabulary and change
    faster than this file: a closed set here would refuse a valid round every
    time a model shipped, which is a worse failure than a typo you can grep
    for. Optional and absent-not-null, like `scaffold` beside it.
    """
    rec = good()
    rec["seats"][0]["model"] = "opus"
    assert arena.validate(rec) == [], arena.validate(rec)
    assert "seats.1.model" not in arena.null_paths(rec)
    for bad in ("", "   ", 1, None, ["opus"]):
        rec["seats"][0]["model"] = bad
        assert any("seats.0.model" in m for m in arena.validate(rec)), bad


def test_the_tuning_digests_must_look_like_digests():
    """`alarms_ron` and `stances_ron` are written by a tool and compared for
    equality across rounds, so a truncated or uppercased one would read as a
    retune that never happened."""
    rec = good()
    rec["ruleset"]["constants"].update(alarms_ron="0a1b2c3d4e5f", stances_ron="deadbeef1234")
    assert arena.validate(rec) == [], arena.validate(rec)
    for bad_digest in ("0A1B2C3D4E5F", "0a1b2c", "0a1b2c3d4e5f9", "not-a-hash"):
        rec["ruleset"]["constants"]["alarms_ron"] = bad_digest
        assert any("alarms_ron" in m for m in arena.validate(rec)), bad_digest


def test_constants_stays_open_for_the_values_nobody_typed_a_rule_for():
    """`mine_gold` is the whole reason the field exists (round 9 -> round 10).
    Only the tool-written keys are typed; the rest is a round saying what it
    was played under."""
    rec = good()
    rec["ruleset"]["constants"].update(mine_gold=5000, some_new_lever="on")
    assert arena.validate(rec) == [], arena.validate(rec)


def test_an_empty_affordance_doc_version_is_not_a_scaffold():
    rec = good()
    rec["ruleset"]["constants"]["affordance_doc"] = "  "
    assert any("affordance_doc" in m for m in arena.validate(rec))


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


def test_a_seat_may_record_how_long_it_took_to_ready():
    """`ready_wait_s` is additive and optional (docs/INTENT.md, "The ready
    handshake"): present on a seat that waited, absent on one that did not, and
    absent on every round recorded before the handshake existed."""
    rec = good()
    rec["seats"][0]["ready_wait_s"] = 41.0
    assert arena.validate(rec) == []
    # The seat that never waited omits it, and that is not a null — nothing
    # lands in `unknown`, so the honesty rule stays quiet.
    assert "seats.1.ready_wait_s" not in arena.null_paths(rec)
    assert arena.validate(rec) == []
    # Zero is a real answer (a seat that readied instantly), not a missing one.
    rec["seats"][1]["ready_wait_s"] = 0
    assert arena.validate(rec) == []


def test_a_ready_wait_that_is_not_a_duration_is_caught():
    rec = good()
    rec["seats"][0]["ready_wait_s"] = "41s"
    assert any("ready_wait_s" in m for m in arena.validate(rec))
    rec["seats"][0]["ready_wait_s"] = -1
    assert any("ready_wait_s" in m for m in arena.validate(rec))
    # `True` is an int in Python and would otherwise sail through.
    rec["seats"][0]["ready_wait_s"] = True
    assert any("ready_wait_s" in m for m in arena.validate(rec))


def test_a_null_ready_wait_must_still_be_declared():
    """The one way to get it wrong: emitting `null` instead of omitting the key
    puts a claim in the record that the honesty rule then insists you own."""
    rec = good()
    rec["seats"][0]["ready_wait_s"] = None
    assert "seats.0.ready_wait_s" in arena.null_paths(rec)
    assert any("not listed in `unknown`" in m for m in arena.validate(rec))


# ---------------------------------------------------------------------------
# Autopilot: the spans, the schema, and the flag
# ---------------------------------------------------------------------------


def intent(t, team, on, ok=True):
    """One `autopilot` line as `intent.rs` writes it."""
    return {"t": t, "team": team, "source": "bridge", "verb": "autopilot",
            "sentence": "hand the faction to the scripted AI", "ok": ok,
            "intent": {"type": "autopilot", "on": on}}


def test_a_closed_span_is_the_two_intents_that_bound_it():
    """r33, exactly: engaged at t=189.3, took the faction back at t=450.7."""
    spans = arena.autopilot_spans(
        [intent(189.3, "Claude", True), intent(450.7, "Claude", False)], end_t=460.5
    )
    assert spans == {"Claude": [{"from": 189.3, "to": 450.7}]}
    assert arena.autopilot_secs(spans["Claude"]) == 261.4


def test_a_span_nobody_closed_runs_to_the_end_of_the_match():
    """r35: engaged at t=316 and never released, winning at t=472.8. Marked
    `to_end` so an inferred close cannot be misread as a commander taking the
    faction back."""
    spans = arena.autopilot_spans([intent(316.0, "Human", True)], end_t=472.8)
    assert spans == {"Human": [{"from": 316.0, "to": 472.8, "to_end": True}]}
    assert arena.autopilot_secs(spans["Human"]) == 156.8


def test_a_refused_handover_is_not_a_span():
    """`BH_NO_AUTOPILOT=1` logs the attempt with ok:false and changes nothing;
    counting it would put ai.rs time on a seat that played the whole match."""
    assert arena.autopilot_spans([intent(100.0, "Claude", True, ok=False)], end_t=300) == {}


def test_engaging_twice_is_one_span_and_a_stray_release_is_nothing():
    """`set_autopilot` is idempotent, so the log's edges are the truth and a
    doubled verb must not open a second span or double the total."""
    spans = arena.autopilot_spans(
        [intent(10.0, "Claude", False),   # never engaged: nothing to release
         intent(20.0, "Claude", True),
         intent(30.0, "Claude", True),    # already on
         intent(50.0, "Claude", False)],
        end_t=100.0,
    )
    assert spans == {"Claude": [{"from": 20.0, "to": 50.0}]}


def test_two_seats_are_counted_apart():
    spans = arena.autopilot_spans(
        [intent(10.0, "Claude", True), intent(20.0, "Human", True),
         intent(30.0, "Claude", False)],
        end_t=100.0,
    )
    assert spans["Claude"] == [{"from": 10.0, "to": 30.0}]
    assert spans["Human"] == [{"from": 20.0, "to": 100.0, "to_end": True}]


def test_without_a_duration_the_span_closes_at_the_last_clock_in_the_log():
    """A floor on the delegation, never an invention past it — the same rule
    `read_log`'s `last_status_t` follows."""
    spans = arena.autopilot_spans(
        [intent(10.0, "Claude", True), {"t": 42.0, "team": "Claude", "verb": "move", "ok": True}]
    )
    assert spans == {"Claude": [{"from": 10.0, "to": 42.0, "to_end": True}]}


def test_a_seat_records_how_long_the_scripted_ai_played_for_it():
    rec = good()
    rec["seats"][0]["autopilot_secs"] = 261.4
    rec["seats"][0]["autopilot_spans"] = [{"from": 189.3, "to": 450.7}]
    rec["seats"][1]["autopilot_secs"] = 0.0
    assert arena.validate(rec) == [], arena.validate(rec)
    # A measured zero is a claim, not a gap: nothing lands in `unknown`.
    assert arena.null_paths(rec) == rec["unknown"]


def test_spans_that_do_not_add_up_to_the_total_are_caught():
    """The stamp and the summary read the same record; if they can disagree,
    the ledger has two answers for one round."""
    rec = good()
    rec["seats"][0]["autopilot_secs"] = 60.0
    rec["seats"][0]["autopilot_spans"] = [{"from": 100.0, "to": 400.0}]
    assert any("add up to" in m for m in arena.validate(rec))


def test_a_malformed_span_is_refused():
    rec = good()
    rec["seats"][0]["autopilot_secs"] = 0.0
    rec["seats"][0]["autopilot_spans"] = []
    assert any("non-empty list" in m for m in arena.validate(rec))

    rec = good()
    rec["seats"][0]["autopilot_secs"] = 10.0
    rec["seats"][0]["autopilot_spans"] = [{"from": 20.0, "to": 10.0}]
    assert any("ends before it starts" in m for m in arena.validate(rec))

    rec = good()
    rec["seats"][0]["autopilot_secs"] = 10.0
    rec["seats"][0]["autopilot_spans"] = [{"from": 0.0, "to": 10.0, "why": "raid"}]
    assert any("unknown keys" in m for m in arena.validate(rec))

    rec = good()
    rec["seats"][0]["autopilot_spans"] = [{"from": 0.0, "to": 10.0}]
    assert any("without autopilot_secs" in m for m in arena.validate(rec))


def test_a_scripted_seat_cannot_carry_autopilot_time():
    """ai.rs is already playing that faction — there is no handover to record,
    and stamping one would put a scripted round in a delegation comparison."""
    rec = good()
    rec["seats"][0]["kind"] = "scripted"
    rec["seats"][0]["persona"] = "scripted"
    rec["seats"][0]["autopilot_secs"] = 0.0
    assert any("cannot delegate" in m for m in arena.validate(rec))


def test_an_unmeasured_round_is_not_a_round_measured_at_zero():
    rec = good()
    assert not arena.autopilot_measured(rec)
    assert arena.autopilot_cell(rec) == ""
    rec["seats"][0]["autopilot_secs"] = 0.0
    rec["seats"][1]["autopilot_secs"] = 0.0
    assert arena.autopilot_measured(rec)
    assert arena.autopilot_cell(rec) == "none"


def test_a_win_the_winner_spent_delegating_is_flagged_in_the_round_table():
    """The bead's whole complaint: r33 and r35 read as unassisted victories."""
    rec = good()                       # winner is Claude, on bridge/red
    rec["seats"][0]["autopilot_secs"] = 261.4
    rec["seats"][1]["autopilot_secs"] = 0.0
    assert arena.winner_delegated(rec)
    assert "red 261s" in arena.autopilot_cell(rec)
    assert "*" in arena.one_line(rec)
    # The loser delegating is recorded but does not qualify the win.
    rec["seats"][0]["autopilot_secs"] = 0.0
    rec["seats"][1]["autopilot_secs"] = 99.0
    assert not arena.winner_delegated(rec)
    assert "*" not in arena.one_line(rec)


def test_the_series_line_separates_unmeasured_rounds_from_clean_ones():
    clean, dirty = good(), good()
    dirty["id"] = "r98"
    for seat in clean["seats"]:
        seat["autopilot_secs"] = 0.0
    dirty["seats"][0]["autopilot_secs"] = 261.4
    dirty["seats"][1]["autopilot_secs"] = 0.0
    line = arena.autopilot_summary([good(), clean, dirty])
    assert "1 of 2 measured rounds (3 on file)" in line
    assert "r98" in line and "r99" not in line.split("\n")[0]
    assert arena.autopilot_summary([good()]) == "autopilot: unmeasured on every round on file"
    assert "none, across 1/1" in arena.autopilot_summary([clean])


def test_the_ledgers_own_autopilot_rounds_are_the_ones_ladder2_names():
    """arena/LADDER2.md's addendum says two of four Haiku seats delegated and
    both ended on the winning side. That claim is now in the ledger as numbers,
    and this is the test that keeps the two in step."""
    rounds = arena.load(LEDGER)
    used = [r["id"] for r in rounds if arena.delegating_seats(r)]
    assert used == ["r33", "r35"], used
    assert all(arena.winner_delegated(r) for r in rounds if r["id"] in used)
    by_id = {r["id"]: r for r in rounds}
    red = [s for s in by_id["r33"]["seats"] if s["seat"] == "bridge/red"][0]
    blue = [s for s in by_id["r35"]["seats"] if s["seat"] == "bridge/blue"][0]
    assert red["autopilot_secs"] == 261.4 and red["autopilot_spans"][0]["from"] == 189.3
    assert blue["autopilot_secs"] == 156.8 and blue["autopilot_spans"][0]["to_end"] is True


def test_reading_an_intent_log_from_an_offset_skips_the_earlier_match():
    """The live log is append-only across matches, so a round that read it
    whole would inherit the previous round's delegation."""
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "intent_log.jsonl"
        first = json.dumps(intent(50.0, "Claude", True)) + "\n"
        path.write_text(first + json.dumps(intent(60.0, "Human", True)) + "\n")
        assert len(arena.read_intent_log(path)) == 2
        later = arena.read_intent_log(path, len(first.encode()))
        assert [r["team"] for r in later] == ["Human"]
        assert arena.read_intent_log(Path(tmp) / "nope.jsonl") == []


def test_a_session_header_and_a_torn_line_are_both_skipped():
    """A log truncated by a crash is still evidence about what it did write."""
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "intent_log.jsonl"
        path.write_text(
            '{"wall_ms":1,"session":"wc3clone-intent-log-v1","note":"..."}\n'
            + json.dumps(intent(10.0, "Claude", True)) + "\n"
            + '{"t":20.0,"team":"Claude","verb":"mov'
        )
        assert [r["verb"] for r in arena.read_intent_log(path)] == ["autopilot"]


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
