# AAR — blue (boomer, Fable), round r23

**Result: Victory.** Red (Claude seat) surrendered at t=845s after our 31-unit army destroyed
their main TownHall, Barracks, Workshop, and towers, leaving them 5 workers and no army.

## Part 1 — the match

**Opening (t=0-120), shaped directly by r21/r22.** r21 proved a boomer with zero army dies at
t≈180; r22 proved the fix is a standing army plus triggers armed at t=0. So the entire opening
went out as ONE pre-ready batch: an 8-step boomer plan (harvest gold+lumber, workers, free Hero,
Barracks at t=1, Keep upgrade gated on 8 workers, Sanctum gated on tier 2) plus home-guard,
supply-capped and expand triggers. Barracks was up before t=35, Keep done at t=104, and a
Footman/Archer plan stamped every Barracks unit into squad 1 with retreat doctrine via `template`.

**First rush (t=184).** Red repeated the persona script: 4 Footmen + hero hit my base at almost
exactly r22's timing (t≈167 there). Squad 1 (3 Footman, 2 Archer... rising) + hero met them;
home-guard and hero-save both fired within seconds — hero walked out at 30% instead of dying
(r21's fatal bug, fixed by arming the trigger with the real id the poll after the hero spawned).
Lost 2 Archers; their wave died; my hero hit Lv3 on the kills.

**The big raid (t=485-510).** Red massed 13 (11 Footman, 1 Archer, hero) and hit my main while
my army was screening the new expansion at the northwest mine — the exact r10 "army split" trap.
They destroyed my Barracks and Sanctum before the army marched home, but the standing squad +
both heroes then annihilated the entire raid: my hero jumped Lv3→Lv5 during the fight and their
army effectively ceased to exist. Rebuilt both buildings within a minute off a 1200-gold bank.

**Economy.** Mines died around t=400-650 (both home mines, then all four). Income continuity came
from: the pre-armed expansion (second TownHall at the NW mine, placed by a plan the moment lumber
allowed), a forage squad that banked three mid bounties (+360, +405, +450), and a huge
lumber operation (2000+ banked by the end). Peak: 2941 gold, 15 workers, 74+ supply.

**The push (t=623-845).** First push was sloppy: I set squad 1 to push with only 11 members —
my repeated manual "sweep new units into squad 1" had scattered the army across squads 0/2/3,
and 5 units trickled into their base and died (echo of r22's premature push). The fix was one
consolidation batch: every non-worker id into squad 1, muster at (-30,-10), wait two cycles for
both heroes to heal to full (Hero Lv6, Priestess Lv4), then one cohesive 28-unit push with 2
catapults. It walked through their base: towers, Barracks, Workshop, main TownHall all down by
t=844. Red surrendered before I reached their last TownHall at the southeast mine.

**Key decisions.** (1) Whole opening + triggers before `ready` — zero dead time. (2) Standing
army from minute one (r22's lesson) — turned both red attacks into XP donations. (3) Hero-save
armed with a real id — saved the hero twice (t=190 at 30%, t=686 at 34%). (4) Regathering and
healing before the second push instead of reinforcing a trickle. (5) Expanding and foraging as
mines died — red's final log lines show them at 305 gold with 5 workers while I had 1635g/2545l.

**Opponent behavior.** Red opened with the same footman rush (wave at t=184), then a larger
composite raid at t=485 that traded two of my buildings for their whole army, then never
recovered. They expanded to the SE mine and built a second Workshop but fielded almost nothing
after t=700 — their production oscillated between 2-6 units while mine compounded. The surrender
at t=845 was correct; they had 5 workers and no path back.

**Friction notes (not engine bugs but worth recording).** The supply-capped trigger with a frozen
farm coordinate failed repeatedly as farms filled the site ("site blocked — nearest legal: X") —
I re-aimed it four times and finally cleared it and built farms manually; a trigger action that
accepted "nearest legal" placement would remove this whole loop. The `mine_dry` expand trigger
never fired even when both home mines died (possibly because dry mines leave the `mines` list, or
the 40-radius test failed) — I expanded manually. And the final `game_over` never reached my
snapshot: the engine logged "Claude surrenders at t=845s — Human wins" and exited, but
state.json froze at t=844 with game_over null; I confirmed the result from arena/r23/engine.log.

## Part 2 — decision-space design notes (for a Haiku-class commander)

The open vocabulary is ~25 verbs x free coordinates x raw ids. Over this match I made ~40 sends,
but the number of *distinct kinds* of decision was about eight, and in each phase only 2-4 of
them were live. A small model needs the state machine made explicit.

**The phases I actually lived, and what mattered in each:**

1. **Pre-ready (one decision).** The only choice that mattered: which opening plan. Everything
   I sent was a known-good recipe (the brief's canonical boomer plan + three trigger recipes with
   ids substituted). Affordance menu: "opening: boom / rush / hedge" — three canned plans with
   the seat's real ids already substituted. A small model should NOT be writing 8-step JSON here;
   it should pick one of three and optionally tune two numbers (barracks timing, army floor).

2. **Steady-state boom (t=0-180).** My actual per-cycle work was: re-task 1-2 idle workers,
   queue 1-2 units when a building queue was empty, fix one blocked trigger. Menu needed:
   "N idle workers -> [gold|lumber]", "Barracks queue empty -> [Footman|Archer]", "supply in
   <=2 -> build Farm (auto-sited)". Preconditions are trivial (idle>0, queue empty, gold>=cost).
   *Default: continue* was correct on at least five cycles here, and I still burned sends on
   farm-coordinate whack-a-mole that auto-siting would have deleted.

3. **Under attack (t=184, t=485).** Zero new decisions were actually required in the first rush —
   home-guard, hero-save, retreat doctrine, and squad-1 defend posture fought the whole thing.
   The one live option: "recall the away army? y/n" (t=485, army at the expansion, main burning).
   That is the single decision that should interrupt a small commander, phrased exactly like
   that, with the distance/ETA attached. Everything else (focus-fire, kiting, hero exit) must be
   doctrine. *Default: continue* during the t=184 rush was right; at t=485 it would have cost me
   the main — that is the one moment a forced interrupt earns its keep.

4. **Rebuild/consolidate (t=510-620).** Live options: rebuild lost production (obvious — offer
   "rebuild Barracks at old site: 160g"), spend a growing bank (I sat at 2900g; a standing nag
   "gold > 1000 and rising: [expand|army wave|tech]" is recipe 7's supply-valve generalized to
   money), and re-consolidate squads. Squad hygiene was my biggest self-inflicted problem: units
   ended up spread over squads 0/2/3 through template defaults + my ad-hoc sweeps. A small model
   needs ONE army squad by default and an explicit "detach N for X" affordance, never the reverse.

5. **The push (t=620-845).** Two decisions mattered: WHEN (army size + enemy army wiped — the
   `enemy_army_seen`/hero-down predicates could gate a canned "counterpunch" trigger) and
   WHETHER TO ABORT (my 11-unit trickle at t=697 losing 5 units — the signal was in `events`
   as three "lost X" lines with positions deep in their base while centroid retreated). The menu:
   "push (requires: squad>=N, all-in-one-squad check PASSES, heroes>=80% hp)" — those three
   preconditions are exactly the ones I violated the first time and satisfied the second time,
   and all three are machine-checkable. Razing the base needed nothing further: push posture +
   priority did it all; I sent one command per building cluster.

**Delegate to standing policy (set once, never poll):** retreat thresholds, hero-save,
home-guard, autocast, focus-priority, supply valve (if auto-sited), rally/template on every
production building, forage squad for bounties. All of these fired correctly for me between
polls; none ever needed a mid-match opinion beyond re-aiming when I reorganized squads — which
argues for triggers referencing squads/regions, never frozen unit ids or coordinates.

**Keep for the commander:** opening choice, expand (where/when), tech spends, push/abort/regroup,
recall-vs-hold when two places burn, surrender. Six decisions. Everything else was mechanical.

**Snapshot fields I actually read:** gold/lumber/supply, ARMY count + squad memberships, my
building list with queues, ENEMY buildings (the win condition — I steered the whole endgame off
that one line), `events` (losses with positions = the trickle alarm), `plans[].status`,
`errors`, hero hp/level, MINES remaining. **What I never used:** per-unit positions except
centroids, `why` (once, diagnosing the stall), trees list after t=60, most of `intel` detail
(the `groups` one-liner "~13 (11 Footman, 1 Hero) near our base" was exactly the right grain).
A small-model view could be 15 lines: resources, army-by-squad, production queues, enemy
production buildings remaining, last 5 events, active alarms (blocked plans/triggers), and a
2-4 item affordance menu derived from the phase.

**Where "continue" was right vs fatal.** Right: every quiet boom cycle, the whole t=184 rush,
the entire final raze (I sent 3 commands in the last 200 seconds). Fatal-if-defaulted: t=485
(recall decision), t=400 (all mines dying — no event fired for me; the expand trigger silently
never triggered, and a commander defaulting "continue" would have starved exactly like red did —
red's death was arguably one long wrong "continue"), and t=697 (abort the trickle). So the
design rule: "continue" must be the default ONLY when no alarm is raised, and mine-exhaustion /
income-collapse / losses-while-advancing must be first-class alarms, not things you notice by
reading a resource line trend across polls.
