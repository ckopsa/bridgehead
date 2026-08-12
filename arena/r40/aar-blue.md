# After-Action Report — Arena r40, BLUE / Human / kingdom, crossings

**Result: LOSS — surrendered at t=544 (9:04). Red wins.**
Seat: bridge/blue, model claude-haiku-4-5, scaffold affordance-doc/2.2, playbook standard-kingdom (pre-declared), BH_NO_AUTOPILOT=1.

> Orchestrator note: seat testimony; specific claims unverified against the log.

## How it unfolded (seat's own account)

**Early (t=0-120, "textbook opening"):** standard-kingdom executed correctly — workers to resources, Barracks by t=61, Farms for supply, 10 workers by t=120, 4 initial army units by t=121.

**Mid I (t=121-241):** squad 1 pushed to midfield to scout (forage posture); TownHall upgraded to Keep at t=349; grew to 8 army + Hero; never sighted the enemy despite scouting.

**Mid II (t=241-400, "economy trap"):** repeatedly FAILED to build the expansion TownHall at northwest mine — attempts at t=213, t=255, t=311 all abandoned when the builder workers were re-tasked. Root cause: workers had been enrolled in squad 0 with a turtle stance, and the squad's leash/retreat policies kept pulling builders back to base. The expand trigger was armed but never fired (mine_dry wants 0%, not low).

**Late (t=400-544, collapse):** t=403 red raid killed 3 Archers + 2 workers; t=421-437 squad 1 wiped trying to consolidate home; t=497 income_collapse — NO workers on gold, squad 0's leash had pinned them; t=513/522 Hero died; t=535 final fight 5 Footmen vs 9, mine at 14%, commit 675/min > income 390/min, no expansion to recover on. Surrendered at t=544.

## Key failures (seat's own account)

1. **Workers in an armed squad (t=82 onward):** squad 0's turtle stance leash overrode individual harvest/build tasks — the game was lost in unit management, not battle.
2. **Expansion timing:** the expand trigger never fired proactively; by the time the urgency was obvious (mine 41%), the builders were trapped.
3. **Solo hero deployment** into enemy territory; died without return.
4. **Consolidation under pressure:** pulling squad 1 home mid-raid got it wiped piecemeal.

## Playbook compliance

- ✓ Opened correctly (workers, Barracks, tier 2)
- ✓ Eyes on mid by t=241
- ✗ FAILED: expansion before the mine ran dry (mine reached 14%, no second hall ever stood)
- ✗ FAILED: worker-squad separation — army policies trapped the economy

## Opponent behavior (seat's read)

Red played defensively until t=403, then executed precision 2-unit raids into a coordinated 8-10 unit push. Better economic management despite expanding later.
