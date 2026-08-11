#!/usr/bin/env python3
"""Calibration sweep for Chain of Command (docs/TEMPO.md §5, issue 8).

Runs headless AI-vs-AI matches across a grid of latency curves plus a flag-off
baseline, and reports what each arm did to match length, decisiveness, and the
link load the armies actually carried.

    tools/link_sweep.py                     # the default grid, ~40 runs
    tools/link_sweep.py --dry-run           # print the plan and the budget
    tools/link_sweep.py --analyze sweep/runs.csv    # re-table an old sweep

WHAT IT MEASURES, AND WHAT IT CANNOT
------------------------------------
`report_link_load` (command.rs) is the only evidence an AI-vs-AI match leaves
about latency: the scripted AI is not a player and writes no intent log. It
prints every 30 game-seconds, *and only when something is in transit*, so a
missing sample means "nothing was travelling at that instant" rather than "no
data". That asymmetry is deliberate upstream and it is the signal this script
leans on — see `classify` below.

Match length in game seconds is inferred as `wall_seconds * BH_SPEED`. The
engine logs no game clock, and Bevy's virtual clock advances by real delta times
the speed multiplier, so the inference is exact up to process start-up and any
frame that hit Bevy's 0.25s max-delta clamp. It is self-checking: a run that
hits `BH_MAX_GAME_SECS` must come out near the cap, and `--analyze` reports the
error on exactly those runs as `cap_err`.

THE CONFOUNDER
--------------
docs/TEMPO.md §7 warns that capped runs in this game are usually the
mine-exhaustion stalemate, not a tempo problem: mines dry, both armies alive,
treasuries banking lumber, nothing in transit. A sweep that reads match length
alone will mistake one for the other and "discover" that latency lengthens
matches. So every capped run is classified rather than counted:

  * `cap-economy`  — capped with an empty in-transit queue through the late
                     game. The economy ran out. NOT a tempo finding.
  * `cap-severed`  — capped with mean link pinned at the ceiling. The armies
                     marched off the end of their own chain of command. This
                     one IS a tempo finding.
  * `cap-unclear`  — capped, flag on, neither signature clean.
  * `cap-baseline` — capped with the flag off, where the signal does not exist
                     at all. Reported as the control rate and nothing more.

EXTENDING THE GRID
------------------
Every axis is a comma-separated flag; the grid is their product, and the
baseline arm is always prepended.

    tools/link_sweep.py --halls 30,45,60,90 --steps 0.3,0.6,1.2 \
                        --per-units 0.01,0.02 --maxes 3.0,5.0 \
                        --seeds 5 --map crossings --budget 7200

Wall-clock cost is roughly `arms * seeds * (match_seconds / speed)`, and the
`--budget` ceiling (seconds) stops the sweep cleanly between runs rather than
leaving a half-finished grid: partial results are still written and still
tabled, with the unrun arms named. Raising `--speed` shortens every run but
coarsens the simulation, so 16 is the value docs/TEMPO.md §4 fixed for sweeps
and the one the published numbers use.
"""

from __future__ import annotations

import argparse
import csv
import itertools
import os
import re
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, asdict, fields
from pathlib import Path

# `chain of command @420s: 3 orders in transit, mean link 1.24s, worst 3.00s
#  (cap 3.0s), 4 command nodes standing`
LINK_LINE = re.compile(
    r"chain of command @(?P<t>[\d.]+)s: (?P<count>\d+) orders in transit, "
    r"mean link (?P<mean>[\d.]+)s, worst (?P<worst>[\d.]+)s "
    r"\(cap (?P<cap>[\d.]+)s\), (?P<nodes>\d+) command nodes standing"
)
# The tail of this line has grown twice — first the reason, then the game clock
# — and each time an exact-match pattern here silently reclassified every
# decisive run as a crash. Anchor on the part that carries the verdict and let
# the rest be optional, so old sweep logs still parse and the next addition is
# not a bug. tools/arena_run.py reads the same line for duration and reason.
DECISIVE = re.compile(
    r"headless: game over — (?P<winner>\w+) wins"
    r"(?: \((?P<reason>\w+)\))?(?: at t=(?P<t>[\d.]+)s)?"
)
TIMECAP = re.compile(r"headless: time cap [\d.]+s — timeout verdict: (?P<verdict>[^(]+)")

