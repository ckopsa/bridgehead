#!/usr/bin/env python3
"""Run one arena round end to end and write it into the ledger.

    # a scripted round, headless, appended to arena/ledger.jsonl when it ends
    tools/arena_run.py --hypothesis "does the tier ladder change scripted pacing?" \
        --seat red=scripted --seat blue=scripted --map crossings --speed 16 --cap 1800

    # a commander round: prepare the seats, then hand them to the orchestrator
    tools/arena_run.py --hypothesis "does the rush still win at 5000g?" \
        --seat red=commander:rusher --seat blue=commander:boomer \
        --map crossings --windowed --cap 3000

    tools/arena_run.py --hypothesis ... --dry-run    # print the plan, launch nothing

WHAT THIS OWNS, AND WHAT IT DOES NOT
------------------------------------
Rounds 9 and 10 were hand-orchestrated: someone typed the launch line, watched
for game over, read two numbers out of a snapshot, and wrote the result into a
memory file. Every one of those steps is a place a round can be recorded wrong,
and the ledger is only worth having if the rounds in it were not.

So this owns the mechanical half: it derives the environment from the seats
rather than trusting a hand-typed `BH_BRIDGE`, refuses to start on top of a
running match, prepares seat directories, waits for a real game over, reads the
duration and the ending out of the engine's own log, and appends a validated
record.

It does NOT spawn commanders. An LLM seat is an agent with a persona, a budget
and a transcript, and deciding when one exists is the orchestrator's job, not a
subprocess call's. For commander seats this prepares the seat directory and
prints the briefing each seat needs, then waits — the orchestrator spawns the
agents against those directories while the match runs.

It also does not write after-action reports, which do not exist yet when the
match ends. `tools/arena.py add-aar` attaches them afterwards.

SAFETY
------
The bridge is a live singleton: one directory per seat, overwritten in place.
An earlier agent destroyed a real match by running a verification against a
seat somebody was playing. Two rules follow, and they are enforced rather than
documented:

  * If any engine process is already running, this refuses to start. It does
    not ask, and it does not kill anything.
  * It never deletes a bridge directory. Directories it creates are its own;
    directories that already existed are reused only with `--reuse-seat`, and
    a pre-existing snapshot is renamed aside, never removed — otherwise a
    stale `game_over` from last match reads as this match ending instantly.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import arena  # noqa: E402

REPO = Path(__file__).resolve().parent.parent

# `headless: game over — Claude wins (razed) at t=324.0s — exiting`
DECISIVE = re.compile(
    r"headless: game over — (?P<winner>\w+) wins"
    r"(?: \((?P<reason>\w+)\))?(?: at t=(?P<t>[\d.]+)s)?"
)
# `headless: time cap 1800s — timeout verdict: Claude wins on score (...)`
TIMECAP = re.compile(r"headless: time cap (?P<cap>[\d.]+)s — timeout verdict: (?P<verdict>[^(]+)")
# `[ 324.0s] Claude: gold 1539 lumber 370 supply 40/100 | 33 units, 12 buildings | 12 Footman`
STATUS = re.compile(
    r"\[\s*(?P<t>[\d.]+)s\] (?P<team>\w+): gold (?P<gold>\d+) lumber (?P<lumber>\d+) "
    r"supply (?P<used>\d+)/(?P<cap>\d+) \| (?P<units>\d+) units, (?P<buildings>\d+) buildings"
)

SIDES = {"red": ("Claude", "bridge/red"), "blue": ("Human", "bridge/blue")}


# ---------------------------------------------------------------------------
# Seats
# ---------------------------------------------------------------------------


def parse_seat(spec: str) -> dict:
    """`red=commander:rusher` / `blue=scripted` -> one seat record.

    The persona is part of the seat and not a separate flag because a seat
    without a creed is exactly the ambiguity the ledger exists to remove: two
    rounds with the same map and the same rules are still different experiments
    if the personas differ.
    """
    if "=" not in spec:
        raise ValueError(f"seat {spec!r} must look like side=kind[:persona]")
    side, rest = spec.split("=", 1)
    side = side.strip().lower()
    if side not in SIDES:
        raise ValueError(f"unknown side {side!r} — use {' or '.join(SIDES)}")
    parts = rest.split(":")
    kind = parts[0].strip().lower()
    if kind not in ("scripted", "commander"):
        raise ValueError(f"seat {side}: kind {kind!r} must be 'scripted' or 'commander'")
    persona = parts[1].strip() if len(parts) > 1 and parts[1].strip() else None
    prompt = parts[2].strip() if len(parts) > 2 and parts[2].strip() else None
    team, seat_dir = SIDES[side]
    return {
        "seat": seat_dir,
        "side": side,
        "team": team,
        "kind": kind,
        # The scripted AI has a doctrine but not a persona; saying so beats a
        # null that `unknown` would then have to carry for every scripted round.
        "persona": persona or ("scripted" if kind == "scripted" else None),
        "prompt": prompt,
    }


def derive_env(seats: list[dict], args) -> dict[str, str]:
    """The launch environment implied by the seats.

    `BH_BRIDGE` and `BH_AI_BOTH` are computed, never passed in, because they
    are two spellings of one fact — who is playing which side — and the two
    rounds that went wrong in this series both went wrong by disagreeing with
    each other about it. Claude's side is scripted-driven by default; the Human
    side needs `BH_AI_BOTH` to be, and a bridge seat takes its team off the
    scripted AI either way (bridge.rs `bridge_startup`).
    """
    by_side = {s["side"]: s for s in seats}
    commanders = [s["side"] for s in seats if s["kind"] == "commander"]
    bridge = {
        (): "0",
        ("red",): "red",
        ("blue",): "blue",
        ("blue", "red"): "both",
    }[tuple(sorted(commanders))]

    env = {
        "BH_MAP": args.map,
        "BH_SPEED": f"{args.speed:g}",
        "BH_BRIDGE": bridge,
        # Only the Human side needs telling; Claude's is always machine-driven
        # unless a seat takes it over.
        "BH_AI_BOTH": "1" if by_side.get("blue", {}).get("kind") == "scripted" else "0",
    }
    if not args.windowed:
        env["BH_HEADLESS"] = "1"
    if args.cap:
        env["BH_MAX_GAME_SECS"] = f"{args.cap:g}"
    # Screenshots file themselves with the round they belong to. Kept
    # repo-relative — this string is copied verbatim into a record that lives in
    # git, and an absolute path there is one machine's private detail.
    env["BH_SHOT_DIR"] = os.path.join(args.out, args.id, "shots")
    for pair in args.env:
        if "=" not in pair:
            raise ValueError(f"--env {pair!r} must look like KEY=VALUE")
        key, value = pair.split("=", 1)
        # Explicit overrides win, and land in the record, so a round run with a
        # probe knob set can never be mistaken for a baseline.
        env[key.strip()] = value.strip()
    return env


# ---------------------------------------------------------------------------
# Safety
# ---------------------------------------------------------------------------


def running_engines(binary: Path) -> list[tuple[int, str]]:
    """Every live process that IS the engine — never this script.

    Matched on the executable, not on a command line containing the binary's
    path: this script's own argv carries that path, and a `pgrep -f` for it
    would find itself and refuse every run. Reading argv[0]'s basename (and the
    `exe` link where permitted) matches the process rather than a mention of it.
    """
    want = binary.name
    found = []
    if not Path("/proc").exists():
        # macOS has no /proc; `ps -o comm=` prints the executable path itself,
        # which keeps the same guarantee — we match the process, not a command
        # line that merely mentions the binary.
        out = subprocess.run(
            ["ps", "-axo", "pid=,comm="], capture_output=True, text=True
        ).stdout
        for line in out.splitlines():
            parts = line.strip().split(None, 1)
            if len(parts) != 2:
                continue
            pid_s, comm = parts
            if not pid_s.isdigit():
                continue
            pid = int(pid_s)
            if pid in (os.getpid(), os.getppid()):
                continue
            if Path(comm).name == want:
                found.append((pid, comm[:120]))
        return found
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        if pid == os.getpid() or pid == os.getppid():
            continue
        try:
            raw = (entry / "cmdline").read_bytes()
        except OSError:
            continue
        argv = [a for a in raw.decode("utf8", "replace").split("\0") if a]
        if not argv:
            continue
        if Path(argv[0]).name != want:
            continue
        try:
            # If the exe link is readable it settles the question outright.
            exe = os.readlink(entry / "exe")
            if Path(exe).name != want:
                continue
        except OSError:
            pass
        found.append((pid, " ".join(argv)[:120]))
    return found


def prepare_seats(seats: list[dict], root: Path, reuse: bool) -> list[Path]:
    """Make each commander seat's directory. Returns the ones we created."""
    created = []
    for seat in seats:
        if seat["kind"] != "commander":
            continue
        # The seat name in the record is repo-relative (`bridge/red`); the root
        # flag can move the whole tree without rewriting the record.
        path = root / Path(seat["seat"]).name
        if path.exists():
            if not reuse:
                raise SystemExit(
                    f"seat {path} already exists — it may belong to a running or "
                    f"unfinished match. Pass --reuse-seat if you are certain it does not; "
                    f"this tool will not delete a seat directory."
                )
            state = path / "state.json"
            if state.exists():
                # Never removed: a stale snapshot still carries the previous
                # match's `game_over`, and reading that as this match's ending
                # is precisely the mistake that would fake a result.
                aside = path / f"state.prev-{int(time.time())}.json"
                state.rename(aside)
                print(f"  moved stale snapshot aside: {aside}")
        else:
            path.mkdir(parents=True)
            created.append(path)
        seat["dir"] = str(path)
    return created


