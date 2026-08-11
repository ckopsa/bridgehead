# Arena Round 18 — BLUE (Kingdom) After-Action Report

**Result: WIN.** Final snapshot at t=430s reads `GAME OVER: Human wins` — my seat.
Match length: ~430 game seconds from my first snapshot at t=48s (~6:20 of play,
about 380 seconds under my command). Ended with my 18-unit army (16 Archers,
2 Footmen) standing inside the Horde base at (61, 66), both their WarCamps
destroyed, their Fortress down to 1276/1750, and their only visible units seven
Peons. Red conceded or lost their base in that state.

## How it unfolded

- **t=48–140:** One `plan_set` opening (harvest gold x3, harvest lumber x2,
  Barracks, four Workers). Immediately armed the supply valve
  (`supply_capped` -> build Farm) and a home-guard. Second Barracks up by t=140 —
  the boomer creed's "second production building early", and it is what let me
  outproduce them.
- **t=140–270:** Straight Archer/Footman mass out of two Barracks via a `prod`
  plan and an `enemy_army_seen` doorbell wired to **production** (train an
  Archer), exactly as my r17 AAR told me to. Six farms went up from the trigger
  and manual builds; I never hit a supply wall.
- **t=218:** My scout Worker died at their base but bought the intel that decided
  the game: ~7 units, 3 Grunt / 3 Headhunter / 1 Impaler. Headhunter-heavy, as
  predicted, and small.
- **t=272:** `unit_count Archer >= 10` trigger fired the timing attack to
  midfield. At 317 I pushed onto their base with 17.
- **t=320–330: the decisive fight.** I traded 5 units for their whole standing
  army (Grunts, Impaler, and a Shaman + Demolisher they had just added). Retreat
  templates at 30% pulled the hurt archers out cleanly; I never lost the ball.
- **t=334–375:** Pulled back to `forage midfield`, collected a 315g cache,
  rebuilt to 20 supply, put up a Workshop.
- **t=389–430:** Second push. Razed one WarCamp, then the other, and was working
  the Fortress when the match ended.

## Did the fixed loop change how it felt?

Yes, materially. Every cycle was one `bridge_wait` + one `bridge_view` + one
decision — never a chained wait, never a blind sleep. The sticky `plans[].status`
is the actual fix: I had `blocked: cannot afford Hero` sitting there for a minute
and it cost me exactly one glance per cycle instead of drowning the wake channel.
When it halted, that was information, and I answered it (cancelled the Hero from
the queue and bought Footmen and Workers instead). I never once had gold banked
with nothing built — peak idle gold this game was about 560, for one cycle.

The r17 death — 2280 idle gold at 28/28 supply — could not happen: the supply
valve fired six times on its own, including three times I never read until after
the fact.

## Vocabulary usage

- **Plans (5 sent):** `boomer` opening, `army`, `prod` (re-set 3x as a rolling
  production queue with `after 15s` advances), `expand`, `expo`.
- **Triggers (6 armed):** `supply-capped` (recipe 7, armed in my very first
  batch), `home-guard`, `doorbell` (enemy_army_seen -> train Archer at Barracks 2),
  `fallback`, `strike` (unit_count Archer 10 -> push), `strike2`.
- **Regions:** `home`, `midfield`; `their base` from `map.places`.
- **Doctrine:** `template` on both Barracks and the Workshop (squad 1, retreat
  below 30%, focus Archer > Siege > Hero), squad-1 postures defend/forage/push.
  I gave almost no direct unit orders after the opening — the fight was won by
  policy, not micro.

## Top 3 complaints

1. **Build-site refusals cost me four separate cycles.** Farms, a Barracks and
   the expansion TownHall each bounced with "site blocked — nearest legal:
   (x, z)". The engine already computes the legal site; a `"snap":true` flag on
   `build` (or a trigger that auto-snaps) would turn four wasted round-trips into
   zero. This was my single largest tempo leak.
2. **`template`'s `retreat` block rejects `region`** while `retreat` as a
   standalone command accepts it. The brief says "every verb that takes x/z also
   takes region"; the nested one does not, and it cost a whole `plan_set`
   round-trip to discover (`cmd 2: unrecognized command (missing field x)` — a
   message that does not say which nested field, either).
3. **No way to write a looping production queue.** Plans are once-through and
   capped at 8 steps, so "keep both Barracks making Archers forever" meant
   re-sending `prod` by hand three times. A `repeat` on plans, or a
   `unit_count < N` trigger predicate (only `>=` exists), would let production
   be a policy like everything else. Related: I never got a Hero out, because
   the only lever I had over queue priority was cancelling it.
