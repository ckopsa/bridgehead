# TEMPO — choosing v2's tempo-equity mechanism

*Design spike for `wc3clone-82i`. Decided before the v2 sim loop is built, because
every option below changes what the sim loop **is**.*

The problem in one sentence: the first human-vs-Claude match was won by the human
partly through reaction speed — hero micro at the point of contact, and timing attacks
aimed at the gaps in a 13-second decision cadence. THESIS.md says victory must be
decided by judgment. So something has to change.

---

## 1. Decision criteria

Derived from THESIS.md, in priority order. These are not equally weighted; 1 and 2 are
the thesis, 3–6 are the constraints it operates under.

**C1 — Equitable decision surface.** Not identical capabilities: *the same decision
surface, the same vocabulary of intent, the same information rights.* The thesis states
the structural form of fairness as "the AI *cannot* act in ways the human cannot,
because there is no other API." The converse is equally binding and is the part we have
been failing (see §2.0).

**C2 — Judgment decides; never reaction speed, never interface bandwidth.** Both are
named. A mechanism that removes reaction speed but converts it into bandwidth advantage
has not satisfied C2 — it has laundered the violation.

**C3 — Incentives, not rules; no artificial clocks.** The thesis rejected time limits
by name and replaced them with world facts: upkeep taxes idle armies, mines run dry,
bounties escalate without cap. Note the shared property of every accepted incentive:
**it is diegetic.** It is a fact about the world that both players read in the snapshot
and plan around. None of them are facts about the referee. This is the sharpest
discriminator in the whole document.

**C4 — The engine does what is fast; the player does what is wise.** Stated with the
addendum "*where the line sits **is** the game design.*" This is not a constraint, it is
a design method — and it is the thesis's own answer to the tempo asymmetry, given
explicitly: "Our answer was **not to slow the human or rush the AI**, but to relocate
fast work into the game itself."

That sentence deserves flagging now rather than in the analysis, because it is the
thesis pre-judging our option list. Option A slows the human. Option B rations the
human. The thesis already declined both and named relocation as the method. That is not
dispositive on its own — the thesis was written before the human-vs-AI duel exposed the
gap, and evidence is allowed to overturn doctrine — but any option that slows or rations
the human is arguing *against* the thesis and carries the burden of proof.

**C5 — Preserve strategies; relocate them.** Cavalry rushes were gated, not deleted.
Applied here: hero micro, militia timing, and TownPortal saves are good content. The
mechanism must move them to a layer both players can reach, not delete them.

**C6 — Buildable, and legible enough to be written about.** Content that never appears
in a winning player's AAR is a bug (principle 4), and the LLM commanders are the
playtest lab (principle 5). A mechanism the commanders cannot *articulate* cannot be
balanced by the process that has balanced everything else. And it has to be built on
14k lines of Bevy 0.16 that were not written with it in mind.

### The WeGo tension, stated up front

C3 says no artificial clocks. A WeGo turn window is a clock. The honest defence is that
turn *structure* is not a *time limit*: a chess clock decides games, a turn boundary
merely quantises them, and the thesis rejected the former. That defence is real and I
accept it as far as it goes. It is answered in §2.1 — not on the grounds that windows
are clocks, but on the grounds that a fixed window is a **deliberation budget imposed on
both players at the length of the shorter one**, which is a new artificial pressure that
does the C2 violation in a fresh costume.

---

## 2. The options

### 2.0 First, the finding that reframes everything

Before comparing mechanisms: **the tempo equalizer the thesis claims to have built does
not run for human players.** Two independent gates, both verified in source:

1. `doctrine.rs:340` `default_squad_autonomy` and `doctrine.rs:384` `run_squad_postures`
   both early-`continue` on `!machine_driven(&ai, &external, team)`
   (`shared.rs:1408`). Squad postures — Defend/Push/Escort/Forage, the cohesive advance,
   the reactive threat answer — **execute only for machine-driven teams.** A human's
   squad posture would be stored and ignored. The comment says so plainly: "a human with
   a mouse keeps full authority."
2. The human cannot set one anyway. The bridge exposes 8 doctrine commands
   (`priority`, `retreat`, `leash`, `autocast`, `squad`, `posture`, `template`, plus
   parameterised forms). `ui.rs` exposes 4, all coarse toggles: `ToggleGuard`,
   `ToggleFallback`, `CyclePriority`, `ToggleAutoCast` (`ui.rs:272-278`). **No squads,
   no postures, no templates**, and no way to set a retreat threshold or a rally point —
   the human gets an on/off switch where the LLM gets a parameter.

So THESIS.md's claim — the doctrine layer "runs at machine speed for **whichever**
player set them" — is currently aspirational for one of the two seats. This is a direct
C1 violation in the reverse direction: *the human cannot act in ways the AI can.*

This matters for the decision in three ways. It means (a) we have never actually tested
the thesis's own answer to the tempo problem, so the v1 duel is not evidence that
relocation failed — it is evidence that relocation was never switched on for the human;
(b) whichever option wins, closing this gap is mandatory, so it is not a differentiator
in the options' favour or against; and (c) it makes Option C's premise substantially
stronger than it looks on paper, because Option C's phase 0 *is* this fix.

### 2.1 Option A — WeGo (orders collect in shared 8–10s windows, resolve simultaneously)

**The strongest case for it.** It is the only option that makes reaction speed
*structurally* irrelevant rather than *incentivally* discouraged — there is no residual
edge to argue about, no calibration that could be wrong by 30%, nothing to playtest into
existence. It matches the LLM's natural 10–15s cadence exactly. It makes the game
discretely legible: turns are numbered, loggable, replayable, and AARs would gain a
vocabulary ("on turn 14 I committed") that is strictly better for the playtest-lab
principle. And it is a proven design — Combat Mission, Laser Squad Nemesis, and
especially Frozen Synapse, which is simultaneous-turn tactical combat and is *good*.
Frozen Synapse also answers the "it kills the drama" objection better than I expected:
its drama is anticipation rather than reaction, and anticipation is pure judgment.

**Against C2 — and this is what actually sinks it.** WeGo removes reaction speed by
converting it into per-window bandwidth. In a 10-second window a human with drag-select,
control groups, and the wall-chain placement loop can compose thirty precise, coordinated
orders. An LLM commander can emit perhaps eight JSON commands and must spend part of the
window reading a snapshot. Reaction speed is gone; **interface bandwidth is
concentrated** — it now decides the whole turn instead of being smeared across it. C2
names bandwidth as forbidden alongside reaction speed. The fix for that is a per-window
command cap, which is Option B nested inside Option A, inheriting all of B's problems
(§2.2) on top of A's.

The alternative — pause until both players declare ready — removes the bandwidth race
but deletes the real-time drama the thesis explicitly wants to keep, and hands the LLM
unbounded deliberation, which is a different inequity pointed the other way.

**Against C3.** A fixed window is a deliberation budget set at the length of the shorter
player. An LLM that needs 18 seconds on a genuinely hard turn is penalised; a human who
decides in 2 seconds waits 8. Neither is a fact about the world. And the necessary
timeout policy for a hung commander is a second clock stacked on the first.

**Against C5.** Hero micro is deleted, not relocated. Militia calls (CallToArms, 90s
cooldown, 40s duration, `shared.rs:830-843`) become a coin flip on window phase.
TownPortal saves die outright: the entire content of a TownPortal is "I saw the trap
closing and got out," which is a sub-window observation. You can pre-commit the portal
and waste it, or not and die. Three pieces of good content, deleted.

**Against C6 — engine reality, and it is brutal.** Verified:

- **There is not a single `SystemSet` in the codebase.** `configure_sets` and `in_set(`
  return zero hits. Cross-plugin ordering across all nine plugins is *unspecified*
  (Bevy ambiguity ordering). True simultaneous resolution requires deterministic
  ordering, and there is none to build on.
- **`Economies::pay` (`shared.rs:1274`) is first-come-first-served at the moment a
  system runs.** Two same-turn purchases race on system order, which is currently
  undefined. WeGo makes this a fairness bug rather than a curiosity.
