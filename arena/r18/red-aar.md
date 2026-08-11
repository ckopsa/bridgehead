# Arena Round 18 — RED (Horde) After-Action Report

**Result: LOSS.** Surrendered at ~7:10 game time (t=430s) with squad 1 wiped, both
WarCamps razed, and 16 Archers + 2 Footmen inside my base. Blue (Kingdom) wins.

## How it unfolded

- **0:30–1:00 — opening.** One `plan_set` ("boomer", 8 steps): 3 peons to the near
  mine, 2 to trees, four Peon trains, WarCamp at (58,58), Burrow at (78,62). It
  completed in ~2 seconds of game time. Clean, and the best part of the match.
- **1:00–3:10 — tech.** Second plan ("tech"): 2 more Peons, Fortress upgrade,
  SpiritLodge, WarMill, WarCamp #2. Fortress finished at 3:08, both tier-2
  buildings up by ~3:30, Weapon Smithing 1 and Armor Plating 1 both completed by
  5:25. Supply valve trigger (recipe 7) fired four times and kept me building
  Burrows without a single manual poll spent on the number.
- **2:46 — intel.** A Peon scout reached their base, saw a Barracks and 4 units
  (2 Archer, 2 Footman), and died. That was my only scouting of the match, and I
  never replaced it. **This is where the game was lost.**
- **5:16 — first contact.** The doorbell trigger reported "6 enemy troops"; the
  actual force was **15**. My 13-unit squad, on a `defend` posture, walked out to
  meet it and lost 7 units in six seconds. It then chased survivors to (16,19) —
  a quarter of the map away — and fed two more.
- **5:40–6:20 — the fatal own-goal.** A trigger I had armed earlier
  (`the-push`: "when we field 4 Shaman, squad 1 pushes their base") fired at
  6:12, *while I was rebuilding from a lost battle*, and sent my 10 remaining
  units toward their base. My `trigger_clear` arrived one cycle too late. I
  recalled them, but the squad was strung out and out of position.
- **6:50–7:00 — the wipe.** 18 Archers arrived. Squad 1 was wiped in 3 seconds,
  both WarCamps fell within 5 seconds of each other. 9 Peons, 150 gold, 530 gold
  left in my only mine, no army, no production. I surrendered.

## What changed vs r17 (and what it cost me)

r17 I won at 7:15 with a 21-unit brick, no hero, one plan, and `unit_count` /
`enemy_hero_down` triggers. R18 I tried to *improve* that line and made it worse:

| r17 | r18 | verdict |
|---|---|---|
| One army building, one composition | Two WarCamps + SpiritLodge + WarMill + Fortress by 3:30 | Over-teched. I spent ~1100 gold on buildings and research and never had more than 13 army units at once. |
| Brick sat home until the count was right | Same trigger idiom, but the count (`4 Shaman`) was a **tech** milestone, not a **strength** milestone | The push trigger fired after I had already lost the army it was written for. A count-based push trigger must be re-checked or re-armed after every lost fight. |
| No hero | Tried to add a Warchief at 4:40 | Never afforded it; the plan blocked and I cleared it. 400g of intent I could not pay, at exactly the moment I needed units. |
| Won the fight I chose | Never chose a fight; both fights were chosen by Blue | Blue massed 16 Archers — a single-unit deathball my Grunt/Headhunter mix has no answer to at range 10.5 vs 14. |

The honest core: **Blue out-produced me roughly 2:1 in army value while I built
infrastructure**, and I never scouted after 2:46, so I found out at 5:16. The
r17 AAR warned my brick "would be punished badly by any real maneuvering." Blue
did not even need maneuver — straight-line mass Archer was enough, because I
was looking at my own build order instead of at them.

## Vocabulary usage

- **Plans (5 set):** `boomer`, `tech`, `camp2`, `shamans`, `chief`, `rebuild`,
  `rebuild2`, `towers`. The opening two were excellent — 14 build-order actions
  for two commands. Blocked-status stickiness worked exactly as briefed: I read
  `blocked: cannot afford X` from `plans[].status`/`events` and never once sat
  waiting out a retry storm. The r17 attention-gap bug is genuinely fixed.
- **Triggers (5 armed):** `supply-valve` (fired 4x, worked, but its build site
  needed re-aiming 3 times as my own Burrows blocked the previous site),
  `home-guard`, `doorbell`, `their-hero-down` (never fired — Blue fielded no
  hero), `the-push` (fired, and it beat me).
- **Regions:** `home`, `front`, `warfront`. Moving `front` from (56,56) to
  (64,64) with one `region_set` re-aimed both `home-guard` and `doorbell` — that
  is the feature working as advertised, and it is the single best thing in the
  vocabulary.
- **Templates:** stamped squad/retreat/priority on both WarCamps and the
  SpiritLodge. Every trained unit arrived enrolled and with `Archer` focus-fire.
  Zero per-spawn commands. Also correct.
- **Not used:** heroes (never afforded), Wolfriders/Wyverns (no Hold), items,
  autocast (Shamans self-cast Bloodlust), `intent_compile.py` (I wrote JSON).

## Top 3 complaints

1. **`enemy_army_seen` size is a floor, and it reads like an estimate.** The
   doorbell fired "6 or more enemy troops" against a force of **15**, then again
   against 18. Both times I made a defend/engage decision on a number that was
   40% of the truth. If the ledger knows it saw 15, the trigger should be able
   to *tell me the count it fired on* — a `sentence` that says "15 seen" instead
   of echoing back my threshold. As it stands, a size-6 doorbell is
   indistinguishable from a size-16 doorbell, which is the difference between
   "hold" and "run".

2. **`defend` posture chases.** My squad, told to defend a 24-radius circle at
   (64,64), pursued fleeing Archers to (16,19) — 60+ units away from the anchor
   — and died piecemeal. I had to bolt on a separate `leash` to get the behaviour
   the word "defend" already promises. Defend should leash to its own radius by
   default; if unleashed pursuit is wanted, that is what `attackmove` is for.

3. **A count-based push trigger has no way to say "and we are healthy".** The
   only composite I can express is one predicate. `unit_count Shaman >= 4` was
   true both when I had 21 units and when I had 10 — and it fired in the second
   case. Either let `when` take an `all_of` list, or add a `squad_above` /
   `squad_size` predicate so "push when squad 1 is 18 strong AND above 80%
   health" is armable. Every commander who arms an offensive trigger is going to
   write this bug once; I wrote it in the round after I won with the safe
   version.

Runner-up complaint: the supply valve's frozen build site is a foot-gun in a
tight base. Three of its fires were refused with "site blocked — nearest legal:
(x,z)", and it does not take the engine's own suggestion. A valve that could say
"build a Burrow *anywhere legal near my hall*" would need re-arming zero times
instead of three.
