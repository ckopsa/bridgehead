# The Iteration Vocabulary

*How this game gets better. The third vocabulary — the first two are for playing
(docs/INTENT.md) and building (tools/BUILDER_BRIEF.md); this one is for the loop
that produces both. Written at the close of the v2/v3 campaigns, which ran ~45
implementation agents, 12 arena rounds, and ~90 tracker issues through the
process described here.*

## The loop

Everything follows one cycle, and every artifact below is a stage of it:

```
observe → name it → file it → decide → dispatch → verify → merge → validate → feed back
```

1. **Observe.** A spectated match, a commander's AAR, a scripted sim sweep, an
   agent's final report. Findings come from watching the game get played, not
   from reading the code.
2. **Name it.** A finding becomes a *lesson* in the arena ledger
   (`tools/arena.py note rN --lesson`) or a line in an agent's report. A finding
   that isn't written down is a finding that gets made twice.
3. **File it.** Lessons that imply work become beads (`bd create`), each carrying
   its *why* — the observation that created it and the evidence behind it. A
   bead with no why gets "improved" into something else later.
4. **Decide.** Design forks and balance levers are the **owner's** — the
   orchestrator presents options with evidence and a recommendation, and waits.
   (Precedents: tower supply cost rejected; mines 3500→5000 chosen; lumber
   deferred pending evidence, then resolved *by* evidence; heroes made free by
   fiat; the free-second-hero spike closed the day it was noticed.)
   Implementation choices inside a decided design are the **agent's**, reported
   honestly.
5. **Dispatch.** One bead → one agent → one isolated worktree → one branch
   (`bead/<id>`). The agent reads `tools/BUILDER_BRIEF.md` (the standing half of
   its instructions) plus the bead plus a short delta. Parallel agents are
   expected; conflicts are handled by the **re-merge pattern**: master moves,
   the agent merges master into its branch and re-verifies, and the *owning*
   agent resolves semantic conflicts in its own code — the orchestrator only
   union-resolves the trivial.
6. **Verify.** The definition of done is a named tier of `tools/verify.sh`
   (`smoke` / `standard` / `full` / `identity`). `identity` — fingerprint
   byte-comparison across seeded fixed-dt sims — is the cheap proof for every
   "no behavior change" claim, and refactors should reach for it first.
7. **Merge.** The orchestrator owns master, the tracker, and the merge order:
   smallest and least-entangled branches first, big refactors last, one
   verification between each landing. Merge commits are the changelog
   (`git log --first-parent`).
8. **Validate.** Balance and design claims are settled in the **arena**: a round
   is a *hypothesis test* (docs/ARENA.md), run through `tools/arena_run.py`,
   recorded with results **separate from** verdicts, unknowns declared, AARs
   attached, lessons extracted. One round is evidence about a ruleset, never a
   measurement of it.
9. **Feed back.** Lessons annotate existing beads or become new ones, and the
   loop turns.

## The roles

- **Owner** — decides design forks and balance levers, spectates rounds, plays
  the acceptance matches only a human can (the Chain-of-Command rematch is the
  standing example). The owner's watching produces findings nobody else catches
  (the invisible fog shading; the unfair staggered start; the free-second-hero
  spike).
- **Orchestrator** — runs the loop: tracker, dispatch, merge train, ledger
  recording, the questions to the owner. Writes nothing to master except merges
  and the occasional one-line fix, and records every round in the ledger.
- **Implementation agents** — one bead, one worktree, one branch, one final
  report. The report is data: what changed, how verified, what was discovered.
  Discoveries in the report become beads; pain points become BUILDER_BRIEF
  lines.
- **Commander agents** — play arena rounds through the bridge, then write AARs.
  Their complaints are the best balance instrument the project has: the tower
  cliff, the lumber answer, the hero-skip meta, and the fire-hose bug were all
  found by commanders playing, not by anyone reading code.
- **The scripted AI** — the baseline instrument. Cheap, fast, always available;
  its sims give statistical pacing evidence commanders are too expensive for.
  Its blind spots are known: it exercises what its script reaches (T3 content
  needed explicit work to appear in its games — content the baseline never
  touches is untested content).

## The decision rules (each one learned the hard way)

- **Evidence before tuning.** The lumber question was deferred on one round of
  data and resolved by a controlled second round (allocation, not pricing — no
  balance change). The two-tower experiment measured a cliff (one tower: 5–8min
  decisive; two: 10–25min stalemates) that eyeballing would have walked off.
- **One lever per change, rematch to validate.** Mines 3500→5000 was one number,
  validated by r10 before anything else moved. A batching "refactor" that made
  the AI 40% more lethal was rejected *because* it was two changes wearing one
  coat.
- **Results are not verdicts.** Round 6 was won by the side that was losing.
  The ledger separates who won from what was proven, and unknowns must be
  declared by name (`unknown[]`) — silence is never data.
- **Honest reporting is load-bearing.** Agents retract claims their own data
  overturns (the latency-lengthens-matches claim), record regressions as new
  ledger rounds rather than editing old ones (r15/r16), and state what they
  could not observe. The process only works because reports can be trusted.
- **Rules carry their why.** In briefs, in bead descriptions, in code comments —
  a bare rule gets optimized away by the next reader; a rule with its reason
  survives contact with someone smarter than its author.
- **Stopping rules.** Follow-up work is *filed, not chased* — each wave closes,
  and the tail of P3 polish folds into the next slate rather than spawning
  infinite recursion. The campaign ends; the tracker persists.

## The artifacts, and where they live

| Artifact | Where | What it is |
|---|---|---|
| Beads | `bd` (Dolt DB; `.beads/` export) | The work, each with its why |
| Branches | `bead/<id>` | One per bead, merged by the orchestrator, deleted after |
| The ledger | `arena/ledger.jsonl` | Rounds as hypothesis tests; the honesty rule enforced by schema |
| AARs | `arena/rN/*.md` | The commanders' own accounts; evidence links from the ledger |
| Lessons | ledger `lessons[]` → beads | The feedback edge of the loop |
| Briefs | `tools/COMMANDER_BRIEF.md`, `tools/BUILDER_BRIEF.md` | The standing instructions for playing and building |
| Verification | `tools/verify.sh` tiers | The definition of done, executable |
| Fingerprints | `WC3_SEED` + `WC3_FIXED_DT` | Byte-level identity proofs for refactors |
| Design docs | `DESIGN.md`, `docs/*.md`, `THESIS.md` | The contracts and the why of the game itself |

## The vocabulary test

The recurring sign that a vocabulary is *right* — observed independently for
doctrine, triggers, regions, and plans — is that a sentence a person would
naturally say aloud maps onto one object in the language ("hold the northwest
ford", "when my base is attacked, come home", "escort the expansion unless...").
When an owner's sentence does NOT map onto one object, that sentence is the next
vocabulary bead. This document exists because the same test applies to the
process itself.
