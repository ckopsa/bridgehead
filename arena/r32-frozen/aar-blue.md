# AAR — Arena Round 32, Blue seat (kingdom vs kingdom mirror), crossings map

## Part 1: The match

### Opening (t=0)
Sent the whole opening as one batch before `ready`: a `boomer`-style economy
plan (3 workers to the southwest mine, 2 to a nearby tree, two Worker trains,
a Barracks build), the standard recipe triggers (hero-save at 35%, home-guard,
expand-on-mine_dry targeting northwest mine, a supply-valve farm trigger), and
a `turtle` stance on squad 0. This landed cleanly — the plan ran to completion
by ~t=17s with no wasted polls.

### Early game (t=0–450)
Standard build-up: Keep upgrade at t=102s (tier 2), second Barracks, a
Workshop, Priestess and a first Hero trained. Economy ran hot — gold
consistently outpaced spending (banked past 1000g by t=90s, past 2000g by
t=190s) because I was one poll cycle behind on queuing; each time I noticed I
threw more Barracks/Workshop production and a second TownHall (northwest
mine expansion) at the surplus. In hindsight I should have opened a second
production building sooner and kept 2-3 standing `train` orders queued by
t=90s rather than by t=190s.

### First contact — the scouting mistake (t≈318–328s)
At t=318s I moved the newly-massed 9-unit army (str 1270, including the
first Hero) to `secure` mid to get intel. An enemy group of "~5 (3 Archer, 2
Footman)" was sighted at the center ford — I read this as favorable odds and
did not pull back. It was a scouting report, not the true force: within 4
seconds a `hero_below` trigger fired (hero at 8%) and moments later the
snapshot revealed the real group was 11 units including 2 Catapults. By
t=328s the whole engaged force was dead — hero included — for the loss of
essentially my entire opening army. **This was the single costliest decision
of the match**: committing a consolidated army toward an unscouted enemy
concentration on a stale, small sighting instead of treating "an army report
I haven't refreshed in the last few seconds" as untrustworthy near a choke
point everyone can reach quickly.

### Home base falls (t≈450–620s)
Rebuilt to 7-9 units twice, but the enemy kept returning in growing waves (10,
then 13, then 14 units, catapults and hero each time) faster than I could
reconstitute defenders. By t=469s the Keep was destroyed; by t=567s both
Barracks and the last Farm at the home base were gone. The main base was
razed. I had, fortunately, already built a second TownHall + Barracks at the
northwest mine expansion earlier (around t=190-200s) opportunistically with
banked gold — that expansion is what kept the match alive past the home base
loss. That "spend the surplus on a second base" decision, made for economic
reasons, turned out to be the thing that prevented an immediate razed-loss.

### Consolidation and the second economic collapse (t≈620–900s)
Rebuilt from the northwest base. All four map mines went to `remaining: 0`
around t≈590-620s — both sides' economies died at roughly the same time
(this is a mirror match, so likely simultaneous). From that point on the game
was pure banked-resource attrition: gold trickled toward zero (banked reserve
ran out by roughly t=900s) while lumber kept climbing uselessly past 3000
(far more workers than trees to use them on, and nothing left worth building
with pure lumber). I never re-tasked the 14-15 idle workers into anything
productive once income died — that's a clear miss; those bodies were dead
weight for the rest of the match.

