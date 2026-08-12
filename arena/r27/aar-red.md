# After-Action Report — RED, arena round r27 (crossings)

## Result
LOSS by surrender at t=791s. Opponent (Blue/Human) wins.

## Part 1: The match as I saw it

**Opening (t=0).** Sent economy orders (workers to nearest mine), queued two
extra workers, put a Barracks down at "our base" (first attempt hit a blocked
site and had to be retried with `"site":"nearest legal site"`), stanced squad 0
`turtle`, and readied. Match started cleanly once both seats readied.

**t=0-270s: solid econ opening.** Textbook build: Barracks -> template squad 0
so trainees auto-enrolled -> second Barracks -> Farm for supply -> Keep upgrade
at t=274s (tier 2 by t=314s). Gold piled up fast (peaked ~3000+) because I was
training army faster than spending on buildings; lumber also ballooned once I
redirected idle workers to trees. By t=270s I had a 10-unit army (str 1370,
100% hp) sitting on turtle at home, completely idle and unengaged — the single
biggest wasted-tempo window of the game.

**t=214s: first mine dries up** (the only gold mine near home). I reacted by
building an expansion TownHall at the southeast mine (a good decision — it kept
a second income stream alive later) and shifting home workers to lumber.

**Key decision, t=294s: sent the 10-unit home army to stage at mid** instead of
leaving it on defense. This was the pivotal mistake. The enemy had scouted (or
independently pushed) a comparable force to the same choke (center ford) and we
traded roughly evenly there, but I lost my hero and several units for it while
the enemy kept coming. Worse, my base was now unguarded for the counter-push.

**t=336-402s: enemy counter-attacked my main base** while my field army was
dead/retreating. Base defense held on turtle stance (bought time, killed some
attackers) but steadily bled down as reinforcements trickled in one at a time
rather than as a concentrated force — the classic "feed the meat grinder"
failure mode. Second hero (Priestess) died at t=524s.

**t=552s: all four map mines confirmed dry** (`MINES: ...:0` for every entry) —
a hard, permanent economic wall (confirmed against the brief: mines do not
respawn). From this point the match was a fixed-gold-bank war of attrition. I
still had ~1000g banked and pushed toward "their base" with a rebuilt 12-unit
army at t=616s, but the push stance pulled the army back home mid-transit to
answer another incoming raid, and that army was ground down to zero by
t=684s in the base defense that followed (lost Keep, both Barracks, Tower, Farm,
Blacksmith in quick succession between t=682-712s).

**t=714s: main base fully destroyed.** Only the expansion TownHall (southeast,
untouched, full HP) survived, with 7 workers and 125 gold. Because every mine on
the map was permanently dry, and Barracks costs 160g (Footman 135g), I had no
way to ever field an army-producing building again — gold was structurally
capped at 125 with no route to more (no live mine, no army to fight for bounty
caches, nothing lumber-only that produces combat units). Lumber bank was
useless without gold to pair it with.

**t=791s: surrendered.** No income path, no army, no realistic way back — the
honest call per the brief's guidance rather than dragging out a decided game.

## Key decisions, ranked by impact
1. **(Worst) Sending the full home-defense army to stage/push at t=294s** while
   my base still needed it, right as the map's gold was about to run dry —
   turned a strong 100%-hp 10-unit army into a wasted trade and left home open.
2. **Feeding reinforcements piecemeal** into base defense (1-2 Footmen per
   train order under fire) instead of banking a war chest and committing a
   full fresh squad at once — repeated `squad 0 wiped` events at t=344s,
   t=524s(ish), t=684s suggest units kept arriving too late/too few to matter.
3. **(Good) Expanding to the southeast mine early (t=294s onward)** — this
   expansion outlived the main base by a wide margin and was the only thing
   keeping the faction alive after t=714s. Should have built the SECOND
   Barracks there proactively as insurance the moment the main base first came
   under serious pressure (~t=372s), instead of only after it was too late.
4. **Not noticing the mine-exhaustion trajectory soon enough.** With 4 mines on
   a symmetric map and no respawn, dry mines are a hard deadline, not a
   temporary event — should have treated the first `mine_dry` alarm at t=214s
   as a signal to bank gold for tier-3 upgrades and army mass BEFORE the wells
   ran out, not just as a worker-reassignment task.

## Part 2: The document as a tool

