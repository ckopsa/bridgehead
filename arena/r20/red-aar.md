# Arena Round 20 — RED (Kingdom, rusher creed) After-Action Report

**Result: LOSS.** Surrendered at t=341s (5:41) with 0 workers, 0 army, TownHall at 74/1200
and 15 enemy units inside my base. Series: Boomer's third straight win.

## The held-at-t=0 planning window — what I did with it
I read COMMANDER_BRIEF, the catalog, and the map (`crossings`: impassable diagonal river,
three fords, my hall at (70,70), theirs at (-70,-70), center ford the only direct lane) before
readying. I sent one batch and then `ready`: an 8-step `opening` plan (3 workers to the
northeast mine, 2 to trees, **Hero first out of the TownHall**, Barracks, Farm, 2 Workers,
second Farm), a `home` region, a supply valve, and a home-guard trigger.

The window worked as advertised — Barracks finished at t=32, hero at t=25 — but I made a
**gold-blind error inside it that decided the match**. Steps 4, 5 and 8 of the plan named
the *same three workers I had put on gold* as the builders. All three left the mine, and the
supply-valve trigger grabbed a fourth. Result: **zero gold income for the first ~53 seconds.**
At t=55 I had 25 gold. That deficit never closed; every later plan spent the match `blocked:
cannot afford`.

## The hero's story
Trained first, free, out at t=25 — the earliest pressure available, exactly as briefed. It
foraged mid, held the center ford as my only scout, and reached (0,0) at ~t=143 without ever
meeting an enemy: the boomer never contested the midfield. I armed a hero-save
(`hero_below 0.45` -> move home), a retreat policy (0.5), autocast, and bought a Healing Potion.

Those saved it **once**: at t=219, when my 11-unit push was shredded at (-47,-50), the hero
escaped at 22% and was back to full HP at home. They did **not** save it the second time. At
t=310 five hostiles hit my forward Barracks; I had a hero and one archer. I set a 16-radius
leash and a defend posture around the hall — and the leash/defend pulled the hero *into* the
five of them. It went from 320/320 to dead in about eight seconds (t=318 "hero low: 2%",
t=320 dead), faster than one poll cycle. The 400g/100l bill never mattered because I never
again had 400 gold. What mattered was that the hero was my last combat unit, my only
command node, and my only vision.

## How it unfolded
- t=0–32: opening plan runs clean, hero + Barracks up. Gold income accidentally zero.
- t=55–190: recovery. All workers to gold, second and third Barracks (t=143, t=211), forage
  posture at mid, 3 Farms + supply valve. Army reached 11 (6 Archer, 4 Footman, hero) by t=184.
- t=194: I saw their mine draining faster than mine and **chose to push immediately** rather
  than wait for a 15-unit `commit` trigger — the r19 lesson said "measure strength, not tech,"
  and 11 with a hero looked like strength.
- t=216–225: it was not. Their 9-unit ball (5 Footman, 4 Archer) plus a hero and a Tower at
  their base met my push as it arrived. **Five archers died in four seconds**, then the
  footmen. Eleven units for zero kills. My squad was on `push` posture, which does advance
  cohesively — but it advanced cohesively into a fully assembled defensive army at *their*
  hall, which is r19's death repeated at larger scale.
- t=225–310: rebuild attempt on ~60 gold/poll. Three Barracks standing, nothing to put in them.
  Tower blocked twice on footprint errors.
- t=309–341: their 15-unit counterattack. Tower, both forward Barracks, hero and all six
  workers gone in ~30 seconds. CallToArms fired (militia trigger) into a lost fight.
  Surrendered.

## Opponent behavior
Pure, disciplined boom. They never scouted me, never contested mid, never poked. They sat at
home behind two Barracks and a Tower, massed, and let my push walk into them — then converted
the free 11 kills into a 15-unit ball and ended it. Their hero was alive and full at every
sighting, and they later fielded a Priestess too. Their mine drained ~2x faster than mine all
game, which is the whole story in one number.

## Vocabulary usage
Used: `plan_set` x5 (opening / army / wave2 / wave3 / pump / rebuild / defend), `plan_clear`,
`trigger_set` x6 (supply-valve, home-guard, hero-save, doorbell, commit, militia),
`trigger_clear`, `region_set` (`home`, re-aimed once), `squad`, `posture` (forage/push/defend,
region-addressed), `template` on all three Barracks, `retreat`, `leash`, `priority`,
`autocast`, `rally`, `harvest`, `build`, `train`, `buy`, `use_item`, `cast` (TownHall
CallToArms), `ready`, `surrender`. Regions + squad-addressed postures were genuinely good:
one `region_set` re-aimed my whole home defense.

## Top 3 complaints
1. **`build` inside a plan silently cannibalizes your mining workers, and nothing warns you.**
   A step that names a worker who is currently on gold is accepted, legal, and economically
   catastrophic; `plans[]` shows `running` while my income sat at zero for 53 seconds. A
   worker's `why` said `plan:opening step 4/8 build` — accurate and useless, because by the
   time I read it the damage was done. I would like `build` to report which resource task it
   interrupted, or a snapshot field for gold-per-minute so "my economy stopped" is a number
   and not an inference.
2. **`leash` and `defend` outrank `retreat` and hero-save, so hero-preservation policy loses
   to hero-positioning policy.** I armed three separate safety rules on my hero and it still
   died in under a poll cycle, because the leash I set to keep it *near home* is what kept it
   *in the fight* at home. If retreat and hero_below are the mechanism for the most expensive
   event in the match, they should win ties against posture, or there should be a "hero never
   tanks" policy that actually removes the unit from combat.
3. **There is no way to measure the enemy army before you are inside it.** `enemy_army_seen`
   requires having seen them, and a boomer who never leaves home is invisible until your push
   is in range of their whole production. My `commit` trigger counted *my* archers again —
   the exact r19 mistake — because counting my own units is the only counting the language
   offers. A push into fog is a coin flip, and the boomer creed wins that flip by construction.

## Honest lesson for r21
The free hero is real and the held start is real; neither compensates for 53 seconds of dead
economy. Build with a *dedicated* builder worker, never with a miner, and never commit into an
unscouted base — scout the ball first, and if you cannot see it, do not push.
