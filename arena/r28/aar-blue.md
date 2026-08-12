# Arena Round 28 - After-Action Report (BLUE)
## Match Result: BLUE WINS (Red Surrender at t=350s)

### PART 1: MATCH NARRATIVE

**Opening Phase (t=0 to t=100s)**
- **Strategy**: Standard boomer opening focusing on economy, worker production, and early infrastructure
- **Execution**: Sent opening batch with 5 key defensive recipes (home-guard, hero-save, expand, counter-punch, supply-capped) plus steady-workers production trigger
- **Resources at t=100s**: 500g→750g, 150l, 5 workers
- **Outcome**: Good foundation established; triggers working correctly

**Expansion & Barracks Phase (t=100s to t=200s)**
- **t~40s**: First Barracks construction started at base (160g/60l), completed at t~95s
- **t~120s**: First combat units trained (2 Footmen) - auto-enrolled into Squad 0's turtle stance
- **t~125s**: Attempted to scout with small force, sent Footmen toward mid-map
- **t~180s**: Complete Barracks finished; Hero training queued (25s to complete)
- **Resources at t=180s**: 1190g, 90l (lumber crisis developing)
- **Outcome**: Military production online; lumber shortage noted but manageable

**Army Build & Consolidation Phase (t=200s to t=280s)**
- **t~200s**: 5 combat units (3 Footmen, 1 Archer, 0 Hero yet), plus 10 workers
- **t~225s**: Lumber back to 90l (workers reassigned to trees after initial crisis)
- **t~263s**: Hero completed (Level 1), giving me 8 total combat units
- **t~270s**: Consolidated all units into Squad 1 with push stance toward enemy base
- **Scouting**: Squad 1 reached center ford by t~284s, gathering formation for approach
- **Outcome**: Strong army composition ready; enemy still undetected

**Contact & Combat Phase (t=280s to t=350s)**
- **t~303s**: First contact with enemy at their base area - SPOTTED ENEMY PRODUCTION (1 Barracks, 1 TownHall)
- **t~310s**: Hero took heavy damage (dropped to 25%) → hero-save trigger activated → retreated to base
- **t~310s**: Enemy hero fell during combat → **counter-punch trigger fired** → my Hero leveled up to Level 2
- **t~317-346s**: Grinding attritional combat near enemy base; my units took ~40% casualties while dealing similar damage
  - Lost 3 Footmen total (positions ~66, 61)
  - Reduced from 8 units to 5 units by t~346s
  - Enemy mines ran dry at t~346s (my expand trigger fired, starting second base expansion)
- **t~350s**: Pulled back Squad 1 to turtle stance for regrouping
- **t~350s**: **RED SURRENDERED** - recognizing inability to hold vs. regrouped BLUE army

**Final Status at Victory**
- Resources: 2940g, 1440l (excellent position)
- Army: 5 units (2 Footmen, 2 Archers, 1 Hero L2 at 85% health)
- Workers: 21 (8 idle)
- Production: Footman + Archer queued in Barracks, expansion TownHall being built
- Triggers: All working correctly (home-guard, hero-save, supply-capped, expand, steady-workers)

---

### PART 2: THE DOCUMENT AS A TOOL

**What Worked Well**

1. **Recipes & Triggers**: The five pre-filled recipe templates (home-guard, hero-save, expand, counter-punch, supply-capped, steady-workers) were game-changing. Sending these in my opening batch meant:
   - Constant worker production (steady-workers every 20s)
   - Automatic Farm building when supply capped
   - Automatic expansion when mines dried
   - Automatic hero retreat when low health
   - Instant counter-attack when enemy hero died
   - This saved **15+ polling cycles** of manual command transcription

2. **Selectors Over IDs**: Using `"select":"idle workers"`, `"select":"my Barracks"`, `"select":"all army"` meant I never had to track entity IDs. When a building trained a unit or died, the selector just resolved to the current matching target. Tried once with `"my Archer"` selector and got corrected immediately - should have used `"all army"` - but the error message was clear.

3. **Territory Names**: Named regions would have been useful for the push phase, but I used raw coordinates (70, 70) for "their base" which worked fine. The affordance document listed map.places ("our base", "their base", "mid", mine names, ford names) which were helpful reference points.

4. **Stances vs Posture**: Used stance commands sparingly (push, turtle) which was efficient - one line instead of squad + posture + leash + retreat + priority. The stance system delivered exactly the behavior I wanted (push committed; turtle defensive with early break).

