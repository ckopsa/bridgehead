# AAR — red (rusher, Sonnet), round r21

**Result:** Victory for red (Claude) at t=219s. Enemy's Keep and all buildings destroyed; game ended with our 8-Footman army intact and only losing 1 unit the entire match.

**How it unfolded:**
- Opening: 5 workers split onto the nearby northeast mine and lumber; a Barracks went up immediately at (75,61), close to the front toward the enemy.
- Mid-game (t=57-138s): Steady Footman production from the single Barracks while economy scaled with two Farms (one proactive, one from a supply-capped trigger). Reached 8 Footmen by t=159s with no scouting resistance — the enemy never contested the map.
- Push (t=138s): Sent the squad on a `push` posture toward "their base" as soon as the army hit 6 Footmen — classic rusher timing, not waiting for tech or a hero.
- Contact (t=168-219s): Found their hero, Barracks, Keep, and a Farm all undefended by any combat units except workers. Set focus-fire priority on Hero, ground down their Barracks and Farm to rubble, chased their hero off (last seen at 92/320 hp), then sat on their Keep until it fell from 1700hp to 0 in about 45 seconds of uncontested sieging.

**Key decisions:**
- Pure rush strategy: single Barracks, continuous Footman spam, no hero investment, no tech detour — matched the "rusher" persona directive to end the game before it settles.
- Armed a `home-guard` trigger and `supply-capped` trigger early so doctrine handled base defense and farm construction between polls, freeing attention for the push.
- Set `priority: Hero` once the enemy hero was sighted, to punish the most valuable target first.

**Opponent behavior:** Blue never sent any army to meet ours, never attacked our base, and appears to have been building/teching passively while their hero and workers were essentially left to defend the base alone. No counter-push, no scouting contact, no harassment. This let a straightforward 8-Footman rush walk in and end the game in under 4 minutes with only one unit lost.

**Minor friction:** Initial `gather`/harvest commands used the wrong verb and target format (had to learn `harvest` needs a specific node `target` id, not a resource-region name) — cost a couple of early cycles with idle workers, but was corrected well before it mattered. The `supply-capped` farm-build trigger reused the same worker id and once hit a blocked-site error, which was harmless but showed the trigger's frozen-id caveat in practice.
