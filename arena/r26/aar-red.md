# Arena r26 — RED (Claude/kingdom) after-action report

**Result: LOSS.** `game_over: "Human"`, `game_over_reason: "surrender"` at t≈994s.
I conceded with 2 workers, no gold income, 6 army units, and an enemy field army
of 10 (8 Archers + Hero) roaming both live mines.

## 1. The match as I saw it

**Map:** `crossings`. Our base (70,70), their base (-70,-70), four mines, three fords.

**Opening (one batch + `ready`, compiled at t=0):**
- `rally` the TownHall onto the northeast mine, `stance squad 0 turtle`.
- An 8-step `boomer` plan: 3 workers on the NE mine, 2 on trees, Farm, 3 Workers,
  free Hero, Barracks.
- Four triggers: `supply-valve` (Farm on supply cap), `hero-save` (hero < 40% → home),
  `home-guard` (base attacked → turtle), `expand` (mine dry → TownHall at SE mine).

The economy opening worked exactly as advertised: by t=88 I had 11 workers, 1170 gold,
tier 1, and the supply valve was building farms on its own. Keep at t=137, Blacksmith,
Shop, 3 Barracks, Sanctum by t≈270; Weapon Smithing 1 and Armor Plating 1 both done by
t=311; Priestess (second free hero) at t≈280. Two repeating "steady production" triggers
(`make-foot`, `make-arch`, later `make-sorc`) kept three Barracks and a Sanctum busy
without me spending polls on it. At t=352 I was at 22 army units, str 2645.

**The decision that lost the match (t≈286).** I armed a `push` chain:
turtle → (7 Footmen) → stage at mid → (6 Archers) → push their base. Both advance
conditions were *already satisfied* when the plan reached them, so the chain fired
straight through in one tick and committed 13 units to a cross-map attack **with zero
intel** — explored 20%, empty intel ledger. I let it run rather than cancelling it.
At t≈374 my army hit their base, saw "~6", and then the real number arrived:
12 (6 Archer, 5 Footman, 1 Hero, later a Catapult). Between t=374 and t=389 I lost
the Priestess and about eleven units. Squad 1 went from 16 to 1.

**The second failure, at the same moment: the northeast mine ran dry (t≈356).** I had
put ~14 workers on one mine and never expanded while I was rich. The `expand` trigger
fired but a TownHall is 385g/205l and I was at 8 gold, because the production triggers
were converting every coin into units that were dying at their base. From then on the
game was a shuttle economy — workers walking 130 units to the SE mine and back to the
Keep at roughly 1–1.5 gold/second.

**The attempted recovery (t=600–990).** I cleared the production triggers to bank gold,
built a TownHall at the SE mine at t≈671 — and their raiders razed it and killed six
workers within 30 seconds (t≈694–698). I sent the remaining 9 units to secure it and
lost 5 more to a force that read as "4 spotted". Rebuilt to 7 workers shuttling to the
northwest mine; at t≈980 an enemy army of 10 (8 Archers) hit the northwest ford and
killed five of them in one sweep. Two workers left, no defensible mine, no income, no
army that beats 10 archers. I conceded rather than play out a decided position.

**Opponent behaviour:** patient. They never attacked my base once in 16 minutes.
They defended their own base with a consolidated force, let my push break on it, then
ran a mine-denial campaign — killing workers, never buildings. That is the correct
counter to what I was doing and they executed it cleanly.

**What I would do differently:** expand at t≈150 while rich, not at t≈670 while broke;
never let a `push` chain whose gates are pre-satisfied fire without a scouted intel
ledger; keep one cheap unit alive as a permanent scout instead of folding every loose
unit into the main squad (I did exactly that at t=264 and blinded myself).

## 2. The document as a tool

**What I actually used.** The `--doc` full render exactly once, at t=0, to learn the
place names, the stance table, the build/train domains with prices and tech gates, and
the predicate list. After that I ran `--digest` on every cycle (~35 cycles) and dropped
to raw `json.load` of `state.json` about eight times when I needed entity ids, mine
`remaining`, worker `why` strings, or squad membership. I sent **raw intents almost
exclusively** — the opening batch, plans, triggers, stances were all hand-written JSON.
I copied no link verbatim and filled no form. The doc's value was as a *reference sheet
at t=0*, not as a per-cycle action menu.

