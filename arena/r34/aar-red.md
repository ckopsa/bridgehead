# After-Action Report — RED — Arena Round r34 (crossings, kingdom vs kingdom)

## Result: LOSS (t≈387s). All RED production buildings destroyed; opponent's
army (~9-14 units, including a levelled hero) overran the base. Game ended
with "GAME OVER: Human wins" (the label the engine used for the opposing
seat/bot).

## Part 1 — The Match

### Opening (t=0)
Sent as one batch before ready: worker-pulse (train Worker every 18s), free
Hero, home-guard/hero-save/expand recipes, squad 0 enrollment + turtle
stance, then `ready`. This followed playbook step 1's "take the free hero"
fork in spirit (queued Hero directly in the batch) while also keeping the
worker pulse running — a hybrid of exits 2 and 3 rather than a clean pick.

### Early game (t=0–200s)
- Mistake caught fast: I forgot to issue an initial `harvest` order. Workers
  sat fully idle until t=~20s when I sent `harvest select:"idle workers"
  target_select:"nearest mine"`. Cost roughly 20 seconds of income — an
  unforced error the playbook's own warning ("harvest first") would have
  caught had I referenced it before the opening batch.
- Lumber was chronically under-supplied for most of the match. Because
  `harvest select:"workers"` reassigns the WHOLE workforce (there is no
  partial-N selector), I kept lurching between "all gold" and periodic
  corrective pulses to trees, rather than holding a steady 70/30 split. This
  produced repeated "cannot afford Barracks/Workshop/Tower" refusals even
  while sitting on 1500-2000+ gold.
- **Key bug I introduced myself**: at t≈145s I set `template:"my TownHall"
  squad:0`. Because a template on a TownHall applies to EVERYTHING it
  trains — including Workers — this silently enrolled newly trained workers
  into squad 0. When I then set squad 0 to `harass` targeting `mid` (an
  attempt to scout), the whole workforce (11 units) was yanked off the mines
  and marched into the open, triggering an "income collapse" alarm and
  losing at least one worker to the enemy hero it stumbled into. I caught
  and fixed this within one cycle (cleared the TownHall template, moved
  workers to their own squad 1, reset squad 0 to turtle), but it cost
  economy and revealed the enemy's position at the same time — a bad trade.
- t≈193-220s: first real contact. Squad 0 (army only, correctly separated
  by then) met a 5-unit enemy force near the center ford while in `turtle`
  stance, which pulled it home as intended. We won that skirmish outright —
  killed the enemy hero, held the base, only lost 2 archers ourselves.

### Mid game (t=220–330s)
Economy scaled well on paper: 12-13 workers, 4 farms, 2 Barracks, a Tower,
gold banked past 2500 with lumber recovering. Squad 0 sat on `turtle`
defending home. This period looked winning — bigger bank, functioning
triggers (steady-production, army-pulse), army growing to 9 units with a
full-health hero.

