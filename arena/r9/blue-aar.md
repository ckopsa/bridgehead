# Round 9 — BLUE (Boomer creed) After-Action Report

**Result: LOSS by surrender at t=325s (~5:26).** Opponent held the field; my army
was wiped, hero killed, Barracks razed, no income path remaining.

## Timeline of key decisions
- **t=2-45s**: Standard boomer open — 5 workers split 3 gold / 1-2 lumber, Barracks at
  (-59,-67), Farm, continuous worker production to 12.
- **t=65s**: Squad 1 created with a production template (retreat 30%, focus Siege/Archer),
  defending my base shoulder.
- **t=131-152s**: Banked ~825g. Started **Keep upgrade** (done t=192) and a Workshop.
  Sent a worker across the map to expand onto the **southeast ford mine** (60,-60) —
  the ford on "my" side of the diagonal river.
- **t=180s**: Expansion TownHall placed at (50,-64) (first site at (56,-56) was rejected
  as blocked — mine footprint). Squad 1 (6-7 units) repositioned to garrison it.
- **t=215-237s**: Bounty spawned mid; sent the lone Raider foraging. Then fog broke:
  an enemy army of **10 (5 Footman, 4 Archer, hero)** was already at (53,-43), i.e.
  through the SE ford, and they had **expanded to (68,-52)** — their hall was closer
  to my expansion than my main was.
- **t=237-263s**: My 7-unit garrison was destroyed piecemeal at the expansion; 6-7 workers
  and the expansion TownHall died with it. My hero arrived from the main base *after*
  the fight (it spawned at t~220 at home, 130 map-units away) and was chased off at 11% HP.
- **t=284-300s**: Retreated everything home, started a Tower, tried Call to Arms —
  the enemy caught the retreat in the open, killed the hero and wiped squad 1 at my base.
- **t=315-323s**: Barracks razed. Left with 5 workers, 90 gold, a home mine holding 270
  gold, no army, no hero, enemy 8-strong inside my base. **Surrendered.**

## What actually killed me
One decision: I put a **remote expansion 130 units from my main, on the far side of the
map from my production, and garrisoned it with a 7-unit squad while my hero was still in
the training queue.** That split my army from my base and my hero from my army. Under
fog I had *zero* information about the enemy's position when I made that commitment —
I had scouted nothing at all by minute 3, which is the real error. The enemy, meanwhile,
had expanded aggressively toward the same ford and simply arrived first with everything.

Secondary: my home mine (3500) was down to ~700 by t=230 with 12 workers on it. The
economy clock is much faster than the fortify-and-tech clock I was playing to.

## New content used, and whether it mattered
- **Tier ladder**: took the Keep (t=192). It bought a second hero slot I never used and
  gated Sanctum/Blacksmith, both of which I queued and never finished. **Did not matter** —
  I died before tier 2 paid off. Getting the Keep at t~150 with only 6 army units standing
  was, in hindsight, the Boomer trap the pacing patch is designed to punish.
- **Sorcerers / Slow**: never fielded. The Sanctum build was cancelled by lack of gold.
- **Knights / Gryphons / Blacksmith research**: never reached.
- **Fords**: I fought at the SE ford but I did **not** fortify it — no tower or wall was
  ever finished there. So this game says nothing about triple-ford wall-off. What it does
  say: **a ford does not defend itself.** The choke funnelled the enemy straight into my
  undefended expansion; terrain rewarded the side that got there with more units.
- **Items/shop**: never built a Shop. In retrospect a TownPortal on the hero would have
  saved the expansion garrison, or at least the hero.
- **Doctrine**: squads/templates/retreat/priority worked as advertised and are clearly the
  right interface — one `template` on the Barracks meant every new unit joined squad 1
  with a retreat rule. `intent_compile.py` compiled "retreat at 35%, autocast at 3,
  focus siege" correctly first try.

## What the fog changed
Everything, and it beat me. Round-8 habits assume you can read the enemy army off the
snapshot. Here I had `explored: 0.1` at minute 2 and never raised it — and I still made a
major territorial commitment. The empty `units` list read like safety. **Fog makes scouting
a build-order item, not a luxury**, and the Raider (vision 24) should have been my third or
fourth unit, not my ninth. I never saw the enemy until they were in contact.

## Pacing feel
Fast — much faster than the creed's line supports. Mines at 3500 with 10-12 workers means
your main is dry around minute 4-5, which forces an expansion **at the same time** the
opponent's first real army is on the map. There is no quiet boom window. Tier 2 at ~3:15
felt on-schedule per the brief and still left me with 6 fighting units against 10.

## Top 3 balance/design complaints
1. **Catalog/engine disagreement on the Raider.** `catalog.json` lists `Raider` under
   `Barracks.trains`, but `train` at the Barracks was rejected with *"Raider requires
   Workshop"*, and training it at the Workshop was rejected with *"Workshop cannot train
   Raider"*. It only worked at the Barracks after a Workshop was standing. The catalog is
   documented as the authoritative tech tree; here the gate is invisible in `requires`.
   This directly cost me my scout timing.
2. **Expansions are too punishing relative to their cost on a fog map.** 385g/205l plus a
   worker walk of ~130 units, and it can be erased by an army you had no way to see, with
   every worker in it. Combined with 3500 mines the game demands an expansion at ~min 4 and
   then makes it near-indefensible for the boomer. A cheaper hall, a slower mine drain, or
   vision on the neutral fords would all fix the same problem.
3. **Building placement rejections are opaque.** `site (56.0,-56.0) is blocked for TownHall`
   with no indication of the required clearance from a mine, and no suggested legal site.
   With command latency and travel time, a bounced placement costs 20+ seconds of a
   worker's life. A `min_distance` field in the catalog, or a nearest-legal-site hint in the
   error, would remove pure guesswork.

## Honest self-assessment
This was not a loss to new content; it was a loss to old-fashioned overextension made fatal
by fog. The creed's "tower-anchored defense, decisive consolidated push" line was never
executed — I never built the towers and I never consolidated. Series stands at Rusher 5,
Boomer 3, 1 draw.