# ---------------------------------------------------------------------------
# Running and reading the match
# ---------------------------------------------------------------------------


def read_log(log: str) -> dict:
    """The verdict, the clock and the closing numbers, from the engine's stdout."""
    out = {
        "winner": None,
        "reason": None,
        "duration_s": None,
        "decisive": False,
        "metrics": {},
    }
    if m := DECISIVE.search(log):
        # Bevy prints the Team enum, whose variants are the team names.
        out["winner"] = m.group("winner")
        out["reason"] = m.group("reason") if m.group("reason") != "unknown" else None
        # A capped match now decides itself (wc3clone-j84), so the engine
        # prints BOTH the timeout verdict and the game-over line — and this
        # branch reads the second one first. `decisive` follows the reason
        # rather than the line it was found on: `score` is a referee's opinion
        # however the engine chose to announce it, and the ledger has recorded
        # capped rounds as undecisive since round 10.
        out["decisive"] = out["reason"] != "score"
        if m.group("t"):
            out["duration_s"] = round(float(m.group("t")), 1)
    elif m := TIMECAP.search(log):
        # A time-cap verdict is a referee's opinion, not a win the game
        # recognises, and the ledger spells it differently on purpose.
        verdict = m.group("verdict").strip()
        out["reason"] = "score"
        out["duration_s"] = round(float(m.group("cap")), 1)
        if verdict.startswith(("Human", "Claude")):
            out["winner"] = verdict.split()[0]
        out["decisive"] = False

    # The last status line each team printed is the closing position.
    finals: dict[str, dict] = {}
    for m in STATUS.finditer(log):
        finals[m.group("team")] = {
            "gold": int(m.group("gold")),
            "lumber": int(m.group("lumber")),
            "supply": f"{m.group('used')}/{m.group('cap')}",
            "units": int(m.group("units")),
            "buildings": int(m.group("buildings")),
            "t": float(m.group("t")),
        }
    for team, final in finals.items():
        for key, value in final.items():
            if key != "t":
                out["metrics"][f"{team.lower()}_{key}_final"] = value
    if out["duration_s"] is None and finals:
        # Better than nothing and honest about what it is: the last clock the
        # engine printed, which is a floor on the match length, not its end.
        out["metrics"]["last_status_t"] = max(f["t"] for f in finals.values())
    return out