# A run whose in-transit queue is empty for this fraction of the match tail is
# a match nothing was being ordered in — the economy-stalemate signature.
LATE_FRACTION = 0.35
# "Pinned at the ceiling": mean link within this of `max` says the armies are
# fighting off the end of their chain of command rather than at the end of it.
PINNED_SLACK = 0.15


@dataclass
class Arm:
    """One point of the grid. `on=False` is the baseline."""

    on: bool
    hall: float
    hero: float
    step: float
    per_unit: float
    max: float

    @property
    def label(self) -> str:
        if not self.on:
            return "baseline (flag off)"
        return f"hall {self.hall:g} / step {self.step:g} / per-unit {self.per_unit:g} / cap {self.max:g}"

    @property
    def key(self) -> str:
        if not self.on:
            return "off"
        return f"h{self.hall:g}-s{self.step:g}-p{self.per_unit:g}-m{self.max:g}"

    def env(self) -> dict[str, str]:
        if not self.on:
            return {"BH_COMMAND_LATENCY": "0"}
        return {
            "BH_COMMAND_LATENCY": "1",
            "BH_LINK_HALL_RADIUS": f"{self.hall:g}",
            "BH_LINK_HERO_RADIUS": f"{self.hero:g}",
            "BH_LINK_STEP": f"{self.step:g}",
            "BH_LINK_PER_UNIT": f"{self.per_unit:g}",
            "BH_LINK_MAX": f"{self.max:g}",
        }


@dataclass
class Run:
    arm: str
    label: str
    on: int
    hall: float
    step: float
    per_unit: float
    max: float
    seed: int
    map: str
    outcome: str           # decisive | cap | crash
    winner: str
    game_secs: float       # inferred, see module docstring
    wall_secs: float
    samples: int           # report_link_load lines seen
    mean_link: float       # mean of the sampled means
    worst_link: float
    mean_in_transit: float
    late_samples: int      # samples in the match tail
    klass: str             # classification, see `classify`


def classify(run: Run, cap_secs: float, arm: Arm) -> str:
    """Name what actually happened, so a stalemate is never read as tempo."""
    if run.outcome == "crash":
        return "crash"
    if run.outcome == "decisive":
        return "decisive"
    if not arm.on:
        # The flag-off arm produces no link telemetry at all, so its caps carry
        # no signature. They are the control rate for how often this matchup
        # stalls on its own, and claiming more than that would be inventing it.
        return "cap-baseline"
    if run.mean_link >= arm.max - PINNED_SLACK and run.samples > 0:
        return "cap-severed"
    if run.late_samples == 0:
        return "cap-economy"
    return "cap-unclear"


def parse(log: str, wall: float, speed: float, cap_secs: float) -> dict:
    """Everything a single run's stdout has to say."""
    out = {
        "outcome": "crash",
        "winner": "",
        "samples": 0,
        "mean_link": 0.0,
        "worst_link": 0.0,
        "mean_in_transit": 0.0,
        "late_samples": 0,
    }
    if m := DECISIVE.search(log):
        out["outcome"] = "decisive"
        out["winner"] = m.group("winner")
    elif m := TIMECAP.search(log):
        out["outcome"] = "cap"
        out["winner"] = m.group("verdict").strip()

    samples = [m.groupdict() for m in LINK_LINE.finditer(log)]
    if samples:
        means = [float(s["mean"]) for s in samples]
        counts = [int(s["count"]) for s in samples]
        # The engine stamps each sample with game time, so the tail is measured
        # against the run's own clock rather than against the inferred length.
        stamps = [float(s["t"]) for s in samples]
        horizon = max(stamps[-1], cap_secs if out["outcome"] == "cap" else stamps[-1])
        out.update(
            samples=len(samples),
            mean_link=round(statistics.fmean(means), 3),
            worst_link=max(float(s["worst"]) for s in samples),
            mean_in_transit=round(statistics.fmean(counts), 2),
            late_samples=sum(1 for t in stamps if t >= horizon * (1.0 - LATE_FRACTION)),
        )
    return out


