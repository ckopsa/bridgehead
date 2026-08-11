# Arena Round 19 — RED (Rusher creed, Kingdom) — After-Action Report

**Result: LOSS.** Surrendered at t=361s (6:01) with 17 enemy units in my base,
zero army, zero standing Barracks at the time the decision was made, and my main
mine at 230 gold. Blue (boomer creed, Kingdom mirror) wins. Standings become
Rusher 7, Boomer 5, 1 draw.

## The hero decision: NO HERO. Why.

I never queued one, and I would make the same call again for the *creed*, though
not for *this game*.

The rusher's whole thesis is that 400g/100l spent at 2:00 buys three Footmen who
are on the map now, and a Champion is 25 seconds of training plus 5 supply for
one body that cannot be in two places. My win condition was a T2-less, tech-less
timing hit at ~2:40 — a hero does not make that hit land earlier, and it makes it
smaller by two units.

What I got wrong is downstream of the hero question, not inside it: **hero-less is
a bet on your first punch connecting.** Mine did not, and hero-less armies have no
second act — no Slam to break a clump, no free-orders command bubble at the front
(latency is real: my "stop, fall back to staging" order at t=183 never reached the
push before it died at t=184-199), and no revivable anchor to rebuild around. Blue
also fielded no hero and won, so the round says nothing about heroes being weak;
it says the hero-less mirror is decided by *mass and composition*, and I lost both.

## How it unfolded

- **0:22** One `plan_set` ran the entire opening in 2 seconds of game time: 3 workers
  to gold, 2 to lumber, **two Barracks**, four Workers queued. Both Barracks finished
  by 0:59. This part was excellent and is the single best thing in the round.
- **1:14** Supply valve (recipe 7) fired for the first time and kept firing all game,
  unattended, through four Farms. It never once cost me a poll after arming.
- **2:41** `push-at-10` (armed as `unit_count Archer >= 6`) launched squad 1 — 10 units,
  7 Archers / 3 Footmen — at their base.
- **3:01-3:19** The push was annihilated in ~16 seconds at (-50,-50) against exactly
  8 defenders (5 Archer, 3 Footman) on their ground. I killed no units, only chipped
  two Barracks. **Wrong composition:** 7 archers behind 3 footmen is not a line, it is
  a pile of 70-hp bodies; their 3 Footmen tanked and their 5 Archers shot through mine.
- **3:20-4:50** I rebuilt to 11 units, took the Keep, put up a Tower and Blacksmith,
  and set an expansion plan. Blue used exactly that window to mass **13 archers**.
- **4:53-5:20** Blue's 13 hit my base. Squad 1 wiped, all three Barracks razed, Tower
  razed. I fired **Call to Arms** off the Keep; the militia plus the tower traded that
  wave down from 13 to 2 — the best fight I had all game and the reason I was still
  alive at 5:40.
- **5:40** Blue's second wave arrived at 17 (14 Archer, 3 Footman) off two Barracks and
  a fresh expansion at the northwest mine, into my 8 workers and one half-built Barracks.
  Call to Arms was on cooldown. I surrendered rather than farm the replay.

## Vocabulary usage

Used: `plan_set` (4 plans: opening, army x2, tech, expand), `trigger_set` (6 distinct
rules: supply valve, home-guard, their-hero-down, push-at-10, push-big), `region_set`
(home / strike / staging / expo), `template` on every Barracks (squad 1 + retreat 30%
+ priority Archer>Siege>Worker), `posture` push/defend, `cast` (Call to Arms), `cancel`,
`upgrade`, `surrender`.

What actually earned its keep: **the opening plan** (six polls compressed into one),
**the supply valve** (I was never supply-blocked after 1:14 despite never thinking about
it again), and **Call to Arms** (a free 40-second army that won a 13-v-0 fight down to
13-v-2). What did not: `their-hero-down` never fired because nobody built a hero, and
`push-at-10` fired *correctly* on a condition I had chosen badly.

## What decided it

Composition, not tempo. Both sides built two Barracks and no hero; the difference is
Blue built a **footman-fronted archer mass and pressed once at 13**, while I built an
**archer-heavy blob and pressed twice, at 10 and at nothing**. I also aimed a trigger at
a unit count instead of an army ratio — `unit_count Archer >= 6` is not "I am stronger
than them", and the engine did exactly what I asked at exactly the wrong moment.

## Top 3 complaints

1. **`template`'s `retreat` clause silently rejects `region`, while `posture`, `leash`
   and standalone `retreat` all accept it.** Two of my three templates were refused with
   `unrecognized command (missing field x)` and my first army spawned with no retreat
   policy at all. The one place doctrine is *stamped* is the place regions do not reach.
2. **`research` needs an undocumented `upgrade` field.** The brief's command reference
   never lists `research` at all; I inferred `{"type":"research","building":id,"id":"attack"}`
   from the catalog and got `missing field 'upgrade'` — inside a plan, which then halted.
   A Blacksmith I paid 140/80 for never researched anything.
3. **A push posture has no abort.** When my 10 units walked into 8 defenders, my
   `posture defend @staging` order took ~15 seconds to change their behaviour — longer
   than the entire fight. `posture` is documented as free and instant, but the squad's
   members were already in combat and died before re-tasking mattered. There is no
   "disengage" verb, and a leash set after contact does not save anyone.

## Honest note

Two of my three complaints are things I could have discovered in a dry run and did not.
The loss is mine: I sent a 7-archer/3-footman ball into a defended base because my creed
says commit, and commitment without a front line is just delivery.
