#!/usr/bin/env python3
"""The arena ledger: every round of the series as one queryable record.

    tools/arena.py series                    # the standings and the round table
    tools/arena.py show r9                   # one round in full
    tools/arena.py rounds --hypothesis mine  # rounds that tested a question
    tools/arena.py rounds --persona rusher --winner Claude
    tools/arena.py lessons --grep tower      # what the players actually learned
    tools/arena.py validate                  # schema + honesty check
    tools/arena.py add-aar r11 --seat bridge/red --path arena/r11/red-aar.md
    tools/arena.py autopilot r33 --write     # stamp delegation from the intent log

WHY A LEDGER AND NOT A CHANGELOG
--------------------------------
For ten rounds the series lived in prose: a memory file, eight AAR markdowns,
and a paragraph in THESIS.md. Prose is where the *story* belongs and it is a
terrible place to keep an *experiment*, because the question a round was asked
to answer is the first thing narrative drops. "Round 9 ended at 5:24" survives
retelling; "round 9 was run to find out whether 3500-gold mines still leave
room for tier 2" does not — and without it, round 10 is just another match
instead of the answer.

So the unit of this file is not a match. It is a HYPOTHESIS TEST. A record says
what ruleset was in force, who sat in which seat, what question the round was
run to answer, what happened, and — separately from what happened — what the
result licensed anyone to believe. `hypothesis` and `verdicts` are the two
fields that make a round evidence rather than an anecdote; docs/ARENA.md is the
long form of that argument.

THE HONESTY RULE
----------------
Rounds 1-10 were played before this file existed and are backfilled from AARs,
commit messages and memory. Some of their fields are simply not recoverable —
round 2 never produced a verdict at all, and `game_over_reason` did not exist
in the engine until after round 10. A backfill that guessed at those would be
worse than no backfill, because nobody downstream could tell the reconstructed
numbers from the recorded ones.

So a missing value is `null`, never an invented one, and `validate` enforces
that every `null` in the record is also named in its `unknown` list — and that
nothing is named there which isn't actually missing. The list is a claim about
what we don't know, and it is checked. `provenance` says which kind of record
you are reading: `recorded` (a tool watched this match end) or `backfilled`.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LEDGER = REPO / "arena" / "ledger.jsonl"

# ---------------------------------------------------------------------------
# The schema
# ---------------------------------------------------------------------------
#
# Written as data rather than as a class hierarchy because the schema doc, the
# validator and the runner's skeleton all have to agree, and one table they all
# read is the cheapest way to make that true.

ID_RE = re.compile(r"^r\d+$")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
# The short content hashes `arena_run.file_digest` writes into
# `ruleset.constants` for the data tables a round was played under. Pinned to a
# shape so a truncated, uppercased or full-length digest cannot land beside the
# twelve-character ones and compare unequal to a round it is identical to.
DIGEST_RE = re.compile(r"^[0-9a-f]{12}$")

MAPS = ("open", "crossings")
TEAMS = ("Claude", "Human")
SEAT_KINDS = ("commander", "scripted", "copilot")
ROUND_KINDS = ("commander", "scripted", "mixed")
PROVENANCE = ("recorded", "backfilled")
# `razed` and `surrender` are the engine's own two endings (shared.rs
# `GameOverReason`). `score` is the headless time-cap verdict, which is a
# referee's opinion and not a win the game recognises — named differently on
# purpose. `none` is a round that stopped without ending.
END_REASONS = ("razed", "surrender", "score", "none")
VERDICT_STATUS = ("confirmed", "refuted", "unresolved")

#: The data tables whose tuning a round is played under but which appear
#: nowhere in `env`, as `ruleset.constants` key -> file in `assets/data/`.
#: `arena_run.ruleset_constants` writes the digests and the validator below
#: checks their shape, so the two read one table rather than agreeing twice.
#:
#: `alarms.ron` decides when a commander is forced to re-decide and
#: `stances.ron` decides what each stance word does; both move every round
#: after a retune, which is precisely what a ledger comparison must be able to
#: see (docs/AFFORDANCES.md constraint 3).
#:
#: `playbooks.ron` is the loudest of the three by that standard, because it is
#: the one that carries authored STRATEGY rather than tuning: constraint 3 is
#: what permits judgment in the scaffold at all, and only on condition that the
#: judgment is versioned in the ruleset. A round played with a retuned playbook
#: and a round played with the old one are two experiments, and the digest is
#: the only thing that can say so.
TUNING_FILES = {
    "alarms_ron": "alarms.ron",
    "stances_ron": "stances.ron",
    "playbooks_ron": "playbooks.ron",
}

TOP_LEVEL = (
    "id",
    "date",
    "kind",
    "provenance",
    "ruleset",
    "seats",
    "hypothesis",
    "result",
    "evidence",
    "verdicts",
    "lessons",
    "unknown",
)

#: The intent-log verb that hands a faction to the scripted AI, and the file it
#: is read out of. `autopilot` is a documented verb ("emergency only",
#: docs/INTENT.md) and therefore legal — but a round's verdict stops measuring
#: the commander the moment it engages, and rounds r33 and r35 read as Haiku
#: victories with nothing in the record saying so.
AUTOPILOT_VERB = "autopilot"
INTENT_LOG = "bridge/intent_log.jsonl"


def read_intent_log(path: Path, start: int = 0) -> list[dict]:
    """Every intent record in one log, in submission order.

    `start` is a byte offset, because the log is append-only across matches
    (`intent.rs`, `IntentLog`) and one file can hold several rounds. The runner
    notes the size before it launches and reads from there; a reader of an
    archived per-round copy passes nothing.

    Session headers and unparseable lines are skipped rather than raised on: a
    log truncated by a crash is still evidence about the part that was written,
    and refusing to read it would lose a round's whole record over its last
    line.
    """
    try:
        raw = path.read_bytes()
    except OSError:
        return []
    if 0 < start <= len(raw):
        raw = raw[start:]
    out: list[dict] = []
    for line in raw.decode("utf8", "replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(rec, dict) and "verb" in rec:
            out.append(rec)
    return out


def autopilot_spans(records, end_t: float | None = None) -> dict[str, list[dict]]:
    """Team -> the stretches of game time the scripted AI was playing its seat.

    Read from the intent log rather than from the engine, because the log has
    the exact edges: `{"verb":"autopilot","t":189.3,"intent":{"on":true}}` and
    its matching `on:false`. Nothing else in a round's evidence records them —
    which is why r33 and r35 sit in the ledger as unqualified wins.

    Three rules, each of which is the engine's own behaviour rather than a
    convention invented here:

      * **Only `ok` records count.** A refusal (`BH_NO_AUTOPILOT=1`) is logged
        with `ok:false` and changed nothing, so it is not a span.
      * **Engaging while already engaged is one span**, because
        `intent.rs`'s `set_autopilot` is idempotent; disengaging while not
        engaged is nothing at all.
      * **A span still open when the match ends closes at the ending**, marked
        `to_end` so a reader can tell an inferred close from a commander who
        actually took the faction back. r35 is exactly this: engaged at t=316,
        never released, won at t=473.

    `end_t` is the round's `duration_s`; without one the last clock in the log
    is used, which is a floor on the span and never an invention past it.
    """
    open_at: dict[str, float] = {}
    spans: dict[str, list[dict]] = {}
    last_t = 0.0
    for rec in records:
        t = rec.get("t")
        if not isinstance(t, (int, float)) or isinstance(t, bool):
            continue
        last_t = max(last_t, float(t))
        if rec.get("verb") != AUTOPILOT_VERB or not rec.get("ok"):
            continue
        team = rec.get("team")
        if not team:
            continue
        intent = rec.get("intent") or {}
        if intent.get("on"):
            open_at.setdefault(team, float(t))
        elif team in open_at:
            spans.setdefault(team, []).append(
                {"from": round(open_at.pop(team), 1), "to": round(float(t), 1)}
            )
    close = last_t if end_t is None else float(end_t)
    for team, start in sorted(open_at.items()):
        spans.setdefault(team, []).append(
            {"from": round(start, 1), "to": round(max(close, start), 1), "to_end": True}
        )
    for team_spans in spans.values():
        team_spans.sort(key=lambda s: s["from"])
    return spans


def autopilot_secs(spans: list[dict]) -> float:
    """How long one seat spent delegating, in game seconds.

    Always a float, including the zero: `sum([])` is an `int` in Python, and a
    ledger where some rounds say `0` and others say `0.0` is a diff that reads
    as a change and a column that sorts by accident.
    """
    return round(float(sum(float(s["to"]) - float(s["from"]) for s in spans)), 1)


def delegating_seats(rec: dict) -> list[dict]:
    """The seats in one round that actually engaged autopilot."""
    return [s for s in rec.get("seats", []) if (s.get("autopilot_secs") or 0) > 0]


def autopilot_measured(rec: dict) -> bool:
    """Whether this round's intent log was read at all.

    An absent `autopilot_secs` is not a zero: rounds recorded before the key
    existed, and rounds whose intent log was not kept, cannot say either way.
    Distinguishing the two is the whole point of stamping a measured zero.
    """
    return any("autopilot_secs" in s for s in rec.get("seats", []))


def winner_delegated(rec: dict) -> bool:
    """Did the side that won hand its faction to the scripted AI at any point?

    The question the ledger could not answer about r33 and r35.
    """
    winner = rec.get("result", {}).get("winner")
    return bool(winner) and any(s.get("team") == winner for s in delegating_seats(rec))


def autopilot_cell(rec: dict) -> str:
    """The round table's autopilot column: who delegated, and for how long."""
    if not autopilot_measured(rec):
        return ""
    used = delegating_seats(rec)
    if not used:
        return "none"
    return ",".join(
        f"{str(s.get('seat', '?')).rsplit('/', 1)[-1]} {int(s['autopilot_secs'])}s"
        for s in used
    )