### The collapse (t≈330–387s)
The opponent had been building a much larger force off-screen (fog of war —
we never scouted their base after the early skirmish). At t=333s an alarm
reported 8, then 13, then a peak of ~14 hostiles converging on our base at
once — nearly double our field army, plus a hero that had clearly leveled
substantially (440 HP vs our fresh hero's 320). Squad 0 (8-9 units) was
wiped in roughly two combat ticks. The hero-save trigger fired but the hero
died anyway (health dropped from safe to dead faster than the 45s trigger
cooldown / retreat could resolve). From there it cascaded fast and
irreversibly:
- t=342 hero died, squad 0 wiped
- t=351-380: Tower, all 4 Farms, both Barracks, and finally the TownHall
  itself were destroyed in sequence — under 40 seconds start to finish.
- With the TownHall gone, worker production and hero revival were both cut
  off; a Hail Mary attempt to found a new TownHall at the southeast mine
  failed for lack of lumber (150l on hand vs 205l needed — the same
  lumber-shortfall pattern from earlier in the match, now fatal).
- t=387: game over.

### What won it for the opponent
Patience and mass. They absorbed one lost skirmish (their hero died at
~t=213s) without panicking, kept building at a scale we never detected
(explored map % never grew because I did not commit to sustained scouting
after the early harass mis-fire), and returned with a force roughly 1.5-2x
our standing army plus a hero far ahead of ours in level. We had no
intelligence on this buildup — the WIN line ("raze their production: none
seen yet") stayed unmet the entire game; we never saw a single enemy
building.

### What lost it for us
1. No sustained scouting/vision after the early skirmish — we had zero
   warning of the size of the second wave until it was already at our
   walls (alarms fired at 4s and 8s notice, far too late to reinforce).
2. Turtle stance concentrated our whole army at home but did not compensate
   for being outnumbered nearly 2:1 when the fight actually arrived — we
   needed either a much larger standing army (lumber-starved production
   held us back) or actual walls/more towers, which lumber shortage also
   blocked.
3. Chronic lumber mismanagement (self-inflicted, see above) repeatedly
   blocked Barracks #2, the Workshop, and Tower builds, and directly caused
   the fatal final TownHall rebuild to fail.
4. The one-shot `template:squad` mistake cost a worker and briefly broke
   the economy at a moment we could not afford it.

## Part 2 — The Opt-Out Playbook As A Tool

**Playbook**: `standard-kingdom`, declared in `bridge/red/prefs.json` at
start, never edited or opted out of during the match.

**How far I got**: Only step 1/10 ("Ten workers before anything clever") was
ever rendered — I never advanced past it in the fold, though in practice I
went well beyond its literal scope (Barracks, army, expansion trigger, second
Barracks) by taking the RAW/off-book route rather than by the fold
mechanically advancing. The doc kept re-showing step 1 in every subsequent
poll because supply/worker-count conditions kept re-triggering its
INVALIDATED state rather than the step formally completing — I never sent a
command that satisfied "Worker 5/10" cleanly (Farm and Hero exits were taken
essentially simultaneously in the opening batch, which the fold treated as
still-open). I did not go back to check whether steps 2-10 ever unfolded;
in hindsight I should have looked at the later steps explicitly instead of
letting play run entirely off-book from mid-game onward.

**Gates that held me honestly**: The `cannot afford` refusals (Barracks,
Workshop, Tower, and fatally the rebuild TownHall) were real, useful
signals — every one of them was lumber-driven and every one of them was
correct given the state of the bank. They repeatedly told me the truth
(lumber, not gold, was the constraint) but I did not fully internalize the
fix (a standing lumber allocation) until it was too late to matter.

**Gates I jumped**: I went straight to off-book intents (`build`, `train`,
`trigger_set` with custom names/cadences) rather than working through the
playbook's own forks for steps 2 onward. This was a deliberate choice for
speed, not an accident — the RTS playbook explicitly allows this
("Off-book is legal and unflagged").

**Exits taken**: In the opening batch I effectively took playbook step 1's
exit 2 ("take the free hero now") and exit 3 (continue the worker pulse)
simultaneously, plus jumped straight to farm-building reactively later
rather than via the offered exit 1. No formal `EXIT` command token was ever
echoed back as accepted/rejected because I never used the documented single
choice — I just fired the underlying intents directly.

**INVALIDATED renders**: Yes — step 1 showed INVALIDATED ("5/10 supply used
with 5 more queued") almost every cycle from t=0 through the last time I
looked at `--doc` output (around t≈340s), because I kept the worker-pulse
armed well past the point its assumption (idle hall, no supply pressure)
held. I never went back to clear/replace it cleanly; I just added competing
triggers (`steady-production`) on top, which is why the same "idle
TownHall/Barracks already has something queued" refusal recurred dozens of
times over the match — a small but real inefficiency in how I used the
trigger system (I should have consolidated to one producer-pulse trigger per
building rather than stacking two that raced each other).

**Did the WHY sentences change decisions?** Partially. The step-1 WHY about
farm-first economics is what made me reactively queue Farms whenever supply
capped, which kept the economy from stalling outright. But I did not let the
WHY reasoning extend to lumber balance or scouting cadence — those gaps were
never covered by a playbook WHY I read, and I didn't proactively look for
guidance there; that is on me, not the doc.

**Did I edit prefs.json (focus, opt-out)?** No. `bridge/red/prefs.json`
was left as-is (`{"playbook": "standard-kingdom"}`) for the entire match —
I never declared a `focus` even when the game clearly turned into an army
and defense problem in the mid-to-late game, which in hindsight would have
been the correct moment to switch focus to `army` and fold the page down to
the relevant recipes/gates for that phase.

**Acceptance NOTEs and what's still missing**: The system worked exactly as
documented — every refusal came with a clear, correct reason (mostly
lumber shortfalls), triggers fired reliably at their stated cadence, and the
`--doc` INVALIDATED/exit-fork rendering was accurate throughout. What is
missing is on the player side, not the tool side: I never used
`region_set`/`plan_set` for a structured scouting routine, and the biggest
practical gap was the lack of a "maintain N of kind" trigger predicate (the
doc calls this out explicitly as "a policy the wire cannot yet say — a filed
want") — with that primitive I could have set-and-forgotten a real lumber
worker quota instead of manually rebalancing by hand each cycle, which is
very plausibly what would have prevented the fatal lumber shortfall at the
TownHall-rebuild moment.