def wait_for_seat_game_over(seat_dirs: list[Path], deadline: float, poll: float = 1.0) -> dict:
    """Windowed runs don't self-exit: watch the seats for the verdict instead."""
    while time.monotonic() < deadline:
        for path in seat_dirs:
            try:
                snap = json.loads((path / "state.json").read_text())
            except (OSError, json.JSONDecodeError):
                continue
            if snap.get("game_over"):
                reason = snap.get("game_over_reason")
                # The wire and the ledger spell a draw differently, and this is
                # the boundary where they are translated. `game_over: "draw"`
                # exists because a commander's poll loop terminates on that key
                # being non-null and a tie has to end the match too
                # (wc3clone-j84); the record keeps ARENA.md's spelling, where a
                # draw is an absent winner and never a sentinel team.
                winner = None if snap["game_over"] == "draw" else snap["game_over"]
                return {
                    "winner": winner,
                    "reason": reason,
                    "duration_s": round(float(snap.get("t", 0)), 1) or None,
                    # `score` is the cap's referee, not a win: same rule as
                    # `read_log`, so the windowed and headless paths cannot
                    # disagree about a round they both watched.
                    "decisive": winner is not None and reason != "score",
                    "metrics": {},
                }
        time.sleep(poll)
    return {"winner": None, "reason": None, "duration_s": None, "decisive": False, "metrics": {}}


