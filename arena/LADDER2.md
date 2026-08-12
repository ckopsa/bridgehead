# The Second Ladder — r30–r33

*Four mirror rounds on `crossings`, 2026-08-12, rerunning the r25–r28 tiers on
`affordance-doc/2.1` — the folded page, acceptance notes, and the
`standard-kingdom` playbook, all landed between ladders. Same map, same speed
(1×), same neutral prompts across all tiers (fixing ladder one's Haiku
asymmetry). Written from the ledger, engine and intent logs, observation
samples, and eight commander AARs — including two recovered from stopped
agents' transcripts after the r32 engine wedge.*

## Results

| round | tier | result | duration | ladder-1 baseline |
|---|---|---|---|---|
| r30 | Fable 5 | red wins, surrender | 388s | r25: 340s |
| r31 | Opus 5 | red wins, surrender | 731s | r26: 994s |
| r32 | Sonnet 5 | **aborted** — engine wedge at t=1496; decided in substance ~4:1 red | — | r27: 791s |
| r33 | Haiku 4.5 | red wins, surrender | 460s | r28: 350s / r29: 292s |

r32 is not in the ledger (it records completed rounds); its full evidence —
both stale snapshots, the intent log, runner logs — lives in
`arena/r32-frozen/`, and both AARs were written afterward by resuming the
stopped commander agents from their transcripts. The wedge is filed P1: all
threads futex-parked, survived window re-visibility, windowed-path-specific
(seven prior rounds on the same binary ran clean; five were headless).

**r33 carries a caveat that redefines what it measured:** red's seat ran
`autopilot` from t=189 to t=451 — 57% of the match, spanning the whole
recovery from blue's worker raid and the winning army buildup — and took
control back nine seconds before the surrender. Legal (the verb is documented
"emergency only," and a seven-worker raid qualifies), and disclosed in the
AAR. But the round's verdict measures Haiku's judgment about *when to
delegate*, not Haiku's play.

## Finding 1 — compression worked at the cost level; triage is tier-dependent

Every seat in every round ran `--doc` (the folded page) every cycle — in
ladder one, every tier abandoned the document for the digest. The 8.5× fold
bought the page back its place in the loop. What it could not buy is
attention *inside* the page: Fable read it whole; Opus read the top and left
ACTIONS unread ("the fold fixed the page's cost, not my triage" — r31 red;
r31 blue self-truncated with `head -16` and silently cut the playbook and
half the alarms); Sonnet and Haiku read what fired at them.

## Finding 2 — what fires at the commander works; what waits to be claimed doesn't

The mechanisms that *interrupt* earned decisions at every tier that met them:

- **Acceptance notes:** r30 red's "intel ledger empty" note directly caused
  the scout that found the enemy army; r31's notes were accurate on every
  gate (blue read its own one cycle late — notes land in the next snapshot,
  a real latency gap, filed); r32 red says a note shaped a batch's
  sequencing. And when a note was read and overridden (r31 red's deliberate
  all-in, r32 red's stale pushes), that is the advisory contract working.
- **INVALIDATED playbook renders:** fired correctly in r30, r31, and r32 —
  accurate numbers, exits promoted — and changed a real decision in r31 and
  r32 ("build the Farm before re-arming the pulse"; the WHY sentence "a
  capped hall trains nothing at any price" was, twice, the only prose a
  commander said moved it).
- **Alarms and teaching errors** kept earning at every tier, as in ladder one
  (r32 blue diagnosed a lumber-not-gold shortage off a refusal's
  both-sides-of-the-comparison wording).

The mechanisms that wait to be *claimed* — declaring a playbook or focus in a
prefs file — show a hard adoption gradient: both Fable seats and both Opus
seats declared; Sonnet split (red declared, blue did not); **neither Haiku
seat wrote a prefs file at all**, leaving the playbook a one-line
advertisement all match. The r28↔r33 pair makes it a controlled result: same
model, same document family — r28's prompt said "trust the document" and the
recipes were armed verbatim; r33's neutral prompt ("use these if they help
you") produced zero adoption. **At small tiers, adoption is prompt-driven,
not discovery-driven.**

## Finding 3 — the anchoring fear did not materialize; its opposite did

The playbook was engineered against over-obedience (every step a fork,
authored exits, invalidation as interrupt). Nobody over-obeyed: every seat
that declared it took the opening steps, then diverged freely and stopped
reading the section. The observed failure is **under-adoption and early
abandonment** — r32 red's precise complaint is coverage: the ten steps are
the first four minutes, and "had nothing to say about the 20+ minutes of
mid/late-game stalemate that followed." Meanwhile r32 blue — undeclared,
off-book — lost to *the exact lesson step 5's WHY sentence states* (all four
mines dead, no paced expansion). The strategy content was right; the delivery
contract (opt-in, opening-only) kept it from the seats and minutes that
needed it.

## Finding 4 — the judgment gradient stands, unmoved by two scaffold generations

- **Fable** won on a timed push, used its one note well, and took playbook
  step 1 verbatim because it agreed with it.
- **Opus** played a real 12-minute game; the loser's fatal push was on a
  sighting of "~6" that was really 15, and its mine died with no expansion —
  judgment, with every relevant fact served.
- **Sonnet** reproduced its r27 signature *through* the playbook: both seats
  tier 1 the entire match, red floating **9,255 lumber** (dwarfing r27's
  4,000 gold), the Keep step's entry condition satisfied and ignored, a
  correct stale-intel note read and overridden. The scaffold served
  everything; the tier did what the tier does.
