# After-Action Report — Arena Round 27, BLUE seat

## Result
**WIN** — `game_over: "Human"` (my team), `game_over_reason: "surrender"`, at t=791s.
Red conceded while I held field control: 18 Footmen at 100% HP pushing near
their base, 1 of their 4 production buildings still standing (down from 4
seen), and I still had 641g/7930l banked plus 31 workers in reserve.

## Part 1 — The match as I saw it

**Opening (t=0).** Map `crossings`: mirrored bases at (-70,-70)/(70,70), a
canyon crossable only at three fords, with the two flank fords doubling as the
only extra gold mines. I opened with a single `plan_set` ("boomer"): 3 workers
to the near mine, 2 to a tree, train a free Hero, train two more workers up to
7, upgrade to Keep at tier 2, then build a Barracks — all queued before
`ready`, plus five triggers (hero-save, home-guard, supply-valve,
counter-punch, doorbell) and a `turtle` stance on squad 0. This landed in one
batch and ran unattended for the first ~80 game-seconds while I only had to
babysit idle workers.

**Economic build (t=80-330s).** Boomer plan completed cleanly (Keep up at
t=82s, Barracks at t=82s). I kept manually re-queuing Footmen and Workers each
cycle (the `keep-training` trigger, armed on `game_time(at=0)` with `repeat:20`,
mostly covered idle-barracks refills but needed my top-ups when queues emptied
faster than 20s). I expanded to a second mine (northwest, t≈256s) and a third
(southeast, t≈406s) as the home mine ran dry, and stacked 3 Barracks. By t=330s
I had a 15-unit, 2280-strength army sitting at home.

**First push (t≈330-420s).** With a comfortable army lead I set `stance push
target="their base"`. This ran into a real enemy force at the center ford
(~9-10 units incl. their Hero) and cost me units and my own Hero's HP badly —
the `hero-save` trigger pulled him home at 14%, but by t=400s he was caught
again mid-fight and died at t=534s (never revived — 400g/100l was a
luxury I chose not to spend once mines started running dry). The `doorbell`
trigger (enemy_army_seen ≥6) kept recalling squad 0 to defend home even when I
badly outnumbered the scouted force, which fought my own intent — I cleared it
at t=380s once I judged the recalls were costing more tempo than they saved.

**Economic exhaustion (t≈430-660s).** All four map mines (my three plus the
one I never got to) ran dry in sequence; `income_collapse` fired repeatedly.
From that point the match became pure attrition on banked gold — I kept
queuing Footmen until gold hit double digits (~t=660s) then stopped, banking
huge unused lumber (7000+, useless without gold to pair it). I regrouped the
survivors, healed to 100% at home, and rebuilt to ~21 Footmen.

**Second, decisive push (t≈626-791s).** Same `stance push` to their base. This
time the fight went overwhelmingly my way: the WIN line's enemy-production
count dropped from 4 seen (2 Barracks, 1 Keep, 1 TownHall) to 1 (just a
TownHall) by t=736s as my numerically superior force (21→18 Footmen, never
dropping below ~90% pooled HP) ground through their base defenses. The squad
then stalled near (48,47) for several cycles — `stance`'s "pressing on" status
said it was advancing but position didn't change; I tried `attackmove` and
then `posture push` with explicit coordinates to break it out, which briefly
reset it to "gathering" at the center ford. Before I could diagnose further,
Red surrendered at t=791s.

**Key decisions:**
- t=0: front-loading the whole opening as one `plan_set` + 5 triggers instead
  of drip-feeding commands — this bought a fast, unsupervised economy ramp.
- t=380s: clearing `doorbell` mid-fight because it was recalling a superior
  force away from a fight I could win — correct in hindsight given how the
  second push went, though the first push (same numbers-ish) still went
  badly, suggesting the recall wasn't actually the deciding factor there.
- t=534s: accepting the Hero's death rather than reviving (economy was about
  to collapse) — the army carried the game without him, so this was the right
  call, but I never re-evaluated whether a revive would have paid for itself
  once gold trickled back in from bounties.
- t≈620-660s: continuing to spend down to near-zero gold on Footmen before the
  second push rather than banking for a hero revive — worked out, since raw
  numbers won the second fight.

