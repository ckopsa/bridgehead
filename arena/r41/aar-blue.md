# After-action report — arena r41, BLUE (Human/kingdom), crossings

**Result: Win by Red's surrender at ~t=1274.** My base and army were intact (14 units, 11 Footmen + 3 Archers, all at full health, turtled at home); Red conceded rather than play out a dry-mine endgame. Autopilot was never used.
Seat: bridge/blue, model claude-opus-5, scaffold affordance-doc/2.2, NO playbook (ki4i pair, bookless half).

**How it unfolded**

- *t=0–60s* — Readied immediately, then opened with a free Hero, a Barracks, and Farms. My first real mistake was here: I did not order harvesting in the opening batch, so at t=20 the engine raised an "income collapse" alarm with all five workers idle. Fixed at t=27 via a `harvest` on `idle workers` → `nearest mine`.
- *t=60–180s* — Built the economy on standing policy rather than micro: repeating `game_time` triggers for worker and army production, a `supply_capped` trigger that auto-built Farms, `home-guard` (base_under_attack → turtle), and `hero-save` at 35%. A second Barracks went up around t=95. This ran itself for two minutes while I only watched the digest.
- *t=170s* — A `go-mid` timed trigger staged squad 1 at the center ford. Still zero contact; I had explored 20% and had no intel at all.
- *t=240s* — Sent the squad to `harass` their base. That bought the win-condition intel (their TownHall and Barracks seen) and revealed only ~9 defenders against my 14.
- *t=248–262s* — Converted harass into a full `push`. First engagement traded well (their 11 → 8, I lost one Archer), but at 68% pooled health I pulled back to `secure` at mid rather than feed into their base.
- *t=330–380s* — Rebuilt to 17 units and pushed again. This second push was my worst decision: the squad spent ~20s "gathering" at the ford because my `enrol` trigger kept adding freshly-trained units to the attacking squad, exactly the tail-refill trap. When it did arrive it ground down against their defense, and my Hero (L3) was chased home and **died at t=407**.
- *t=397–420s* — Red counter-raided my base with 5 Footmen. `home-guard` fired correctly, squad 1 turtled, and I repelled it cleanly — back to 13 Footmen at 95% within half a minute.
- *t=446–744s* — The real crisis was economic, and self-inflicted. My southwest mine hit 0% at t=658. Worse, my `expand2` trigger fired every 15s, grabbed workers to build a TownHall it could not afford, and left them idle; combined with a depleted tree, I sat at **+0 gold/min with 11 idle workers for roughly 90 seconds**. Repeated `harvest` orders by unit id did nothing because I was using stale ids.
- *t=631–643s* — The fix that worked: a two-step `plan_set` (`move workers to southwest mine`, then `harvest nearest mine`). Moving them first unstuck the whole worker line and dumped 100 banked lumber into the treasury. Lesson: a stuck worker needs a move, not another harvest.
- *t=744–1274s* — Attempted the northwest-mine expansion twice; both attempts were abandoned ("worker could not reach the site", t=1039). I held a full-health defensive army at home, spent the banked gold on Archers, and Red surrendered before the dry-mine endgame resolved.

**Opponent behavior:** Red played a home-defense game — never contested mid, never showed at my base before t=397, then used repeated 5-Footman raids to harass my economy while defending their own base successfully against two pushes. They killed my Hero. They surrendered while I still held a healthy 14-unit army.

**How I structured my game:** Almost entirely on the *standing-policy* vocabulary rather than per-unit micro — `trigger_set` for production pulses, supply, home defense and hero preservation; `stance` on a single consolidated squad for every army decision (turtle → stage → harass → push → secure → turtle); `template` to auto-enrol new units. That let me spend one short command batch per cycle. Where it bit me was that policies compose badly when they fight each other: the enrol trigger fought the push's cohesion rule, and the unaffordable expand trigger fought the harvest orders. The two things I would do differently are order harvesting in the very first batch, and never leave a repeating build trigger armed that the bank cannot pay for.
