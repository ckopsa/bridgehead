# Arena Round 29 - BLUE (Human) vs RED (Claude) After Action Report

## Match Result
**VICTORY** - RED surrendered at t=292s (4:52 match time)

## Match Overview: Opening to Victory

### Phase 1: Economy Setup (t=0 to t=90s)
- Started with 5 workers, TownHall, 500g/150l
- Opening batch: Immediately harvested (3 workers to gold, 2 to wood)
- Built Barracks at t~25s (160g/60l)
- Trained 2 initial workers and queued Hero
- Critical Issue: Income collapse at t=158s when all workers went off gold during farm building
- Recovery: Redirected workers back to mining, income restored by t=166s

**Key Decision**: Aggressive farm building to expand supply almost cost the match - learned to maintain gold income priority.

### Phase 2: Army Formation (t=90 to t=180s)
- Hero finally trained at t=130s (was delayed by worker queue management)
- Army reached 8 units by t=202s (4 Footman, 3 Archer, 1 Hero)
- Supply management: Hit cap at 28/28, built multiple farms to expand
- Resources: Recovered to 445g/470l by t=212s despite income collapse
- Organized army into Squad 1 with push stance toward enemy base

**Key Decision**: Prioritized farm building over perfect timing; once Hero existed, immediately used it to scout.

### Phase 3: First Contact & Combat (t=220 to t=275s)
- **t=244s**: Enemy spotted (~4 units: 3 Footman, 1 Archer) near their base
- **t=247s**: Hero-save trigger fired as my hero took heavy damage (to 17% HP) in early skirmish
- Enemy army defeated; my squad 1 reduced to 9 units at 66% HP
- **t=260s**: Bounty spawned at mid (-7, -9); claimed +270g
- **t=264-274s**: Destroyed enemy Barracks; lost 2 Footmen in continued fighting
- My army position: (5, 43) to (43, 48), pushing toward their base
- Hero recovered to 36% then 62% HP as it retreated to safety

**Key Decision**: Pushed aggressive advantage immediately after defeating enemy army; destroyed their production before they rebuilt. This proved decisive.

### Phase 4: Victory (t=275 to t=292s)
- Enemy surrendered after seeing:
  - Destroyed Barracks production building
  - My army pressing toward their base
  - Resource disadvantage (1580g vs their minimal economy)
  - No path to rebuild army production in time

**Final Army Composition**: 10 units (7 Archer, 2 Footman, 1 Hero L1)

---

## Key Decisions & Strategy

1. **Economy Priority**: Harvesting > everything else. Income collapse nearly cost me despite army advantage.
2. **Farm Placement**: Manual placement required because base site was blocked; auto-trigger kept trying base location.
3. **Hero Scouting**: Sent hero to enemy base early; revealed enemy position and army size.
4. **Numerical Advantage**: With 10 vs 4 army units, pushed immediately to capitalize on superiority.
5. **Bounty Play**: Lucky timing - spawned during my army's push, gave 270g swing.
6. **Archer Focus**: Trained 7 Archers by endgame (superior ranged unit for composition).
7. **Production Focus**: One Barracks was sufficient for the match duration; early Barracks destruction limited enemy rebuild.

---

## Part 2: The Protocol as a Tool

### What Worked Well
1. **Triggers**: Hero-save and home-guard triggers provided automatic protection without constant monitoring. Supply-valve trigger, though imperfect, kept farms coming despite base site issues.
2. **Stance Doctrine**: Setting `push` stance on Squad 1 kept army moving cohesively toward objective - no need to micromanage movement every cycle.
3. **Selectors**: Using `select: "idle workers"` and `select: "my hero"` simplified commands and avoided ID staleness.
4. **Regions**: Attempted to name regions (failed on built-in names), but could have used them effectively for defensive positioning if I'd used unique names.
5. **Plans**: Did not use plans, but could have used one for opening sequence to avoid manual re-issuing every cycle.

### What Was Hard
1. **Supply Management**: No automatic hard limit on training - had to manually build farms. The supply-valve trigger helped but couldn't place buildings at base (site blocked).
2. **Unit ID Tracking**: Building IDs are ephemeral; the first enemy Barracks destruction was lucky (units just happened to be attacking it). Tried hardcoded IDs later (failed). Would benefit from a "select enemy building" selector.
3. **Income Collapse Detection**: The alarm helped, but I'd already lost economic momentum. Needed earlier warning system or auto-redirect policy.
4. **Worker Dispatch**: No automatic "return and retask" when income is needed. Manually redirecting all workers was a single-cycle decision, but no safeguard.
5. **Map Fog**: Explored only 30% by end. Hero scouting was slow; wished for faster reconnaissance or cheaper scouts.
6. **Latency Between Cycles**: 15-second polling meant hero took significant damage before hero-save fired. Faster trigger eval (4Hz) helped, but LLM latency is the real bottleneck.

### Information I Struggled to Track
1. **Real-time income rate**: Knew I was at 0 gold briefly, but no "gold/sec" metric to predict when I'd afford next unit.
2. **Enemy economy**: Never saw their worker count, only their 2 remaining workers at end. No intel on their income strategy.
3. **Building progress**: Farm construction times, Barracks training queue status - had to infer from snapshots. A "predicted completion time" for each item would help.
4. **Map awareness**: Bounty spawned unexpectedly; had no prediction or "foraging" vision. Only units with vision saw it.

### Errors Hit and Fixed
1. **Region naming conflict** (cmds 2-7): Tried to re-name built-in places (our-base, mid, etc.). Fixed by not re-defining them.
2. **Squad assignment with no army** (cmd 11): Tried to move units to squad 1 before they existed. Resolved once units trained.
3. **Invalid target ID** (cmd 12): Guessed at enemy TownHall ID incorrectly. Had to look it up in the state after.
4. **Worker selector collision** (cmd 1, seq=8): Tried to send idle workers to build farms but they were already assigned. Learned that selectors resolve at command time, not send time.
5. **Hero damage not predicted**: Hero took 83% damage in early fight before save trigger fired. Trigger worked, but latency meant reactive, not preventive.

### Protocol Strengths
- **Stances**: Single word (push, turtle) encoded full doctrine (posture, leash, retreat, priority). Powerful abstraction.
- **Triggers with repeat**: 30-45s repeat timers meant I didn't have to poll for conditions like base_under_attack.
- **Affordance document** (`--doc` flag): Would have helped predict what I could do next; didn't use it but saw it was available.
- **Error messages**: Clear, actionable feedback (target not found, site blocked, selector empty).

### Wishlist for Future Matches
1. **Auto-rebuild farming**: Once I lose workers to death, auto-resume farming. State doesn't show "farming resumed" so had to re-send every cycle.
2. **Income prediction**: "At current rate, gold afforded at t+X sec" for planning.
3. **Enemy intel tracking**: Store observed enemy unit types/counts over time. Current snapshot is too volatile (observed 4 units then fight, then 2 workers).
4. **Selective target mode**: `{"type":"attack","select":"squad 1","target_select":"nearest enemy building"}` instead of hardcoded IDs.
5. **Bounty/forage automation**: "Squad X forage mid" instead of waiting for scout to stumble on it.

---

## Closing
This match was won on early army production, aggressive scouting, and capitalizing on numerical advantage. The income collapse was a scare that exposed the need for safeguards (maybe an auto-harvest trigger on the nearest mine if income stops). Overall, the protocol's trigger and stance system shone; the biggest friction was manual worker dispatch and reactive hero protection.