def collect_snapshots(seats: list[dict], out_dir: Path) -> list[str]:
    """Keep each seat's final snapshot with the round. The bridge is overwritten
    by the next match; this is the only copy that survives it."""
    kept = []
    for seat in seats:
        if not seat.get("dir"):
            continue
        src = Path(seat["dir"]) / "state.json"
        if not src.exists():
            continue
        dst = out_dir / f"final-{Path(seat['seat']).name}.json"
        dst.write_text(src.read_text())
        kept.append(str(dst.relative_to(REPO)) if dst.is_relative_to(REPO) else str(dst))
    return kept


def record_readies(waiting: list[str], last, by_side: dict, now: float) -> None:
    """Attribute a `ready` to every seat that just left the waiting list.

    Split out of the polling loop below so the part that decides what reaches
    the ledger is a pure function of two observations, and can be tested
    without a thread, a clock or a file.

    A seat we never saw waiting gets nothing — `last is None` on the first
    observation, so a seat that was already ready before our first poll is not
    credited with a wait it may not have had. Omission over invention.
    """
    for side in set(last or []) - set(waiting):
        seat = by_side.get(side)
        if seat is not None:
            seat["ready_wait_s"] = round(now, 1)


def watch_handshake(seats: list[dict], started: float, stop) -> None:
    """Follow the ready handshake and record when each seat spoke.

    docs/INTENT.md, "The ready handshake": a bridged seat holds the match at
    t=0 until every such seat sends `{"type":"ready"}`. That wait is the one
    interesting thing that happens before the match, and it is invisible in the
    engine log's game-time timeline — it happens entirely at t=0 — so it has to
    be measured on the wall clock from out here.

    Runs on a thread because the headless launch below blocks in
    `subprocess.run` until the engine exits; there is no other seam from which
    to watch a file while that call owns the main thread.

    Writes `ready_wait_s` onto the seat dicts as a side effect. Seats we never
    observed waiting simply do not get the key — see `build_record` for why
    that is omission rather than a null.
    """
    watched = [s for s in seats if s["kind"] != "scripted"]
    if not watched:
        return
    # `waiting_for` is a global fact, identically present in every seat's
    # snapshot, so the first readable one answers for all of them.
    paths = [Path(s["dir"]) / "state.json" for s in watched]
    by_side = {s["side"]: s for s in seats}
    held, last = False, None
    while not stop.wait(0.5):
        snap = None
        for path in paths:
            try:
                snap = json.loads(path.read_text())
                break
            except (OSError, json.JSONDecodeError):
                continue
        if snap is None:
            continue
        now = time.monotonic() - started
        waiting = snap.get("waiting_for")
        if waiting is None:
            if held:
                print(f"  match started at wall {now:.0f}s", flush=True)
            return
        if not held:
            print(f"  match held at t=0 (the clock starts when every seat readies)", flush=True)
        held = True
        if waiting != last:
            # Everyone who just left the list said the word between the last
            # poll and this one; the wall clock at this poll is the honest
            # resolution we have, and it is a half-second.
            record_readies(waiting, last, by_side, now)
            print(f"  waiting for seats: {' '.join(waiting)}", flush=True)
            last = waiting