- Continuous state is everywhere and much of it is keyed to *absolute game-time
  deadlines*: `UnderConstruction.remaining` (`shared.rs:611`), `TrainingQueue.progress`
  (`shared.rs:621`), `Hero.ability_cooldown`/`mana`, `AbilityCooldown`,
  `Militia.until`, `LastDamaged.at`, `Bounty.expires_at`, `BountySchedule.next_at`,
  `HarvestJob.timer`, per-unit `CombatState.cooldown`, and projectiles genuinely in
  flight (`combat.rs:90`). Every one needs a turn-boundary policy.
- There is **no pause primitive anywhere** — no `Time<Virtual>::pause()`, no `States`,
  no `OnEnter`. Only `set_relative_speed` (`shared.rs:1659`).
- The bridge has **no barrier that waits for both seats**. Seats are polled
  independently in a `for seat in &mut bridge.seats` loop (`bridge.rs:1411`); red can
  commit on a frame where blue has written nothing. A two-seat handshake is new
  protocol.

There is a cheap counterfeit — freeze *input* for 10s while the sim runs free ("WeGo
lite") — which costs almost nothing and needs no resolver. But it is not simultaneous
resolution; it is real-time with a rate limit expressed as a clock, so it earns A's C3
cost while delivering B's benefits and neither's guarantees. Real WeGo is a v3-scale
rewrite of the simulation core. That is a legitimate thing to build; it is not a
mechanism you bolt onto v1.

**Verdict.** A is the correct answer to a different question. If the goal were a
turn-based simultaneous tactics game, this is the design. As a tempo fix for this
codebase it costs a rewrite, trades away three pieces of named good content, and does
not actually satisfy C2 without absorbing Option B.

### 2.2 Option B — real-time with per-player command budgets

**The strongest case for it.** It preserves real-time completely, costs almost nothing
to build (one `Resource`, one gate in `ui.rs`, one in `bridge.rs::apply_batch`, zero
simulation changes), and targets the actual quantity in dispute rather than a proxy.
"Micro is a spent resource" is genuinely interesting design: it creates a real tradeoff
between microing the hero fight and setting up the expansion, which is a judgment call.
And a *soft* variant — over-budget orders take effect late rather than being refused —
is meaningfully incentive-shaped rather than rule-shaped.

**Against C2 — and this is the fatal one.** A budget caps *volume*. It does not touch
*timing precision*. With forty orders a minute a human can still spend three of them at
exactly the right 200 milliseconds to blink the hero out of a slam and re-focus the
catapults. The LLM cannot do this at any budget, at any volume, ever. Rationing makes
each action scarcer and therefore **more decisive** — it concentrates the residual
reaction-speed advantage instead of dissolving it. Option B optimises the wrong
variable, and plausibly makes the measured problem worse.

**Against C3.** This is the clearest case in the document. Every accepted incentive in
the thesis is a fact about the world: armies eat, mines empty, treasure grows. "You may
issue forty orders per minute" is a fact about the referee. It does not appear in the
fiction, it cannot be reasoned about in-world, and a regenerating pool is a metronome
strapped to each player — a clock by another name. B is a rule wearing an incentive
costume.

**Against C6 — the accounting problem has no honest answer.** Does a drag-select of
twelve units cost 1 or 12? If 1, the human gets 12× leverage per budget point and the
budget does nothing. If 12, drag-select is dead — and with it control groups, formation
moves, and the wall-chain placement loop (`ui.rs:2201-2223`), i.e. most of what `ui.rs`
is. Meanwhile `bridge.rs` fans a single `move` command over N units through `own_units`
(`bridge.rs:2198`), so the two interfaces disagree on what "one command" means at the
protocol level. Calibration is worse: set the budget at LLM cadence (~8 commands/15s)
and human play becomes miserable; set it where human play is pleasant and it never
binds. There is no number that is both fair and playable, and the reason is that the
two interfaces have different natural granularity — which is the C1 problem, unsolved,
now with a cap on top.

Secondary: it would emit a steady stream of `cmd i: budget exhausted` into the bridge
error channel, spending LLM context on bookkeeping.

**Verdict.** Cheapest to build, and it fails the criterion it exists to satisfy.

### 2.3 Option C — Chain of Command (DECIDED — full statement in §3)

Real-time, no windows, no budgets, no clock. Two parts:

1. **Doctrine parity.** The human gets the complete doctrine vocabulary the bridge has —
   squads, postures, templates, parameterised retreat/leash/priority — and the executor
   runs for whoever set the policy, not for whoever is a machine. This is §2.0's bug
   fixed, and it is mandatory under any option.
2. **Orders propagate; standing orders are local.** A *direct* order to a unit does not
   take effect instantly. It arrives after a latency that scales with the unit's
   distance from the nearest **command node** — your TownHalls, your hero, and (later) a
   forward Outpost. Doctrine executed by the engine has **zero** latency, because the
   unit already has its standing orders and does not need to ask.

Assessment against the criteria, including where it is weak:

**C1/C2.** A human's 200ms edge is swallowed by a 1.5–3s propagation delay at the point
of contact — which is precisely where the v1 duel was decided. Both players issue orders
into the same latency and must therefore *anticipate*, which is judgment. It does not
eliminate reaction advantage structurally: near your own hero, a human still out-reacts
an LLM. It bounds that advantage to a zone whose position is itself a strategic choice
both players make with the same vocabulary. This is C's honest weakness and Option A's
best counterargument; it is answered in §3.

**C3.** No clock, no cap, nothing forbidden. You may micro across the map — it just
arrives late, and lateness is a consequence of a world fact (how far your units are from
your command) that both players read in the snapshot and plan around. It sits in the
same family as upkeep and mine exhaustion: diegetic, readable, plannable.

**C4.** It moves the line exactly where the thesis says to move it — fast work goes to
the engine (doctrine), and the player's hands stop being able to reach past it. It makes
doctrine *strictly better than micro at range*, so pre-positioned policy beats live
intervention, for both players, by construction rather than by rule.

**C5.** Nothing is deleted. Hero micro survives and gets *better*: the hero is a command
node, so where you put your hero is where your hands work — one high-value judgment call
both players express identically. CallToArms is a TownHall cast at your base: node,
instant. TownPortal is an item on the hero: node, instant. All three named content items
survive **by construction**, because all three already happen at command nodes. Micro is
relocated (to node positioning), not removed.

**C6.** It builds v2's stated feature rather than bolting on a referee — THESIS.md
already names "every unit able to answer *why are you doing that?* with its chain of
command" as a v2 goal. It is highly articulable: "I lost because my push had no forward
command node" is exactly the sentence an AAR should contain. And it creates new
strategy: forward command structures become a real build decision with a real counter
(raze the Outpost, sever the arm).

Buildability, verified: the player-command surface is **23 call sites** — `ui.rs` 8,
`bridge.rs` 6, `ai.rs` 9 — all routable through one helper. Engine follow-through
(`economy.rs` 7, `combat.rs` 1, `units.rs` 1, all of `doctrine.rs`) stays instant and is
untouched. No `SystemSet` work, no resolver, no barrier, no pause primitive, additive-only
bridge protocol.

---

## 3. DECISION

> **v2 adopts Chain of Command: real-time with order-propagation latency from command
> nodes, and full doctrine parity between the human and bridge interfaces. WeGo windows
> and command budgets are both rejected.**

Concretely and unambiguously:

- The game remains free-running real time. No turn windows. No command caps. No new clocks.
- **Doctrine parity ships first, and ships alone**, as its own release: control groups
  become squads, postures/templates enter the human command card, and the
  `machine_driven` gate on the doctrine executor is replaced by a "does this unit have a
  squad with a live posture" test. This is a bug fix, not a mechanism, and it is
  independently valuable.
- Then: a direct `Order` written by a *player interface* (`ui.rs`, `bridge.rs`, or the
  scripted `ai.rs` — all three, identically) is queued as a `PendingOrder` and dispatched
  after `latency(distance to nearest own command node)`.
- `Order`s written by the *engine* — doctrine's squad postures, retreat triggers, leash
  recalls, economy's harvest follow-through, combat's chase — dispatch instantly. This
  asymmetry is the whole mechanism: **standing orders are local; direct orders travel.**
- Command nodes at launch: every completed TownHall, and your living hero. A forward
  Outpost building follows in phase 3.
- Latency is a tunable curve behind `WC3_COMMAND_LATENCY`, defaulting **off** until the
  headless sweep and a human-vs-Claude rematch justify a default.

### Answering Option A's best counterargument

*"Only WeGo proves equity. Yours merely reduces the advantage, and an unproven amount at
that. You are trading a guarantee for a tuning parameter."*

Correct on the facts, and I accept the trade. Three reasons.

First, the thesis never asked for structural elimination. It asked that victory be
*decided* by judgment, and it explicitly chose the engine-side method: "not to slow the
human or rush the AI, but to relocate fast work into the game itself." A guarantee
purchased by slowing the human is not the equity this project said it wanted.

Second, WeGo's guarantee is narrower than it appears. It guarantees the elimination of
*reaction speed* while concentrating *interface bandwidth*, which C2 forbids in the same
breath. To close that hole WeGo must adopt command budgets, at which point it owns
§2.2's unanswerable accounting problem too. The guarantee is real but partial, and the
part it misses is the part that gets worse.

Third, the residual advantage under C is bounded by something the design controls and
both players share: it exists only within a command node's radius, and node placement is
a decision made in the same vocabulary by both seats. A human who parks their hero at
the front to buy fast hands has put their most valuable unit in the most dangerous place
— which is a judgment call with a real cost, not a free interface dividend. That is
exactly what C5 asks for: the strategy preserved, relocated to where counterplay exists.

And if the sweep shows the residual advantage still decides matches, the fallback is not
Option B — it is to shrink node radii toward zero, which approaches "doctrine-only" (the
brief's third option) continuously and without a rewrite. C degrades gracefully into the
strictest possible answer. WeGo does not degrade into anything; it is a rewrite or it is
nothing.

### Answering Option B's best counterargument

*"Yours is just a budget with extra steps — latency is a soft cap, and you admitted a
soft budget is incentive-shaped. You picked the complicated version of my idea."*

The mechanisms are genuinely different in the variable they act on and in where they
live. A budget is a scalar attached to *the player*, decrementing regardless of what is
happening in the world, refilling on a timer nobody in the fiction can see. Latency is a
function of *the unit's position in your command structure* — it varies across your army
at every instant, it changes when you move your hero or build an Outpost, it is readable
in the snapshot, and the enemy can attack it. One is a referee's counter; the other is
terrain.

The practical consequence: a budget makes each action scarcer and therefore more
decisive, concentrating the reaction advantage. Latency makes *distant* action slower
while leaving *local* action fast, which dissolves the advantage precisely where the v1
duel showed it mattering and leaves it where doctrine was already covering. They point
in opposite directions on the one measurement we have.

### Answering strict doctrine-only

*"If doctrine is the equalizer, delete direct orders entirely. The 1–4 Hz executor is
the whole game. Why keep a micro layer at all?"*

This is the cleanest design in the document and I nearly took it. It fails C5: deleting
hero micro, right-click movement, and reactive TownPortal is deletion, not relocation,
and this project has an explicit principle against exactly that move (cavalry were gated,
not removed). It also fails C6 on adoption — right-click-to-move is the RTS's grammar,
and a game where you cannot tell a unit to go somewhere is a hard sell as "the real-time
drama, kept."

Chain of Command reaches the same destination on a dial rather than a cliff: as node
radius → 0, C *is* doctrine-only. Building C means we can playtest our way to strict
doctrine-only if the evidence points there, having lost nothing by starting closer to
the game people already know.

---

## 4. Implementation sketch

### New file: `src/command.rs` (`CommandPlugin`)

Registered in `main.rs` alongside the existing nine plugins. Keeps `doctrine.rs`'s module
contract intact.

**Components**

```rust
/// A player-issued order in transit. `ready_at` is absolute game seconds —
/// same idiom as Militia.until / Bounty.expires_at / LastDamaged.at.
#[derive(Component, Clone, Debug)]
pub struct PendingOrder { pub order: Order, pub ready_at: f32 }

/// Inserted on completed TownHalls and living heroes. Data-driven so the
/// forward Outpost is a table entry, not a code change.
#[derive(Component, Clone, Copy, Debug)]
pub struct CommandNode { pub radius: f32 }
```

**Resources**

```rust
/// Tunables, so the headless sweep can vary them without a rebuild.
#[derive(Resource)]
pub struct CommandLatency { pub inside_node: f32, pub per_world_unit: f32, pub max: f32 }

/// Cache of (team, pos, radius), rebuilt on a 2 Hz on_timer so the issue
/// helper stays a pure function and ui.rs's 15-system chain needs no new query.
#[derive(Resource, Default)]
pub struct CommandNodes(pub Vec<(Team, Vec3, f32)>);
```

**Systems** (`Update`, chained): `refresh_command_nodes` (2 Hz `on_timer`) →
`dispatch_pending` (per frame: `time.elapsed_secs() >= ready_at` ⇒ `try_insert(order)` +
`try_remove::<PendingOrder>()`).

**The shared helper** — one function in `shared.rs`, called from all 23 player-command
sites:

```rust
pub fn issue_order(commands: &mut Commands, nodes: &CommandNodes, lat: &CommandLatency,
                   now: f32, team: Team, unit_pos: Vec3, entity: Entity, order: Order)
```

Zero delay ⇒ insert `Order` directly (preserves current behavior exactly when the
feature is off, and keeps `Changed<Order>` timing identical for the v1 path).

### Systems that change

| File | Change |
|---|---|
| `ui.rs` | 8 `Order` writes route through `issue_order`. Sites: `issue_ground_order` (:1040), `right_mouse` branches (:2516, :2530, :2547, :2564), `left_mouse` Build (:2219) + attack-move (:2241), `minimap_input` (:2134), `command_input` Stop (:1814). |
| `bridge.rs` | 6 `Order` writes in `apply_batch` route through `issue_order`. No protocol break. |
| `ai.rs` | 9 `Order` writes route through `issue_order` — **the scripted AI pays latency too**, or autopilot becomes a cheat and C1 is violated at the third seat. |
| `doctrine.rs` | Writes stay **instant** (that is the mechanism). But two filters must be added — see the integration hazard below. |
| `economy.rs`, `combat.rs`, `units.rs` | Untouched. Their `Order` writes are engine follow-through, not commands. |

### Integration hazard — call this out to the implementer

`doctrine.rs::run_squad_postures` treats a unit as re-taskable when it is `Order::Idle`
with no `MoveTo` (`re_taskable`, `doctrine.rs:138`). During a latency window a unit
looks exactly like that, so the squad executor will re-task it and **clobber the direct
order before it ever arrives.** `enforce_leash` (`doctrine.rs:252`) has the same
problem. Fix: add `Without<PendingOrder>` to both queries, matching the existing
`Without<Retreating>` idiom. This is the single most likely source of a
"my orders sometimes just vanish" bug report.

Secondary: `rearm_retreat` (`doctrine.rs:235`) is `Changed<Order>`-driven, so a
retreating unit now un-latches at *dispatch* time rather than *issue* time. That is
correct — the unit is back on duty when the order actually reaches it — but it should be
asserted deliberately, not discovered.

### Bridge protocol changes — additive only

- `StateOut` gains `command_nodes: [{x, z, radius}]` (own team only — this is an
  information right, and it must be symmetric with what the HUD shows the human).
- `UnitOut` gains `link: f32` (seconds of latency this unit's next order would take) and
  `pending: bool`.
- Batch application reports realised delay per command, folded into the existing
  `errors`-adjacent channel as an `applied: [{cmd, delay}]` array, so a commander can
  *learn* the mechanic rather than infer it.
- `seq` gating, `last_seq`, the 4 Hz poll and 1 Hz snapshot all stay exactly as they are.
- No new commands. `squad`/`posture`/`template` already exist on this side — the work is
  all on the human side.

### UI changes

- **Control groups become squads.** `Ctrl+1/2/3` (`ui.rs:2037`) already exists; bind it
  to `SquadId(1..3)`. The human's existing muscle memory becomes the shared vocabulary
  at near-zero UI cost — the single highest-leverage item in this document.
- **Doctrine page on the command card.** The 3×3 card (`CMD_SLOTS = 9`) is full and only
  `J` and `U` are unclaimed (`ui.rs:606`). Use `U` as a page toggle to a doctrine page:
  posture select (Defend/Push/Escort/Forage) + click-to-place the posture point, and
  "stamp template on this building" when a production building is selected.
- **Ungate the executor.** Replace `!machine_driven(...)` in `default_squad_autonomy`
  and `run_squad_postures` with a test on "unit has a `SquadId` whose `(team, id)` has a
  posture entry." This preserves the original intent — a human who assigns nothing keeps
  their units exactly where they put them — while delivering parity to a human who opts in.
- **Latency feedback.** An in-transit ghost marker at the order's destination (reuse the
  `update_ghost` / `update_rally_flag` machinery), and a `link: 1.8s` readout in the
  selection panel near `Slot::Hints`. A third `Slot` in the top bar (`ui.rs:1140`) can
  carry a node-coverage indicator. Without this feedback the mechanic reads as input lag
  and will be hated.

### Migration path from v1 free-running realtime

- **Phase 0 — doctrine parity.** Ships alone, changes no tempo. Re-run the human-vs-Claude
  duel. This alone may move the result, and if it does, that is a finding worth having
  before we build anything else.
- **Phase 1 — latency core behind `WC3_COMMAND_LATENCY`, default off.** Same env-flag
  precedent as `WC3_SPEED` / `WC3_AI_BOTH` / `WC3_BRIDGE`. Default-off means v1 behavior
  is bit-identical and no existing match setup breaks.
- **Phase 2 — calibration.** Headless sweep (`WC3_HEADLESS=1 WC3_AI_BOTH=1 WC3_SPEED=16`)
  across the latency curve, checking that match length and the counter-triangle survive.
  Then the acceptance test, per thesis principle 4: **the mechanic must appear in a
  winning player's AAR.** Flip the default when it does.
- **Phase 3 — forward Outpost** as a buildable command node, giving the mechanic its
  strategic counterplay (project power forward; the enemy razes it to sever your arm).

### Open questions for the implementer

- Latency curve shape: linear in distance, or a step (fast inside radius / slow outside)?
  The step is more legible for LLM commanders; the linear is smoother to play. Start with
  a step plus a short ramp, and let the sweep decide.
- Does latency apply to *building placement* (`Order::Build`)? Argument for: consistency.
  Argument against: the worker walks to the site anyway, so latency is invisible and just
  taxes the economy. Recommend exempting `Order::Build` initially.
- Should a unit already inside a squad with a live posture receive direct orders faster
  (it has a local commander)? Attractive, and it doubles the incentive to use doctrine —
  but it stacks two mechanics and should wait for phase 3.

---

## 5. Cut into issues

1. **Human doctrine parity: control groups become squads** — Bind `Ctrl+1/2/3` /
   `1/2/3` (`ui.rs:2037`) to write `SquadId(1..3)` in addition to the existing selection
   recall, so a human's control group is the same object the bridge's `squad` command
   creates. Highest leverage item in the spike: it grants the human the squad vocabulary
   at near-zero UI cost by reusing muscle memory that already exists.

2. **Ungate the doctrine executor from `machine_driven`** — `default_squad_autonomy` and
   `run_squad_postures` (`doctrine.rs:340`, `:384`) currently skip human-driven teams
   entirely, so THESIS.md's "runs for whichever player set them" is false for the human
   seat. Replace the `machine_driven` test with an opt-in test (unit has a `SquadId`
   whose `(team, id)` has a posture entry), preserving the original intent that an
   unassigned human unit is never yanked around.

3. **Posture and template UI for the human command card** — Add a doctrine page toggled
   by the unclaimed `U` key: posture select (Defend/Push/Escort/Forage) with click-to-place
   point/radius, plus `DoctrineTemplate` stamping on a selected production building, and
   parameterised retreat threshold / leash radius to replace the current on/off toggles
   (`ui.rs:272-278`). Closes the last of the 8-vs-4 command gap against the bridge.

4. **Chain of Command core: `PendingOrder`, `CommandNode`, `command.rs`** — New
   `CommandPlugin` with the two components, the `CommandNodes`/`CommandLatency`
   resources, and the `dispatch_pending` system; route all 23 player-command `Order`
   writes (`ui.rs` 8, `bridge.rs` 6, `ai.rs` 9) through one `shared::issue_order` helper.
   Gate the whole behavior behind `WC3_COMMAND_LATENCY`, default off, so v1 behavior is
   bit-identical until phase 2 flips it.

5. **Doctrine/latency integration: exempt in-transit units from re-tasking** — Add
   `Without<PendingOrder>` to the `run_squad_postures` and `enforce_leash` queries, since
   `re_taskable` (`doctrine.rs:138`) reads a unit awaiting a delayed order as idle and
   will clobber it. Also assert the intended new behavior of `rearm_retreat`, which now
   un-latches `Retreating` at dispatch rather than issue time. Without this the mechanic
   ships with orders that silently vanish.

6. **Bridge snapshot: expose command nodes, per-unit link latency, and applied delays** —
   Additive `StateOut`/`UnitOut` fields (`command_nodes`, `link`, `pending`) plus an
   `applied: [{cmd, delay}]` acknowledgement, so an LLM commander can see and reason
   about propagation cost instead of inferring it from failure. No changes to `seq`
   gating or the poll/snapshot cadence; symmetry with the human HUD is the requirement.

7. **Latency feedback in the HUD** — In-transit ghost marker at the pending order's
   destination (reusing `update_ghost` / `update_rally_flag`), a `link: N.Ns` readout in
   the selection panel, and a command-node coverage indicator in the top bar. Without
   this the mechanic is indistinguishable from input lag; this issue is what makes it
   read as a game rule rather than a bug.

8. **Calibration sweep and human-vs-Claude rematch acceptance** — Headless sweep
   (`WC3_HEADLESS=1 WC3_AI_BOTH=1 WC3_SPEED=16`) over the latency curve checking match
   length, counter-triangle integrity, and that bounty contests still happen; then a
   human-vs-Claude rematch. Acceptance per thesis principle 4: flip
   `WC3_COMMAND_LATENCY` on by default only once command nodes appear in a winning
   player's after-action report.

---

## 6. Phase 0 as built (issues 1–3, shipped)

Doctrine parity shipped as its own release, as §3 required. What landed differs
from the sketch in three places, and the differences are the interesting part.

**The executor gate.** `run_squad_postures`'s `!machine_driven(...)` early-return
is gone. The opt-in test turned out to be simpler than "does this unit have a
`SquadId` whose `(team, id)` has a posture entry": that test is already the loop
the executor runs — it iterates `SquadOrders`, and a unit with no squad is never
in it. So the whole gate reduces to *one* carve-out:

```rust
if squad == DEFAULT_SQUAD && !machine_driven(&ai, &external, team) { continue; }
```

`DEFAULT_SQUAD` is not something a player said — it is the anti-idle floor
`default_squad_autonomy` seeds to compensate for a slow machine commander, and
that function keeps its machine-only gate exactly as it was (so a human's idle
units still never self-organise). The carve-out is therefore also the F9
handback rule: take a team back from the autopilot and you inherit its squads,
not its autopilot. Four tests in `doctrine.rs` pin all four cases.

**The page key is `[I]`, not `[U]`.** §4 proposed `[U]`; the upgrade bead
claimed it in the meantime. `[I]` is also *both* a button and a raw hotkey: a
worker selection spends all nine card slots on the classic build layout, so the
button yields — and a route to doctrine that one stray worker in the drag box
can close is not a route.

**Squads are minted by the gesture.** A posture button pressed on a selection
that is not already one squad submits `squad` first and `posture` second — two
sentences, the same way a mixed right-click already compiles to two. So `[I][W]`
works without a `Ctrl+N` first, and the log reads identically to a commander who
sent both commands by hand.

**Scoreboard against §2.0's 8-vs-4.** The human now has all seven doctrine verbs
(`priority`, `retreat`, `leash`, `autocast`, `squad`, `posture`, `template`),
parameterised rather than as toggles, with the coarse `[G]/[V]/[P]/[T]` presets
kept on page one.

**Range, closed (wc3clone-137).** Three of the four remaining gaps were about
*range* rather than vocabulary, and they are gone:

* **Retreat and leash are free-entry.** `[F]`/`[G]` still step the
  25/35/50% and 10/18/30 presets — they are the fast path, and most of the time
  a preset is what you want. `[-]`/`[=]` nudge the threshold and `[[]`/`[]]`
  nudge the radius, one increment per press across the whole legal range, so
  the human can say 0.375 exactly as a commander can. Both controls submit the
  identical `Intent::Retreat`/`Intent::Leash`; only the arithmetic differs.
* **`posture escort` names any own unit.** It arms a click like the other three
  postures, and the click picks a unit instead of a point. Escorting the hero
  is now one click rather than zero; escorting a Catapult, a Priestess or an
  expanding Worker is now possible at all.
* **`autocast` is per ability.** The doctrine page carries one toggle per
  ability slot (`Z`/`X`/`C`, named after the ability), each submitting
  `ability: <slot>` and editing that one rule. Page one's `[T]` is unchanged
  and still means slot 0.

Two affordances came with them: a translucent disc marks the pending posture
point during click-to-place (at `DEFEND_RADIUS` for Defend, so the circle you
aim is the circle you get), and every selection tile carries its squad id in the
corner — the doctrine card speaks for the FIRST unit's squad, and a drag box
that scooped up two squads used to look exactly like one that scooped up one.

What is still narrower than the wire: squad ids from a gesture are 1–3 where the
wire takes any `u8`. That one is deliberate — a gesture squad is a control
group, and there are three control groups.

---

## 7. Phase 1 as built (issue 4, shipped)

The latency core landed behind `WC3_COMMAND_LATENCY`, default off. What differs
from §4's sketch is mostly *cheaper than planned*, plus one thing the sketch
could not have known because only a sim could find it.

**There was no 23-site refactor.** §4 budgeted 23 player-command `Order` writes
across three files and proposed a `shared::issue_order` helper to route them
through. The intent compiler (docs/INTENT.md) landed in between and collapsed
`ui.rs` and `bridge.rs` into one choke point, so latency for those two seats is
a substitution inside `compile_intent`'s order arms and nothing else. The
"integration hazard" §4 flagged survived intact and was worth every word.

**The verb table.** §4 asked the implementer to decide the exact set. It is in
`command.rs`'s module docs, in full, with a reason per row. In summary:

| Verb | Latency |
|---|---|
| `move`, `attackmove`, `attack`, `harvest`, `return`, `follow`, `stop` | **pays** |
| `build` | exempt — §4's open question, answered as recommended |
| `train`, `upgrade`, `cancel`, `research`, `rally` | exempt — addressed to a building, which stands at a node |
| `cast` | **pays** for a unit caster, exempt for a building one — see below |
| `use_item`, `buy` | exempt |
| `priority`, `retreat`, `leash`, `autocast`, `squad`, `posture`, `template` | exempt — doctrine IS the fast path |
| `trigger_set`, `trigger_clear` | exempt — arming a rule is doctrine, and the rule's own firing is too (below) |
| `plan_set`, `plan_clear` | exempt — writing a sequence down is doctrine, and the plan's own steps are too (below) |
| `autopilot`, `surrender` | exempt — match level |

The `cast` row was originally not a carve-out but an identity: every caster in
the game either *was* a command node (a hero) or *sat on* one, so a computed
link would be zero for all of them, and charging it would have been ceremony.
`every_caster_is_a_command_node` asserted exactly that so the claim could not
rot in silence.

**It rotted, and the test caught it.** The Sorcerer (bead/1qq4y0) is a caster
that is not a hero — it stands in the middle of an army and debuffs. The test
failed on the merge, which is the system working: the *identity* broke, the
*framework* did not. Re-derived from the framework, the answer is unambiguous
and the row moved:

- **`cast` at a unit caster pays.** Hand-firing Slow on a Sorcerer in the middle
  of a fight is precisely "reaching past your chain of command at the point of
  contact", which is the thing this mechanism exists to price. For a hero it
  still computes zero — a hero *is* a node — so hero micro is exactly as fast as
  it was, and §C5's promise about TownPortal is untouched.
- **`cast` at a building caster stays exempt**, because `abilities_of_building`
  is still `is_hall`-only and a hall is a node. `every_building_caster_is_a_command_node`
  is the old test, narrowed to the half that still holds, and it is what keeps
  CallToArms instant.
- **`autocast` stays exempt**, and this is the row that makes the Sorcerer
  *interesting* rather than annoying: turning the debuff into standing policy
  costs nothing and runs at machine speed, while hand-firing it at range costs
  the link. C4 — "doctrine strictly better than micro at range" — landing on a
  unit the design never anticipated, for free. The Sorcerer is even *born* with
  an autocast policy, so the fast path is the default and the slow path is the
  deliberate one.

Mechanically a cast is an event rather than a component, so the delayed form is
a second component, `PendingCast`, and a second dispatcher. Deferring the event
rather than the verdict is what makes a late cast fizzle honestly: combat.rs
reaches its mana/cooldown decision when the cast *arrives*, so an ability whose
mana was spent while the order travelled simply does not go off — exactly as if
the player had been slow.

**Triggers pay nothing, and that is the point** (`wc3clone-pec`, v3). A
`trigger_set` arms a condition the engine watches at 4 Hz; when it fires, the
engine submits the stored intent through the ordinary compiler. That submission
is **exempt from the link**, whatever verb it carries — a trigger-fired `move`
lands in the frame it fired even from the far corner with no halls standing.

The row is derived from this table's own rule rather than carved out of it.
*Standing orders are local; direct orders travel*, because a unit under standing
policy already has its orders and does not need to ask. A trigger is standing
policy whose condition happened to come true: the commander reached the unit
when they ARMED it, and charging the link again on firing would price one reach
twice.

**Plan steps pay nothing either, on the identical argument** (`wc3clone-c5b`,
v3). A `plan_set` hands the engine a named sequence; when a step's turn comes,
the engine submits that step's stored intent through the ordinary compiler, and
that submission is **exempt from the link** whatever verb it carries. Same
derivation, one rung along: a plan is standing policy the engine executes
unattended, its author reached the units when they wrote the sequence down, and
step 4 firing four minutes later is the engine doing what it was told rather
than a new order travelling out from a commander.

It also has to be exempt for the mechanism's own incentive to survive. If each
step paid, a five-step plan would cost five links and be *strictly worse* than
typing the same five commands by hand from the same place — which inverts C4
("doctrine strictly better than micro at range") at exactly the layer where the
tempo argument is strongest, because a plan is the one construct that is
*entirely* decided in advance.

Both rows are selected by the same named constructor: `CommandLink::exempt_issuer`,
reached when `SubmitIntent` carries either a `trigger` or a `plan` stamp. One
call site, two rows, and the constructor exists rather than a boolean precisely
so this table has somewhere to point.

It also extends C4 one rung. "Doctrine is strictly better than micro at range"
was an argument about *continuous* work; with triggers exempt, *pre-arming a
rule is strictly better than hand-answering an alarm at range* — the same
incentive, now covering reaction. That is the whole reason a commander should
want triggers rather than a faster poll loop, and it is a fact about the world
(you thought ahead) rather than a fact about the referee.

Mechanically it is `CommandLink::exempt_issuer` — a second constructor rather
than a boolean on the first, so "who is allowed to skip the link" is a named
decision with the reasoning attached to it. `SubmitIntent::trigger` is what
selects it, and that field is set by exactly one caller (`trigger.rs`).

**The curve** is the recommended step plus ramp, with per-node radii so the
phase-3 Outpost is one arm of `building_node_radius`:

```
slack   = distance to nearest own node - that node's radius   (0 if inside)
latency = 0                                   when slack <= 0
        = min(max, step + per_unit * slack)    otherwise
        = max                                  when the team has NO nodes
```

Defaults, all env-overridable for the sweep: hall radius 30, hero radius 18,
step 0.6s, ramp 0.02s per world unit, cap 3.0s. That puts a midfield engagement
(~100 units from home, which on this map is the centre) at ~2.0s and the far
corners at the cap — the 1.5–3s §C1 asked for. The hero's radius is deliberately
smaller than a hall's: the mobile node is the one you buy fast hands with, and
it should cost something to place.

**All three seats pay.** `ai.rs` does not speak through the intent compiler, so
it calls the same `OrderIssuer` directly at its nine unit-order sites. Its
`build` is exempt on the same row as everybody else's. Two tests pin it, in both
flag states — if autopilot ever stops paying, the suite says so.

**The doctrine guards** (§4's integration hazard, and issue 5's subject) are in:
`run_squad_postures` and `enforce_leash` both gained `Without<PendingOrder>`,
because a unit awaiting a delayed order is indistinguishable from an idle one
and would otherwise be re-tasked out from under its own orders. `rearm_retreat`
now un-latches `Retreating` at *dispatch* rather than issue time, which is the
behaviour §4 predicted and asked to have asserted deliberately. One knock-on
worth naming: `run_squad_postures` computes its cohesion point from the
`members` query, so in-transit members no longer contribute to the squad's
centre of mass for the second or two they are travelling.

**What a sim found that review did not.** With latency on, a `crossings` run hit
the time cap with two workers frozen and the telemetry reading "2 orders in
transit, mean link 3.00s" for twenty minutes of game time. The team had lost
every hall and its hero, so it paid the cap on every order — and `ai.rs`
re-issues its standing decision once a second, so each repeat replaced the last
and restarted the clock. Nothing ever landed.

The fix is a rule that is better design than the bug was a bug: **saying the
same thing again does not restart the journey.** An order that matches the one
already in transit is a no-op; a genuinely different one supersedes it and pays.
Latency is the cost of *changing your mind* at range, not a tax per click — which
also means a human holding down right-click cannot accidentally paralyse their
own army, and neither can a commander re-sending an unchanged batch. Six sims
across both maps after the fix: all decisive, no caps, no frozen units.

**The snapshot** gained `command_nodes` (own team only) on `StateOut` and
`link` / `pending` on `UnitOut`, all omitted when the feature is off — a
flag-off snapshot is the same 16 keys it always was, verified live. What issue 6
still owns is the `applied: [{cmd, delay}]` acknowledgement, which needs the
compiler to report per-command realised delay back to the seat that sent it.

**Evidence and observability.** The intent log annotates delayed sentences —
`move unit 4294968182 to (0.0, 0.0) (+0.7s link)` — plus a structured `link`
field, both absent when nothing was delayed, so a flag-off replay is
character-for-character a v1 replay. `ai.rs` is not a player and writes no
intent log, so an AI-vs-AI sweep would otherwise produce no evidence at all;
`command::report_link_load` covers that with a periodic line giving orders in
transit, mean link and worst link. That series is what issue 8's calibration
should read: near-zero mean means the curve is not binding, a mean pinned at the
cap means the armies have marched off the end of their own chain of command.

**Not built here, by design:** the HUD feedback (issue 7) — without it the
mechanic reads as input lag, which is exactly why the default stays off — and
the forward Outpost (phase 3), which is one table entry in
`building_node_radius` when its bead comes up.

### Reconciled against master (bead/polish, bead/ai-bundle, bead/1qq4y0)

Three beads landed while this one was in flight. What each cost is worth
recording, because two of the three cost nothing and the reason is the same
reason in both cases.

**The ghost right-click (bead/polish) was priced without being touched.** It is
a genuinely new direct-order path — the human can now attack a *remembered*
building the picker previously refused — and it arrived after this mechanism was
designed, with no knowledge of it. It pays the link correctly anyway, because it
compiles to `Intent::Attack` and there is exactly one `Attack` arm.
`a_ghost_attack_pays_the_link_like_any_other_direct_order` pins it. This is the
choke point (docs/INTENT.md) paying for itself: **a new way of speaking cannot
accidentally arrive at a privileged speed.** Had this bead been the 23-site
refactor §4 budgeted, the ghost path would have been site 24 and nobody would
have noticed.

**ai.rs grew by ~1250 lines and needed no re-wiring.** Towers, ford
fortification, reactive mixes, the Castle trigger rework and Shop usage all
landed in the same function this bead had edited, and the merge left all ten
issue sites intact with zero direct `try_insert(Order::…)` remaining in the
file. The new AI *does* re-assert standing decisions more aggressively than the
one this was written against — which would have made the livelock above worse,
not better, and is a good argument for having fixed it as a rule rather than as
a special case. Its new `buy`/`use_item` calls are exempt on the same row as
every other seat's, and its Slam now goes through `issue_cast`: zero for a hero,
but the day the script learns to hand-fire a Sorcerer it pays for that reach
automatically instead of quietly not paying. Master had independently invented
the same `AiEvents` bundle this bead needed for the parameter ceiling, which is
two people finding the same wall.

**Multiple heroes are multiple mobile nodes.** Hero slots per tier means a team
can field more than one, and `refresh_command_nodes` collects the whole
`With<Hero>` query rather than "the" hero, so this works by construction —
`every_living_hero_is_its_own_command_node` keeps it working and pins the
dead-hero rule beside it. It is a real strategic object: two heroes are two
fast-hands zones, bought by putting two expensive units in two dangerous places.

**One thing for the calibration bead — and a correction.** After the
bead/ai-bundle merge this section reported that latency-on lengthened matches
enough to matter: seven flag-off runs on `open` finished in 390-810s while
seven flag-on runs produced two that hit the 1800s cap. **That reading did not
survive the next merge and should not be carried forward.** Re-run against
master after bead/ge4, the two arms are indistinguishable: flag-off gave 4
decisive out of 5 with one cap on `crossings`; flag-on gave 5 decisive out of 6
with one cap on `open`. Caps now appear in *both* arms at similar rates, so what
was attributed to link latency was mostly master's own drift — the scripted AI
grew towers and ford holds over the same period, and defensive play lengthens
games whoever is issuing the orders.

What is worth keeping from the observation is the shape of the failure, because
it is the one the sweep must learn to recognise: every capped run in either arm
has the same signature — mines dry, both armies alive and supplied, treasuries
banking lumber, and (with the flag on) *no orders in transit* through the late
game. That is the mine-exhaustion stalemate, not a tempo problem, and a sweep
that reads match length alone will mistake one for the other. Read
`report_link_load` alongside it: a stalemate with an empty in-transit queue is
the economy running out, while a mean link pinned at the cap is armies that have
marched off the end of their own chain of command.

### Reconciled against bead/ge4 (the `why` layer)

ge4 gave every unit an answer to "why are you doing that?" — a `Provenance`
stamped in the same `Commands` call that mints the behaviour, so the answer
cannot drift from the behaviour. Latency puts a gap between minting an order and
the unit receiving it, which is exactly the case that layer had not met yet.

**A delayed order carries its reason and stamps it on arrival.** `PendingOrder`
holds the `Provenance` the compiler minted, so the verb and the interface that
spoke it travel with the order; `dispatch_pending` rewrites its `at` to the
arrival time as it lands. The alternative — stamping speech time — would have
made `Provenance.at` mean two different things depending on the cause, since
every other rung (doctrine's postures, a building's template) records the moment
the behaviour *began*. A unit would have claimed to have been obeying an order
for two seconds before it had received it.

The speech time is not lost, and the two records join:

```
intent_log.jsonl : t=8.3  link=0.7  why="order:move by bridge t=9"
                   sentence="move unit 4294968174 to (0.0, 0.0) (+0.7s link)"
units[].why      :                  "order:move by bridge t=9"     // 8.3 + 0.7
```

This is a live capture, and the interesting line is the one not shown: while the
order was in transit the unit answered `why: "idle"`. It had not started obeying
yet, and it said so. That is the two layers agreeing rather than merely
coexisting — the `why` layer describes what a unit is doing, and during a
latency window a unit is genuinely still doing the old thing.

Making that join exact needed two small choices: the log's `why` is rendered at
`t + link` rather than at `t` (it is the join *key*, so it must be
character-for-character what the unit will answer), and `at` is set from
`ready_at` rather than from the dispatching frame's clock, so the join holds to
the log's 0.1s resolution instead of drifting by a frame. For a group order
spread across the map the log names the worst link, so its `why` joins against
the last unit to receive the order; the others answer with their own, earlier
arrival.

**`PendingCast` carries no provenance, deliberately.** A cast mints no `Order`,
so there is nothing to re-time: a unit's reason for being where it is is not
changed by having thrown a spell, and overwriting it with `"cast"` would replace
a standing answer with a momentary one. The log side is unchanged — a delayed
cast annotates its sentence exactly as a delayed order does.

**ai.rs composed cleanly.** ge4 wraps the script's orders in `script(what, now)`
and this bead routes them through `OrderIssuer`; the composition is
`issuer.issue(…, script(what, now))`, so the stamp goes *through* the latency
layer rather than around it and survives the deferred dispatch. All eleven sites
compose, and no direct `Order` write remains in either the compiler or the
script.

---

## 8. Phase 2 as built (issues 5–8)

The four follow-ups that turn the latency core into a mechanic a player can
*see*, an LLM can *read*, and a maintainer can *tune*. Issue 8's other half —
the human-vs-Claude rematch — is not here and cannot be, for reasons §8.4 states
plainly.

### 8.1 The doctrine audit (issue 5, completed)

Phase 1 added `Without<PendingOrder>` to `run_squad_postures` and
`enforce_leash` and noted a knock-on it did not resolve: in-transit members no
longer contributed to a squad's cohesion centroid. Walking every doctrine
consumer in turn produced one real fix and five deliberate non-fixes, and the
non-fixes are the more interesting half — each is now a comment at the system
that answers "why is there no guard here?" before someone adds one.

**The fix: an in-transit member is still a body in the formation.** The guard
had shipped as a query *filter*, so it applied to two different questions at
once. "Who may I re-task?" wants it. "Where is this squad standing?" does not: a
unit awaiting a delayed order has not moved an inch, it is standing in the blob
and in range of whatever the blob is in range of. Filtering it out made a
squad's centre of mass lurch the moment a player spoke to half of it, and the
other half would then regroup on a point that ignored the squadmates standing
right beside them. The filter became `Option<&PendingOrder>` plus one `continue`
in the member loop: counted for cohesion, skipped for re-tasking.

Retreaters stay filtered out of the centroid entirely, and the asymmetry is the
rationale. A retreater is deliberately *leaving* the formation under a policy
the commander set, so the squad must not gather around a unit running for home.
An in-transit unit is going nowhere yet.

The five decisions not to guard:

| System | Verdict | Why |
|---|---|---|
| `trigger_retreat` | no guard | A unit bleeding out is not "busy waiting", and a retreat threshold is the commander's own standing order — the fast path. It never cancels what is in transit, so nothing is swallowed. |
| `rearm_retreat` | no guard (already) | Un-latches at *dispatch*, as §4 predicted. Now asserted rather than assumed. |
| `idle_instinct` | no guard | A unit whose last order finished while a new one travels genuinely is idle, and "idle" is the true answer. Suppressing it would make the unit claim to be obeying an order it had already completed, to hide a latency window. |
| `default_squad_autonomy` | no guard | Enrolment writes a `SquadId`, never an `Order`. It decides who may re-task the unit *later*; the posture executor then declines to while the order travels. The two compose. |
| `recover_retreaters` | no guard | Only removes a marker, handing the unit back to an executor that will decline to touch it. |
| `auto_cast_abilities` | no guard | The tempting one, and backwards: it would let a player *suppress* the fast path by reaching for the slow one. Left alone, the standing policy fires now and the hand-fired copy arrives to find the ability on cooldown and fizzles — the honest-fizzle rule `PendingCast` was built around. |

`trigger_retreat`'s non-guard has a consequence worth naming, because it is C4
in miniature and it is now a test. A player orders a unit forward; the unit
breaks before the order arrives; the order lands and un-latches the retreat; the
unit is still under its threshold, so the policy fires again a quarter of a
second later. **An order bought at range loses the argument with a policy set in
advance, and loses it within 250ms.**

Outside doctrine, the audit cleared two more consumers. `economy.rs`'s
depletion auto-rebalance writes `HarvestJob`/`MoveTo`, not commands, and never
touches a `PendingOrder`. `ai.rs`'s `rebalance_mines` *does* pay the link, and is
protected from the stale-state loop it would otherwise have — it re-derives crew
counts from positions that do not change until the order lands, so its re-picks
are byte-identical orders and the "saying the same thing again does not restart
the journey" rule absorbs them.

### 8.2 The acknowledgement (issue 6, completed)

Phase 1 shipped `command_nodes`, `link` and `pending` and left the
`applied: [{cmd, delay}]` acknowledgement open. It is in, and it cost less than
expected because both halves already existed: `OrderIssuer.max_delay` is the
realised worst link for one sentence, and `SubmitIntent.tag` is already the
string `"cmd 3"` that the error channel prefixes its messages with.

So `applied` is the positive half of a verdict the wire already carried:

```json
"errors":  ["cmd 5: unit 4294968182 is not yours"],
"applied": [{"cmd": "cmd 3", "delay": 1.8}]
```

Same batch, same identity scheme, no second correlation mechanism. `IntentApplied`
is shaped like `IntentErrors` down to the per-team split and the "cleared when a
batch is accepted, appended by the compiler, copied into the next snapshot"
lifecycle.

Two decisions worth recording. **Bridge-sourced only**: a UI gesture's seat is a
person looking at the selection panel, and echoing their every right-click into
the other seat's snapshot would be noise for a reader who is not there.
**Silence means instant**: only commands that actually paid are listed, on the
same reasoning the intent log omits its `link` field when nothing was delayed.
Together those keep the channel — and its wire key — permanently empty with the
feature off, so `tools/verify_intent_bridge.py`'s both-directions key-set
assertion still passes untouched.

`tools/COMMANDER_BRIEF.md` gained a **chain of command** section: what
`command_nodes` / `link` / `pending` / `applied` mean, and the three rules a
commander would otherwise learn the hard way — repeating an order is free,
`pending: true` with `why: "idle"` is not a lost order, and doctrine is strictly
faster than micro at range.

### 8.3 The HUD (issue 7, completed)

§4 named this the issue that decides whether the mechanic "reads as a game rule
rather than a bug", and it is why the default stays off. Three readouts:

- **A closing ring at the destination.** Every selected unit with an order in
  transit gets a marker where the order is going, in the rally flag's gold
  because the player already reads that colour as "somewhere I told something to
  go". Its radius *is* the countdown: full when the order is spoken, tight as it
  lands. This is the piece that matters — a player who clicks and sees nothing
  concludes the game dropped the click, while a player who sees a marker appear
  and tighten concludes the order is on its way, which is both true and the
  information they need to decide whether to wait.
- **A link line in the selection panel**, under `Why` and for the same reason it
  sits there: a unit's reason and the cost of changing it are one thought.
  Tallied like `why_line` but sorted **worst first** — a player deciding whether
  to reach for a strung-out selection is asking about its slowest unit, not its
  typical one. In-transit orders are reported by the link they are paying, not
  the time remaining; the countdown belongs to the ring on the ground.
- **Hairline coverage rings and a top-bar count.** Each own command node draws
  its free radius, and the bar reads `Chain: 3 nodes · 8/12 in reach`. Own team
  only, symmetric with the snapshot, because the enemy's chain of command is
  something you learn by razing it. The rings needed a second, thinner torus
  mesh: the existing ring's band is 16% of its radius, which at a hall's 30
  world units is a five-unit-wide donut over the base rather than a circle.

**Flag off is pixel-identical, and mostly by construction rather than by check.**
No `PendingOrder` can exist with the feature off, so the transit-marker query is
empty and not one marker entity is ever spawned; both text lines collapse to the
empty string, which occupies nothing in a left-packed bar or a text column. Only
the node rings ask `latency.on`, because they are the one readout that could
otherwise draw a stale circle from a cache nobody is refreshing.

One structural cost: `update_hud` was already on Bevy's 16-parameter ceiling, so
the three things this needed (the curve, the selection's positions, the whole
army's positions) arrive as one `SelectionReasons` bundle that absorbed the
existing `sel_why` query — net zero parameters. The UI's `Update` chain crossed
Bevy's 20-element tuple limit and is now two chained groups, split where it reads
best: everything that takes input, then everything that draws the result.

### 8.4 Calibration (issue 8, first half — the sweep)

`tools/link_sweep.py` runs the grid headless and classifies what it finds.
Thirty-nine runs on `open`: a flag-off baseline plus twelve curves — hall radius
{30, 45, 60} × step {0.3, 0.6} × ramp {0.01, 0.02}, cap fixed at 3.0s, hero
radius fixed at 18 — three replicates each, `WC3_HEADLESS=1 WC3_AI_BOTH=1
WC3_SPEED=16`, 1800s game cap.

| arm | n | decisive | median length (game s) | mean link | worst link | mean in transit | caps (classified) |
|---|---|---|---|---|---|---|---|
| baseline (flag off) | 3 | 3/3 | 405 | — | — | — | — |
| hall 30 / step 0.3 / ramp 0.01 | 3 | 3/3 | 449 | 1.18 | 1.70 | 2.4 | — |
| hall 30 / step 0.3 / ramp 0.02 | 3 | 3/3 | 403 | 1.71 | 2.15 | 2.5 | — |
| hall 30 / step 0.6 / ramp 0.01 | 3 | 3/3 | 424 | 1.55 | 2.07 | 2.9 | — |
| hall 30 / step 0.6 / ramp 0.02 *(today's default)* | 3 | 3/3 | 424 | 2.21 | 3.00 | 2.9 | — |
| hall 45 / step 0.3 / ramp 0.01 | 3 | 3/3 | 414 | 1.18 | 1.56 | 2.2 | — |
| hall 45 / step 0.3 / ramp 0.02 | 3 | 3/3 | 416 | 1.87 | 3.00 | 2.0 | — |
| hall 45 / step 0.6 / ramp 0.01 | 3 | 2/3 | 645 | 1.31 | 1.85 | 2.2 | 1 (cap-economy) |
| hall 45 / step 0.6 / ramp 0.02 | 3 | 2/3 | 795 | 1.85 | 3.00 | 2.9 | 1 (cap-economy) |
| hall 60 / step 0.3 / ramp 0.01 | 3 | 3/3 | 490 | 1.30 | 1.33 | 1.0 | — |
| hall 60 / step 0.3 / ramp 0.02 | 3 | 3/3 | 484 | 1.58 | 2.46 | 3.9 | — |
| hall 60 / step 0.6 / ramp 0.01 | 3 | 3/3 | 429 | 1.32 | 1.69 | 2.4 | — |
| hall 60 / step 0.6 / ramp 0.02 | 3 | 3/3 | 490 | 1.84 | 3.00 | 2.0 | — |

Plus `crossings`, as an off-grid check that the finding is not one map's
accident: baseline 3/3 decisive at a 499s median, and the recommended curve
below 3/3 decisive at a 494s median.

**37 of 39 decisive, and neither cap is a tempo finding.** Both landed in
hall-45 arms with an empty in-transit queue through the late game — the
mine-exhaustion signature §7 warned the sweep would have to learn to recognise,
and which the script classifies as `cap-economy` rather than counting. That the
two fell in adjacent arms is n=3 noise; nothing about hall 45 distinguishes it
from 30 or 60 on any other column.

**Latency does not lengthen matches.** Excluding the two economy stalemates,
every arm's median sits between 403 and 490 game seconds against a baseline of
405 — inside the run-to-run spread of the baseline itself. This confirms §7's
own correction (which retracted an earlier "latency lengthens matches" reading
as master's drift) with a proper grid behind it rather than seven runs a side.

**The curve is binding without being punishing.** Mean link across arms runs
1.18–2.21s, so orders are genuinely paying, and mean orders-in-transit sits
around 2–3 — the armies are reaching past their chain of command constantly, and
still resolving their games.

A methodological note that the script enforces rather than assumes: match length
is inferred as `wall_seconds × WC3_SPEED`, because the engine logs no game
clock. The inference is self-checking, and checked — capped runs landed on the
1800s cap with **0.0% error**.

#### Recommended tuning

**hall 30 · hero 18 · step 0.6 · ramp 0.01 · cap 3.0** — one change from
today's defaults: halve the ramp.

The reason is where the cap starts binding. At ramp 0.02 the curve reaches its
3.0s ceiling at 120 world units of slack, which on this map is a little over
half the distance between the two bases — so across much of the ground that is
actually fought over, the curve has stopped discriminating and every distant
order costs a flat 3.0s. That shows up in the table as today's default being the
only arm whose mean link (2.21) sits near the top of §C1's 1.5–3s band with its
worst pinned at the ceiling. At ramp 0.01 the ceiling is not reached until 240
units, past the base-to-base distance, so distance means something everywhere on
the map and the cap goes back to meaning the thing it was designed to mean: the
severed arm, the penalty for owning no command nodes at all.

The resulting arm measures mean link 1.55s, worst 2.07s, median 424s, 3/3
decisive on `open` and 3/3 on `crossings` — mid-band, never pinned,
indistinguishable from baseline on match length.

Hall radius barely mattered across 30/45/60, which is itself worth recording:
the free bubble's *size* is not the lever, the ramp is. That leaves hall radius
free to be chosen for legibility ("your base") rather than for balance, and it
is the parameter the phase-3 forward Outpost will want to reuse.

#### What this sweep does NOT establish, and who has to decide

**The default stays OFF.** Nothing above is grounds to flip it, and the reason
is that this sweep cannot answer the question the mechanism exists to answer.

Every one of these 45 runs is the scripted `ai.rs` against itself. That AI does
not micro heroes at the point of contact, does not time attacks against a human's
decision cadence, and is not slow in the way an LLM commander is slow — so what
the grid measures is whether command latency *breaks* the game, not whether it
*equalises* the thing docs/TEMPO.md was written about. It does not break it, on
either map, at any point of this grid. That is a necessary result and not a
sufficient one.

The sufficient one is thesis principle 4, and §4's migration path already named
it: **flip `WC3_COMMAND_LATENCY` on by default only once command nodes appear in
a winning player's after-action report.** That means a human-vs-Claude rematch,
played with the flag on and the HUD from §8.3 in front of the human, whose AAR
talks about where the halls and the hero were — not about input lag.

**That acceptance step needs the project owner.** It is not automatable and this
bead did not attempt it: an agent cannot sit in the human seat of a
human-vs-Claude match and report honestly on whether the mechanic felt like a
game rule. The sweep, the tuning recommendation and the HUD are the preparation
for that match. The verdict is the owner's.

Other honest limits of the numbers above: n=3 per arm, one map for the grid;
`report_link_load` only emits when something is in transit, so each arm's mean
link rests on 1–5 samples per match; and the engine has no seed control, so
replicates differ only by bounty-placement RNG and frame timing rather than by a
controlled seed. Widening any of these is a flag away —
`tools/link_sweep.py --help` documents the axes.

### Reconciled against master (bead/hre, co-command)

Co-command landed while this bead was in flight, and the interesting part is
that the two beads collided in exactly the same three places — twice by
converging on the same answer independently, which is usually a sign the answer
was forced by the shape of the problem rather than chosen.

**Both hit Bevy's 16-parameter ceiling on `write_snapshot`, and both solved it
the same way.** This bead bundled the compiler's two verdict channels as
`SeatVerdicts`; hre bundled the copilot queue and the intent journal as
`CoCommand`, with a comment giving `TeamTech` as its precedent — the same
precedent this one had cited. Merged, the two bundles sit side by side and the
system is back to fifteen parameters, with headroom neither bead had alone.

**Both hit Bevy's 20-element tuple limit on the UI's `Update` chain.** hre added
`update_posture_marker` (the doctrine page's click-to-place point finally got a
ground visual) and grouped it with `update_ghost` as an unordered pair, on the
honest grounds that two armed-gesture previews cannot observe each other. This
bead added two Chain of Command systems and split the chain into two internally
chained groups. Both were needed: the pair alone is 20 elements and this bead's
two would have made 22. The merged chain keeps hre's pair *inside* this bead's
second group.

**`IntentApplied` and `IntentJournal` turned out to be siblings.** Both are
per-team, both are written only by the compiler, both are read only by
bridge.rs, and both were registered in `IntentPlugin` next to `IntentErrors`
with near-identical reasoning in the comment. They now sit together, and the
comment says "all three" rather than "both".

Nothing about the mechanism itself needed revisiting. The one thing worth
re-checking was the flag-off wire, because a copilot seat now adds keys of its
own: `tools/verify_intent_bridge.py` still passes on a plain seat with the
latency flag off, which is the assertion that a v1 snapshot is still exactly its
historical sixteen keys. Re-run after the merge, the acceptance sims hold too —
`open` 2/2 decisive in both arms, `crossings` 5/6 flag-off (the one cap a
baseline stalemate, in the arm where the classifier has no link telemetry to
read) and 6/6 flag-on.
