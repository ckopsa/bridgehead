# Arena Round 20 — BLUE (Kingdom, boomer creed) — After-Action Report

**Result: BLUE WINS.** Match length **5:41 (t=341s)**. Zero buildings lost.
Map: `crossings`. Blue base (-70,-70), Red base (70,70).

## The held-at-t=0 planning window

The whole opening was armed before the clock started. One batch at t=0 carried:

- `plan_set "boomer"` — 8 steps: 3 workers to the southwest mine, 2 to trees,
  2 Workers queued, Barracks at (-56,-70), Farm at (-84,-76), the free **Hero**,
  one more Worker.
- `region_set` x2 — `home` (r30 over the base) and `approach` (-40,-40, r26), the
  diagonal the enemy must walk from the center ford.
- 4 triggers — supply valve, home-guard, doorbell (`enemy_army_seen` 4 within 30s),
  and approach-watch (`enemy_in approach`).
- `ready`.

The engine executed steps 1–8 between t=0 and t=2. Barracks placed at t=1,
Farm at t=2, Hero queued at t=2. That is the entire value of the handshake:
my build order cost zero polls and zero seconds of game clock.

## How it unfolded

- **0:00–1:00** — Plan ran itself. Hero out at 0:54. Barracks finished, second
  Barracks placed at 1:22. Supply valve fired repeatedly and kept farms coming.
- **1:54** — Scout worker (sent to `mid`) died at the center ford but paid for
  itself: `~5 (2 Archer, 2 Footman, 1 Hero) near the center ford`. That was the
  rusher's first punch, and I saw it before it arrived.
- **3:36** — Red hit with **11 units** (4 Archer, 3 Footman, 1 Hero). `approach-watch`
  and `doorbell` both fired and pulled squad 1 home before contact. I lost one
  Footman and one Archer; my squad had chased out to (-50,-48), so I re-anchored
  the defend posture tight at (-64,-64) r16 under the Tower and Keep. Red's attack
  broke off. Net: 2 units for a repelled 11-stack.
- **4:00** — Keep completed, free **Priestess** trained. Split templates: both
  Barracks now stamp new units into **squad 2** (home defense), while **squad 1**
  (17 units + both heroes) went to `push`.
- **4:48–5:35** — Squad 1 crossed the center ford unopposed and claimed a 315g
  bounty cache on the way. Both heroes outran the main body and arrived at the
  enemy base alone — the one moment I was genuinely nervous — but Red's army was
  not home. The Champion went **L1 → L4** and the Priestess **L1 → L3** killing
  buildings and whatever defended them.
- **5:41** — Enemy TownHall and Barracks down. **Game over, Blue wins**, with my
  main body still at (24,19) and squad 2 never having fired a shot.

## The heroes' story

Both heroes were fielded and **both survived; the 400g/100l revival bill was never
paid.** The Champion trained at 0:54 (free, 5 supply — which is why a Farm had to
precede it in the plan), the Priestess at ~4:15 the moment the Keep finished.

`hero-save` was armed at `hero_below 0.40`, later tightened to **0.45** and widened
to name both heroes; explicit `retreat` policies backed it at 45%/50%. Neither ever
fired. The Champion's lowest point all match was 367/440 (83%), reached while
solo-raiding the enemy base. That is the honest verdict on the free-hero rule: the
insurance cost me nothing and the heroes *won the game* — they arrived first, and
they were the units that actually killed the TownHall. The judgment call the brief
posed ("field it, but a dead hero is a Barracks of gold") resolved cleanly in favour
of fielding, but only because the pull-back reflexes were armed before first contact.

The one thing I would do differently: I twice had to hand-issue `move` to drag the
heroes back to the squad. A push squad is supposed to advance cohesively, but the
heroes' speed still put them 25–30 units ahead of the line. It worked out here
because Red's army was elsewhere. Against a defended base that is a dead Champion.

## Vocabulary used

`plan_set` (boomer / army / rax2 / farms — 4 plans across the match, 2 live at a
time), `plan_clear`, `trigger_set` (supply-valve, home-guard, doorbell,
approach-watch, hero-save, militia, commit), `trigger_clear`, `region_set`
(`home`, `approach`), `template`, `squad`, `posture` (defend/push), `priority`,
`retreat`, `autocast`, `harvest`, `build`, `train`, `upgrade`, `cast`
(CallToArms), `move`, `ready`. Regions paid off exactly as advertised: three
different triggers said `defends home` and I re-aimed all of them by moving one
circle. I did not use `intent_compile.py` — the JSON was faster once I had the ids.

## Top 3 complaints

1. **Farm siting is a footgun and the supply valve makes it repeat.** Five separate
   `site blocked for Farm — needs 4x4 clear ... nearest legal: (-76,-96)` errors,
   each costing a poll to re-aim the trigger by hand. The error already computes the
   nearest legal site — the valve should be allowed to take it (`"nearest_legal":true`),
   or `build` should accept a region and pick a free spot inside it.
2. **Completed plans keep occupying a plan slot.** `plan farms` was silently rejected
   because `boomer` sat there with `status: done`. Nothing in `events` or `errors`
   said "plan cap reached" — the command just vanished, and I only found it by dumping
   `plans` myself. Either auto-retire `done` plans or reject with a reason.
3. **Push squads do not actually keep heroes with the line.** The brief promises "a
   strung-out Push squad gathers before pressing on, so slow units set the pace" —
   in practice my two heroes were 30+ units ahead of 21 footmen and archers for the
   entire approach. A `posture push` option like `"cohesion":true`, or heroes honouring
   the squad's slowest member, would remove the only micro I had to do all match.

Minor: `supply_used` went to 62/58 — over cap — while production kept running, which
made the `supply_capped` predicate a lagging rather than leading indicator.
