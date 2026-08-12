#!/usr/bin/env python3
"""Atomically write a command batch to a bridge seat's commands.json with auto seq.

Usage: bridge_send.py [--seat DIR] '<json array of commands>'
  --seat defaults to "bridge" (single-seat layout). Two-seat matches use
  --seat bridge/red or --seat bridge/blue.

The engine keeps no queue: it polls commands.json at 4 Hz and only the newest
batch matters. A second send inside that window used to overwrite a batch the
engine had not read yet — arena round 22 lost both seats' opening batches to a
send followed immediately by `ready`. So an unconsumed batch is never
clobbered: we wait briefly for the engine to take it (the normal case, one
poll interval), and if it still sits there — engine busy, held, or not up —
its commands are carried forward at the front of the new batch instead.
BH_SEND_WAIT_SECS tunes the wait (default 2.0; 0 skips straight to the merge).
"""
import json
import os
import sys
import time


def _read_json(path):
    try:
        with open(path) as f:
            return json.load(f)
    except Exception:
        return None


def _seq_applied(state_file):
    state = _read_json(state_file)
    return state.get("seq_applied", 0) if isinstance(state, dict) else 0


def send(seat, cmds, wait_secs=None):
    """Write `cmds` to the seat, carrying forward any unconsumed batch.

    Returns the batch written (dict with `seq`, `commands`, and — when a
    pending batch was folded in — the count in `carried`).
    """
    if wait_secs is None:
        wait_secs = float(os.environ.get("BH_SEND_WAIT_SECS", "2.0"))

    state_file = os.path.join(seat, "state.json")
    commands_file = os.path.join(seat, "commands.json")

    applied = _seq_applied(state_file)
    pending = _read_json(commands_file)
    pending_seq = pending.get("seq", 0) if isinstance(pending, dict) else 0
    pending_cmds = pending.get("commands") if isinstance(pending, dict) else None

    carried = []
    if isinstance(pending_cmds, list) and pending_seq > applied:
        # The engine has not read the last batch. Give it a moment — it polls
        # four times a second, so the honest case resolves in one interval.
        deadline = time.monotonic() + wait_secs
        while time.monotonic() < deadline:
            time.sleep(0.05)
            applied = _seq_applied(state_file)
            if applied >= pending_seq:
                break
        if applied < pending_seq:
            carried = pending_cmds

    # Next seq: one past whatever the game last applied or we last sent.
    seq = max(applied, pending_seq)

    batch = {"seq": seq + 1, "commands": carried + cmds}
    tmp = os.path.join(seat, "commands.tmp")
    with open(tmp, "w") as f:
        json.dump(batch, f)
    os.replace(tmp, commands_file)
    if carried:
        batch["carried"] = len(carried)
    return batch


def main():
    args = sys.argv[1:]
    seat = "bridge"
    if args and args[0] == "--seat":
        seat = args[1].rstrip("/")
        args = args[2:]
    cmds = json.loads(args[0])
    assert isinstance(cmds, list), "pass a JSON array of command objects"

    batch = send(seat, cmds)
    note = ""
    if batch.get("carried"):
        note = f" (+{batch['carried']} unconsumed carried forward)"
    print(f"sent seq={batch['seq']} ({len(cmds)} commands){note} to {seat}")


if __name__ == "__main__":
    main()
