# ARENA r12 — RED (Rusher) After-Action Report

**Result: RED wins at t=567s (9:27). `game_over_reason: surrender` — BLUE conceded with their
Castle at 372/2200 hp, their Barracks, Workshop, Sanctum and all three towers already razed.**

Series: Rusher 7, Boomer 3, 1 draw.

## Length vs the 10-20 min target
9:27 — again *below* the 10-minute floor, same as R10 (9:21). Two commander rounds in a row have
ended inside 10 minutes. The map's economics push this: main mines were dry (~490g left) by 7:00,
so whoever has an army at minute 8 simply takes the game.

## Lumber allocation — the round's assignment
I managed it deliberately and it worked, but the constraint *moved*.

| time | lumber workers | note |
|---|---|---|
| 1:03 | 1 of 5 | opening split 3 gold / 1 lumber / 1 builder |
| 1:20 | 3 of 7 | pushed hard into lumber early on purpose |
| 3:05 | 3 of 10 | lumber peaked at **710** while gold sat at 620 — I over-invested |
| 3:45 | 2 of 12 | pulled a cutter back to gold; lumber was ahead of plan |
| 8:06 | 4 of 16 | re-loaded lumber once Knights/Catapults started drawing it |

Ladder actually funded: Barracks 60 + Farm×10 200 + Keep 160 + Barracks#2 60 + Sanctum 130 +
Expansion TownHall 205 + Workshop 100 + Castle 240 + army lumber (7 archers ×30, 2 sorcerers ×45,
5 knights ×60, 2 heroes ×100) ≈ **1900 lumber earned and spent.**

**T3 arrived at 7:02** (Keep started 4:05, done 4:45; Castle started 6:12, done 7:02) — comfortably
inside a 10-20 min game, and inside a *9-minute* one. So the hypothesis' first half is answered:
yes, T3 fits, and cheaply, if you put 3 of your first 7 workers on trees and never stop.

The R10 failure mode did **not** repeat in kind — but it repeated in spirit: I ended with
**909 gold and 245 lumber banked** and a Catapult still in the queue. Gold, not lumber, was the
idle resource this time. Lumber discipline moved the bottleneck rather than removing it.

## Boom-into-tech vs tempo
BLUE played the boom line and got *further* than me: they had a **Castle at 6:38** (first sighting),
a Blacksmith, a Sanctum and three towers. And they lost, because at 6:38 their standing army was
4 Spearmen, 2 Archers, 2 Sorcerers — roughly 8 supply of fighters against my 16. My first push
(6:12) traded 5 Footmen into their towers and *deleted their entire field army*. From t=410 to the
end BLUE never showed another armed unit. Their main mine died at 8:06 and they never expanded.

The honest rusher answer to the round's open question: **tempo still beats tech — but the correct
rusher line is not "stay at T2."** Taking the Castle myself cost me nothing I needed, because my
first push had already broken them and the gold was piling up anyway; the Knights it bought
(4 of them, 350hp each) are exactly what walks through a tower line that Footmen die to. The
sequencing that won was *push at T2 timing, tech with the gold the push frees up.*

## Targeted Slow
**Barely mattered, and I cannot claim it did.** Two Sorcerers, autocast at min_enemies 2, both
trained at 5:46 — *after* the fight that decided the game. They never had a clump to hit: BLUE's
army was already dead when they reached the field, and buildings do not care about Slow. I issued
zero manual `cast Slow` commands. The new point-cast is untested by this round.

## Timeline
- 0:52 first snapshot (I woke with 5 idle workers — lost ~50s of harvest before my first batch)
- 1:03 harvest ordered, Barracks placed
- 2:05 first Footman; 3:57 Hero
- 4:05 Keep started → **4:45 Keep (T2)**
- 4:56 Sanctum; 5:11 Barracks #2
- 5:46 expansion TownHall at (62,-52) beside the 5000g mine at (60,-60) — the single best decision of the game
- 6:12 **Castle started + first push launched simultaneously** (16 units, forage→push)
- 6:38 first contact: BLUE already at Castle, 3 towers, 8 supply of army
- 6:44-6:54 lost 5 Footmen to towers; killed their whole field army
- **7:02 Castle (T3)**; Knights start
- 7:27/8:32/8:52 bounty caches claimed (+315, +360, +405 untaxed — ~1080g, a third of my spending)
- 9:05 second push (20 units, 4 Knights, Champion Lv3 + Priestess) razes all 3 towers, Workshop, Barracks, Sanctum
- **9:27 BLUE surrenders**

## Top 3 complaints
1. **I started the match blind and idle at t=52.** Five workers sat doing nothing for the first
   52 seconds of game time before my seat's first snapshot was actionable. In a 9-minute game
   that is 10% of the match handed away, and neither commander can control it.
2. **`push` posture strings out badly and my hero out-ran it.** At 6:44 the Champion was alone at
   (-50,-60) tanking three towers at 63/320 hp while the squad centroid was 45 units behind at
   (-16,-20). The brief promises push gathers cohesively; a 10.5-speed hero with free orders
   plainly does not wait. I had to hand-set a 50% retreat to save it.
3. **Gold has nowhere to go fast enough.** I ended +909 gold with both Barracks and the Workshop
   short of queue capacity relative to my income, because unit *lumber* and unit *supply* both
   gate before gold does. R10 banked 1539 idle gold, R12 banked 909 — the sink is still missing.
   Also: two placement rejections (Sanctum, expansion TownHall) — the nearest-legal-site hint is
   genuinely good and both were fixed in one cycle, which is the one thing I have no complaint about.