def skeleton(round_id: str, hypothesis: str) -> dict:
    """An empty record with every field present. The runner fills this in."""
    return {
        "id": round_id,
        "date": None,
        "kind": None,
        "provenance": "recorded",
        "ruleset": {"map": None, "env": {}, "constants": {}, "commit": None, "notes": ""},
        "seats": [],
        "hypothesis": hypothesis,
        "result": {
            "winner": None,
            "winner_persona": None,
            "duration_s": None,
            "game_over_reason": None,
            "decisive": None,
        },
        "evidence": {"aars": [], "logs": [], "shots": [], "sources": [], "metrics": {}},
        "verdicts": [],
        "lessons": [],
        "unknown": [],
    }


def null_paths(value, prefix: str = "") -> list[str]:
    """Every dotted path in a record whose value is `null`.

    Lists are indexed (`seats.0.persona`) so a single unknown persona in a
    four-seat round is nameable. `unknown` itself is excluded — a list of
    what's missing cannot coherently be missing.
    """
    found: list[str] = []
    if isinstance(value, dict):
        for key, sub in sorted(value.items()):
            if not prefix and key == "unknown":
                continue
            found += null_paths(sub, f"{prefix}.{key}" if prefix else key)
    elif isinstance(value, list):
        for i, sub in enumerate(value):
            found += null_paths(sub, f"{prefix}.{i}")
    elif value is None:
        found.append(prefix)
    return found


