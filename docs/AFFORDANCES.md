# Stances, Affordances, and the Small-Commander Problem

*Design distilled from arena rounds r21–r23 (see `arena/ledger.jsonl` and each
round's AARs) and the orchestration discussion of 2026-08-11. Status: design,
not yet implemented. The work breakdown is the Implementation plan section at
the end of this document.*

## The problem

The bridge exposes the full intent vocabulary — ~25 verbs across orders,
squads, postures, doctrine, triggers, plans — and a frontier-class commander
can drive it. A smaller (Haiku-class) commander drowns: r21's boomer boomed
with zero army because nothing ever forced a re-evaluation; its hero-save
trigger was armed with a placeholder `"units":[]` that fired and moved
nothing; its defense was policies with no assets behind them. These are not
intelligence failures so much as decision-*surface* failures: the game asks an
open-ended question every cycle, and open-ended questions are exactly what
small models answer worst.

Two Fable-class commanders played r23 and were then asked, as a first-class
deliverable, how they would prioritize the decision space for a smaller model.
Their independent answers converged, and this document is that convergence
plus the constraints that keep it inside the project's philosophy.

## What the fairness invariant does and does not constrain

THESIS.md: fairness is structural — the AI *cannot* act in ways the human
cannot, because there is no other API. INTENT.md: no commander mutates game
state except through intent submission; both seats reach the same verdicts;
**rendering is where the two seats are allowed to differ.**

The human seat already receives enormous free cognitive scaffolding — minimap,
health bars, alert sounds, ghost previews, `[Space]`-to-warning. A computed
state summary and affordance menu is the bridge seat's equivalent: it gives an
LLM the situational chunking a human gets for free. That is rendering, and
rendering may differ. Three hard constraints keep it that way:

1. **Advisory, never enforcing.** A human's self-imposed "turtle" is a habit,
   revocable mid-frame the moment they smell an opening. If negative
   affordances *reject* out-of-state commands, the winning move nobody
   anticipated becomes inexpressible and the invariant fails in reverse (the
   human can act in ways the AI cannot). The menu is a floor for weak models,
   never a ceiling for strong ones. The full vocabulary stays open.
2. **Fog-legal.** One rule of knowability (FOG.md). Affordance readiness is
   computed only from what that seat's snapshot may know. An affordance that
   says "their base is lightly defended" from omniscient state is an intel
   leak, not a rendering.
3. **Recorded in the ruleset.** Once the scaffold encodes any judgment, an
   arena result measures model+scaffold. That is fine — it is the experiment
   we want — but the scaffold version must appear in the round's `ruleset`
   so ledger comparisons stay honest.

## The design

### Stances: five named states per squad

Both r23 commanders mapped their entire match onto a handful of states with
2–5 live options each (red: pre-ready, build-up, contact, after-a-wipe,
dead-bank endgame; blue: ~8 decision kinds, 2–4 live at any moment). A
*stance* is a named doctrine preset per squad — posture + anchor region +
retreat threshold + leash + focus priority — so the engine's existing doctrine
machinery executes the sub-second half between polls, which r23 showed it
already does well (blue's t=184 rush defense was won almost entirely by
triggers).

Start with a fixed engine-defined vocabulary — roughly *turtle*, *stage*,
*push*, *secure*, *harass* — not commander-defined bundles. Fixed stances keep
the affordance graph small and legible; the arena will tell us what is
missing. **The default is persistence**: no command means "continue current
stance," turning model silence from a bug (r21's 98-second idle Barracks)
into a policy.

### Affordances: all transitions rendered, two annotation channels

From the current stance, every transition is always listed (advisory — see
constraint 1), annotated on two strictly separated channels:

- **Readiness** — engine-computed, factual, fog-legal. Each option carries its
  precondition and current truth with a reason: "stage attack: NOT READY —
  squad 1 has 4/6 units, hero at 40%." This is the channel that catches the
  empty-squad trigger at arm time instead of fire time. Two annotations the
  r23 commanders each named as the one that would have saved them:
  - *Intel staleness* (red): "visible: 4 workers; last-seen enemy army: 17
    troops, 190s ago, not since." Red's game ended on reading current sight
    as ground truth at t=490.
  - *Push gates* (blue): one consolidated squad, size ≥ N, heroes ≥ 80% —
    the exact three conditions blue violated in its failed t=697 trickle-push
    and satisfied in the winning t=787 one.
- **Preference** — commander-declared, engine-sorted. The commander (or its
  persona prompt) declares doctrine once ("aggression: high, risk: low");
  the view orders the menu under it and flags consistency. The engine never
  generates the preference — it renders the commander's own values back.
  The engine computes readiness; it does not have opinions.

### Chains: stance plans with late-bound references

"Turtle until the hero is healed, then secure an expansion" is a plan whose
steps are stance transitions with wait-conditions — compiled through the one
compiler into stance-sets plus armed transitions, extending the existing
trigger/plan choke points, never bypassing them. Validation stays
teaching-only: arming a chain step whose target is unscouted does not refuse;
it arms and reports "chain holds at step 1: target unresolvable until
scouted."

This requires **late-bound references**, the single highest-value change in
this document. Both r23 commanders independently condemned frozen entity IDs:
red spent half its command-authoring effort on ID plumbing and traced four
error classes to it (dead hero IDs in triggers, stale unit lists in
`priority`, a memorized tree chopped out from under a harvest order, the
wrong worker frozen into a repeating trigger); blue's fixed-coordinate farm
trigger looped on "site blocked" all game. Triggers, plans, and orders should
accept role/region selectors — "my hero," "all army," "squad 1's current
members," "nearest tree," "nearest legal site to (x,z)" — resolved at *fire*
time by `resolve_places`/the compiler. r21's `"units":[]` corpse becomes
inexpressible, and unit-ID rosters can leave the small commander's snapshot
entirely.

### Alarms: forced re-decisions, defaulting to continue

Blue's sharpest line: *"alarms, not vocabulary, are what a small commander is
missing"* — red's loss was "one long wrong continue" through an income
collapse nothing flagged. Stance persistence is safe while no enemy is in
sight and deadly the moment contact information is stale or the stance's
anchor no longer covers what matters (red kept "defend forward" at t≈690
while the buildings being shot were elsewhere). So the harness forces a fresh
choice on a tiny set of events and defaults to continue on everything else:

- enemy army sighted ≥ N (red's proposal)
- own squad below half strength (red)
- income collapse: all mines dry / worker starvation (blue — no such alarm
  existed in r23)
- multiple places under attack at once, with recall ETA attached (blue)

**An alarm is never the first responder.** A commander answers at LLM latency
— tens of seconds — so anything with a shorter deadline cannot live at the
commander layer (red's hero died mid-retreat *while the retreat order was in
flight*; TEMPO.md makes this latency deliberate). Every decision belongs to
the fastest tier that can hold it:

1. **Reflexes** (doctrine/triggers, sim-tick speed): retreat thresholds,
   home-guard, autocast, leash. r23 proved they carry real weight — blue's
   t=184 rush defense was fought and won by triggers between polls.
2. **Pre-armed policies** (decided at leisure, executed instantly): stance
   chains are answers to future alarms given when there is no time pressure —
   "if my push meets ≥12 defenders, break contact to the staging anchor"
   moves commit/withdraw from fire time to arm time.
3. **Alarms** (LLM latency): only the residue — decisions that are
   policy-shaped and stay valid over a 30-second window. "Full recall or
   sacrifice the expansion?" is still the right question a minute later;
   "dodge the ambush" never was.

Two rules follow. **Every alarm names its running default**: the menu line is
not "base under attack — respond!" but "base under attack — home-guard is
recalling squad 1 (ETA 22s). Confirm, or override: sacrifice expansion / full
retreat." A slow or silent commander gets the reflex's outcome, which is safe
by construction — the default is not *freeze*, it is *the stance's reflex* —
and a weak model that only ever confirms defaults is playing acceptably. And
**an alarm fires only after the reflex has**: its payoff is attention, not
speed. Income collapse and multi-front pressure are slow-burn conditions
where r23's failure was that no cycle ever surfaced the fact, not that an
order arrived 20 seconds late. Alarms fix the attention failure; reflexes fix
the latency one. The one tight case, commit/withdraw at first contact
(~10–30s window), gets a reflex half too: an engagement-break rule in the
stance, so the commander's possibly-late answer chooses between resuming and
staying withdrawn, with the safe branch already taken. The sim never waits on
the wire — the hold-at-t=0 ready handshake stays the only place the game
waits for a commander.

An alarm re-renders the affordance menu with the triggering fact on top. It
never acts.

### Snapshot diet: the ~15-line commander digest

Blue steered its entire endgame from roughly fifteen lines: resources,
army-by-squad, production queues, enemy production buildings remaining (the
win-condition line), last five events, active alarms. Red's drop list agrees:
per-unit ID rosters (obsolete once selectors exist), tree IDs, per-farm HP,
the full plans echo. This is a *view* (`bridge_view.py` or a `--digest`
mode), not a change to `state.json` — the full snapshot remains for any
commander that wants it, and the wire format stays append-only.

### Composition counter-hints (static table, maybe)

Red rebuilt the same melee comp twice into an archer/heal ball and proposed
the menu annotate rebuild options with a static counter table ("enemy last
force: 10 Archer + heal → melee spam trades down"). This flirts with
constraint 3's line between fact and judgment — a counter table is strategy
authored into the scaffold. If adopted, it is data (`assets/data/`), clearly
versioned in the ruleset. Lowest priority; the arena can decide whether it is
needed after the rest lands.

## Implementation plan

Ordered by value and dependency; each item is independently mergeable and
verifiable (`tools/verify.sh identity` for any "no behaviour change" claim).

1. **Late-bound selectors** *(first — both r23 commanders' top item; unblocks
   3 and 6)*. Triggers, plans, and orders accept role/region selectors — "my
   hero", "all army", "squad N's current members", "nearest tree", "nearest
   legal site to (x,z)" — resolved at fire time via `resolve_places`/the
   compiler, never frozen at arm time. Kills r21's `"units":[]` corpse, red's
   four ID error classes, and blue's farm site-blocked loop (auto-accept the
   engine's nearest-legal suggestion). Extend the one resolver and one
   compiler; wire keys additive only.
2. **Stances** *(unblocks 3 and 6)*. Five fixed doctrine presets per squad —
   turtle / stage / push / secure / harass — each a named bundle of posture +
   anchor + retreat + leash + priority, executed by existing doctrine between
   polls. Default is persistence: no command means continue. All mutation via
   `Intent` → `apply_intents`.
3. **Affordance menu** *(needs 2; benefits from 1)*. From the current stance,
   render ALL transitions — advisory, never blocking — with the two channels
   above: fog-legal readiness (precondition truth + reason, intel staleness,
   push gates) and commander-declared preference ordering. View layer:
   `bridge_view.py` / the digest.
4. **Alarms** *(independent; pairs with 2's reflexes)*. The four events above,
   each naming its running default, re-rendering the menu, never acting.
5. **Commander digest** *(independent)*. The ~15-line view: resources,
   army-by-squad, queues, enemy production buildings remaining, last 5
   events, active alarms. View-only; `state.json` unchanged; wire stays
   append-only.
6. **Stance chains** *(needs 1 and 2)*. `stance_plan`: steps are stance
   transitions with wait-conditions, compiled through the one compiler into
   stance-sets plus armed transitions; teaching-only validation ("chain holds
   at step 1 until scouted").
7. **Haiku A/B validation** *(needs 3 and 5)*. The experiment in the next
   section.

Engine and tooling defects surfaced by the same rounds, fixable independently
of this design: `bridge_send.py` overwrites `commands.json`, so a batch sent
immediately before `ready` is silently clobbered (both r22 seats lost their
openings to it); `bridge_wait.py` assumes a Linux-style marker directory and
re-announces stale events on macOS until it exists; the engine can exit on
game over before the final snapshot write, leaving `game_over` null in
`state.json` (r23 — the runner reads `engine.log` instead, but a commander
polling the snapshot would hang); blue-r23's `mine_dry` expand trigger never
fired; and `posture: push` stalled ~400s at the crossings fords in r22
(squad-cohesion pathing).

## Validation

The whole design exists to be falsified cheaply: run Haiku vs Haiku with the
digest+affordance view and the same matchup without it, same map and
personas, and diff the failure modes against r21's (zero standing army, empty
triggers, no re-evaluation, ID errors). The ruleset field records which
scaffold each round used. If the scaffolded Haiku stops dying to its own
empty policies, the design earned its place; if not, the ledger says so.

## Evidence index

- r21: boomer with zero army razed at t=219; `"units":[]` hero-save fired as
  "move 0 units," rejected, hero died 3s later. (`arena/r21/`)
- r22: boomer that read r21's AAR queued Footmen from t=52, crushed the same
  rush, won at t=1148. Rusher's `posture:push` stalled ~400s at the fords.
  Both seats lost their opening batch to `bridge_send.py` overwrite-on-send.
  (`arena/r22/`)
- r23 (Fable vs Fable): informed rusher surrendered at t=845 after a
  fog-hidden 17-unit army erased its best-timed strike; both commanders'
  Part 2 design notes are the primary sources for this document.
  (`arena/r23/aar-red.md`, `arena/r23/aar-blue.md`)