def build_record(args, seats: list[dict], env: dict, verdict: dict) -> dict:
    rec = arena.skeleton(args.id, args.hypothesis)
    rec["date"] = time.strftime("%Y-%m-%d")
    kinds = {s["kind"] for s in seats}
    rec["kind"] = kinds.pop() if len(kinds) == 1 else "mixed"
    rec["provenance"] = "recorded"
    rec["ruleset"] = {
        "map": env.get("BH_MAP"),
        "env": dict(env),
        "constants": {},
        "commit": args.commit,
        "notes": args.notes,
    }
    # `prompt` only appears on seats that have one: an absent briefing is not a
    # fact anybody is missing, and a null here would clutter every scripted
    # round's `unknown` list with something nobody wants to know.
    #
    # `ready_wait_s` follows exactly that precedent (docs/INTENT.md, "The ready
    # handshake"): wall seconds from launch until this seat sent `ready`, and
    # ABSENT — never null — on a seat that never waited. A scripted seat is born
    # ready and a round from before the handshake existed never had the key at
    # all; emitting `null` for either would put a line in `unknown` claiming we
    # failed to learn something there was nothing to learn.
    rec["seats"] = [
        {
            k: v
            for k, v in s.items()
            if k in ("seat", "team", "kind", "persona", "prompt", "ready_wait_s")
            and not (k == "prompt" and v is None)
        }
        for s in seats
    ]
    rec["result"] = {
        "winner": verdict["winner"],
        "winner_persona": None,
        "duration_s": verdict["duration_s"],
        "game_over_reason": verdict["reason"],
        "decisive": verdict["decisive"],
    }
    for seat in seats:
        if seat["team"] == verdict["winner"]:
            rec["result"]["winner_persona"] = seat["persona"]
    rec["evidence"]["metrics"] = verdict["metrics"]
    rec["unknown"] = arena.null_paths(rec)
    return rec


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--hypothesis", required=True, help="the question this round is run to answer")
    p.add_argument("--id", help="round id (default: one past the last in the ledger)")
    p.add_argument("--seat", action="append", default=[], metavar="SIDE=KIND[:PERSONA]",
                   help="red|blue = scripted|commander, e.g. red=commander:rusher")
    p.add_argument("--map", default="open", choices=list(arena.MAPS))
    p.add_argument("--speed", type=float, default=1.0)
    p.add_argument("--cap", type=float, default=1800.0, help="BH_MAX_GAME_SECS; 0 for none")
    p.add_argument("--windowed", action="store_true", help="run with a window (default headless)")
    p.add_argument("--env", action="append", default=[], metavar="KEY=VALUE",
                   help="extra environment; overrides the derived value")
    p.add_argument("--notes", default="", help="what changed in the ruleset this round")
    p.add_argument("--commit", default=None, help="the commit the round was played at")
    p.add_argument("--bin", type=Path, default=REPO / "target" / "debug" / "bridgehead")
    p.add_argument("--out", default="arena", help="where round evidence goes (repo-relative)")
    p.add_argument("--bridge-root", type=Path, default=REPO / "bridge")
    p.add_argument("--ledger", type=Path, default=arena.LEDGER)
    p.add_argument("--reuse-seat", action="store_true", help="allow an existing seat directory")
    p.add_argument("--keep-open", action="store_true", help="leave a windowed engine running")
    p.add_argument("--dry-run", action="store_true", help="print the plan; launch nothing")
    p.add_argument("--no-append", action="store_true", help="record the round but don't ledger it")
    args = p.parse_args(argv)

    if not args.seat:
        args.seat = ["red=scripted", "blue=scripted"]
    try:
        seats = [parse_seat(s) for s in args.seat]
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    sides = [s["side"] for s in seats]
    if len(set(sides)) != len(sides):
        print(f"error: two seats on the same side: {sides}", file=sys.stderr)
        return 2

    args.id = args.id or arena.next_id(arena.load(args.ledger))
    if any(r.get("id") == args.id for r in arena.load(args.ledger)):
        print(f"error: round {args.id} is already in {args.ledger}", file=sys.stderr)
        return 2

    try:
        env = derive_env(seats, args)
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    commander_seats = [s for s in seats if s["kind"] == "commander"]
    if args.windowed and not commander_seats:
        print(
            "error: a windowed run has no way to report its own ending — the engine "
            "only self-exits headless. Use --windowed with at least one commander "
            "seat (whose snapshot carries game_over), or drop --windowed.",
            file=sys.stderr,
        )
        return 2

    # Every relative path in this tool is relative to the repo, not to wherever
    # it was invoked — including the `bridge/` the engine itself creates, which
    # is why the child is launched with the repo as its working directory.
    out_dir = Path(args.out)
    if not out_dir.is_absolute():
        out_dir = REPO / out_dir
    out_dir = out_dir / args.id
    print(f"round {args.id}: {args.hypothesis}")
    print(f"  seats:  " + ",  ".join(f"{s['side']}={s['kind']}:{s['persona']}" for s in seats))
    print(f"  env:    " + " ".join(f"{k}={v}" for k, v in sorted(env.items())))
    print(f"  binary: {args.bin}")
    print(f"  out:    {out_dir}")
    if args.dry_run:
        print("\n(dry run — nothing launched)")
        return 0

    if not args.bin.exists():
        print(f"error: {args.bin} does not exist — cargo build first", file=sys.stderr)
        return 2

    live = running_engines(args.bin)
    if live:
        print("\nrefusing to start: the engine is already running —", file=sys.stderr)
        for pid, cmd in live:
            print(f"  pid {pid}: {cmd}", file=sys.stderr)
        print("a second match would overwrite the live one's bridge seats.", file=sys.stderr)
        return 1

    out_dir.mkdir(parents=True, exist_ok=True)
    created = prepare_seats(seats, args.bridge_root, args.reuse_seat)
    for path in created:
        print(f"  created seat {path}")

    # The skeleton lands on disk before the match does, so a round that crashes
    # still leaves behind what it was trying to find out.
    pending = out_dir / "round.json"
    pending.write_text(json.dumps(build_record(args, seats, env, read_log("")), indent=2))
    print(f"  skeleton: {pending}")

    if commander_seats:
        print("\ncommander seats are prepared and waiting — spawn them now against:")
        for seat in commander_seats:
            brief = seat["prompt"] or "tools/COMMANDER_BRIEF.md"
            print(f"  {seat['persona'] or 'unnamed'}: seat {seat['dir']}, brief {brief}")

    launch = dict(os.environ)
    launch.update(env)
    launch.pop("BH_INTENT_LOG", None)
    timeout = (args.cap / max(args.speed, 0.01) + 180) if args.cap else 3600
    # The handshake budget, on top. The engine holds at t=0 until every bridged
    # seat readies, and that hold is wall time the game cap knows nothing about
    # — the game clock is frozen throughout it. Without this term a round whose
    # commanders take their full thinking time gets killed for "outliving its
    # wall timeout" while it is still doing exactly what it was told to.
    if commander_seats:
        timeout += float(launch.get("BH_READY_TIMEOUT", 120.0))
    started = time.monotonic()
    print(f"\nlaunching (wall timeout {timeout:.0f}s)...", flush=True)

    stop_watch = threading.Event()
    watcher = threading.Thread(
        target=watch_handshake, args=(seats, started, stop_watch), daemon=True
    )
    watcher.start()

    log = ""
    if args.windowed:
        proc = subprocess.Popen(
            [str(args.bin)], env=launch, cwd=REPO,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        verdict = wait_for_seat_game_over(
            [Path(s["dir"]) for s in commander_seats], time.monotonic() + timeout
        )
        if not args.keep_open:
            # Our own child, by handle — never a pattern match against the
            # process table.
            proc.terminate()
            try:
                log = proc.communicate(timeout=30)[0] or ""
            except subprocess.TimeoutExpired:
                proc.kill()
                log = proc.communicate()[0] or ""
        from_log = read_log(log)
        # The snapshot is authoritative for the verdict; the log fills the rest.
        verdict["metrics"] = from_log["metrics"]
        if verdict["duration_s"] is None:
            verdict["duration_s"] = from_log["duration_s"]
    else:
        try:
            proc = subprocess.run(
                [str(args.bin)], env=launch, cwd=REPO,
                capture_output=True, text=True, timeout=timeout,
            )
            log = proc.stdout + proc.stderr
        except subprocess.TimeoutExpired as exc:
            log = (exc.stdout or "") + (exc.stderr or "")
            if isinstance(log, bytes):
                log = log.decode("utf8", "replace")
            print("warning: the engine outlived its wall timeout", file=sys.stderr)
        verdict = read_log(log)

    stop_watch.set()
    watcher.join(timeout=2.0)

    wall = time.monotonic() - started
    log_path = out_dir / "engine.log"
    log_path.write_text(log)

    rec = build_record(args, seats, env, verdict)
    rec["evidence"]["logs"] = [str(log_path.relative_to(REPO)) if log_path.is_relative_to(REPO) else str(log_path)]
    rec["evidence"]["logs"] += collect_snapshots(seats, out_dir)
    shots = Path(env["BH_SHOT_DIR"])
    if not shots.is_absolute():
        shots = REPO / shots
    if shots.is_dir():
        rec["evidence"]["shots"] = sorted(
            str(s.relative_to(REPO)) if s.is_relative_to(REPO) else str(s)
            for s in shots.glob("*.png")
        )
    rec["evidence"]["metrics"]["wall_seconds"] = round(wall, 1)
    rec["unknown"] = arena.null_paths(rec)
    pending.write_text(json.dumps(rec, indent=2))

    print()
    if verdict["winner"]:
        print(
            f"{verdict['winner']} wins ({verdict['reason'] or 'reason unrecorded'}) at "
            f"t={verdict['duration_s']}s game, {wall:.0f}s wall"
        )
    else:
        print(f"no verdict after {wall:.0f}s wall — recorded as undecided")

    problems = arena.validate(rec)
    if problems:
        print("\nthe round does not validate:", file=sys.stderr)
        for msg in problems:
            print(f"  {msg}", file=sys.stderr)
        print(f"record kept at {pending}", file=sys.stderr)
        return 1
    if args.no_append:
        print(f"not appended (--no-append); record at {pending}")
        return 0
    arena.append(rec, args.ledger)
    print(f"appended {rec['id']} to {args.ledger}")
    print(f"AARs attach later: tools/arena.py add-aar {rec['id']} --seat <seat> --path <file.md>")
    return 0


if __name__ == "__main__":
    sys.exit(main())
