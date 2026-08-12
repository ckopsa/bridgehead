# AAR — blue (boomer, Sonnet), round r21

**Result: Loss.** Opponent (Claude/red) destroyed all my production buildings (Barracks then Keep) around t=173-219s via a rush of 8 Footmen while I had zero standing army.

**How it unfolded:**
- Opening followed a clean boomer economy plan: workers to mine/lumber, queued workers, took the free Hero, upgraded to Keep by t=57s, built a Barracks at t=58s. Economy was healthy — gold climbed to 1000+ by t=140s, later 2000+.
- Fatal gap: my "army" plan (train Footman/Archer from the Barracks) was sent late (around t=140s) and never got units out before the enemy arrived. By t=167-173s an 8-Footman enemy force hit my base with literally zero defenders — the home-guard trigger fired but squad 1 was empty (no army units existed to defend with).
- The Barracks fell at t=179s, my hero (already low, 29% HP) died at t=181s before the hero-save trigger could relocate it (trigger fired but referenced an empty unit list — a bug in my own trigger setup, since I'd written `"units":[]` as a placeholder that never got populated with the real hero id).
- Farm destroyed at t=196s, workers picked off one by one, Keep whittled from 1700 to 8 HP and destroyed by t~219s. Game over.

**Key decisions and mistakes:**
1. Prioritized economy/tech (Keep, free hero, farm) correctly per the boomer persona, but delayed committing to any standing military production far too long — pure greed with zero hedge on defense.
2. The `hero-save` trigger was armed with `"units":[]` (a placeholder, since the hero didn't exist yet at plan-set time) and was never corrected once the hero spawned — it fired but did nothing, and the hero died with no evacuation.
3. Never scouted the opponent, so the incoming 8-Footman rush was completely unseen until it was already in my base — no warning, no time to react.
4. The `army` plan sent at t~140s was compiled too late; by the time Footmen would have trained, the enemy was already at the walls.

**Opponent behavior:** Red (Claude) played an aggressive early rush — built up 8 Footmen and committed them as a single timed strike directly at my main base once my Barracks was up but before I had any defenders, correctly punishing a pure-economy opening with no military hedge.
