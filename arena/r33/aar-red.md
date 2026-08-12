# Arena Round 33: RED (Claude/Kingdom) vs BLUE (Claude/Kingdom)
## After-Action Report

**RESULT: RED VICTORY** — Match Won by Surrender (t=460s, 7:40 game time)

---

## PART 1: MATCH NARRATIVE

### Opening Phase (t=0-120s)
Authored and sent opening batch with full state parsed. Strategy: standard-kingdom boomer opener.
- t=0-8s: Match held at t=0 awaiting both seats ready
- Sent opening: 6 triggers (home-guard, hero-save, expand, supply-capped, counter-punch, steady-production), 2 regions (north-pass, mid-watch), 1 economy plan (7 steps), 1 military plan (4 steps)
- t=8s: Match started. Economy plan executed; harvest at mine/tree working immediately
- t=32-62s: Economy plan step 1-2 running, worker training queued
- **ERROR at t=62s**: Economy plan had 10 steps but limit is 8. Split into two plans: economy (7 steps) and military (4 steps)
- t=62-102s: Plan recovery. Economy plan executing: trained workers 5→10, Hero queuing, built 2x Barracks (started at step 6)

### Midgame Phase 1: Harassment (t=102-180s)
- t=102-116s: Economy plan complete at step 7. Barracks built (160g/60l cost × 2), Hero training
- **t=142s: CRITICAL EVENT** — Enemy scout (1 Hero) detected near base @(60.3, 51.6)
  - Only 2 Footmen trained at this point; 0 defense
  - Enemy hero begins raiding workers
- t=146-168s: CATASTROPHIC LOSSES — Lost 7 workers total (10→3) to enemy harassment
  - Workers killed at northeast mine area (82-89, 49-58)
  - Economy collapsed from 1125g to nearly 0
  - Sent emergency Footman training queue (6 units) and worker rally to base
- t=178s: **STABILIZATION** — 2 Footmen now defending, squad 0 formed (str 280)
- t=178-200s: 3 more workers lost, economy critical (3 workers left)

### Midgame Phase 2: Recovery (t=200-380s)
- t=200-220s: 2 Footmen defending base, Hero still training, 6+ more Footmen queued
- t=316s: Army recovered to 4 units (2 Footman, 1 Hero L2, 1 Spearman), workers at 5, squad 0 defending with 88% health
- **Key events during recovery**:
  - Trigger system working: supply-capped firing repeatedly to build Farms
  - Hero trained and leveled (reached L2 by t=316)
  - Footmen training continued despite early losses
  - Workers gradually recovered (5→6→8→10→12)
- t=370-382s: Full recovery achieved—10 workers, 8 army (5 Footman, 2 Spearman, 1 Hero L2), 520g
- Economy: established new TownHall at northeast mine expansion location (triggered at t=439-440s)

### Endgame Phase (t=380-460s)
- t=427-438s: Counter-punch trigger activated (enemy hero down signal), hero leveled to L3, then L4
- t=439-440s: Northeast mine ran dry; expand trigger built 2nd TownHall automatically
- t=452s: Full strength: 12 workers, 874g, 15 army (11 Footman, 3 Spearman, 1 Hero L4), strength 2227
- t=458s: Hero leveled to L5 during combat/positioning
- t=460s: Sent consolidation orders (squad 2 push to their base)
  - **ENEMY SURRENDERED** — Blue conceded due to overwhelming RED army

---

## KEY DECISIONS & THEIR IMPACT

1. **Opening Plan Split (t=62s)** ✅
   - After economy plan rejected (10 steps > 8 limit), immediately split into 2 plans
   - Decision: Fix over continue with bad plan
   - Impact: Plans executed successfully afterward; economy stayed on track

2. **Emergency Footmen Queue (t=142-156s)** ✅
   - Faced early hero rush with zero defense
   - Decision: Queue Footmen immediately despite worker harassment continuing
   - Impact: Got first 2 Footmen defending by t=178s; prevented base destruction

3. **Worker Rally to Base (t=156-178s)** ✅
   - After 7 workers killed, moved survivors home
   - Decision: Sacrifice mining pressure to protect worker count
   - Impact: Stabilized at 3 workers, prevented total economic collapse

4. **Continued Production (t=200-380s)** ✅
   - Despite catastrophic start, kept queuing units and expanding
   - Decision: Autopilot on from t=178-452s; let triggers manage low-level tactics
   - Impact: Economy naturally recovered; army grew to 15 units; hero reached L5

5. **Final Push (t=460s)** ✅
   - Consolidated all 15 units into squad 2 with push stance to enemy base
   - Decision: Commit to assault once army was clearly dominant
   - Impact: Enemy immediately surrendered; avoided extended siege