def validate(rec: dict) -> list[str]:
    """Everything wrong with one record, as human sentences. Empty means valid."""
    bad: list[str] = []

    def want(cond: bool, msg: str) -> None:
        if not cond:
            bad.append(msg)

    missing = [k for k in TOP_LEVEL if k not in rec]
    extra = [k for k in rec if k not in TOP_LEVEL]
    want(not missing, f"missing fields: {', '.join(missing)}")
    want(not extra, f"unknown fields: {', '.join(extra)}")
    if missing:
        # Every check below indexes into the record; without the keys there is
        # nothing further to say that isn't a cascade of the same complaint.
        return bad

    want(bool(ID_RE.match(str(rec["id"]))), f"id {rec['id']!r} is not r<number>")
    want(
        rec["date"] is None or bool(DATE_RE.match(str(rec["date"]))),
        f"date {rec['date']!r} is not YYYY-MM-DD",
    )
    want(rec["kind"] in (None,) + ROUND_KINDS, f"kind {rec['kind']!r} not in {ROUND_KINDS}")
    want(rec["provenance"] in PROVENANCE, f"provenance {rec['provenance']!r} not in {PROVENANCE}")
    want(
        isinstance(rec["hypothesis"], str) and rec["hypothesis"].strip() != "",
        "hypothesis is empty — a round with no question is not a round",
    )

    rules = rec["ruleset"]
    want(isinstance(rules, dict), "ruleset must be an object")
    if isinstance(rules, dict):
        want(rules.get("map") in (None,) + MAPS, f"map {rules.get('map')!r} not in {MAPS}")
        want(isinstance(rules.get("env"), dict), "ruleset.env must be an object")
        want(isinstance(rules.get("constants"), dict), "ruleset.constants must be an object")
        consts = rules.get("constants") if isinstance(rules.get("constants"), dict) else {}
        # `constants` stays open — it is where a round writes whatever balance
        # value it was played under, and closing it would mean a bead per
        # number. Three keys are typed anyway, because they are written by a
        # tool rather than by a person and a silent format drift in one of them
        # would make two identical rounds compare as different ones.
        #
        # `affordance_doc` is the scaffold version the round was played with
        # (docs/AFFORDANCES.md constraint 3: "once the scaffold encodes any
        # judgment, an arena result measures model+scaffold"). Present only on
        # rounds where a seat actually read the document — an unconditional
        # stamp would make the scaffolded and bare rounds indistinguishable,
        # which is the comparison the field exists for. WHICH seat read it is
        # `seats[].scaffold`.
        doc = consts.get("affordance_doc")
        want(
            "affordance_doc" not in consts
            or (isinstance(doc, str) and doc.strip() != ""),
            "ruleset.constants.affordance_doc must be a non-empty version string "
            "(tools/affordances.py DOC_VERSION) when present",
        )
        for key in TUNING_FILES:
            want(
                key not in consts or bool(DIGEST_RE.match(str(consts[key]))),
                f"ruleset.constants.{key} must be a {DIGEST_RE.pattern} content digest "
                f"of assets/data/{TUNING_FILES[key]} when present",
            )

    want(isinstance(rec["seats"], list) and rec["seats"] != [], "a round needs at least one seat")
    for i, seat in enumerate(rec["seats"] if isinstance(rec["seats"], list) else []):
        where = f"seats.{i}"
        want(isinstance(seat, dict), f"{where} must be an object")
        if not isinstance(seat, dict):
            continue
        want(bool(seat.get("seat")), f"{where}.seat is empty")
        want(seat.get("team") in TEAMS, f"{where}.team {seat.get('team')!r} not in {TEAMS}")
        want(seat.get("kind") in SEAT_KINDS, f"{where}.kind {seat.get('kind')!r} not in {SEAT_KINDS}")
        want("persona" in seat, f"{where} has no persona field")
        # `scaffold` — the media-type version of the affordance document THIS
        # seat played with (docs/AFFORDANCES.md constraint 3). Additive and
        # OPTIONAL on the same terms as `ready_wait_s`: a seat that played bare
        # omits the key rather than nulling it, so an A/B round says which
        # chair had the document without claiming ignorance about the other.
        want(
            "scaffold" not in seat
            or (isinstance(seat["scaffold"], str) and seat["scaffold"].strip() != ""),
            f"{where}.scaffold must be a non-empty version string when present",
        )
        # `playbook` — the strategy library the seat's starter prefs declared
        # (opt-out since the second ladder; LADDER2.md Finding 2). A round
        # played with the book open is a different experiment from one
        # played off-book, and both are different from one where the seat
        # opted out mid-match — the ledger records what was DECLARED.
        want(
            "playbook" not in seat
            or (isinstance(seat["playbook"], str) and seat["playbook"].strip() != ""),
            f"{where}.playbook must be a non-empty id when present",
        )
        # `model` — WHICH MODEL sat in this chair. The other half of the pair
        # docs/AFFORDANCES.md constraint 3 names: "an arena result measures
        # model+scaffold", and until this key existed the ledger recorded the
        # scaffold and left the model to a commit message. Two rounds with the
        # same persona, the same map and the same ruleset are still different
        # experiments if one was opus and one was haiku, and the ladder is
        # nothing but that comparison.
        #
        # Free-form string, not an enum: model ids are somebody else's
        # vocabulary and they change faster than this file does. A closed set
        # here would mean a bead every time a model shipped, and the failure
        # mode of the wrong enum (a valid round refused) is worse than the
        # failure mode of a free string (a typo you can grep for).
        #
        # OPTIONAL on the same terms as `scaffold`: absent, never null, on a
        # scripted seat and on every round recorded before the key existed.
        want(
            "model" not in seat
            or (isinstance(seat["model"], str) and seat["model"].strip() != ""),
            f"{where}.model must be a non-empty model id when present",
        )
        # `ready_wait_s` — wall seconds this seat took to send `ready` before
        # the match clock started (docs/INTENT.md, "The ready handshake").
        # Additive and OPTIONAL: rounds recorded before the handshake existed
        # do not have it and are not wrong, and a seat that never waited omits
        # it rather than nulling it, so the `unknown[]` honesty rule below has
        # nothing to say about it either way. Typed when present, because a
        # duration that arrived as a string would go unnoticed until somebody
        # tried to average the series.
        want(
            "ready_wait_s" not in seat
            or (isinstance(seat["ready_wait_s"], (int, float))
                and not isinstance(seat["ready_wait_s"], bool)
                and seat["ready_wait_s"] >= 0),
            f"{where}.ready_wait_s must be a non-negative number when present",
        )
        # `autopilot_secs` / `autopilot_spans` — how much of this round the
        # scripted AI played on this seat's behalf. `autopilot` is documented
        # and legal ("emergency only"), so this is not a foul flag; it is the
        # fact that r33 and r35 sat in the ledger as Haiku victories in which
        # 57% and 33% of the match, both including the winning stretch, were
        # ai.rs. A verdict that measures the model has to be able to say so.
        #
        # ZERO IS A VALUE HERE, and this is the one place the ledger's
        # absent-not-null rule cuts the other way: the key is stamped on every
        # commander seat of a round whose intent log was read, `0.0` included,
        # because "measured, and nobody delegated" is a different claim from
        # "nobody looked". Absent still means unmeasured — every round before
        # this key existed, and any round whose log was not kept.
        secs = seat.get("autopilot_secs")
        want(
            "autopilot_secs" not in seat
            or (isinstance(secs, (int, float))
                and not isinstance(secs, bool)
                and secs >= 0),
            f"{where}.autopilot_secs must be a non-negative number of game "
            f"seconds when present",
        )
        want(
            not (seat.get("kind") == "scripted" and "autopilot_secs" in seat),
            f"{where} is scripted and cannot delegate — ai.rs is already playing "
            f"it, and `autopilot` is a bridge seat handing its faction over",
        )
        spans = seat.get("autopilot_spans")
        if "autopilot_spans" in seat:
            # Spans without a total would leave the summary line to re-derive a
            # number the record could simply carry, and a total without spans
            # is fine (a round may keep the sum and drop the detail).
            want("autopilot_secs" in seat,
                 f"{where}.autopilot_spans without autopilot_secs — the total is "
                 f"what the ledger reads")
            want(isinstance(spans, list) and spans != [],
                 f"{where}.autopilot_spans must be a non-empty list when present "
                 f"(no spans is autopilot_secs 0, not an empty list)")
            total = 0.0
            for j, span in enumerate(spans if isinstance(spans, list) else []):
                at = f"{where}.autopilot_spans.{j}"
                if not isinstance(span, dict):
                    bad.append(f"{at} must be an object")
                    continue
                extra_keys = set(span) - {"from", "to", "to_end"}
                want(not extra_keys, f"{at} has unknown keys: {', '.join(sorted(extra_keys))}")
                ends = [span.get("from"), span.get("to")]
                if not all(isinstance(e, (int, float)) and not isinstance(e, bool) for e in ends):
                    bad.append(f"{at} needs numeric `from` and `to` game seconds")
                    continue
                want(ends[1] >= ends[0], f"{at} ends before it starts")
                want(span.get("to_end", False) in (True, False),
                     f"{at}.to_end must be true or false")
                total += ends[1] - ends[0]
            want(
                not isinstance(secs, (int, float)) or isinstance(secs, bool)
                or abs(total - secs) <= 0.5,
                f"{where}.autopilot_secs is {secs} but its spans add up to "
                f"{round(total, 1)}",
            )

    res = rec["result"]
    want(isinstance(res, dict), "result must be an object")
    if isinstance(res, dict):
        # `winner: null` is a real outcome (round 2 deadlocked), which is why
        # the draw is spelled by the winner being absent rather than by a
        # sentinel team nobody plays.
        want(res.get("winner") in (None,) + TEAMS, f"result.winner {res.get('winner')!r} not in {TEAMS}")
        want(
            res.get("game_over_reason") in (None,) + END_REASONS,
            f"result.game_over_reason {res.get('game_over_reason')!r} not in {END_REASONS}",
        )
        dur = res.get("duration_s")
        want(dur is None or (isinstance(dur, (int, float)) and dur > 0), "result.duration_s must be a positive number or null")
        want(res.get("decisive") in (None, True, False), "result.decisive must be true, false or null")
        want(
            not (res.get("winner") is None and res.get("decisive") is True),
            "result says decisive but names no winner",
        )
        # A score round is never decisive, enforced at the boundary rather than
        # trusted to the two readers upstream (wc3clone-j84). `read_log` and
        # `wait_for_seat_game_over` each derive `decisive` from the reason, and
        # they agree today; a third reader, a hand-written record or a
        # backfill has no such habit. The claim is the ledger's, so the ledger
        # checks it: a time-cap verdict is a referee's opinion about who was
        # ahead, and the whole reason it is spelled differently from the
        # engine's own two endings is that it must never be quoted as a win.
        want(
            not (res.get("game_over_reason") == "score" and res.get("decisive") is True),
            "result says decisive on a `score` round — a time-cap verdict is the "
            "referee's opinion about who was ahead, not a win the game recognises",
        )

    ev = rec["evidence"]
    want(isinstance(ev, dict), "evidence must be an object")
    if isinstance(ev, dict):
        # `sources` is where a backfilled round says where it came from — the
        # commit, the memory file, the bead. For a recorded round it is usually
        # empty, because the round *is* its own source.
        for key in ("aars", "logs", "shots", "sources"):
            want(isinstance(ev.get(key), list), f"evidence.{key} must be a list")
        want(isinstance(ev.get("metrics"), dict), "evidence.metrics must be an object")

    want(isinstance(rec["verdicts"], list), "verdicts must be a list")
    for i, v in enumerate(rec["verdicts"] if isinstance(rec["verdicts"], list) else []):
        want(isinstance(v, dict) and bool(v.get("claim")), f"verdicts.{i}.claim is empty")
        want(
            isinstance(v, dict) and v.get("status") in VERDICT_STATUS,
            f"verdicts.{i}.status not in {VERDICT_STATUS}",
        )

    want(isinstance(rec["lessons"], list), "lessons must be a list")
    want(isinstance(rec["unknown"], list), "unknown must be a list")

    # The honesty rule, both ways.
    if isinstance(rec["unknown"], list):
        actual = set(null_paths(rec))
        claimed = set(rec["unknown"])
        for path in sorted(actual - claimed):
            bad.append(f"{path} is null but is not listed in `unknown` — say what you don't know")
        for path in sorted(claimed - actual):
            bad.append(f"`unknown` lists {path}, which is not missing")
    return bad


