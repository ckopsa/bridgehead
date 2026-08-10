# Round 9 AAR — RED (Rusher creed) — WIN at t=324s

## Result
RED (Claude) wins. `game_over: "Claude"` at game second ~324 (~5:24). Blue's Barracks was razed
by my main army inside their base; the win resolved with their Keep and Workshop still standing,
so this was a production-kill/concede resolution, not a full base wipe.

## Timeline of key decisions
- Match was restarted at ~0:00 for spectating; I resumed at t=41s having lost my first opening.
- t=41: 3 workers gold, 1 lumber, 1 builds Barracks at (62,62); continuous Worker production to 11.
- t=72-150: Barracks up, Footman/Archer mix, Farms ahead of cap, Workshop at (56,70).
  Set a production TEMPLATE on the Barracks (squad 1, retreat 30%, focus Archer/Siege/Worker)
  so every unit spawned with doctrine and joined the push automatically. This was the single
  highest-value command of the game — I never hand-managed a reinforcement.
- t=170: squad 1 set to FORAGE at (20,20) to own the midfield and scout through fog.
- t=192: trained the Champion (hero slot 1/1).
- t=214: noticed both starting mines burning down fast (mine at 3500 -> ~800 by t=214).
  Committed to an expansion TownHall near the southeast ford mine (68,-52).
- t=249: FOG PAYOFF — my foraging squad walked onto Blue's own expansion TownHall at (50,-64),
  under construction, with their hero at 36 HP defending. I switched squad 1 from defend to
  PUSH on it. Their expansion died and their hero fled.
- t=278-296: pushed straight on through the southeast ford into their base with 9-10 units,
  hero at Lv2-3. Set retreat 35% on the hero only, kept squad posture push.
- t=296-324: attacked the Barracks at (-59,-67); reinforcements auto-flowed via template.
  Queued Catapults for the Keep. Game ended before they were needed.

## New content: used and mattered?
- FORDS / chokepoint map: decisive. My forage squad and their expansion collided at the SE ford,
  which is exactly where the map forces contact. The ford also meant my push could not be
  flanked or avoided — once I owned it, their expansion was dead and mine was safe.
- FOG OF WAR: changed my play the most. I never once "checked" their base early; I learned
  everything by walking a forage squad into the middle. Empty `units` genuinely read as
  "no information," and the ghost buildings (Keep/Barracks/Workshop with last_seen) were how I
  picked the Barracks as the target.
- TEMPLATE + SQUAD POSTURE: carried the whole match. One `template` on the Barracks plus
  posture flips (defend -> forage -> push) was 90% of my army control.
- BOUNTIES: a 270g cache spawned at (11,-3) and was taken within 3s — I could not tell by whom;
  worth noting the feed says "gone," not "who took it."
- NOT used (game too short): Keep/Castle tier, Sorcerers/Slow, Blacksmith research, Knights,
  Gryphons, the Shop and items, the second hero slot. Catapults were queued but never fired.
  This is the honest gap: the new tier ladder never entered a 5-minute game.

## Pacing
Too fast at the top, and specifically: mines drain far faster than the brief's "die around minute
10" — my starting mine was empty at ~t=280 (under 5 minutes) with 11 workers. That forced an
expansion at minute 4 and it is what created the collision that decided the game. A 5-minute
decisive game means tier 2 is basically theoretical content unless both sides turtle.

## Top 3 balance/design complaints
1. Mine depletion outruns the tech ladder. 3500 gold vs ~11 workers is ~4 minutes; Keep (320/160)
   plus Sanctum plus Sorcerers cannot land before the economy phase-changes. Either double mine
   size or halve T2 costs, otherwise "rush" is not a strategy choice, it is the only tempo the
   economy supports.
2. Building an expansion at a contested ford is currently a coin flip you lose to whoever walks
   past. Blue's expansion TownHall died to a squad that was not even hunting it. Some warning
   (or a cheap tier-1 defensive option that finishes faster than 40s) would make expanding a
   decision rather than a gamble.
3. Placement feedback is thin: "site is blocked for TownHall/Barracks" with no hint of a legal
   nearby site cost me two full cycles of guessing coordinates. A returned suggested position,
   or a radius-snap, would remove pure trial-and-error from the build verb.
   (Minor, same family: the win triggered with their Keep and Workshop alive, so I cannot tell
   from the feed whether I met the raze condition or they conceded — the game_over payload
   should say which.)
