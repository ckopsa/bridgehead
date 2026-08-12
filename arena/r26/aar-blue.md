# Arena r26 — BLUE (Human/kingdom) after-action report

**Result: BLUE WINS — opponent surrendered at t≈994s** (`game_over: "Human wins (surrender)"`).
Map: `crossings`. Doc version: affordance-doc/1.3.

## Part 1 — the match as I saw it

### Opening (t=0, one batch)
Read the map (canyon river, three fords, flank fords are the neutral mines), then sent a single
batch ending in `ready`: a `home` region, `stance squad 1 turtle` at our base, an 8-step `boomer`
plan (3 workers on the southwest mine, 2 on trees, workers → Farm → Barracks → free Hero), and
three triggers: `supply-valve` (farm on supply cap), `hero-save` (hero_below 0.35 → move my hero
to our base), `home-guard` (base_under_attack → turtle). The whole plan was accepted inside 2
game-seconds — the plan tier did in one command what would otherwise have cost six polls.

### Economy phase (t=0–300)
Keep at t=140, Blacksmith, Workshop, three Barracks, Weapon Smithing 1 at t=306. By t=300 I had
17 workers, ~1700 banked gold and 12 army units. **The mistake that decided the shape of the whole
match happened here: I never expanded before my only mine went dry at t=297.** I had the gold for
a second hall from t≈240 and spent it on army instead.

### First defence (t=353–390)
Their all-in arrived: 12 (6 Archer, 4 Footman, Hero, Priestess) at (-44,-60), growing to 16. I lost
a Barracks, a Farm and most of my worker line, but `secure` at our base plus the Keep won the fight
outright — hero went L1 → L4 in twenty seconds, their army broke and ran to the center ford.

### The counter-punch that cost me the game (t=398–465)
I cleared `home-guard` and sent `stance push their base` with 11 units against a force I had scouted
as ~5. By the time I arrived they were back to 8 with a Sorcerer, and their base was reinforcing.
I lost five Footmen in five seconds, the hero dropped to 14%, and the push became a rout. With my
home mine already dry, that traded my only army for nothing while I had no income to replace it.

### The trough (t=465–760)
115 gold, no mine, one worker. I long-hauled workers from home to the southeast mine (a ~260-unit
round trip) at roughly 60–80 gold/minute, saved to 405, and built a TownHall at the southeast mine
at t=686. Their army caught the expansion at t≈722: squad wiped, hero dead, seven workers killed.
I was one worker and 350 gold. I did not surrender because the SE TownHall survived — that hall
turned out to be the whole match.

### The recovery (t=760–994)
`rebuild` plan pumped workers out of the SE hall onto the fresh mine. Income restored inside 20s;
850 gold by t=841; hero revived (L5) at t=869; a 585-gold bounty at t=882. From there: 16 army
units, hero L6, 15 workers, 720g/495l, and squad 1 holding the northwest ford while squad 2 turtled
home. The opponent, who had spent two armies to kill mine twice and never finished the job,
conceded at t=994 with my army whole and my third expansion going down.

### What won it
Two things. (1) The engine's doctrine tier: `turtle`/`secure` stances plus `hero-save` did all the
defensive fighting between my polls, including the t=353 defence I never issued an order for.
(2) Refusing to concede at t=742 — one surviving expansion hall next to a live mine was a real
income path, and the opponent gave me the ninety seconds it needed.

### What nearly lost it
Not expanding before the mine dried, and a push launched on 30-second-old intel against a base
that reinforces. Both are the same error: acting on the map I remembered instead of the one in
front of me.

## Part 2 — the document as a tool

**What I actually used.** Almost entirely `--digest` (every cycle, ~35 of them) and raw intents.
I read the full `--doc` exactly once, at t=0, and never again — it is a good orientation page and
a poor loop page, because at 15-second cadence the digest's eight lines carry the decision and the
doc's 15 actions do not. I sent **zero links verbatim** and **zero forms as served**; I wrote every
command by hand from the brief. I used the doc's `domain` blocks twice as a price/tech lookup
(build kinds with costs, train kinds with supply) — that was its most valuable service to me, and
it is the part I would put in the digest.

**Annotations that changed a decision.** Three. The `DEFAULT` line ("nothing recovers this
automatically") on the income-collapse alarm at t=318 is what made me treat a dry mine as an
emergency rather than a status; the alarm's `running_default` naming `hero-save` at t=454 told me
retreat was already in flight so I did not waste a cycle re-ordering it; and `squads[].status`
`gathering` vs `pressing on` told me at t=447 that my push had strung out — I read it too late,
but it was the right sentence in the right place. The WIN line's "3 remembered, oldest 234s ago"
staleness stamp is excellent and I under-weighted it: I pushed at t=398 on intel already ~30s old
and it was wrong by a factor of two.

**What misled me or was noise.** The `PRODUCTION … building: TownHall(0%)` field is a lie of
omission — it showed a hall "building" at t=686–715 that was never in `buildings[]` and never
existed. Worse, three separate expansions and one Blacksmith silently vanished because
**`select:"workers"` always resolves to the lowest-id worker**, so a `build` and any other
worker-directed order in the same batch — or the `supply-valve` trigger firing a second later —
retarget the same body and the earlier build is discarded *with no error line at all*. That single
behaviour cost me ~150 game-seconds of expansion and, indirectly, the match's midgame. Nothing in
the digest or the doc reports "an accepted build order was overridden before ground was broken."

**What the document should serve that it does not.**
1. A **construction ledger**: ordered / walking / laying foundation / building%, per building, with
   the assigned worker id — so a stolen builder is visible instead of silent.
2. **Selector collision warnings** at compile time: "cmd 2 and cmd 4 both resolve to worker X".
3. A **gold-runway line** in the digest: bank, income per minute, and mine remaining per hall.
   `income_collapse` fires when it is already too late; "your only mine is 20% depleted" is the
   alarm that would have made me expand at t=240.
4. **Steady production** as an armable rule, not a recipe I re-send by hand: there is no predicate
   for "a producer is idle", so keeping three barracks working cost me a command every single
   cycle — the one thing in this match I genuinely wanted a trigger for and could not express.