# ---------------------------------------------------------------------------
# Storage
# ---------------------------------------------------------------------------


def load(path: Path = LEDGER) -> list[dict]:
    """Every round, in file order. A blank or absent ledger is an empty series."""
    if not path.exists():
        return []
    rounds = []
    for n, line in enumerate(path.read_text().splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        try:
            rounds.append(json.loads(line))
        except json.JSONDecodeError as err:
            raise SystemExit(f"{path}:{n}: not valid JSON — {err}") from err
    return rounds


def dumps(rec: dict) -> str:
    """One record, one line, keys in schema order.

    Stable key order because this file is in git and reviewed by humans: a diff
    should show what changed about a round, not that a dict reordered itself.
    """
    ordered = {k: rec[k] for k in TOP_LEVEL if k in rec}
    ordered.update({k: v for k, v in rec.items() if k not in ordered})
    return json.dumps(ordered, ensure_ascii=False)


def append(rec: dict, path: Path = LEDGER) -> None:
    """Add one validated round to the end of the ledger."""
    problems = validate(rec)
    if problems:
        raise ValueError("record does not validate:\n  " + "\n  ".join(problems))
    if any(r.get("id") == rec["id"] for r in load(path)):
        raise ValueError(f"round {rec['id']} is already in the ledger")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as fh:
        fh.write(dumps(rec) + "\n")


def rewrite(rounds: list[dict], path: Path = LEDGER) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(dumps(r) + "\n" for r in rounds))