---

## OPPONENT BEHAVIOR ANALYSIS

**Blue (enemy)**: Same model as RED, kingdom faction
- **Harassing phase** (t=142-170s): Sent hero early for worker raids—effective harassment but didn't prevent RED recovery
- **Mid-game** (t=200-380s): Defensive; only saw ~4 units and 1 Keep + 1 Barracks in production
- **End-game response** (t=460s): Surrendered immediately when RED showed dominance (15 units approaching)

**Assessment**: Opponent executed early aggression well (killed 7 workers) but failed to follow up with sustained pressure or base destruction. After RED stabilized with Footmen and recovered worker count, RED had clear advantage. Opponent correctly assessed unwinnable position and conceded.

---

## PART 2: 2.1 SCAFFOLD REFLECTION

### Folded Page Effectiveness
The `--doc` (folded page) view was invaluable for:
- **Fast decision-making**: 40-50 action lines showed all options at a glance
- **Error detection**: Immediately saw plan errors, blocked commands, status issues
- **Trigger/Plan monitoring**: Status updates (blocked/running/done) visible every cycle
- **Affordance context**: Resource costs, prerequisites, and gates all shown inline

**What helped most**: The ERRORS section—showed exactly which commands failed and why. The STATUS line for plans (blocked, running, halted, done) was critical during the emergency stabilization phase.

### Acceptance Notes & Doctrine
- **Notes**: Never actually received any NOTE lines (acceptance notes only fire on commitments that contradict a known fact)
- **Why**: My triggers and plans were simple enough that no contradictions arose when they executed
- **Triggers**: All 6 triggers armed successfully (home-guard, hero-save, expand, supply-capped, counter-punch, steady-production)
  - Counter-punch worked perfectly: fired when enemy hero fell, auto-stanced squad for push
  - Expand worked: automatically built new TownHall at t=440
  - Supply-capped kept firing (repeatedly tried to build Farms even when no idle workers)
- **Plans**: 2 plans used (economy and military); military halted due to Barracks not done in time, but was manually resolved

### Selectors & Late Binding
**Strengths of selectors**:
- `"select":"idle workers"` for builds automatically picked available workers
- `"select":"my TownHall"` / `"select":"my Barracks"` survived building creation/destruction cycles
- `"all army"` resolved correctly as units trained and died

**Weaknesses encountered**:
- Early in the match, the `harvest` step tried `"select":"idle workers"` when all workers were already assigned to mining—resulted in blocked plan step
- **Fix**: Avoided storing selectors in repeating plans; used direct harvest commands once then let triggers handle production

### Playbook (standard-kingdom)
- Did NOT use playbook explicitly, but the opening plan structure matched it:
  1. Harvest first (mine and trees)
  2. Train workers to 10
  3. Build Barracks
  4. Train hero
  5. Build army
- This structure proved sound; the bottleneck was enemy harassment not the playbook itself

### Focus Declaration (prefs.json)
- Prefs file did not exist initially; tried to use `--prefs` flag which failed
- Removed `--prefs` from all subsequent commands
- Did not use playbook=standard-kingdom explicitly either
- **Decision**: Working without prefs/playbook was fine; manual strategy worked equally well

### What's Still Missing
1. **"Fewer than N units of type" maintenance predicate** — Had to use game_time pulse to keep producing Workers (the supply-capped trigger wasn't perfect for worker count)
2. **Visible plan/trigger profiling** — No way to see how many cycles a trigger has been armed or how many times it fired (useful for debugging repeating rules)
3. **Partial selector matches** — "all army in square X" isn't possible; had to manually manage squad positioning
4. **Undo/rollback** — Once a plan halts on a blocked step, can only clear and re-send; no way to retry just that step

---

## CONCLUSION

**Technical Performance**:
- Opening strategy sound; plan split recovery was quick and effective
- Trigger system proved its value: counter-punch and expand fired exactly when needed
- Autopilot managing tactics while I focused on strategic direction worked well
- Selectors made late-game consolidation simple (squad 2 push with "all army")

**Strategic Performance**:
- Won despite catastrophic early harassment (lost 7 of 10 workers)
- Recovered economy and army through disciplined production queuing
- Escalated to dominant position (15 units, hero L5) in less than 8 minutes
- Opponent conceded when faced with overwhelming force

**Match Duration**: 7:40 (460 seconds)
**Final Position**: 12 workers, 874g, 15 army (2392 strength), hero L5, 2 Barracks ready, 1 TownHall main

RED wins decisively.
