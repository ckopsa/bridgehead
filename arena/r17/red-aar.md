# Arena Round 17 — RED (Horde) After-Action Report

**Result: RED (Horde) WINS.** Match length: ~435s of game clock (I took my seat at t=49s,
so roughly 6.5 minutes of play). Win condition: all Kingdom production razed — their
Barracks, Tower, Blacksmith and Keep fell to one 21-unit Horde push; only Farms and a
worker remained when the game ended.

## How it unfolded

- **t=49–100 — one plan, whole opening.** First command batch was a `plan_set "boomer"`
  (3 peons to the near mine, 2 to trees, 4 peon trains, WarCamp at 58,58, Warchief),
  plus `region_set "home"` and a `base_under_attack` home-guard trigger. The whole
  opening executed in ~2 seconds of engine time across 8 steps. That is the single
  biggest thing that happened all match.
- **t=100–180 — the Warchief mistake.** I queued a Warchief early. Because gold is
  deducted on completion and the WarCamp queue kept draining the treasury, the Warchief
  sat in the Fortress queue for ~130 seconds doing nothing while my Fortress upgrade
  blocked on `cannot afford`. I eventually cancelled it and never fielded a hero at all.
  I won a hero-less game against a Kingdom that had one.
- **t=180–300 — mass.** Fortress → WarMill → SpiritLodge, then Grunt/Headhunter pairs on
  a loop out of the single WarCamp, Burrows for supply, attack and armor research. A
  `template` on the WarCamp stamped squad 1 + retreat-below-30% on every unit as it
  spawned, so I never issued a per-unit order.
- **t=300–435 — the push.** `strike` trigger (`unit_count Grunt >= 8`) fired the army
  toward their base on its own. My squad picked up a 315g bounty at mid en route. Their
  main mine ran dry around t=380 while mine still had 900; that was the game. The line
  walked into their base and ate Barracks → Tower → Blacksmith → Keep in about 50 seconds
  of contact. Their hero died in front of my army (my `enemy_hero_down` trigger fired and
  re-pushed), their counterattack never materialised beyond a lone Spearman and a hero
  poke that killed one Grunt at (-19,-19).

## How the Horde felt

**Strong.** The Grunt/Headhunter pair is a genuinely excellent equal-gold brick: 165 HP
Grunts absorb, Headhunters at 17 damage delete Archers and would have answered air.
Burrows are a great supply unit — free-ish perimeter teeth and I could scatter nine of
them without thinking. The WarCamp trains the entire ground army from one building,
which made my whole army policy exactly one `template` command; the Kingdom's split
Barracks/Stable/Workshop cannot do that. Two heroes at Fortress and cheap revival is
generous on paper.

**Weak, exactly as advertised.** Slow. My army crossed the map at a crawl, and the
`push` posture's cohesion made it look like it was going backwards for two polls while
stragglers caught up. I had no answer to a maneuvering opponent — if Blue had raided my
peons with Knights while my brick walked, I would have been in real trouble; the game
was decided by the fact that they simply did not.

**Missing / what I never got to use.** No Wolfriders, no Demolishers, no Wyvern, no
Shaman Bloodlust that I ever saw land, no hero. Gold, not lumber, is the Horde's binding
constraint — I ended with 500 lumber and 197 gold and a dry mine. That means the
lumber-heavy tier-3 kit (Wyvern 265/110, Demolisher 170/110) is priced for an economy
I never built. The 5x-vs-cavalry Impaler is a lovely idea I had no reason to touch.

## Vocabularies used, and whether they earned it

- **Plans — yes, decisively.** Five plans (`boomer`, `army`, `tech`, `mid`, `pump`/`pump2`/
  `pump3`). The opening plan compressed six polls into one. `blocked:` status was honest
  and readable every time.
- **Triggers — yes.** `home-guard` (never needed to fire, correctly), `doorbell`
  (fired once, and I cleared it because it would have yanked my push home), `strike`
  (`unit_count Grunt>=8` → push; fired and launched the winning attack without me),
  `their-hero-down` (fired at t=390 and re-pointed the army at their base the moment
  their hero died — I learned their hero was dead FROM the trigger line, not from a poll).
- **Regions — yes, lightly.** One region `home`, used as the anchor for home-guard and
  squad 2's defend. `map.places` (`mid`, `their base`) did more work than my own region.
- **Intel — yes.** `enemy army spotted: ~7 (3 Archer, 2 Footman, 2 Spearman)` and the
  remembered enemy building list (including watching their mine tick to zero) is what told
  me the push was safe. My scouting peon died buying that first sighting; worth it.

## Top 3 complaints

1. **Queued heroes silently starve the economy.** A 400g hero sitting in a hall queue
   competes for gold with everything else for as long as it takes, with no indication in
   the snapshot that it is the thing eating my income. I lost ~130 seconds of tech to it
   and only diagnosed it by staring at `q=[Warchief]` not moving. A hero in queue should
   either reserve its cost up front or report "waiting on gold".
2. **Build placement is trial-and-error by error message.** I burned five separate polls
   on `site blocked ... nearest legal: (x, y)`. The error helpfully names a legal site —
   so just build there, or let me pass `"snap":true`. Every rejection cost a full cycle.
3. **`push` cohesion fights reinforcement.** New units auto-joining squad 1 dragged the
   whole push backwards to gather them, twice. The fix (a `template` into a separate
   home squad) is discoverable but non-obvious; a squad should probably have a "closed to
   reinforcement while pushing" property, or `push` should gather only members within
   some radius of the vanguard.

Honorable mention: the squad centroid in `bridge_view` is actively misleading when two
members are at home and nineteen are in the enemy base — I nearly issued a panic order
before dropping to raw `state.json` to see per-unit positions.
