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
kept on page one. What remains bridge-only is *range*, not vocabulary: the card
steps retreat through 25/35/50% and leash through 10/18/30 where a commander
writes any float; squad ids from a gesture are 1–3 where the wire takes any
`u8`; `posture escort` targets the team's hero where the wire names any own
unit; and per-ability `autocast` rules are still one rule on slot 0. Each is a
UI affordance, not a missing verb — the intent submitted is the same value.