**What won it:** attrition math. Both sides hit the same hard mine-exhaustion
wall (4 mines total on `crossings`, symmetric), and whoever had spent that
window building more Barracks and training more bodies came out ahead once
gold ran out for both. I had 3 Barracks running continuously via the
`keep-training` trigger plus manual top-ups; that compounding production edge
is what let the second push walk through 3 of 4 enemy production buildings
before the opponent gave up.

## Part 2 — The document as a tool

**Usage mix, roughly:** `--doc` used once (t=0, to read the full opening
affordance list); `--digest` used for every other poll (roughly 35 cycles) —
it was the right size for "keep moving" polling. Raw intents (not
form-derived) dominated my sends — almost every command was hand-written
JSON (`plan_set`, `trigger_set`, `stance`, `train`, `harvest`, `build`) rather
than copy-pasted `[READY]` action templates from the doc; I read the doc once
for vocabulary and then worked from the brief + digest thereafter. I never
sent a `[NOT READY]` form with filled judgment fields — the one case I saw
(`stance:squad-0:turtle` refused for an empty squad at t=0) I just let the
plan's own sequencing catch up rather than re-forming the command.

**Annotations that changed a decision:**
- The `running_default` on the `enemy_army_sighted`/`doorbell` alarm ("your
  trigger doorbell fired... squad 0 holds the push stance") is what told me
  the trigger was actively overriding my push order, not just sitting idle —
  that's what prompted me to `trigger_clear` it once I judged the recall
  wrong. Without that sentence I'd have had to diff two snapshots' squad
  postures to notice the fight.
- The `income_collapse` alarm's `default: nothing recovers this
  automatically — workers continue their current assignment` was the single
  most load-bearing sentence in the whole match: it told me plainly that 12+
  idle/misassigned workers were not going to self-correct, and that spending
  the remaining bank on units rather than more workers was the right call
  once all 4 mines were confirmed dry.
- `plans[].status` going from `running` to silently `complete (7 steps)` in
  `events` was reliable enough that I never needed to re-read the plan body —
  I trusted the event line and moved on.

**What misled or was noise:**
- The digest's squad status line said `"gathering"` vs `squads[].status`
  saying `"pressing on"` for the *same* moment in a couple of polls (compare
  t=736s digest "pressing on" against t=780s digest "gathering" right after I
  re-issued `posture push`) — reissuing the same doctrine word reset a squad
  that was already advancing back into a full regroup, which cost real time
  in the middle of a winning fight. The brief documents this ("gathering"
  after 12s of stragglers gives up and presses on by itself) but doesn't
  warn that *re-sending the stance* restarts that clock, which is exactly
  the trap I fell into trying to "help" a stalled-looking squad.
- Repeated `trigger:keep-training: cannot afford Footman` errors once gold
  ran dry were correctly non-waking noise (per the brief's edge-only wake
  policy), but the digest's top-line `ERRORS 1: ...` banner made each of them
  look like a fresh problem on every view even though nothing had changed —
  a small "unchanged since t=X" annotation on repeated identical errors would
  have saved a few seconds of re-reading each cycle.
- I never got a clean read on *why* the squad's on-screen position stalled at
  (48,47) for ~40 game-seconds while `status: "pressing on"` claimed active
  advance — no fog/vision reason, no combat report, was given. Given that I
  was in fact winning that fight (enemy production visibly dropping in the
  WIN line during that exact window), the "stall" may have been my own
  misreading of a coarse digest position field updating less often than the
  underlying sim — the document doesn't say how fresh `squads[].posture`'s
  echoed x/z is relative to `t`.

**What the document should serve that it does not:** a squad-level "why is it
not moving" explanation analogous to the alarm `running_default` sentences —
something like "engaged in combat, net progress this poll: +2m" vs "no enemy
contact, path blocked by <reason>" vs "regrouping, N stragglers Y m behind" —
would have resolved the stalled-push confusion in one read instead of three
speculative re-orders (`attackmove`, then `posture push` with new
coordinates, both of which cost a regroup reset I didn't want).

## Files
- AAR: /home/ckopsa/dev/wc3clone/arena/r27/aar-blue.md
- Seat state read throughout: /home/ckopsa/dev/wc3clone/bridge/blue/state.json