**Annotations that changed a decision.**
- `alarms[].running_default` twice, and both times the useful half was the "nothing"
  wording: *"income collapse … none of your 14 workers is on gold — nothing recovers
  this automatically"* is what told me the expand trigger had fired-and-failed rather
  than fired-and-worked. Later, *"no armed trigger covers a sighting"* on the
  10-archer alarm was the sentence that made me concede instead of counter-attack.
- The alarm `fact` line carrying the enemy composition (`enemy army of 10 (8 Archer,
  1 Footman, 1 Hero)`) was the single most decision-relevant string in the whole match.
- `squads[].status: "gathering"` vs `"pressing on"` caught a real bug in my own play:
  squad 1 sat "gathering" for 40 game-seconds at the center ford because a **Worker**
  had been enrolled into it by a `template` I set on the Keep. Nothing else in the
  snapshot would have told me that. The brief's note that a stalled gather means "the
  tail keeps being refilled" pointed me at the roster immediately.
- The `WIN raze their production: N seen … M remembered, oldest 603s ago` line, with the
  staleness clock, correctly stopped me from treating a 10-minute-old sighting as news.

**What misled me or was noise.**
- **A silently ineffective `build`.** Three separate `{"type":"build","select":"workers",
  "kind":"Barracks","region":"our base","site":"nearest legal site"}` commands were
  *accepted* (the plan step even logged "step 7/8: worker workers builds Barracks at the
  nearest legal site to our base") and no Barracks ever appeared, no error was raised.
  The same command with an explicit `"worker":<id>` and explicit `x`/`z` worked first
  try. Whatever the cause — a busy lowest-id worker, or the site search — an accepted
  build that produces nothing and reports nothing is the worst possible failure mode:
  it cost me roughly 90 seconds of Barracks timing and I only noticed by diffing the
  buildings array by hand.
- **`return` and `harvest` on out-of-range workers.** Twelve workers sat `why: "idle",
  carrying: true` at the southeast mine with no hall in range. `{"type":"return"}` was
  accepted and did nothing; only an explicit `move` to `our base` followed by `return`
  got the gold banked. Neither the digest nor the alarm said "your workers have cargo
  and no reachable dropoff" — the income-collapse alarm said "none of your workers is on
  gold", which was true but not the actionable fact.
- **Repeating-trigger error spam.** `trigger:make-foot: cannot afford Footman` in
  `ERRORS` on most cycles is expected behaviour for a repeat rule, but it crowds the two
  error slots the digest shows, so a *real* refusal can be pushed out of view.
- The `"squad N has no members - the stance is set and waiting"` message is correct and
  helpful the first time and pure noise on every subsequent stance-before-enrol batch.

**What the document should serve and does not.**
1. **An economy line with a derivative.** `RESOURCES gold 165` tells me the level; I
   needed *gold per second* and *mine remaining at the mine my hall works*. The single
   fact that would have saved this match is "NE mine: 900 left, ~3 min at current rate"
   printed before it hits zero, not an alarm after. `mine_dry` as a predicate fires too
   late by construction; the doc should print the countdown.
2. **A push gate that is actually gating.** The brief documents push gates (consolidated
   squad, size 6, hero 80%) but my `push` chain fired through both `advance` conditions
   in one tick because they were already true when their turn came, and nothing warned
   me. A plan step whose `advance` predicate is *already satisfied at plan_set time*
   should say so in the `plan_set` echo — "step 1 will advance immediately" — the same
   way an unresolvable region name is flagged.
3. **Intel staleness on the alarm, not just on the WIN line.** `enemy army spotted: ~4`
   and `enemy army of 10` were the same force minutes apart; the digest's ARMY-sighting
   events do not carry a confidence or a "this is what you can see, not what is there"
   qualifier the way the `intel:` block under a link does. I acted on "~4" twice and was
   wrong twice.
4. **Worker-assignment truth.** `workers 7 (idle 1)` is not enough. I want
   `workers: 5 gold (avg round trip 130m), 2 lumber, 1 idle` — the shuttle distance is
   the number that decided the last eight minutes of this game and it was nowhere on
   the page.
5. **A squad roster warning for non-combatants.** "squad 1 contains 1 Worker" belongs in
   the digest, next to the `gathering` status.