**What I used, and how often (~48 command batches, ~35 wait/view cycles):**
- **Digest view (`--view --digest`)** was my workhorse — used on essentially
  every cycle after the opening. It was fast to scan and the "DEFAULT if you
  say nothing" line was genuinely load-bearing: I relied on it every quiet
  cycle to confirm doctrine (turtle stance, production queues) was still
  running without me re-sending anything, which it always was.
- **The affordance doc (`--doc`)** was used exactly twice: once at match start
  to learn the command surface, and once mid-game to inspect the `build`
  action's site/cost table when deciding whether to afford a Keep upgrade. I
  did not lean on it during play — the digest plus my own memory of the brief
  covered almost everything, and the doc's verbosity (150+ lines for the full
  action list) made it slower to re-consult mid-crisis than just sending a
  known-good command and reading the error.
- **Links verbatim vs hand-written intents:** mostly hand-written intents
  built from selectors (`"select":"idle workers"`, `"select":"my barracks"`,
  `"select":"my hall"`) and stances (`turtle`/`stage`/`push`) rather than
  copy-pasting doc action templates. The selector vocabulary (`idle workers`,
  `nearest mine`, `nearest legal site`, `my hall`) did almost all the real
  work and was reliable throughout — never once produced a wrong-target
  surprise.
- **Forms:** not used as forms per se; I read the stance/region tables once at
  setup and then wrote raw JSON from memory for the rest of the match.
- **Raw intents:** used for everything not covered by a one-word stance —
  `upgrade`, `cast` (CallToArms, which needed a `caster` id since `select`
  is not a valid channel for `cast` — this produced one avoidable error;
  the brief's own text does say `cast` takes `hero`/`caster`, but I first
  tried `select` out of habit from other verbs, and paid a wasted cycle).

**Annotations that changed a decision:**
- The `income_collapse` alarm (`the one gold mine your hall works is dry`)
  directly triggered my decision to redirect workers to lumber and then to
  scout/build an expansion hall at a live mine — this was the single most
  useful alarm in the match.
- `enemy_army_sighted` alarms with unit counts (`enemy army of 14 (13 Footman,
  1 Hero)...`) were what I used to gauge whether to hold turtle or reinforce;
  the size numbers were trustworthy and matched the casualty counts I saw a
  few seconds later.
- The squad `status` field (`"gathering"` vs `"pressing on"`) was checked once
  when a push stance appeared stuck — useful, confirmed the squad genuinely
  was moving even though the digest's cached position line looked stale for
  two consecutive polls (`pressing on ... near our base` when it had actually
  engaged 15 units back home). That mismatch (digest naming a stale anchor
  place instead of updating to wherever the squad actually is/was fighting)
  was the one place the document actively misled me — I spent a cycle
  confused about whether my push order had even been accepted before I
  cross-checked raw unit positions.

**What misled or was noise:**
- The digest's `SQUAD 0 push (near their base), pressing on` line kept
  reporting the *target* name even while the squad was, in fact, standing at
  home fighting off a raid — the "near X" phrase reads as current location but
  is actually the stance's anchor/target label. This cost me a diagnostic
  cycle (had to drop to raw JSON + per-unit positions to find out where the
  army really was).
- Repeated identical `train: 'idle barracks' matches none of your finished
  buildings — all N already have something queued` errors were technically
  correct but not useful signal — once queue depth (7) is understood, this
  refusal fires constantly during normal play and is closer to noise than an
  alarm; it never once indicated an actual mistake, just eagerness.

**What the document should serve that it does not:**
- A **fuel/runway forecast** would have been the single highest-value addition
  given how this match ended: a line like "gold-per-second: 0 (all N mines
  dry); at current spend rate you can field K more units" would have told me
  at t=552s (when the last mine ran dry) that my banked ~1000g was a hard
  ceiling on remaining army production, rather than something I only realized
  by inference after the fact. The existing `income_collapse` alarm reports
  the *event* but not the *consequence* (how many more units that bank can
  buy).
- **Squad location vs. squad target** should be two separate lines in the
  digest. Right now `SQUAD 0 <stance> (near <anchor>), <status>` conflates
  "where I told it to go" with "where it actually is," and during a fast-moving
  fight that ambiguity cost real decision time.
- A rough **army trade-efficiency signal** (e.g. "your last engagement: lost 4,
  killed ~2" inferred from before/after enemy sighting deltas) would have
  flagged the staged-army trade at t=336-346s as a bad trade in real time,
  rather than something visible only in hindsight from the event log.