- **Haiku** produced the ladder's most interesting decision by *delegating* —
  which is either an indictment (it couldn't play the midgame) or a
  legitimately wise move (it knew it couldn't), and deciding which is an
  owner call about what ladder rounds permit.

## The unanimous asks (four rounds, one direction)

Economy foresight, again and louder: the runway/depletion forecast decided
r31 outright (mine dry at t=320, 200s of zero income, game over) and is now
asked for in **five** rounds across both ladders; maintain-N
(workers/production) in four; workers-have-no-doctrine-tier (r33 blue's
raid deaths are un-answerable at LLM latency) is new and of a piece; plus
idle-workers-with-nothing-left, squad stall *reasons*, note escalation
deltas ("still stale" vs "newly informative"), and a reacquire-sighting
recipe (r32 red lost ~900s to a scouting detour).

## Recommendations, in order

1. **Make the channels opt-out for small tiers:** `arena_run` (or the
   persona) writes the starter prefs file at seat creation — sanctioned by
   AFFORDANCES.md's own text ("the commander *or its persona prompt*
   declares"), two lines in the runner, and it converts Finding 2's adoption
   cliff into the fork-consumption the machinery was built for.
2. **Author the playbook's missing chapters:** mid-game (tier exploitation,
   stalemate-breaking) and the dry-map endgame — the minutes where r32
   actually died. The opening chapter is proven; the coverage gap is the
   complaint.
3. **The economy-foresight family as one wave:** runway line (gold *and*
   lumber), depletion alarm-before-dry, maintain-N verb, worker doctrine
   tier. Five rounds of unanimous evidence; no design ambiguity left.
4. **Fix the P1 windowed wedge** before further windowed rounds (headless is
   immune; a windowed round also silently pauses when occluded — ARENA.md
   line either way).
5. **Owner call:** is mid-match `autopilot` legitimate ladder play? It is
   legal in-game and arguably the deepest judgment Haiku showed; it also
   means a mirror round stops measuring the model. Decide before ladder
   three, and record it in the round rules.

## The two knobs, after two ladders

Data and affordances moved everything mechanical: the floor held twice, the
page got read, the warnings landed at decision time, and the strategy
content — where it was consumed — changed real decisions. What the knobs did
not move is what remains: **judgment** (the tier gradient, intact through
both scaffold generations) and **adoption** (solvable — recommendation 1).
The honest formulation for ladder three: scaffold delivery is now a solved
problem down to the prompt line; whether a playbooked-by-default small model
closes distance on a bigger one is the next measurable question — and the
executor arm (a script that only ever confirms) is the control that finally
separates the playbook's strength from the model's.

---

## Addendum: the opt-out rematches (r34–r35)

*Same day. Recommendation 1 implemented — `arena_run` writes
`{"playbook": "standard-kingdom"}` into every scaffolded seat's prefs file at
creation (`--playbook none` opts out; a seat's own edits are never clobbered;
`seats[].playbook` lands in the ledger) — and the Sonnet and Haiku mirrors
rerun with the book open by default.*

| round | tier | result | duration | the split inside the mirror |
|---|---|---|---|---|
| r34 | Sonnet 5 | blue wins, **razed** | 387s | book-follower beat book-abandoner |
| r35 | Haiku 4.5 | blue wins, surrender | 473s | **autopilot from t=316** beat book-follower |

**r34 is the strongest result either ladder produced.** The first raze in ten
LLM rounds — every prior game ended in surrender. Blue kept the book open all
match, consumed two accurate INVALIDATED forks, and played with genuine
tempo: push at t=200, lose half the army, regroup and heal (hero-save caught
its hero at 17%), decisive second push. That regroup-then-recommit
discipline is steps 8–9's exact lesson, executed by the tier that floated
4,000 gold in r27 and 9,255 lumber in r32. Red went off-book after a muddled
step 1 and lost the old Sonnet way — lumber-starved past honest gates it
read and never acted on. One mirror, follower versus abandoner, decided by
raze.

**r35 sharpened the autopilot question past deferring.** Red played the
floor tier's best own-hands game on record — the book to step 6, 29 workers,
two halls, every automation correct — and surrendered to a seat that took
the book through step 4, wiped its army at t=290, and handed the faction to
the scripted AI at t=316, never taking it back. Everything red lost to
(tier 2, the Spearman counter, the pressure) was scripted play. Across
r33+r35 that is two of four Haiku seats delegating and both ending on the
winning side: **at the floor tier, delegation dominates playing.** Either
ladder rules ban mid-match autopilot, or the measured skill at small tiers
is officially "knowing when to hand off" — but the ledger cannot keep
calling those rounds Haiku victories without a seat-flag. (Also the fourth
instance of Haiku's self-report diverging from the logs — blue reported a
raze; the ledger says surrender.)

**Verdict on recommendation 1:** adoption solved. Three of four pre-declared
seats engaged with the book (r34 both, r35 red); the fourth consumed its
opening before delegating. The remaining authoring gaps are exactly where
the book goes quiet: a tech-path decision point (Keep vs Workshop), a
scout-before-commit gate on the push step, and the mid/late-game chapters —
all filed. The next measurable question is unchanged from the report body,
now with a working delivery mechanism: the cross-tier round, playbooked
small model versus a larger one, with the confirm-everything executor as
control — and an autopilot ruling in the round rules first.
