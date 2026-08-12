# The Model Ladder — r25–r29

*Five LLM-vs-LLM rounds on `crossings`, 2026-08-12: mirror matches at four
capability tiers, every seat commanding through the file bridge, plus an
unscaffolded control. The experiment `docs/AFFORDANCES.md` was built for:
does a rendered decision surface — stances, alarms, the hypermedia document —
turn r21's small-commander collapse into playable games? Written by the
orchestrator from the ledger, the engine logs, the intent logs, the
per-2-minute observation samples, and all ten commander AARs.*

## Results

| round | tier | scaffold | result | duration | doc | commit |
|---|---|---|---|---|---|---|
| r25 | Fable 5 | doc 1.2 | red wins, surrender | 340s | timing attack at mid | `946d2d2` |
| r26 | Opus 5 | doc 1.3 | blue wins, surrender | 994s | mine-denial campaign | `946d2d2` |
| r27 | Sonnet 5 | doc 1.3 | blue wins, surrender | 791s | passive macro mirror | `62a7176` |
| r28 | Haiku 4.5 | doc 1.3 | blue wins, surrender | 350s | 4-unit raid decides it | `62a7176` |
| r29 | Haiku 4.5 | **none** | blue wins, surrender | 292s | Barracks snipe + push | `62a7176` |

All five decisive, zero cap-outs, zero crashes, zero hung seats. Ledger:
`arena/ledger.jsonl` r25–r29, each row carrying scaffold version, tuning
digests, and commit per constraint 3.

## Finding 1 — the floor held: r21's collapse modes are extinct

Across ten commander seats, none of the four r21 failure classes appeared:

- **Zero standing army:** every seat fielded one. The closest call (r27 blue's
  first combat unit at t=165) was greed, not blindness — and the digest said
  `ARMY 0` every cycle of it.
- **Empty policies:** none. Selectors made `"units":[]` inexpressible. The
  sharpest proof is r28's hero-save: it fired at t=308 and *moved the real
  hero* (revived-id-proof via `"my hero"`); the hero died at t=311 to focus
  fire mid-retreat — TEMPO.md's burst-through case, physics rather than
  plumbing. In r21 the same trigger moved zero units.
- **One long wrong continue:** alarms broke every sustained wrong-continue
  observed. r28 red answered two income collapses; r27 blue's
  `enemy_army_sighted` running-default exposed one of its *own triggers*
  silently overriding its push — the attention layer catching a self-inflicted
  policy conflict.
- **ID plumbing errors:** zero, in ~200 commands across eight scaffolded seats.

## Finding 2 — the usage inversion

The document was designed as a floor for weak models and a ceiling for none.
The tiers used it exactly that way, in a direction we did not predict:

| tier | full doc reads | links verbatim | forms as served | loop page |
|---|---|---|---|---|
| Fable | 1 (t=0) | 0 | recipes at open | digest |
| Opus | 1–2 | 0 | 0 | digest |
| Sonnet | 1–2 | 0 | ~6 conceptually | digest |
| Haiku (prompted) | 1 + refs | several | recipes verbatim | digest |

Every tier converged on: **document as orientation page, digest as loop
page.** Blue-r26 said it best: *"a good orientation page and a poor loop page
— at 15-second cadence the digest's eight lines carry the decision and the
doc's 15 actions do not."* All four scaffolded rounds' commanders disobeyed
their own spawn instruction to re-read the document each cycle; when both
seats of three different models pay the instruction-violation cost to avoid a
render, the render is uneconomical at loop cadence, period. Only Haiku —
prompted to trust the document — used links and forms as served, and called
the recipes "transformative" (six triggers armed in the opening batch, zero
transcription errors).

