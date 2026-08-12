# AAR — r32 (RED), bridgehead arena match — engine froze at t=1495.7

## Part 1: The match

**Opening (t=0).** Sent the full opening as one batch before `ready`: armed five
triggers (worker-pulse, home-guard, hero-save, expand, counter-punch), took the
free Hero, queued a Farm, set squad 0 to `turtle`, then readied. This front-loaded
doctrine so the early game ran on rails while I made build decisions.

**Early game (t=0–280s).** Economy scaled cleanly: Barracks, a second Barracks,
Workshop, and a chain of Farms kept pace with a `worker-pulse` trigger that
retrained at the TownHall every 18s. Corrected a real mistake early: my first
`build Farm` was silently refused because I didn't pass `site` explicitly even
though the template implied a default — cost me nothing (build-abandoned is
free) but taught me to always pass `"site":"nearest legal site"` explicitly
rather than trust an implied default.

**First engagement (t~325s).** `counter-punch` trigger fired on enemy-hero-down
and pushed squad 0 to mid — good, since I'd forgotten I'd have to redirect the
army manually otherwise. Hero leveled 2→3 in one fight from kill credit.

**Mid-game economic transition (t~450–580s).** All four map mines ran dry in
sequence around minute 8–9 (matches the documented "mines die around minute
10"). I expanded to two more halls (northeast/southeast mine sites) via manual
`build TownHall` — the armed `expand` trigger (mine_dry → build TownHall) fired
once automatically but I had to manually retry several more times because
lumber was consistently the binding constraint, not gold (I had gold
overflowing while lumber lagged behind — should have put a bigger fraction of
workers on wood earlier). Around t~500 I lost my hero at 17% HP because the
`hero-save` trigger (fires at 30%, 45s cooldown) pulled it a beat too late
against burst damage; revived it for 400g/100l a few seconds later, no lasting
harm since bank was healthy.

**The scouting problem (t~560–1490s).** By t~565 I'd razed their visible
production down to 2 Barracks + 1 TownHall, all subsequently destroyed
(win line read "1 seen" then eventually nothing) — but `game_over` never
fired. This was the single biggest time-sink of the match: I assumed the
enemy had a second, unscouted base and spent a very long stretch (~900
game-seconds of loop cycles) sweeping named places — southwest mine, both
fords, out to the map's southwest corner — with the main army, without result,
because the stance/posture engine kept reasserting its own anchor over raw
`move`/`attackmove` commands issued to a squad carrying a stance (a real
lesson: a stanced squad's own doctrine wins every tick; to redirect it you
must reissue `stance`/`posture`, not a bare unit-list `move`). The actual find
came from **intel**, not from patrolling: a single scouted enemy Worker
sighting at (-44, 17) pointed roughly toward their relocated production, and
following that vector further north turned up "enemy army spotted near the
northwest ford" plus a resight of "3 seen (2×Barracks 1×TownHall)" — they'd
rebuilt a base in the map's north, away from both original corners. I
redirected the main push there and was mid-transit, army at 98% HP, 23 units,
banked 768g/9255l lumber, when the match froze.

**What I'd do differently.** (1) Balance gold/lumber harvesters earlier and
more actively — I let lumber starve production for long stretches while gold
piled past 2000 unspent. (2) Don't fight a stanced squad's own doctrine with
raw `move` commands; either clear the stance first or express the new
objective as a `stance`/`posture` update. (3) Once production vanished from
sight, trust `intel.sightings` (transient unit sightings) as the scouting
signal *before* brute-force sweeping named places — it would have saved
several hundred seconds of loop cycles. (4) Stop auto-training army past the
point diminishing supply capacity or dead-gold economy makes it a net drain —
I kept two training-pulse triggers running well past the point they were just
queuing behind a full queue.

**The end.** By the observer's account the position was decided in substance:
23 units (9 Archer, 8 Catapult, 5 Footman, Hero L6) at 98% HP pressing the
newly found enemy base at the northwest ford, 768g/9255l lumber banked,
against blue's 6 units and 75g — roughly 4:1 on assets. The engine wedged
permanently at t=1495.7s in a windowed-rendering deadlock unrelated to any
faction decision, and the match was recorded as decided rather than played
out to a formal `game_over`.

## Part 2: The 2.1 scaffold as a tool

**The folded page vs. the old full render.** I read `--doc --all` exactly
once at the very start (the ~600-line unfolded version) to learn the roster,
then read the folded `--doc` every single cycle thereafter — genuinely at
loop cadence, not just at decision points. The fold worked as designed: most
cycles it was a 15–20 line skim (DIGEST + PLAYBOOK fork + a handful of
in-focus ACTIONS) that I could process in one pass, and I did in fact read the
ACTIONS block on cycles where I wasn't sure of a verb's exact field names
(e.g., confirming `build`'s `site` parameter, or `train`'s `select` domain
values) rather than guessing from memory — the doc paid for itself there. Its
weakness surfaced in the long dead-clock stretch: once doctrine was fully
armed and the only real work left was reacting to alarms/events, the folded
page's steady-state content (RESOURCES/ARMY/PRODUCTION/WIN) became repetitive
across many consecutive polls with nothing materially different, and I found
myself skimming past it rather than re-reading every field — a mature UI
choice would probably compress unchanged blocks further in a long-idle
mid-game, though I recognize that's exactly what the fold already does at the
ACTIONS layer and not yet at the PROPERTIES layer.

**Acceptance NOTE lines.** These fired and mattered at least twice. (1) A
`squad`-into-`stance` batch NOTE ("A `squad` and a `select:"squad N"` in the
SAME batch do not see each other") shaped how I sequenced re-merging the
stray scout archer back into squad 0 — I did the enrol and the later posture
change as two separate sends rather than trusting same-batch ordering for
that particular pair. (2) The stale-intel NOTE the observer flagged
("last enemy sighting Ns stale, threshold is 45s") appeared on repeated
`stance`/`attackmove` pushes toward "their base" late in the match — it was
honest and correct (my sighting genuinely was minutes old), but I'll admit I
overrode it every time rather than treating it as a signal to scout with a
cheap unit before committing the whole army; in hindsight that NOTE was
telling me exactly what turned out to be true — I was pushing on outdated
information toward a base that had already been abandoned/relocated, and a
NOTE that fires identically on five consecutive sends starts to read as
background noise rather than a fresh warning. That's a real gap: the NOTE
doesn't distinguish "still stale for the same reason as last time" from "here
is new information," and by cycle three or four I'd stopped registering it
as actionable.

**Playbook.** `prefs.json` declared `standard-kingdom`. I used step 1
("Ten workers before anything clever") directly — took EXIT option 3 (free
hero) rather than the CONTINUE fork, since scouting value outweighed the
tenth worker at that exact moment. It rendered **INVALIDATED** almost
immediately after ("broken assumption: 5/10 supply used with 5 more queued")
because my Farm-first build had already changed the supply picture the
playbook's step-1 assumption was written against — and the WHY sentence on
that break was genuinely useful, it told me plainly that the pulse trigger
was about to try training into a capped hall, so I built the farm before
re-arming the pulse. That is the one moment the playbook actively changed a
decision. After that I went fully off-book: economy → tech → the long
scouting saga → the north-base find were never covered by any of the
declared 10 steps' forks in a way I consulted again, and I stopped reading
the PLAYBOOK block once I was well past its early-game framing — it never
re-surfaced anything relevant to the mid/late-game problems I actually had
(supply-cap production stalls, mine depletion economics, stanced-squad
redirection, scouting after losing sight of enemy production). That's the
real gap: a 10-step playbook keyed to the first ~4 minutes has nothing to say
about the 20+ minutes of stalemate that followed, and I had no equivalent
structured guidance for "your win-condition sighting has gone stale, here is
how you re-acquire it."

**Focus.** Declared `"focus":"economy"` in prefs, which expanded the economy
section of ACTIONS in full every cycle (build/train/rally/template/expand
recipe) while folding army/tech sections unless an alarm broke through. This
matched my actual cadence of decisions for roughly the first half of the
match (worker counts, farm timing, expansion sites) and I never felt the urge
to switch it even once the game turned into an army-management and
scouting problem — the alarm-breakthrough mechanism (income_collapse,
enemy_army_sighted, squad_below_half) reliably surfaced the things I needed
to see outside the declared focus, so the static focus declaration didn't
become a liability the way I expected it might.

**What's still missing.** (1) No cheap, declarative "find their production
again" affordance — I had to reconstruct scouting logic by hand (splitting a
unit into its own squad, sending raw moves, discovering they get overridden
by a stanced sibling squad) with no recipe or trigger primitive for it,
despite the win condition being explicitly about seeing enemy production. A
`recipe:reacquire-sighting` in the same vein as `recipe:expand` would have
saved most of the ~900-second detour. (2) Stanced-squad vs. raw-command
precedence isn't documented anywhere I found in the brief — I had to
discover by experiment that a stance's own re-tasking beats a bare `move`
issued to member units. That's a sharp edge worth a line in the doctrine
section. (3) The NOTE de-duplication gap above — repetition without a "this
is still true, and here is why it's now more urgent" escalation made me tune
it out exactly when it was most informative.
