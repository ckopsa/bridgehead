# AAR — red (rusher, Sonnet), round r22 (rematch of r21)

**Result:** Defeat. Blue's counter-army destroyed both our Barracks and then our TownHall around t=1130-1148s.

**How it unfolded:**
- Opening was slowed by an early tooling mistake: my first command batch (harvest/build/train/triggers) was overwritten by a `ready` sent immediately after, before the engine consumed it — the opening was effectively lost and had to be resent from scratch once the match had already started, costing real time.
- Rebuilt a standard rush: Barracks up by ~t=73s, steady Footman production, a scout skirmish at t=148s caught Blue's hero alone and chased it down to 20/320 HP — a promising start.
- First real push (t=170-220s) with 5 Footmen ran into a prepared Blue army (4 Footman) at their base and was wiped for a 4-1 unfavorable trade. Blue had clearly read its own r21 AAR and built a defensive hedge this time.
- Rebuilt to 13 mixed Footman/Archer, pushed again around t=430-450s, won a midfield engagement outright (10 vs 1), then pushed toward their base. This second push stalled badly at the map's fords — the `posture:push` order oscillated near the center and southeast fords for nearly 400 seconds without making net progress (a squad-cohesion/pathing issue), wasting most of the tempo advantage the persona depends on.
- Switched to direct `attackmove` orders once push posture proved unreliable, which did get the army moving, but by the time it reached Blue's base (t~1080s) Blue had massed a much larger combined-arms force (15 Archer, 4 Footman, Hero, Priestess) — my 12 units were destroyed in seconds.
- Blue then counter-pushed with a 22-unit army (15 Archer, 5 Footman, Hero, Priestess) straight into our undefended base, destroyed both Barracks within minutes, and then the TownHall, ending the game.

**Key decisions:**
- Correctly read both prior AARs: expected Blue to hedge on defense this time, so opened with the same rush timing but was prepared to punish a lone scouting hero — this worked once.
- Misjudged the escalation: kept committing footman/archer waves into an opponent that had teched to a Keep and a 15-archer ranged army, without matching composition (no siege, no comparable mass) or building a proper standing defense at home before pushing again.
- The `posture:push` pathing stall across the fords was the costliest single technical issue — it burned roughly 300-400 seconds of tempo, which is fatal for a rusher persona whose entire thesis is speed.

**Opponent behavior:** Blue (the former "boomer" persona from r21) adapted hard: built defense early, scouted with its hero, teched to Keep, and out-massed with an archer-heavy composition (15 Archers by the end) that punished melee-heavy pushes. Once its army was large enough, it counter-pushed directly into our economy-only base and ended the game — a mirror of exactly how red won round r21, this time executed by blue.
