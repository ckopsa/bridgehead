# Quickstart: two LLMs battling each other

This walks you from a fresh clone to a finished LLM-vs-LLM match, recorded in
the arena ledger. Total setup is one build, one launch command, and two agent
prompts. You need the Rust toolchain, `python3`, and something that can run an
LLM agent with shell access — the examples use [Claude Code](https://claude.com/claude-code),
but any agent that can run `python3 tools/bridge_*.py` in a loop can command a
faction. That is the point of the project: the bridge is three small scripts
and a JSON file, not an SDK.

## 1. Build the engine

```bash
cargo build        # dev profile only; the binary lands at target/debug/bridgehead
```

## 2. Launch the round

`tools/arena_run.py` owns the mechanical half of a match: it derives the
environment from the seats, prepares `bridge/red/` and `bridge/blue/`, launches
the engine, waits for a real game over, and appends a validated record to
`arena/ledger.jsonl`. It does **not** spawn the commanders — that's you, in
step 3.

```bash
python3 tools/arena_run.py \
  --hypothesis "does the rush beat the boom on crossings?" \
  --seat red=commander:rusher \
  --seat blue=commander:boomer \
  --map crossings --windowed --cap 1800
```

- `--windowed` gives you a spectator window (F1–F4 change game speed live).
  Drop it for a headless run — the engine then self-exits on game over.
- `--cap 1800` is a safety cap in *game* seconds, not a game rule.
- The persona after `commander:` is just a label for the ledger; the actual
  persona is whatever you put in the agent's prompt.
- If a previous match left seat directories behind, add `--reuse-seat`
  (stale snapshots are moved aside, never deleted).

The runner prints the seat directories and then waits:

```
commander seats are prepared and waiting — spawn them now against:
  rusher: seat /path/to/repo/bridge/red, brief tools/COMMANDER_BRIEF.md
  boomer: seat /path/to/repo/bridge/blue, brief tools/COMMANDER_BRIEF.md
```

**The match holds at t=0 until both commanders send `ready`** — nothing moves,
no gold is mined, and neither side loses opening time to a slow-connecting
opponent. `BH_READY_TIMEOUT` (default 120s wall) starts the match without a
seat that stays silent.

## 3. Spawn the two commanders

Open two terminals (or spawn two subagents from one orchestrating session —
this repo ships `.claude/agents/commander.md` as a ready-made agent type for
that). Each commander gets a prompt like:

```
You are an RTS faction commander. Read tools/COMMANDER_BRIEF.md and follow it
exactly — it is your complete protocol reference.

Your seat directory: bridge/red
Your persona: rusher — tempo above everything; end the game before it settles.

Read the map, send your opening orders, then send {"type":"ready"}. Loop with
bridge_wait.py / bridge_view.py / bridge_send.py until game_over is non-null,
then stop and write a short after-action report: what you did, what you saw,
what you would change.
```

With Claude Code, that's:

```bash
cd /path/to/repo && claude -p "$(cat <<'EOF'
...the prompt above...
EOF
)"
```

(the second terminal gets the same thing with `bridge/blue` and the opposing
persona.)

The commander's loop, from `tools/COMMANDER_BRIEF.md`, is event-driven — no
polling, no screen parsing:

```bash
python3 tools/bridge_send.py --seat bridge/red '[{"type":"ready"}]'   # once, after reading the map
python3 tools/bridge_wait.py --seat bridge/red --max 15               # blocks, wakes early on events
python3 tools/bridge_view.py bridge/red/state.json                    # compact tactical readout
python3 tools/bridge_send.py --seat bridge/red \
  '[{"type":"posture","id":0,"posture":{"type":"push","x":-70.0,"z":-70.0}}]'
```

Everything a commander can do — orders, squads, doctrine, research, items,
plans, triggers — compiles to the same intent vocabulary the human UI uses.
Rejected commands come back as errors that name the fix.

## 4. Watch, finish, read the record

Spectate in the window, or peek at either seat from a third terminal with
`bridge_view.py` (reading `state.json` is always safe; only the seat's own
commander should *write*). When the game ends:

- the runner appends the round to `arena/ledger.jsonl` and keeps each seat's
  final snapshot in `arena/<round-id>/`;
- each commander sees `game_over` in its snapshot and stops;
- after-action reports can be attached with `tools/arena.py add-aar`.

Twenty recorded rounds of exactly this kind of match — and the balance patches
they forced — are in [ARENA.md](ARENA.md) and `arena/ledger.jsonl`.

## The manual path (no ledger)

If you just want the engine up with two bridged seats and no record-keeping:

```bash
cargo build && BH_BRIDGE=both BH_MAP=crossings ./target/debug/bridgehead
```

`bridge/red/` and `bridge/blue/` each get a `state.json` written once per
second and a `commands.json` polled four times per second. Spawn commanders as
in step 3. One warning, learned the hard way: **the bridge is a live
singleton** — one directory per seat, overwritten in place — so never point a
second engine or a stray verification script at a seat directory while a match
is using it.

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| runner says `refusing to start: the engine is already running` | another match is live; it will not kill it for you — find and stop your own process by PID |
| runner says seat `already exists` | leftover seat directory from an earlier match; pass `--reuse-seat` |
| snapshot shows `match_started: false`, `t: 0` | the ready handshake — some seat hasn't sent `{"type":"ready"}` yet (`waiting_for` names it) |
| commands seem ignored | read `errors` in the next snapshot; every rejected command explains itself there |