def next_id(rounds: list[dict]) -> str:
    """The id after the highest one on file — `r11`, not `r1` again."""
    highest = max((int(r["id"][1:]) for r in rounds if ID_RE.match(r.get("id", ""))), default=0)
    return f"r{highest + 1}"


# ---------------------------------------------------------------------------
# Queries
# ---------------------------------------------------------------------------


def personas(rec: dict) -> list[str]:
    return [s.get("persona") for s in rec.get("seats", []) if s.get("persona")]


def winning_persona(rec: dict) -> str | None:
    """Which creed won, if the record can say."""
    res = rec.get("result", {})
    if res.get("winner_persona"):
        return res["winner_persona"]
    for seat in rec.get("seats", []):
        if res.get("winner") and seat.get("team") == res["winner"]:
            return seat.get("persona")
    return None


def series(rounds: list[dict]) -> dict[str, int]:
    """Wins per persona plus draws — the line every AAR ends on."""
    tally: dict[str, int] = {}
    for rec in rounds:
        for p in personas(rec):
            tally.setdefault(p, 0)
        if rec.get("result", {}).get("winner") is None:
            tally["draws"] = tally.get("draws", 0) + 1
            continue
        won = winning_persona(rec)
        if won:
            tally[won] = tally.get(won, 0) + 1
    return tally


