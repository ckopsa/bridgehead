#!/usr/bin/env python3
"""The arena ledger: every round of the series as one queryable record.

    tools/arena.py series                    # the standings and the round table
    tools/arena.py show r9                   # one round in full
    tools/arena.py rounds --hypothesis mine  # rounds that tested a question
    tools/arena.py rounds --persona rusher --winner Claude
    tools/arena.py lessons --grep tower      # what the players actually learned
    tools/arena.py validate                  # schema + honesty check
    tools/arena.py add-aar r11 --seat bridge/red --path arena/r11/red-aar.md

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
TUNING_FILES = {
    "alarms_ron": "alarms.ron",
    "stances_ron": "stances.ron",
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
    return (
        f"{rec['id']:>4}  {rec.get('date') or '?':10}  {rec.get('ruleset', {}).get('map') or '?':9}  "
        f"{'/'.join(personas(rec)) or '?':16}  {won:8}  {mmss(res.get('duration_s')):>6}  "
        f"{res.get('game_over_reason') or '?':9}  {rec.get('hypothesis', '')[:64]}"
    )


HEADER = (
    f"{'id':>4}  {'date':10}  {'map':9}  {'seats':16}  {'won':8}  "
    f"{'length':>6}  {'ending':9}  hypothesis"
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
    return 0


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

    ap = subs.add_parser("append", help="append a record from a JSON file")
    ap.add_argument("file")
    ap.set_defaults(fn=cmd_append)

    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
