#!/usr/bin/env python3
"""Atomically write a command batch to a bridge seat's commands.json with auto seq.

Usage: bridge_send.py [--seat DIR] '<json array of commands>'
  --seat defaults to "bridge" (single-seat layout). Two-seat matches use
  --seat bridge/red or --seat bridge/blue.
"""
import json
import os
import sys


def main():
    args = sys.argv[1:]
    seat = "bridge"
    if args and args[0] == "--seat":
        seat = args[1].rstrip("/")
        args = args[2:]
    cmds = json.loads(args[0])
    assert isinstance(cmds, list), "pass a JSON array of command objects"

    state_file = os.path.join(seat, "state.json")
    commands_file = os.path.join(seat, "commands.json")

    # Next seq: one past whatever the game last applied or we last sent.
    seq = 0
    try:
        with open(state_file) as f:
            seq = max(seq, json.load(f).get("seq_applied", 0))
    except Exception:
        pass
    try:
        with open(commands_file) as f:
            seq = max(seq, json.load(f).get("seq", 0))
    except Exception:
        pass

    batch = {"seq": seq + 1, "commands": cmds}
    tmp = os.path.join(seat, "commands.tmp")
    with open(tmp, "w") as f:
        json.dump(batch, f)
    os.replace(tmp, commands_file)
    print(f"sent seq={batch['seq']} ({len(cmds)} commands) to {seat}")


if __name__ == "__main__":
    main()