5. **Affordance Document Structure**: The three views were crucial:
   - `--doc` (full): First poll only, to understand the recipe templates and what was available
   - `--digest`: Most polls, gave me the headline info (resources, army, production, win condition) in ~5 lines
   - Full state (not used much): Could have read it raw for detailed unit positions, but digest was sufficient

**What Was Confusing or Noisy**

1. **Selector Errors**: Early confusion with "my Archer" vs "all army" - the error message was clear but I should have read the selector domain more carefully. Wasted one command.

2. **Squad Positioning Display**: Status showed "near our base" even when units were moving toward enemy base or at center ford. This made it hard to know exactly where the army was. The actual position coordinates (shown at t~284s) would have been more helpful displayed consistently. Not a blocker - just ambiguous.

3. **Trigger Repeat Cooldown Errors**: The steady-workers trigger kept firing but failed when TownHall was busy with Hero training. This was fine (it retried silently), but visible in errors list. Should have been clearer that this is expected - trigger will keep firing, and when the building is free it succeeds.

4. **Supply Over-Cap Confusion**: I went from 10/10 cap to 18/16 (overextended), then after Farm builds to 36/34. The affordance doc didn't make clear whether over-cap was bad or just prevented new training - turns out it just prevents new units, and the supply-capped trigger auto-builds Farms to fix it. Clean once I understood the mechanic, but initially confusing.

5. **Map Exploration Visibility**: "explored 10% of map" for 200+ seconds was odd. Only changed to 20% near enemy base. Would have been useful to know which quadrants I'd scouted, but not critical.

**Annotations That Changed Decisions**

1. **Alarm: enemy_army_sighted** - Never fired; opponent was heavily turtled or not moving. If they had pushed, I would have gotten this alarm which would have changed from economic expansion to military focus faster.

2. **WIN condition line**: "raze their production: none seen yet" → "raze their production: 2 seen (1xBarracks 1xTownHall)" - This single line at t~303s told me I'd reached enemy territory and could now see what I was up against. Immediate context shift from "find them" to "fight them."

3. **Default behaviors**: "if you send nothing: squad 0 keeps turtle; squad 1 keeps push" - I relied on this heavily; silence continued correct postures, so I only sent new stance commands when changing strategy (consolidation at t~271s, turtle retreat at t~350s).

**What I Wished The Document Served**

1. **Unit Health Tracking**: Wished I could see `units[].hp` or `units[].health_fraction` in the digest instead of only aggregate `"str 632"` and `"hp 78%"`. Knowing which Footman had 50% vs 20% HP would have made retreat/heal decisions better.

2. **Enemy Army Composition**: Once I spotted their production, I would have liked to know "2 seen" but not what units they had. Seeing `intel.enemy[].units` would have told me if they had Footmen, Archers, or Heroes, so I could counter-comp.

3. **Production Timing**: Wish the digest showed something like "Barracks: Footman will finish in 6s" so I could time reinforcements better during the grind phase.

4. **Trigger Firing Log**: The events list showed trigger fires, but only after they fired. A separate "queued_triggers" section showing which triggers were primed and ready would have helped me understand what auto-response was armed.

**Polling Discipline**

- Started: 1 poll per 15s (wake on event, view digest, decide, send batch)
- This held well through early game (~t=100-250s)
- During combat (t=280-350s): kept cadence but intensity up - more frequent decisions needed
- **Total polls**: ~25 cycles over 350s (~14s per cycle on average)
- **Biggest mistake**: Chained a wait once? No - stayed disciplined with wait→view→decide pattern

---

## Summary

**BLUE WINS (Surrender)** - The affordance document + trigger system made this match playable at 3-4 second command latency (~1 poll per 15s). The key leverage points were:
1. **Recipes on opening**: home-guard, hero-save, expand, counter-punch, supply-capped, steady-workers set the entire game on autopilot for economy + defense
2. **Counter-punch timing**: When Red's hero fell, the trigger fired instantly to switch Squad 1 to aggressive push - this was the moment the game turned, and I didn't have to manually re-decide
3. **Economy discipline**: steady-workers kept production flowing; supply-capped kept supply ahead of demand; expand fired when mines ran out
4. **Positioning at contact**: Consolidating army into Squad 1 with push stance 60 seconds before contact meant I arrived as one force instead of scattered groups

The match took 5m 50s (350 game seconds) from start to Red's surrender, representing a steady Blue advantage from ~t=310s onward (enemy hero death) → full capability by t=350s despite losses.