### The failed offensive (t≈1037–1280s)
With the army rebuilt to 15 units (str ~1890) I pushed for their base,
partly opportunistically grabbing a 720g bounty en route (+720g, useful).
Scouted 4 of their production buildings (win-condition intel), but their
army — reported growing from 15 to 17 to 19 units with 4-6 Catapults — was
converging on the same ground. I broke off the push back to `turtle` rather
than trade, which was the right call given the odds, but the retreat itself
bled the squad down from 15 to 6-7 units to stragglers picked off one at a
time along the return path (pathing seems to have looped through the center
ford rather than a direct line, and units that fell behind the main group
died alone near the enemy's territory). A tighter retreat — e.g. `retreat`
with a hard threshold rather than relying on the push stance's own fallback,
or consolidating with `squad` more aggressively during the withdrawal — would
have preserved more of that army.

### Final positions before the freeze (t=1495.7)
By the end I was down to 6 units at 75% HP and 75 gold with income
permanently dead, watching a 23-unit enemy siege ball (9 Archer, 8 Catapult,
5 Footman, Hero) mass at the northwest ford outside my last base — with
farms and a tower already falling. The observer's asset accounting (6 units/
75g for me vs 768g/9255l and a 23-unit army for red) makes the score-decided
outcome honest: this was a lost position, not a photo finish. **The match
itself then wedged at t=1495.7s due to a windowed-rendering deadlock in the
engine** (unrelated to either seat's play) and was put down by the
coordinator; the round is recorded as decided-in-substance against Blue.

### What I'd do differently
1. Never commit a full army off a sighting that is seconds old near a choke
   point without a `retreat` doctrine already governing that exact fight
   (hero-save existed but was pointed at the WRONG scale — it saved the
   hero's body, not the squad around it).
2. Keep 2+ production buildings permanently queued from minute 2 onward
   instead of noticing surplus gold reactively every ~100 game-seconds.
3. Once the mines went dry (~t=600s), immediately re-task idle workers —
   either into a `CallToArms`-style temporary defense boost, disbanding
   excess into the fight, or at minimum stopping new Worker training instead
   of continuing to add bodies with nothing to do.
4. On the failed offensive's retreat, use an explicit `retreat` trigger
   with a firm threshold and squad consolidation rather than trusting the
   push stance's own 25%-fallback to bring stragglers home as one body.

## Part 2: The 2.1 scaffold as a tool

**Folded page vs full render — did I read ACTIONS at loop cadence?**
Read `--doc --all` once at t=0 for the full picture, then used
`--digest`-flavored views (via `bridge_view.py bridge/blue/state.json
--digest`) almost every subsequent cycle rather than re-reading the full
`--doc` action list each time. In practice this means I was mostly reading
DIGEST + ALARMS + EVT every cycle and only pulling the full `--doc --all`
action listing a handful of times mid-match (when I needed to check
`upgrade`/hero-training availability or double-check the stance/gate
mechanics). That was a reasonable trade for tool-budget reasons in a long
match, but it also means I likely missed some `ACTIONS` entries that would
have been useful — e.g. I never used `rally`, `template` consistently across
all producers, or the `research`/Blacksmith upgrade path once a Blacksmith
existed (it was built and then destroyed with the home base before I ever
used it). The fold worked as advertised — nothing was hidden, only
de-prioritized — but I did fall into exactly the failure mode the docs warn
about: reading the page once and then leaning on memory rather than
re-checking the ready/not-ready facts every cycle, particularly for `stance
push` gates. Before the disastrous t=318s engagement I did not re-check the
push/secure gate's "intel ledger empty" warning that the doc would have shown
me for the `push` stance — I used `secure` there specifically, but the
subsequent all-in reaction to a stale 5-unit sighting was a judgment call
made off the DIGEST/EVT stream, not off a re-read of the full action page.

**Acceptance NOTE lines — did any fire, did they change a decision?**
I did not see a classic "push gates not met" acceptance NOTE fire in this
match, mostly because I avoided sending `stance push` until late (t≈1037s),
by which point the squad was consolidated and past the gate thresholds. The
NOTE-adjacent signal I *did* get repeatedly and *did* act on was the
plain-language `errors` array — e.g. `cannot afford Archer (90g 30l)` (which
correctly diagnosed a lumber shortage, not a gold shortage, and I
re-tasked workers to trees in response), and the repeated `build: 'idle
workers' matches none of your units right now — nothing was ordered` lines
from the supply-valve trigger, which told me every idle-worker slot was
already claimed and was a genuinely useful "nothing to do here" signal I
came to expect and ignore correctly. So: the acceptance-note *mechanism*
(gate-violation NOTE) never fired for me directly, but its sibling channel
(refusal errors with both sides of the comparison) shaped several real-time
corrections — most usefully the lumber-vs-gold diagnosis.

**Playbook** — I never declared `standard-kingdom` in a prefs file and played
entirely off-book. In hindsight this was the single biggest missed tool
affordance: the playbook's own summary line ("Ten workers, a Barracks before
three minutes, eyes on mid before four, a second mine before the first runs
dry, and a tier bought only when there is something to spend it on") is
almost exactly the build order I was improvising by hand, one poll at a
time, and the "second mine before the first runs dry" clause in particular
is the lesson I learned the hard way at t≈590-620s when all four mines went
dry simultaneously with no queued response beyond the (already-armed)
`expand` trigger — which had already fired once, early, opportunistically,
rather than being paced against the actual depletion curve. Declaring the
playbook and reading its "you are here" fork every few cycles would likely
have caught the coming income collapse earlier and prompted either a third
expansion attempt or an earlier pivot to a war economy. I did not use it
because I defaulted to writing my own plans/triggers by hand from the brief,
which worked but left this exact gap unaddressed.

**Focus** — I did not declare a `focus` in a prefs file. No `bridge/blue/
prefs.json` was ever written for this match. In retrospect an `army` focus
during the mid-game production plateau (t≈650-900s, when gold was cratering
and I was manually re-deciding train orders every cycle) or an `economy`
focus in the opening 90 seconds (when I was one cycle behind on queuing
surplus gold) would have surfaced the relevant fields — supply, farm cost
domains, producer idle-status — without me having to pull `--doc --all`
by hand to check them.

**What's still missing, from this seat's experience:**
- No predicate/trigger exists for "N of my units are idle with nothing to
  mine" — I had 14-15 idle workers for the last ~900 seconds of the match
  with no armed rule to re-task or disband them; I had to notice it by eye
  every few cycles and mostly didn't act on it.
- No "maintain N of kind" trigger (the brief itself flags this as a filed
  want under `recipe:steady-production`) — I approximated it with a
  `game_time`-pulsed repeating `train`, which worked but silently failed
  whenever both Barracks were simultaneously busy, generating a steady
  stream of harmless-but-noisy `errors` I had to keep mentally filtering
  out.
- A squad's automatic pathing around the map's canyon geometry was not
  transparent from the digest: my push toward "their base" stalled twice
  at intermediate coordinates for two full poll cycles each with no
  indication *why* (blocked by canyon, waiting to regroup, or genuinely
  pathing) until I forced it with an explicit `attackmove` to a named
  region. A `squads[].status` reason distinguishing "pathing blocked by
  terrain" from "gathering stragglers" would have saved at least two
  wasted cycles and might have prevented the costly straggler losses during
  the retreat.