def matches(rec: dict, args) -> bool:
    """Filter one round against the CLI's selectors (all of them must hold)."""
    if args.hypothesis and args.hypothesis.lower() not in rec.get("hypothesis", "").lower():
        return False
    if args.map and rec.get("ruleset", {}).get("map") != args.map:
        return False
    if args.persona and args.persona not in personas(rec):
        return False
    if args.winner and rec.get("result", {}).get("winner") != args.winner:
        return False
    if args.reason and rec.get("result", {}).get("game_over_reason") != args.reason:
        return False
    if args.kind and rec.get("kind") != args.kind:
        return False
    return True


def mmss(secs) -> str:
    if secs is None:
        return "?"
    return f"{int(secs) // 60}:{int(secs) % 60:02d}"


def one_line(rec: dict) -> str:
    res = rec.get("result", {})
    won = winning_persona(rec) or ("draw" if res.get("winner") is None else "?")
    # The asterisk is the point of the column beside it: a round the winner
    # spent on autopilot is a win by ai.rs on that seat's behalf, and r33/r35
    # read as unassisted model victories for a fortnight because nothing in
    # this table said otherwise.
    if winner_delegated(rec):
        won += "*"
    return (
        f"{rec['id']:>4}  {rec.get('date') or '?':10}  {rec.get('ruleset', {}).get('map') or '?':9}  "
        f"{'/'.join(personas(rec)) or '?':16}  {won:9}  {mmss(res.get('duration_s')):>6}  "
        f"{res.get('game_over_reason') or '?':9}  {autopilot_cell(rec):11}  "
        f"{rec.get('hypothesis', '')[:56]}"
    )


HEADER = (
    f"{'id':>4}  {'date':10}  {'map':9}  {'seats':16}  {'won':9}  "
    f"{'length':>6}  {'ending':9}  {'autopilot':11}  hypothesis"
)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def cmd_series(args) -> int:
    rounds = load(args.ledger)
    if not rounds:
        print("the ledger is empty")
        return 0
    print(HEADER)
    print("-" * len(HEADER))
    for rec in rounds:
        print(one_line(rec))
    tally = series(rounds)
    draws = tally.pop("draws", 0)
    standings = ", ".join(f"{k} {v}" for k, v in sorted(tally.items(), key=lambda kv: -kv[1]))
    print()
    print(f"{len(rounds)} rounds — {standings}" + (f", {draws} draw(s)" if draws else ""))
    lengths = [r["result"]["duration_s"] for r in rounds if r.get("result", {}).get("duration_s")]
    if lengths:
        lengths.sort()
        print(
            f"length: median {mmss(lengths[len(lengths) // 2])}, "
            f"shortest {mmss(lengths[0])}, longest {mmss(lengths[-1])} "
            f"({len(lengths)}/{len(rounds)} rounds timed)"
        )
    backfilled = sum(1 for r in rounds if r.get("provenance") == "backfilled")
    print(f"provenance: {len(rounds) - backfilled} recorded, {backfilled} backfilled")
    print(autopilot_summary(rounds))
    return 0


