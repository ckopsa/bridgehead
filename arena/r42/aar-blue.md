# After-action report — arena r42, BLUE (Human, kingdom), crossings

**Result: Loss by surrender at t≈268s. Red (Claude) wins.** No autopilot was used at any point.
Seat: bridge/blue, model claude-opus-5, scaffold affordance-doc/2.2, playbook standard-kingdom (pre-declared).

## How it unfolded

- t=0: read `--doc --all`, readied immediately with an opening batch: 3 workers queued, Farm + Barracks, rally, `home-guard` and `hero-save` triggers, squad 1 on `turtle`.
- t=2: first Barracks build abandoned — the Farm had taken the site ("the ground was no longer clear"). Re-sent it at t=4; it finished ~t=40.
- t=4–160: economy build-out. Big early mistake: my opening batch never put the starting workers on a resource — RUNWAY read `+0/min` at t=4 and I only fixed it at t=6. I then repeatedly leaked worker time: four workers stalled idle holding lumber at a far tree corner (t=163–185), so real gold income sat around 330–360/min instead of ~500+, while `commit > income` was flagged on nearly every RUNWAY line and I kept pumping anyway.
- t=83: free Hero out. Army built to 8–9 (4 Footman, 3 Archer, Hero) by t=200 via `pump-army` / `pump-archer` repeating triggers plus a `consolidate` trigger folding all army into squad 1.
- t=199: **the losing decision.** With zero intel (ledger empty all match — my scout worker's move orders kept getting overwritten, explored only 10%), I moved squad 1 to `stage` at center ford, then at t=211, on seeing an enemy army of ~9 with a hero right there, escalated to `secure` at the ford instead of pulling home. My 9 met their 9 in the open with my hero at 86%.
- t=215–223: hero dead at t=216, whole squad wiped by t=223 for very little in return. From 9 units to 1.
- t=233: their 8 hit my base. `home-guard` fired correctly (turtle), but there was nothing to turtle with. CallToArms cast at t=252 came too late.
- t=238–262: both Farms razed, workers picked off, TownHall destroyed at t=262 (tier dropped to 0). With 0 gold, no hall, one archer trickling out of a Barracks under attack, I surrendered at t=268.

## Key decisions, honestly

1. Not putting starting workers on the mine in the ready batch — cost ~40s of income and set the whole game behind.
2. Letting four workers sit idle on a dead tree for ~50 game seconds; I saw `idle 4/5` for several cycles before diagnosing it by dumping unit orders.
3. Committing an even-numbers, no-intel fight at mid at t=199–211. The affordance doc had been telling me on every `push` link that my intel ledger was EMPTY; I read it and staged forward anyway. `secure` at t=211 rather than `turtle` home was the specific order that lost the match.
4. Never fixing the scout — the plan `scout` completed at t=191 but the worker had been re-tasked by my blanket `harvest select: workers` commands, so I fought blind.

## Opponent behavior

Efficient and conventional — a same-size army massed at center ford by ~t=210, took the even fight, then went straight to my base and razed Farms → TownHall without pause. They never split or harassed early; one decisive engagement decided it.

## Vocabulary

`stance` (turtle/stage/secure), repeating `trigger_set` for production pulses (`pump-army`, `pump-archer`, `pump-worker`, `supply`, `remine`, `consolidate`), `build` with `region` + `nearest legal site`, selectors (`workers`, `all army`, `my TownHall`, `idle Barracks`), one `plan_set` for scouting.

## Did the RUNWAY line / alarms / recipes change decisions?

RUNWAY did — `commit 1125/min > income 400/min` at t=133 is why I cleared `pump-worker` at 10 workers, and the mine depletion estimate (~10 min) is why I dropped the `expand` trigger rather than chase a second base. Recipes gave me `home-guard`, `hero-save`, `expand` and `steady-production` essentially verbatim. The alarms and the empty-intel warnings on the `push`/`harass` links were the ones I should have obeyed and did not — they were correct, and they were on screen before I committed.