**Caveat:** the Haiku prompts were deliberately strengthened ("trust the
document; take READY links when unsure"), so tier-4 usage is partly
prompt-driven. Defensible — a production deployment of a small model would be
prompted exactly this way — but r28 tests document-as-lifeline, not
document-discovery.

## Finding 3 — the A/B verdict: the vocabulary holds the floor; the document is the authoring aid

r28 (scaffolded) vs r29 (unscaffolded), same models, same engine, same map:
**both were real games.** The unscaffolded seats did not collapse — because
"unscaffolded" could only remove the *document*. The engine-side vocabulary
the epic built — selectors, stances, triggers with fog-legal predicates,
alarms in the snapshot, errors that teach — cannot be turned off, and r29's
seats used all of it freely (r29 blue: *"triggers + stances eliminated 80% of
the micromanagement"*; 6 `trigger_set`, 4 `stance`, selector phrases
throughout its intent log).

The document's isolated contribution at the Haiku tier was **pregame
completeness and error avoidance**, not survival: r28's recipes arrived
correct on the first send; r29's hand-rolled equivalents hit exactly the
frictions the forms exist to remove — a supply trigger that couldn't name a
legal site, enemy targeting by guessed entity ids that failed, one
near-fatal income collapse handled late. r29 also ran 17% shorter and its
loser died of tactical errors, not protocol ones.

So the honest decomposition: **the floor is the vocabulary; the document is
leverage on top of it** — concentrated at t=0 (the one moment with no time
pressure, where every tier consumed it) and at decision moments (where nobody
currently reads it — see Finding 5).

Against the true baseline the comparison is unambiguous: r21's Haiku-class
seat boomed with zero army, armed an empty hero-save, and died at t=219
having never re-evaluated. r28 and r29 both played 300-second games with
armies, working policies, answered alarms, and surrenders on sound reads.

## Finding 4 — the quality gradient is judgment, and that is the thesis working

What separated the tiers was not mechanics — the scaffold delegated those —
but judgment:

- **Fable** won with a timed 12-unit strike and spent its counter-punch
  window correctly; its silences were deliberate ("always after reading the
  DEFAULT line").
- **Opus** produced genuine strategy: a 994-second mine-denial campaign that
  never attacked the enemy base — it killed 11 workers across three income
  rebuilds and won by economic strangulation.
- **Sonnet** held the floor but played shallowly (owner-corroborated live):
  a naked Keep at t=27 with zero army, tier 2 never exploited (no Knights,
  no Blacksmith), a 4,000-gold float, mono-composition Footmen, and two
  stance changes + one posture + one cast in 35 cycles of otherwise pure
  economy chores. It won because its mirror was equally passive.
- **Haiku** played small but real; both r28 seats' fatal errors were resource
  judgment (an overrunning worker pulse; a supply crisis), not protocol
  failures.

The engine refused to have opinions, so the opinion gap is exactly what the
ladder measured. `gold 4002` was in Sonnet's digest 35 times; no rendering
change makes a commander act on it. This is THESIS.md's claim — victory
decided by judgment, not interface bandwidth — observed end to end.

## Finding 5 — the one structural gap: annotations unread at decision time

The document's readiness and staleness annotations directly addressed the
mid-tier losing moves — r26 red committed 13 units into 12 defenders on an
empty intel ledger; the push link's own text warns *"a body of troops you
have not seen is not a body of troops that is not there."* But those
annotations live in the action render nobody re-opens mid-loop, so they were
served every cycle and read never. The two filed fixes attack this from both
ends (see the tuning queue): **acceptance notes** (a command contradicting a
served readiness fact gets one advisory line in its echo — unavoidable, at
the decision moment, never blocking) and **document compression +
commander-declared focus** (the owner's phase proposal made fair: collapse
NOT-READY actions to annotated one-liners; let the commander declare a focus
that expands one section — declared, never inferred, with alarms breaking
through any focus).

## What the ladder fixed while it ran

Each caught by a round, fixed in the gap before the next, commit split
recorded in the ledger:

- **r25 → 1.3:** the steady-production recipe didn't compile (a `when` shape
  no predicate has). A printed template must compile; every served `when` is
  now schema-validated against the catalog the document itself publishes.
- **owner observation during r25:** `nearest legal site` could wall the
  hall↔mine haul. The picker now ranks corridor intrusion ahead of distance
  (identity-clean — scripted AI has its own scanner).
- **r26:** batched builds silently re-tasked the same lowest-id worker (blue
  lost three expansions and a Blacksmith to it). Builds now pick distinct,
  idle-preferred workers; every abandonment window emits an event with the
  true economics — which exposed that **nothing was ever spent** (builds are
  paid at ground-break; both AARs' "lost spend" was a false belief), and that
  the "phantom TownHall(0%)" was a real, nearly-finished hall rendered from
  the wrong progress field.

## Tuning queue (all filed as beads, evidence attached)

| priority | bead | what |
|---|---|---|
| P2 | acceptance notes | readiness contradictions speak in the command echo |
| P2 | compression + declared focus | the phase idea, made fair; doc 2.0 candidate |
| P2 | build price reservation | **owner decision**: free-to-abandon orders vs reserve/refund |
| P3 | gold runway | income rate + depletion + "bank buys K" — asked by two tiers |
| P3 | pricing pair | digest afford-annotation + `--prices` card |
| P3 | squad line | anchor vs actual location, un-conflated |
| P3 | teaching gaps | gates-already-true warning, stranded workers, error dedup, why-not-moving |
| P3 | ai.rs pick_site/pick_builder | same blind spots as the fixed compiler paths; balance-affecting, wants a match matrix |
| P3 | legibility | hero death names the in-flight retreat; Haiku misdiagnosed twice from the same gap |

Closed by the arena: **counter-hints (0uu.8)** — zero asks across ten seats;
the wish-lists wanted economy legibility and decision-time warnings instead.

## Methodology notes

Mirrors measure floor and style, not absolute strength; n=1 per tier;
every result was a surrender, not a raze. Engine commit split r25–26 /
r27–29 (placement fixes) and document 1.2 (r25 only) / 1.3 — both recorded
per round. BH_SPEED=1 throughout (owner's call: normal operating
constraints — a poll cycle covers the same game-time fraction a human's
attention would). Haiku self-reports were unreliable twice (a fired trigger
reported as unfired; claims unverifiable against logs) — ground truth for
this report is the logs, snapshots, and observation samples, with AARs as
testimony. r27's per-command intent log was lost to seat cleanup (fixed:
preserved per round from r28).

## Recommended next steps

1. **Land the two P2 document changes together as `affordance-doc/2.0`**
   (acceptance notes + compression/declared-focus), then rerun the Haiku
   mirror — the cheapest test of whether decision-time delivery closes the
   annotations-unread gap.
2. **Owner decisions:** build price reservation; whether the scripted AI
   inherits the corridor/builder rules (one lever + rematch each).
3. **A cross-tier round** (Haiku vs Sonnet, both scaffolded) — mirrors
   measured the floor; the asymmetric round measures how much scaffold +
   judgment gap converts into win rate.
4. The economy-legibility P3 family (runway, prices, squad line) as one
   small wave — every tier's wish-list pointed the same direction.
