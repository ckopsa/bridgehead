#!/usr/bin/env python3
"""Tests for bridge_send's no-clobber rule.

    python3 tools/test_bridge_send.py

The regression these exist for is arena round 22: the engine polls
commands.json at 4 Hz and keeps no queue, so a batch followed immediately by a
second send (canonically `ready`) overwrote commands the engine had never
read, and both seats played their opening from an empty book. The rule under
test: a batch the engine has not consumed is never clobbered — it is carried
forward at the front of the next one.

Every case drives the real send path against a real seat directory, because
the thing under test is two writes racing an absent reader via files on disk.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import bridge_send  # noqa: E402

TOOL = Path(__file__).resolve().parent / "bridge_send.py"


def write_state(seat: Path, seq_applied: int) -> None:
    (seat / "state.json").write_text(json.dumps({"seq_applied": seq_applied}))


def read_batch(seat: Path) -> dict:
    return json.loads((seat / "commands.json").read_text())


def test_fresh_send(tmp_path):
    """First send to a quiet seat: seq 1, commands as given."""
    batch = bridge_send.send(str(tmp_path), [{"type": "ready"}], wait_secs=0)
    assert batch["seq"] == 1
    on_disk = read_batch(tmp_path)
    assert on_disk == {"seq": 1, "commands": [{"type": "ready"}]}


def test_consumed_batch_is_not_carried(tmp_path):
    """A batch the engine already applied is history, not cargo."""
    write_state(tmp_path, 3)
    (tmp_path / "commands.json").write_text(
        json.dumps({"seq": 3, "commands": [{"type": "stop", "units": [1]}]})
    )
    batch = bridge_send.send(str(tmp_path), [{"type": "ready"}], wait_secs=0)
    assert batch["seq"] == 4
    assert batch["commands"] == [{"type": "ready"}]


def test_unconsumed_batch_is_carried_forward(tmp_path):
    """The r22 case: opening batch, then ready, no engine in between."""
    write_state(tmp_path, 0)
    opening = [{"type": "train", "building": 5, "unit": "Footman"}]
    bridge_send.send(str(tmp_path), opening, wait_secs=0)
    batch = bridge_send.send(str(tmp_path), [{"type": "ready"}], wait_secs=0)
    assert batch["carried"] == 1
    on_disk = read_batch(tmp_path)
    assert on_disk["commands"] == opening + [{"type": "ready"}]
    assert on_disk["seq"] == 2


def test_wait_yields_to_a_live_engine(tmp_path):
    """If the engine consumes mid-wait, nothing is carried and nothing doubles."""
    write_state(tmp_path, 0)
    bridge_send.send(str(tmp_path), [{"type": "stop", "units": [1]}], wait_secs=0)

    def consume():
        time.sleep(0.15)
        write_state(tmp_path, 1)

    eater = threading.Thread(target=consume)
    eater.start()
    batch = bridge_send.send(str(tmp_path), [{"type": "ready"}], wait_secs=2.0)
    eater.join()
    assert "carried" not in batch
    assert batch["seq"] == 2
    assert read_batch(tmp_path)["commands"] == [{"type": "ready"}]


def test_malformed_pending_is_replaced(tmp_path):
    """Garbage on disk is the engine's error to report, not ours to preserve."""
    write_state(tmp_path, 0)
    (tmp_path / "commands.json").write_text("{not json")
    batch = bridge_send.send(str(tmp_path), [{"type": "ready"}], wait_secs=0)
    assert batch["seq"] == 1
    assert read_batch(tmp_path)["commands"] == [{"type": "ready"}]


def test_cli_reports_carry(tmp_path):
    """The CLI says out loud when it carried commands forward."""
    write_state(tmp_path, 0)
    env = {"BH_SEND_WAIT_SECS": "0", "PATH": "/usr/bin:/bin"}
    subprocess.run(
        [sys.executable, str(TOOL), "--seat", str(tmp_path), '[{"type":"stop","units":[1]}]'],
        check=True, capture_output=True, env=env,
    )
    out = subprocess.run(
        [sys.executable, str(TOOL), "--seat", str(tmp_path), '[{"type":"ready"}]'],
        check=True, capture_output=True, text=True, env=env,
    ).stdout
    assert "sent seq=2 (1 commands) (+1 unconsumed carried forward)" in out


if __name__ == "__main__":
    import pytest

    raise SystemExit(pytest.main([__file__, "-v"]))