def run_one(arm: Arm, seed: int, args, log_dir: Path) -> Run:
    env = dict(os.environ)
    env.update(
        BH_HEADLESS="1",
        BH_AI_BOTH="1",
        BH_SPEED=str(args.speed),
        BH_MAP=args.map,
        BH_MAX_GAME_SECS=str(args.cap),
        # The sweep must not be perturbed by a stray bridge or replay log.
        BH_BRIDGE="0",
    )
    env.update(arm.env())
    env.pop("BH_INTENT_LOG", None)

    started = time.monotonic()
    # Generous: the cap is in game seconds, so the wall ceiling is that over
    # the speed multiplier, plus room for start-up and a slow frame or two.
    timeout = args.cap / args.speed + 120
    try:
        proc = subprocess.run(
            [args.bin],
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        log = proc.stdout + proc.stderr
    except subprocess.TimeoutExpired as exc:
        log = (exc.stdout or "") + (exc.stderr or "")
        if isinstance(log, bytes):
            log = log.decode("utf8", "replace")
    wall = time.monotonic() - started

    (log_dir / f"{arm.key}-seed{seed}.log").write_text(log)
    parsed = parse(log, wall, args.speed, args.cap)
    run = Run(
        arm=arm.key,
        label=arm.label,
        on=int(arm.on),
        hall=arm.hall,
        step=arm.step,
        per_unit=arm.per_unit,
        max=arm.max,
        seed=seed,
        map=args.map,
        game_secs=round(wall * args.speed, 1),
        wall_secs=round(wall, 1),
        klass="",
        **parsed,
    )
    run.klass = classify(run, args.cap, arm)
    return run


def build_grid(args) -> list[Arm]:
    baseline = Arm(on=False, hall=0, hero=0, step=0, per_unit=0, max=0)
    grid = [
        Arm(on=True, hall=h, hero=args.hero, step=s, per_unit=p, max=m)
        for h, s, p, m in itertools.product(args.halls, args.steps, args.per_units, args.maxes)
    ]
    return [baseline] + grid


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def table(runs: list[Run]) -> str:
    """The markdown table docs/TEMPO.md §calibration carries."""
    by_arm: dict[str, list[Run]] = {}
    for r in runs:
        by_arm.setdefault(r.arm, []).append(r)

    def fmt(v: float, nd: int = 1) -> str:
        return f"{v:.{nd}f}"

    lines = [
        "| arm | n | decisive | median length (game s) | mean link | worst link | mean in transit | caps (classified) |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for key, group in by_arm.items():
        label = group[0].label
        n = len(group)
        decisive = sum(1 for r in group if r.klass == "decisive")
        lengths = sorted(r.game_secs for r in group)
        med = statistics.median(lengths) if lengths else 0.0
        linked = [r for r in group if r.samples > 0]
        mean_link = statistics.fmean([r.mean_link for r in linked]) if linked else 0.0
        worst = max((r.worst_link for r in group), default=0.0)
        in_transit = statistics.fmean([r.mean_in_transit for r in linked]) if linked else 0.0
        caps = [r.klass for r in group if r.klass.startswith("cap")]
        cap_note = ", ".join(sorted(set(caps))) if caps else "—"
        if caps:
            cap_note = f"{len(caps)} ({cap_note})"
        lines.append(
            f"| {label} | {n} | {decisive}/{n} | {fmt(med, 0)} | {fmt(mean_link, 2)} | "
            f"{fmt(worst, 2)} | {fmt(in_transit, 1)} | {cap_note} |"
        )
    return "\n".join(lines)


def cap_error(runs: list[Run], cap: float) -> str:
    """Self-check on the inferred clock: capped runs should land on the cap."""
    capped = [r for r in runs if r.outcome == "cap"]
    if not capped:
        return "no capped runs — the length inference has nothing to check itself against"
    errs = [abs(r.game_secs - cap) / cap for r in capped]
    return (
        f"{len(capped)} capped runs; inferred length off the {cap:g}s cap by "
        f"{statistics.fmean(errs) * 100:.1f}% on average "
        f"(worst {max(errs) * 100:.1f}%)"
    )


def write_csv(path: Path, runs: list[Run]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=[f.name for f in fields(Run)])
        writer.writeheader()
        for r in runs:
            writer.writerow(asdict(r))


def read_csv(path: Path) -> list[Run]:
    types = {f.name: f.type for f in fields(Run)}
    runs = []
    with path.open() as fh:
        for row in csv.DictReader(fh):
            typed = {}
            for k, v in row.items():
                t = types[k]
                typed[k] = int(v) if t is int else float(v) if t is float else v
            runs.append(Run(**typed))
    return runs


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    csvlist = lambda cast: (lambda s: [cast(x) for x in s.split(",") if x])  # noqa: E731
    p.add_argument("--bin", default="./target/debug/bridgehead")
    p.add_argument("--out", default="sweep", help="output directory")
    p.add_argument("--map", default="open", choices=["open", "crossings"])
    p.add_argument("--speed", type=float, default=16.0)
    p.add_argument("--cap", type=float, default=1800.0, help="BH_MAX_GAME_SECS")
    p.add_argument("--seeds", type=int, default=3, help="replicate runs per arm")
    p.add_argument("--halls", type=csvlist(float), default=[30.0, 45.0, 60.0])
    p.add_argument("--steps", type=csvlist(float), default=[0.3, 0.6])
    p.add_argument("--per-units", type=csvlist(float), default=[0.01, 0.02])
    p.add_argument("--maxes", type=csvlist(float), default=[3.0])
    p.add_argument("--hero", type=float, default=18.0)
    p.add_argument("--budget", type=float, default=5400.0, help="total wall seconds")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--analyze", metavar="CSV", help="re-table an existing runs.csv")
    args = p.parse_args()

    if args.analyze:
        runs = read_csv(Path(args.analyze))
        print(table(runs))
        print()
        print(cap_error(runs, args.cap))
        return 0

    arms = build_grid(args)
    total = len(arms) * args.seeds
    print(f"{len(arms)} arms x {args.seeds} seeds = {total} runs on `{args.map}` "
          f"at {args.speed:g}x, cap {args.cap:g}s game")
    print(f"wall budget {args.budget:g}s; worst case per run {args.cap / args.speed:.0f}s")
    if args.dry_run:
        for a in arms:
            print(f"  {a.key:28} {a.label}")
        return 0

    out = Path(args.out)
    log_dir = out / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    csv_path = out / "runs.csv"

    runs: list[Run] = []
    started = time.monotonic()
    stopped_early = None
    for i, (arm, seed) in enumerate(itertools.product(arms, range(args.seeds)), 1):
        spent = time.monotonic() - started
        if spent > args.budget:
            stopped_early = f"budget of {args.budget:g}s wall exhausted after {i - 1}/{total} runs"
            break
        r = run_one(arm, seed, args, log_dir)
        runs.append(r)
        write_csv(csv_path, runs)  # crash-safe: the CSV is complete after each run
        print(
            f"[{i}/{total}] {arm.key:28} seed {seed}  {r.klass:12} "
            f"{r.game_secs:6.0f}s game  link {r.mean_link:.2f}/{r.worst_link:.2f}  "
            f"{r.samples:3d} samples",
            flush=True,
        )

    print()
    print(table(runs))
    print()
    print(cap_error(runs, args.cap))
    if stopped_early:
        done = {r.arm for r in runs}
        missing = [a.key for a in arms if a.key not in done]
        print(f"\nSTOPPED EARLY: {stopped_early}")
        if missing:
            print(f"arms never run: {', '.join(missing)}")
    print(f"\nrows: {csv_path}   logs: {log_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