def autopilot_summary(rounds: list[dict]) -> str:
    """The series line for delegation — who handed the faction over, and won.

    Kept beside `provenance` because it is the same kind of sentence: a
    statement about how much of this series is what it appears to be. A round
    with no measurement is counted separately from a round measured at zero,
    for the reason `autopilot_measured` exists.
    """
    measured = [r for r in rounds if autopilot_measured(r)]
    used = [r for r in measured if delegating_seats(r)]
    if not measured:
        return "autopilot: unmeasured on every round on file"
    if not used:
        return f"autopilot: none, across {len(measured)}/{len(rounds)} measured rounds"
    detail = "; ".join(f"{r['id']} {autopilot_cell(r)}" for r in used)
    won = [r["id"] for r in used if winner_delegated(r)]
    line = (
        f"autopilot: {len(used)} of {len(measured)} measured rounds "
        f"({len(rounds)} on file) — {detail}"
    )
    if won:
        line += (
            f"\n  * marks a win the winner spent on autopilot: {', '.join(won)} "
            f"— those verdicts measure when the seat delegated, not how it played"
        )
    return line


def cmd_rounds(args) -> int:
    rounds = [r for r in load(args.ledger) if matches(r, args)]
    if not rounds:
        print("no rounds match")
        return 0
    print(HEADER)
    print("-" * len(HEADER))
    for rec in rounds:
        print(one_line(rec))
    print(f"\n{len(rounds)} round(s)")
    return 0


def cmd_show(args) -> int:
    for rec in load(args.ledger):
        if rec.get("id") == args.id:
            print(json.dumps(rec, indent=2, ensure_ascii=False))
            return 0
    print(f"no round {args.id}", file=sys.stderr)
    return 1


def cmd_lessons(args) -> int:
    found = 0
    for rec in load(args.ledger):
        for lesson in rec.get("lessons", []):
            if args.grep and args.grep.lower() not in lesson.lower():
                continue
            found += 1
            print(f"{rec['id']:>4}  {lesson}")
    if not found:
        print("no lessons match")
    return 0


def cmd_validate(args) -> int:
    rounds = load(args.ledger)
    problems = 0
    seen: set[str] = set()
    for rec in rounds:
        rid = rec.get("id", "<no id>")
        for msg in validate(rec):
            problems += 1
            print(f"{rid}: {msg}")
        if rid in seen:
            problems += 1
            print(f"{rid}: duplicate round id")
        seen.add(rid)
    print(f"{len(rounds)} rounds, {problems} problem(s)")
    return 1 if problems else 0


def cmd_add_aar(args) -> int:
    """Attach an after-action report to a round that is already on file.

    AARs are written by the commanders after the match, so the runner cannot
    have them at append time — it records the round and this fills the evidence
    in afterwards, rather than making the ledger wait on a human's markdown.
    """
    rounds = load(args.ledger)
    for rec in rounds:
        if rec.get("id") != args.id:
            continue
        entry = {"seat": args.seat, "path": args.path}
        if entry in rec["evidence"]["aars"]:
            print(f"{args.id} already cites {args.path} for {args.seat}")
            return 0
        rec["evidence"]["aars"].append(entry)
        problems = validate(rec)
        if problems:
            print("refusing to write — " + "; ".join(problems), file=sys.stderr)
            return 1
        rewrite(rounds, args.ledger)
        print(f"{args.id}: {args.seat} AAR -> {args.path}")
        return 0
    print(f"no round {args.id}", file=sys.stderr)
    return 1


def cmd_note(args) -> int:
    """Write the half of a round no match can produce: what it meant.

    The runner records who won. Whether that CONFIRMS anything is a judgement,
    and it is usually made minutes or days later, once the after-action reports
    are in. Without a verb for it the judgement goes back where it came from —
    into prose nobody can query — so this is how a verdict or a lesson gets
    onto a round that is already on file.
    """
    rounds = load(args.ledger)
    for rec in rounds:
        if rec.get("id") != args.id:
            continue
        if args.verdict:
            claim, status, *rest = [p.strip() for p in args.verdict.split("|")]
            if status not in VERDICT_STATUS:
                print(f"status must be one of {VERDICT_STATUS}", file=sys.stderr)
                return 1
            rec["verdicts"].append(
                {"claim": claim, "status": status, "note": rest[0] if rest else ""}
            )
        for lesson in args.lesson or []:
            rec["lessons"].append(lesson)
        problems = validate(rec)
        if problems:
            print("refusing to write — " + "; ".join(problems), file=sys.stderr)
            return 1
        rewrite(rounds, args.ledger)
        print(f"{args.id}: {len(rec['verdicts'])} verdict(s), {len(rec['lessons'])} lesson(s)")
        return 0
    print(f"no round {args.id}", file=sys.stderr)
    return 1


