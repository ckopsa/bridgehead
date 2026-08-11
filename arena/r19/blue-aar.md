# Arena Round 19 — BLUE (Boomer, Kingdom) — AAR

## Result
**BLUE WINS.** Game over at t=361s (~6:00 of game time), ~24 bridge cycles.
Final state: BLUE 24 army (20 Archer, 4 Footman), 12 workers, 3 Barracks, 2 Town Halls,
6 Farms, zero buildings lost. RED finished with 4 workers, no army, no unit production —
every RED Barracks razed, Keep at 234/1700.

## How it unfolded
- **t=17s** One `plan_set boomer` (8 steps) sent before anything else: 3 workers to gold,
  2 to lumber, 4 Workers queued, then **two** Barracks. Same batch armed the supply valve,
  home-guard, and an `enemy_army_seen>=5` doorbell.
- **t=80s** Both Barracks up. `template` stamped both into squad 1 with retreat-at-30% and
  focus Siege>Archer>Footman, then a rolling Archer/Footman plan. Queues never emptied again.
- **t=115s** Worker scout reached RED's base, saw 1 Archer + 1 Footman and one Barracks, then died.
  That single scout was the whole read: RED was building units, not economy.
- **t=180-195s** RED's timing push: 4 units, then 7 Archers, into my Barracks cluster. Home-guard
  fired but anchored on the Hall at (-70,-70), pulling my line *off* the buildings being hit.
  I fixed it in one command by defining region `home` at the Barracks cluster and re-pointing
  both home-guard and the doorbell at the name. Cost: 1 Footman + Barracks chip damage. Their
  attack broke against reinforcements arriving from three Barracks.
- **t=224-260s** Third Barracks. `commit` trigger armed: *when we field 15 or more Archer,
  squad 1 pushes their base.* It fired at t=260s with 21 units. I immediately cleared home-guard
  and the doorbell (they would have yanked the push home) and re-templated all three Barracks
  into squad 2 as the home garrison.
- **t=285-310s** The army walked into RED's base, killed their ~6 defenders and all three
  RED Barracks. A 270g bounty was picked up in transit.
- **t=308s** My main mine ran dry, exactly at saturation, exactly on schedule — the expansion
  Hall at (-58,52) on the northwest mine finished at t=349s. Income never stopped.
- **t=361s** Keep down to 234, RED with no production left. Game over.

## The hero question — I skipped it again, and I would again here
No hero. 400g/100l/5 supply buys three Archers plus change and 12s less build time.
Reasons, honestly:
1. **In a Kingdom mirror, Archers are the scaling unit and the hero is not.** 20 Archers put out
   280 dps at range 14. A Champion adds 24 dps at range 2.4 and eats the supply of 2.5 Archers.
2. **A hero is a single point of failure on a 15-second poll.** I cannot micro it; it dies inside
   one cycle and takes 500 resources and a hero slot with it. The Archer mass has no such cliff.
3. **The 400 gold went into the third Barracks instead**, and the third Barracks is what made my
   defense at t=190 self-repairing — reinforcements arriving faster than RED could kill them is
   what broke their push, and no hero does that.
The honest caveat: had RED massed melee, or had the game gone past 10 minutes into T3, Slam AoE
into clumped Archers would flip this. The hero is a *late* purchase in this matchup, not an
opening one, and this game never got late. Two wins with zero heroes is now a pattern, not a fluke.

## Vocabulary used
`plan_set` x7 (opening, two eco, four rolling army plans), `trigger_set` (supply valve — fired
**six** times unattended and built six Farms; home-guard; enemy_army_seen doorbell;
`unit_count Archer>=15` commit trigger — the r18 machinery, unchanged and decisive),
`trigger_clear` (disarming the recall triggers at commit is the single most important command
I sent), `region_set` (`home`, `our-ford`) plus map places (`their base`, `mid`),
`template` (all three Barracks, re-stamped from squad 1 to squad 2 at commit),
`squad`/`posture` push+defend, `priority` (retargeted from units to Buildings once RED's army died),
`retreat` (loosened from 30% to 12% mid-push).

## What decided it
Two Barracks before their one, three before their three, and **the commit trigger firing itself**
at 15 Archers while I was reading a snapshot. RED spent on a Tower and a Blacksmith; I spent on
production. Their timing push arrived into a base that was producing faster than they could kill.

## Top 3 complaints
1. **Squad push cohesion fought my attack.** After the retreat rule pulled 5 hurt units home, the
   whole 18-unit push reversed to regroup — centroid went 46,44 -> 34,33 -> 20,16 while RED's base
   sat empty. I lost ~40 game-seconds of a decided attack to a gather I never asked for. A push
   should leave stragglers, or `posture` should take a `cohesion:false`.
2. **`retreat` in a `template` is a trap on offense.** The 30% retreat I stamped at t=80 for
   defense silently became a mid-siege desertion order at t=300. Templates need a posture-aware
   retreat, or at least a way to say "retreat only while defending".
3. **The supply valve can only know one Farm site.** It got refused twice ("site blocked, nearest
   legal: X") and I had to spend a command re-aiming it each time. The trigger already knows the
   legal site — it should be allowed to take the engine's own suggestion, or accept a region and
   place anywhere inside it.
