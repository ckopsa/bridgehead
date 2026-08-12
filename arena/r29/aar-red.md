# Arena Round 29 - RED Faction After-Action Report

## Match Result
**LOSS** via surrender at t=292s. Human faction victory.

## Part 1: Match Chronicle

### Opening Phase (t=0-50s)
I executed an aggressive opening plan with 5 initial workers on a single mine (the southeast mine at 82,55) plus lumber harvesting. I issued multiple train commands to build workers immediately and queued Barracks, Farm, and Tower construction. The opening was sound in theory - workers on resources, unit production building.

However, I made a critical error at t=64s: my harvest command moved ALL workers off the mine toward the middle of the map, triggering an **income_collapse** alarm. No workers were on gold, destroying my cash flow for ~10 seconds. This cost me approximately 50-100g that could have been reinvested. By t=74s when I recovered the workers, the economic damage was done.

### Mid-Game Contraction (t=50-200s)
Stuck in a growth ceiling. With only 8 workers, my harvesting rate could not generate the 320g + 160l needed for a Keep upgrade. I trained 2 Footmen and 1 Archer (cost 360g total) but this was a mistake - I spent my meager resources on units I couldn't sustain rather than on economy acceleration. At t=78s, my plan halted when the Keep upgrade was blocked by insufficient resources. This plan never recovered; the 60-second retry window expired and the system gave up.

By t=146s, I had reversed my earlier mistake: only 45g, needing 275 more to keep progressing. But my single mine operation meant glacial income (~5-10g per cycle). I was economically stalled. Meanwhile, the opponent was presumably building to tier 2, expanding, and fielding a stronger army.

### Battle Phase (t=242-292s)
The enemy attacked at t~246 with approximately 5 units (3 Footman, 2 Archer). My defensive doctrine engaged: squad 0 defended at the base in turtle formation. But the force composition was deceptive - the enemy had already been probing and accumulating strength.

Within 8 seconds, the enemy revealed **9 units including a Hero** (4 Archer, 4 Footman, 1 Hero). The Hero is a game-changing unit: much higher stats than basic troops and a powerful ability. My 4 units could not contest this. Losses cascaded:
- t=248s: My Farm destroyed, TownHall takes damage
- t=256s: My Barracks destroyed (killing my ability to train ground units)
- t=260-276s: My remaining Footmen and Archers killed
- t=272-280s: Enemy units hunted my workers, killing 4 in rapid succession

By t=280s, my squad 0 was wiped. I had 0 army, 4 workers (and declining), and a Barracks under construction (too late). The enemy pressed relentlessly, killing workers faster than I could recover. At t=292s, my TownHall was at 444/1200 hp under sustained attack. Surrender was the merciful end.

## Part 2: Protocol Analysis - The Wire as a Command Tool

### What Worked
1. **Selectors were powerful**: `"select":"all army"`, `"select":"idle workers"`, and `"select":"my barracks"` allowed me to avoid tracking unit IDs across the match. When my Footman died, I didn't have to chase its corpse ID.

2. **Triggers and plans were well-designed**: My triggers (`home-guard`, `supply-capped`) executed correctly when fired. The trigger system correctly identified when conditions were met and submitted orders at sim-tick speed.

3. **Digestible information display**: The `--digest` flag showed resources, army composition, production queue, and the single-sentence "default if you do nothing" all on one screen. Useful for fast decision-making.

4. **Clear error messages**: When commands failed, the system told me why. "Tower requires Barracks" (instead of silently refusing) taught me the tech tree. "Cannot afford Keep" was unambiguous.

### What Was Hard

1. **Economy feedback loop was opaque**: I had no way to directly observe individual worker harvest rates or project forward how long until I'd afford an upgrade. I sent workers to mine but couldn't see "currently 3 on gold, yielding ~7g/s". I had to infer from resource deltas across 15-second polls. This made planning difficult.

2. **No visibility into opponent economy until combat**: The state shows "`explored 0% of the map`" and "`ENEMY buildings 0: none seen yet`" until I scouted. I had no way to estimate opponent army size, tech level, or expansion status until their units appeared. This asymmetry meant I was always reacting, not preparing.

3. **Build command ambiguity**: The brief says "You pay when the worker breaks ground, not when accepted." I issued a build command and waited for the resource hit. But when a build was abandoned (worker re-tasked, site blocked), I got a quiet status message in events rather than immediate feedback. I almost didn't notice my Barracks had finished building because there's no blip in the production queue - it just transitions from "building" to "idle".

4. **Supply management was reactive**: I hit the supply cap (14/16) with 2 Footmen in queue. My trigger tried to build a Farm to recover, but no idle workers existed to build it. I'd already over-committed my economy. A "projected supply if all queue items finish" would have let me plan ahead.

5. **No way to retarget committed workers mid-action**: Once a worker was en route to a build site, I couldn't cancel it without watching for the "build abandoned" message. This cost me time and resources during the income-collapse incident.

6. **Plan step sequencing was rigid**: My plan was blocked on "Keep upgrade" for 60 seconds, during which nothing else could run. I couldn't insert a "train workers while we wait for gold" step conditionally. The plan is sequential only; no loops, no branches. For simple build orders this is fine, but complex strategies need more flexibility.

### What I Wish Existed

1. **Predicted resource graph**: A one-sentence readout like "at current rate (8g/sec) you'll afford Keep in 20s" would let me time my decisions.

2. **Enemy intelligence**: A summary of "enemy explored X%, owns Y buildings (last seen Z seconds ago), last spotted heading DIRECTION" would help predict attacks.

3. **Alternative plan syntax**: A way to express "try to upgrade, but if you can't, train workers instead" without creating two separate plans.

4. **Build cancellation**: `{"type":"cancel_build","worker":id}` to yank back a worker who's en route, refunding nothing (since nothing was paid yet).

5. **Worker efficiency display**: Show me actual harvest rates per mine and per lumber site so I can reason about "how many workers do I need to hit 320g by t=300?"

### Protocol Failures I Hit

1. **Early income collapse** (t=64s): My `move` command to send idle workers to mid fired incorrectly, moving ALL workers off gold. This was my command error, but the system did warn me when it happened.

2. **Plan halting** (t=76s): My plan tried to upgrade Keep every 5 seconds for 60 seconds before giving up. That grace window was correct (I needed time for gold to arrive), but I should have monitored `plans[].status` more actively to pre-empt the halt.

3. **Squad assignment confusion**: I assigned squads 1 and 2 but neither had units. When I tried to move squad 0, it succeeded but the squad command to explicitly enroll units in a new squad should have been prioritized.

### Honest Assessment

I lost because:
1. **Economic mismanagement**: Single-mine economy meant stunted growth. I should have expanded to a second TownHall at t~120s when I had 300+ workers-worth of saved labor.
2. **Misallocated military spending**: 360g on 2 Footmen + 1 Archer when I should have saved for Keep tech.
3. **Late scouting**: I didn't scout actively until t~180s. By then, the opponent had a Hero and superior unit count.
4. **Wrong doctrine response**: My `home-guard` trigger fired correctly but arrived too late to prevent building losses. The trigger needed to fire at t~240 when the first probe appeared, not after the army was already engaged.

The opponent was disciplined: they built a strong economy, upgraded to tier 2, fielded a hero, and executed a coordinated multi-pronged attack (buildings AND workers). They won because they out-economied and out-fought me.

The protocol itself was fair and clear. My failures were strategic and tactical, not because the wire was unintelligible.

