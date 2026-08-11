#!/usr/bin/env python3
"""Tests for bridge_wait's novelty rule — especially the `errors` half.

    python3 tools/test_bridge_wait.py

The regression these exist for is arena round 17, where a blocked plan step's
repeating refusal woke the commander's pacing loop on every call, the commander
chained waits to escape the noise, and the match was decided in the ~100 game
seconds of silence that produced. The engine no longer re-emits that error
(plan.rs emits transitions); this file pins the second, independent defence:
`bridge_wait` wakes on a **new** error set and never on the same one twice.

Every case below drives the real script in a subprocess against a real
`state.json`, because the thing under test is a decision made across two runs
via a file on disk — a mocked marker would test the mock.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import bridge_wait  # noqa: E402

TOOL = Path(__file__).resolve().parent / "bridge_wait.py"
SEAT = "bridge/waittest"


class Seat:
    """A throwaway seat directory plus its marker, cleaned up on exit.

    `--max 0` everywhere: the deadline is checked AFTER the wake checks, so a
    zero-second budget gives an instant quiet cycle when nothing is new and an
    instant wake when something is. The cadence is not what these test.
    """

    def __init__(self, tmp: str):
        self.dir = Path(tmp)
        (self.dir / SEAT).mkdir(parents=True)
        self.state = self.dir / SEAT / "state.json"
        self.marker = Path("/tmp/claude-1000") / ("bridge_wait_" + SEAT.replace("/", "_"))
        self.marker.parent.mkdir(parents=True, exist_ok=True)
        self.marker.unlink(missing_ok=True)
        self.t = 0.0

    def write(self, *, errors=None, events=None, game_over=None, bump=1.0):
        self.t += bump
        state = {"t": self.t, "errors": list(errors or []), "events": list(events or [])}
        if game_over:
            state["game_over"] = game_over
        self.state.write_text(json.dumps(state))

    def wait(self) -> str:
        out = subprocess.run(
            [sys.executable, str(TOOL), "--seat", SEAT, "--max", "0"],
            cwd=self.dir, capture_output=True, text=True, timeout=30,
        )
        assert out.returncode == 0, out.stderr
        return out.stdout.strip()

    def close(self):
        self.marker.unlink(missing_ok=True)


def seat_test(fn):
    """Run `fn(seat)` against a fresh seat, and always clean the marker up."""
    def wrapped():
        with tempfile.TemporaryDirectory() as tmp:
            seat = Seat(tmp)
            try:
                fn(seat)
            finally:
                seat.close()
    wrapped.__name__ = fn.__name__
    wrapped.__doc__ = fn.__doc__
    return wrapped


# -- the fingerprint itself -------------------------------------------------

def test_fingerprint_is_a_set_not_a_list():
    """Same two refusals in the other order is the same news."""
    a = bridge_wait.fingerprint(["cannot afford Footman", "queue full"])
    b = bridge_wait.fingerprint(["queue full", "cannot afford Footman"])
    assert a == b, "order must not count as novelty"
    assert bridge_wait.fingerprint(["x", "x", "x"]) == bridge_wait.fingerprint(["x"]), \
        "a repeated string within one array is one error"


def test_an_empty_error_set_has_an_empty_fingerprint():
    """So 'no errors' is its own state, and a returning error reads as change."""
    assert bridge_wait.fingerprint([]) == ""
    assert bridge_wait.fingerprint(None) == ""
    assert bridge_wait.fingerprint(["boom"]) != ""


def test_a_different_error_is_a_different_fingerprint():
    assert bridge_wait.fingerprint(["a"]) != bridge_wait.fingerprint(["b"])
    assert bridge_wait.fingerprint(["a"]) != bridge_wait.fingerprint(["a", "b"])


def test_the_marker_reads_the_old_bare_float_format():
    """A marker written by an older checkout degrades, it does not crash.

    The pre-v2 file was a float and nothing else. Reading one must give back
    the event position it really carries and an empty error memory — which
    costs exactly one extra wake, once, and never a traceback.
    """
    with tempfile.TemporaryDirectory() as tmp:
        old = Path(tmp) / "marker"
        old.write_text("412.5")
        assert bridge_wait.read_marker(old) == (412.5, "")
        old.write_text("not a number at all")
        assert bridge_wait.read_marker(old) == (0.0, "")
        assert bridge_wait.read_marker(Path(tmp) / "nope") == (0.0, "")


def test_the_marker_round_trips_both_halves():
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "marker"
        bridge_wait.write_marker(path, 61.25, "deadbeef")
        assert bridge_wait.read_marker(path) == (61.25, "deadbeef")


# -- the behaviour that lost round 17 ---------------------------------------

@seat_test
def test_an_identical_error_never_wakes_twice(seat):
    """**The r17 fix.** One blocked step, twelve retries, one wake.

    The error stays in the array the whole time — that is what a persistent
    condition looks like on a channel with no timestamps — and the commander
    is told once.
    """
    blocked = "cmd 0: cannot afford Footman (135g 0l)"
    seat.write(errors=[blocked])
    first = seat.wait()
    assert "WAKE: new command errors" in first, first
    assert "cannot afford Footman" in first

    for _ in range(12):
        seat.write(errors=[blocked])
        again = seat.wait()
        assert again.startswith("WAKE: quiet cycle"), \
            f"a re-emitted identical error must not wake: {again!r}"


@seat_test
def test_a_new_distinct_error_always_wakes(seat):
    """Because a refusal that changed its words is a different problem."""
    seat.write(errors=["cmd 0: cannot afford Footman (135g 0l)"])
    assert "new command errors" in seat.wait()
    seat.write(errors=["cmd 0: cannot afford Footman (135g 0l)"])
    assert "quiet cycle" in seat.wait()

    # A second, different refusal arrives beside the first: the SET changed.
    seat.write(errors=[
        "cmd 0: cannot afford Footman (135g 0l)",
        "cmd 1: building 424242 not found/not yours",
    ])
    woke = seat.wait()
    assert "new command errors" in woke, woke
    assert "424242" in woke


@seat_test
def test_an_error_that_clears_and_returns_is_news_again(seat):
    """An empty array between the two is what makes the second one real.

    This is the case a naive "remember the last error string" would get wrong:
    the same refusal after a genuine recovery is a genuine regression, and a
    commander who fixed their economy and broke it again should hear about it.
    """
    seat.write(errors=["cmd 0: cannot afford Footman (135g 0l)"])
    assert "new command errors" in seat.wait()
    seat.write(errors=[])
    assert "quiet cycle" in seat.wait()
    seat.write(errors=["cmd 0: cannot afford Footman (135g 0l)"])
    assert "new command errors" in seat.wait(), "a returning error is news"


@seat_test
def test_events_still_wake_it_while_a_stale_error_stands(seat):
    """The channel that must never be dulled by the one being quietened.

    A commander whose plan is blocked is exactly the commander who most needs
    to hear that their base is under attack.
    """
    blocked = "cmd 0: cannot afford Footman (135g 0l)"
    seat.write(errors=[blocked])
    assert "new command errors" in seat.wait()
    seat.write(errors=[blocked])
    assert "quiet cycle" in seat.wait()

    seat.write(errors=[blocked], events=[[seat.t + 1, "enemy army spotted: ~16"]])
    woke = seat.wait()
    assert "enemy army spotted" in woke, woke


@seat_test
def test_game_over_still_short_circuits_everything(seat):
    seat.write(errors=["cmd 0: cannot afford Footman (135g 0l)"], game_over="Claude")
    assert "WAKE: game over" in seat.wait()


@seat_test
def test_a_quiet_cycle_still_records_where_it_looked(seat):
    """Marker hygiene: a timeout must remember both halves, or the next call
    re-announces an error it already slept through."""
    blocked = "cmd 0: cannot afford Footman (135g 0l)"
    seat.write(errors=[])
    assert "quiet cycle" in seat.wait()
    seat.write(errors=[blocked])
    assert "new command errors" in seat.wait()
    t, fp = bridge_wait.read_marker(seat.marker)
    assert t == seat.t, "the marker moved to the state it woke on"
    assert fp == bridge_wait.fingerprint([blocked]), "and remembered what it saw"


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
