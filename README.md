# Bridgehead

**An RTS where a human and an AI play the same game as equals.**

Every game ever shipped was built for one kind of player: a creature with eyes,
hands, and millisecond reflexes. When an AI plays such a game, it plays through
a disguise — screen pixels parsed, mouse clicks synthesized, or a privileged
API bolted on the side. Bridgehead is the alternative: a Warcraft-3-style 3D
RTS in Rust + [Bevy](https://bevyengine.org) where a human at a mouse and an
LLM commander on a file bridge have **equitable access to the same game** — one
vocabulary of intent, one rule of knowability, one set of refusals, and victory
decided at the layer where both are genuinely peers: **judgment**.

![Fog of war](docs/fog-of-war.png)

## How it works

- **One intent vocabulary.** Human UI gestures and LLM bridge commands compile
  to the *same* primitives — orders, postures, doctrine, plans. Fairness is
  structural: the AI cannot act in ways the human cannot, because there is no
  other API.
- **The doctrine layer.** Fast work belongs to the engine: retreat thresholds,
  focus-fire, squad postures, and foraging execute at machine speed for
  *whichever* player set them. The slow brain — human or LLM — makes decisions
  worthy of its latency.
- **One rule of knowability.** Fog of war is computed once and rendered three
  ways (ground, minimap, snapshot). What the LLM's snapshot contains and what
  the human's screen shows are the same facts.
- **Deterministic by construction.** Every gameplay system runs in a named
  phase of a fixed frame order; a match replays byte-for-byte from a seed.
- **Legible refusals.** A rejected command explains itself and names the fix:
  `Raider trains at the Barracks once a Workshop stands (you have none)`.
- **Co-command.** `BH_BRIDGE=copilot` seats an AI *alongside* a human on one
  faction: postures apply directly, anything spending the shared treasury
  arrives as a proposal the human approves or vetoes.

The full argument lives in [THESIS.md](THESIS.md); the arena ledger in
[docs/ARENA.md](docs/ARENA.md) records the LLM-vs-LLM and human-vs-AI rounds
that drove the design.

## Quick start

```bash
cargo build                 # dev profile only — deps are already opt-level 3

make watch                  # spectate the scripted AIs fighting (windowed)
cargo run                   # play yourself: Human vs the scripted AI
make sim                    # one headless AI-vs-AI match, result on stdout
```

Seat an LLM commander through the file bridge:

```bash
BH_BRIDGE=red cargo run     # red is played through bridge/red/
```

The commander's manual is [tools/COMMANDER_BRIEF.md](tools/COMMANDER_BRIEF.md):
it reads `bridge/red/state.json`, writes JSON command batches, and blocks on
`tools/bridge_wait.py` — an event-driven loop, no polling, no screen parsing.

**Want two LLMs battling each other?** Follow
[docs/LLM_MATCH_QUICKSTART.md](docs/LLM_MATCH_QUICKSTART.md) — one build, one
launch command, two agent prompts, and the result lands in the arena ledger.

## Verify

```bash
tools/verify.sh smoke      # compiles, tests pass, one headless match to game over
tools/verify.sh standard   # + both maps + the four bridge protocol verifiers
tools/verify.sh identity   # two seeded runs, world fingerprints diffed byte-for-byte
```

## Documentation

| Document | What it settles |
| --- | --- |
| [THESIS.md](THESIS.md) | why the project exists: both seats, same language, same knowability |
| [DESIGN.md](DESIGN.md) | the module contract, the frame order, the data-file rules |
| [docs/INTENT.md](docs/INTENT.md) | the intent vocabulary and the fairness invariant |
| [docs/FOG.md](docs/FOG.md) | one rule of knowability, computed once, rendered three times |
| [docs/TEMPO.md](docs/TEMPO.md) | why command latency exists and what it taxes |
| [docs/ARENA.md](docs/ARENA.md) | the dogfooding ledger and how a round is run |
| [tools/COMMANDER_BRIEF.md](tools/COMMANDER_BRIEF.md) | the wire protocol as its users read it |

## License

MIT — see [LICENSE](LICENSE).
