# wc3clone — the short commands. Everything here delegates to the real tools;
# see CLAUDE.md (Build & Test) and tools/verify.sh for the full story.
#
#   make watch              spectate the scripted AIs fighting (windowed)
#   make watch MAP=open     ...on the open map (default: crossings)
#   make watch SPEED=4      ...at 4x game speed (F1-F4 also work live)
#   make sim                one headless AI-vs-AI match, result on stdout
#   make verify             tools/verify.sh standard (TIER=smoke|standard|full|identity)
#
# `watch` is the scripted baseline only — no bridge seats, no LLM commanders.
# For commander rounds use tools/arena_run.py (docs/ARENA.md).

MAP   ?= crossings
SPEED ?= 1
SEED  ?=
TIER  ?= standard

# Windowed spectator needs no time cap (close the window to stop); headless
# sims get the automation safety cap per the project rule.
WATCH_ENV = WC3_AI_BOTH=1 WC3_MAP=$(MAP) WC3_SPEED=$(SPEED) $(if $(SEED),WC3_SEED=$(SEED))
SIM_ENV   = WC3_HEADLESS=1 $(WATCH_ENV) WC3_MAX_GAME_SECS=2400

BIN = target/debug/wc3clone

.PHONY: watch sim build test verify

build:
	cargo build

# cargo build first, run the binary by path — `cargo test` does not rebuild
# the bin, and a stale binary is the oldest trap in the builder's brief.
watch: build
	$(WATCH_ENV) ./$(BIN)

sim: build
	$(SIM_ENV) ./$(BIN)

test:
	cargo test

verify:
	tools/verify.sh $(TIER)
