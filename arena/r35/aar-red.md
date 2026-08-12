# Arena Round r35 — RED vs Blue (Blue AI, Kingdom) — After-Action Report

## PART 1: The Match — Opening, Decisions, and What Won

### Opening Phase (t=0-180s): Economy Setup
**Strategy**: Followed the playbook's standard-kingdom sequence: worker-pulse, defensive triggers, and early expansion.

**Key Actions**:
- Armed worker-pulse (train Workers every 18s at idle TownHall)
- Armed defensive triggers: home-guard, hero-save, expand, supply-capped
- Rallied all workers to northeast mine for early income
- Hit 10 workers by t=92s (completed playbook step 1)
- Built Barracks at t=120s with 160g/60l (completed step 2 by t=196s with 3 Footmen)

**Outcome**: Strong early economy. 12-13 workers by t=150s, healthy gold/lumber flow, good supply management via triggered Farm building. This phase was executed very well.

### Mid Game Phase (t=180-350s): Expansion and Army Building
**t=206**: Expand trigger fired when northeast mine ran dry — built second TownHall at northeast mine. This was critical for scaling income.

**t=240s+**: Playbook advanced to step 4 (stage at mid). Staged squad 1 at center ford for scouting. Armed second Barracks build to double production capacity. Both Barracks online by t=329s, producing 2 Footmen per cycle.

**t=276s**: First enemy contact — ~5 Footmen spotted at mid. Hero took damage in skirmish. Hero-save trigger pulled hero back, preventing early loss.

**Outcome**: Successfully ramped to 15-16 Footmen + 1 Hero by t=400s. Consolidation into squad 1 complete. However, **failed to upgrade to Keep or build Workshop** — stayed pure tier 1. This was the first strategic error. Should have pivoted to tier 2 tech around t=300s.

### Late Game Phase (t=400-473s): The Push and Collapse
**t=408s**: Squad 1 ordered to push toward enemy base. Army strength: 15 Footmen + Hero.

**t=435-444s**: First large-scale battle near Blue's base. Discovered:
- Blue has Keep (tier 2 hall)
- Blue has 2 Barracks minimum
- Blue has ~7 visible units + Hero
- Blue has Spearmen (anti-cavalry that hard-counter pure Footmen)

**Critical Failures**:
1. Did not scout with hero first to learn enemy composition
2. Pushed into unknown terrain/army without knowing Spearman counter existed
3. Lost 10+ units in rapid succession (t=442-449s: lost 4 at once)
4. Hero dropped to 11% health and had to retreat
5. By t=462s: Down to 7 units vs Blue's 17

**t=462s Decision**: Assessed position:
- Enemy 17 units (13 Footman, 5 Spearman, 1 Hero) vs my 7 units (6 Footman, 1 Hero)
- Low gold (681g) — no path to tier 2 fast enough
- No Workshop built — can't counter Spearmen with my own Spearmen
- Two TownHalls but Blue would break through my defense (squad 0: 3 units) within 1-2 cycles
- **No viable recovery path**

**t=473s**: Surrendered. Blue wins.

---

## PART 2: The Playbook as a Tool — How Far, Which Gates, Which Exits

### Playbook Progression
- **Step 1/10** (Workers to 10): COMPLETED by t=92s. Trigger worked perfectly.
- **Step 2/10** (Barracks + 3 Footmen by t=180s): COMPLETED by t=196s. On time.
- **Step 3/10** (Template + 5 Footmen): COMPLETED by t=239s. Template and squad consolidation worked smoothly.
- **Steps 4-5**: SKIPPED (auto-advanced). Step 4 was "stage at mid by t=240s" — I did this (squad 1 at center ford t=264s). Step 5 unclear but likely got skipped via trigger/alarm flow.
- **Step 6/10** (Upgrade Hall): NEVER REACHED. Game ended before I could act on this gate. When I got to step 6 view, the "why" warned: "do not start it unless you mean to finish it" (Keep costs 320g+160l). This was correct advice — I lacked the resources and safe time to upgrade.

### Gates and Exits Taken
- **Step 1**: Continued with worker-pulse (CONTINUE option). Correct choice.
- **Step 2**: Continued with Barracks build (CONTINUE option). Did not take EXIT options (Farm first, Hero first). Correct — army was priority.
- **Step 3**: Did both CONTINUE (template) and EXIT (enrol existing army). Combined approach worked.
- **Step 4**: Took CONTINUE (stage at mid). Did not take harass exit or hero-scout-alone exit. Stage was correct for map control.

### Playbook as Decision Tool
**Did WHY sentences change decisions?**
- Yes, one key time: Step 6's "why: do not start unless you mean to finish" reinforced that I should not upgrade naked without tier 2 follow-up. Had I seen this earlier (t=300s), I might have prioritized tier 2 differently.

**Did you edit prefs.json?**
- No. Kept the playbook default ("standard-kingdom") throughout. This was correct — it provided good pacing.

### Key Playbook Misses
The playbook's step progression suggests a smooth path to mid-game with good heuristics. But **the playbook does not address the tier 2 transition explicitly enough**. Steps 1-6 are tier 1-centric. Step 6 mentions upgrading the hall but the "why" warns against doing it naked. This left a gap:

**Should have built:**
- Workshop (140g/100l) to unlock Raiders and Catapults by t=350s
- OR Keep upgrade (320g/160l + follow-ups) by t=350s

**Why I didn't:**
- Focused on production volume (two Barracks) instead of tech
- Assumed pure Footman army would be enough
- Misread the opponent's tech trajectory

The playbook's "steady-production" recipe was helpful but not enough — I needed a tech-progression trigger or plan alongside it.

### Acceptance Notes and Errors
- **NOTE at t=444**: "push gates not met (not consolidated: 2 of your 16 army units are outside squad 1)" — Hero was in squad 0, broke consolidation. Tried to fix but was too late.
- **ERRORS**: Mostly "idle Barracks matches none of your finished buildings — all 2 of your Barracks are busy" — this was expected when both queues filled. Not a problem.

### What's Still Missing from Playbook
1. **Tech progression gate**: A step that says "by t=X, decide: upgrade to Keep OR build Workshop + Raiders". The current playbook leaves tier 2 tech optional-feeling.
2. **Counter-unit triggers**: `{"type":"trigger_set",...when player has no counter to Spearmen, start training Spearmen}`. The playbook assumes single unit type success.
3. **Scout-then-commit rule**: Push should only fire after hero scouting reveals enemy isn't Spearman-heavy. This needs a predicate or conditional advance.

---

## Summary: Why It Unfolded This Way

**Won early, blundered mid, lost late.**

- **Economy execution** was excellent: 29 workers, 2 TownHalls, solid income by t=300s.
- **Doctrine/trigger system** was powerful: home-guard, hero-save, and supply-capped all worked seamlessly, freeing me to think strategically.
- **Critical error**: Pushing to enemy base at t=408 without hero scout + tier 2 tech. Blue had:
  - Keep (tier 2 hall)
  - Multiple Barracks + Spearmen
  - Hero
  - 17 units vs my 15

- **Result**: Lost 10 units in 20 seconds. Conceded at t=473s (7 minutes of game time). Game was decided by tech + counter-comp, not economy or early game.

**Key lesson**: The playbook works best as a pacing tool for economic ramp. But it does NOT guarantee late-game viability without explicit tech/counter decisions. Next time: upgrade to Keep by t=350s OR commit to Workshop + Raiders counter by t=300s. Scout the opponent before committing army to their base.