def stamp_autopilot(rec: dict, records: list[dict], log_path: str | None = None) -> dict:
    """Write one round's delegation into its own record. Returns what changed.

    Split from the CLI so the runner and the retroactive backfill reach the
    ledger through one function: `arena_run` stamps a round it just watched,
    `arena.py autopilot` stamps one whose log survived in `arena/<id>/`, and
    the two cannot disagree about what a span is.

    Every non-scripted seat gets the key, `0.0` included — see the validator
    for why a measured zero is not the same fact as an absent key. A scripted
    seat never gets it: ai.rs is already playing that faction and there is no
    handover to record.
    """
    spans = autopilot_spans(records, rec.get("result", {}).get("duration_s"))
    stamped: dict[str, float] = {}
    for seat in rec.get("seats", []):
        if seat.get("kind") == "scripted":
            continue
        mine = spans.get(seat.get("team"), [])
        seat["autopilot_secs"] = autopilot_secs(mine)
        if mine:
            seat["autopilot_spans"] = mine
        else:
            seat.pop("autopilot_spans", None)
        stamped[seat.get("seat", "?")] = seat["autopilot_secs"]
    if log_path and log_path not in rec["evidence"]["logs"]:
        # The record cites the file the numbers came from, so a reader can
        # recompute them rather than take the stamp on faith.
        rec["evidence"]["logs"].append(log_path)
    rec["unknown"] = null_paths(rec)
    return stamped


def cmd_autopilot(args) -> int:
    """Stamp a round with what its intent log says about delegation.

    The retroactive half of the machinery. r33 and r35 were recorded before
    anything read the intent log for this, and both are cited in
    arena/LADDER2.md as results that need a number attached; their logs are in
    the repo, so the numbers are recoverable rather than guessable and the
    honesty rule has nothing to complain about.
    """
    rounds = load(args.ledger)
    for rec in rounds:
        if rec.get("id") != args.id:
            continue
        log = Path(args.log) if args.log else REPO / "arena" / args.id / "bridge-logs" / "intent_log.jsonl"
        if not log.exists():
            print(
                f"no intent log at {log} — a round whose log was not kept stays "
                f"unmeasured, which is a fact the ledger already spells as an "
                f"absent key",
                file=sys.stderr,
            )
            return 1
        records = read_intent_log(log)
        cited = str(log.relative_to(REPO)) if log.is_relative_to(REPO) else str(log)
        stamped = stamp_autopilot(rec, records, cited)
        problems = validate(rec)
        if problems:
            print("refusing to write — " + "; ".join(problems), file=sys.stderr)
            return 1
        for seat, secs in sorted(stamped.items()):
            print(f"{args.id}: {seat} autopilot {secs}s")
        print(f"{args.id}: {autopilot_cell(rec)}"
              + ("  (the winner delegated)" if winner_delegated(rec) else ""))
        if not args.write:
            print("(not written — pass --write to stamp the ledger)")
            return 0
        rewrite(rounds, args.ledger)
        print(f"{args.id}: stamped into {args.ledger}")
        return 0
    print(f"no round {args.id}", file=sys.stderr)
    return 1


def cmd_append(args) -> int:
    rec = json.loads(Path(args.file).read_text())
    try:
        append(rec, args.ledger)
    except ValueError as err:
        print(str(err), file=sys.stderr)
        return 1
    print(f"appended {rec['id']} to {args.ledger}")
    return 0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--ledger", type=Path, default=LEDGER)
    subs = p.add_subparsers(dest="cmd", required=True)

    subs.add_parser("series", help="standings, round table, pacing").set_defaults(fn=cmd_series)

    r = subs.add_parser("rounds", help="rounds matching a filter")
    r.add_argument("--hypothesis", help="substring of the question the round asked")
    r.add_argument("--map", choices=MAPS)
    r.add_argument("--persona", help="a creed that sat in the round")
    r.add_argument("--winner", choices=TEAMS)
    r.add_argument("--reason", choices=END_REASONS)
    r.add_argument("--kind", choices=ROUND_KINDS)
    r.set_defaults(fn=cmd_rounds)

    s = subs.add_parser("show", help="one round, in full")
    s.add_argument("id")
    s.set_defaults(fn=cmd_show)

    ls = subs.add_parser("lessons", help="what the rounds taught")
    ls.add_argument("--grep")
    ls.set_defaults(fn=cmd_lessons)

    subs.add_parser("validate", help="schema + honesty check").set_defaults(fn=cmd_validate)

    a = subs.add_parser("add-aar", help="cite an after-action report on a round")
    a.add_argument("id")
    a.add_argument("--seat", required=True)
    a.add_argument("--path", required=True)
    a.set_defaults(fn=cmd_add_aar)

    n = subs.add_parser("note", help="add a verdict or a lesson to a recorded round")
    n.add_argument("id")
    n.add_argument("--verdict", metavar="CLAIM|STATUS|NOTE",
                   help=f"status is one of {', '.join(VERDICT_STATUS)}")
    n.add_argument("--lesson", action="append", metavar="TEXT")
    n.set_defaults(fn=cmd_note)

    au = subs.add_parser(
        "autopilot",
        help="stamp seats[].autopilot_secs from a round's intent log",
    )
    au.add_argument("id")
    au.add_argument("--log", help="the intent log to read "
                                  "(default arena/<id>/bridge-logs/intent_log.jsonl)")
    au.add_argument("--write", action="store_true", help="write the stamp into the ledger")
    au.set_defaults(fn=cmd_autopilot)

    ap = subs.add_parser("append", help="append a record from a JSON file")
    ap.add_argument("file")
    ap.set_defaults(fn=cmd_append)

    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
