# Arena r31 — BLUE (kingdom) after-action report

**Result: LOSS by surrender at t=731s.** Red wins.

## Part 1 — The match

### Opening (t=3–36)
I connected late: my first snapshot read `t=3s` with the event
`match start — 120s ready timeout expired without red blue; starting anyway`.
The clock was already running while I read the brief, so the opening batch went
out at ~t=33 instead of t=0. That is roughly a 30-second economic handicap I
gave myself, and it compounded.

The batch was one 8-step `plan_set boomer` (3 workers to the southwest mine,
2 to trees, Farm, 2 Workers, free Hero, Barracks, Worker), plus the three
standing rules — `supply-valve`, `hero-save`, `home-guard` — and a `turtle`
stance on squad 1.

Two build steps in that plan (Farm and Barracks) collided on the same ground and
both reported `build abandoned … the ground was no longer clear`. Cheap in gold
(nothing is spent before ground breaks) but it cost two build cycles.

### Boom (t=36–250)
`worker-pulse` and `rax-pulse` triggers ran production without polls; that part
worked well. Keep at t=184, Priestess, Blacksmith, Weapon Smithing 1 at t=250,
three Barracks, 12 workers. At t=243 I was at 1013 gold, 42 supply, upkeep
already down to 70%.

**Mistake A (lumber).** I let all workers drift onto gold twice and had to
hand-correct; lumber sat at 20–60 for long stretches and blocked the Sanctum
and the extra Farms. `harvest select:"idle workers"` kept re-tasking the wrong
bodies.

### The mid fight — the losing decision (t=291–315)
At t=290 the digest reported `enemy army spotted: ~6 (5 Footman, 1 Hero) near
the center ford`. I had 14 units staging at mid. I cleared the `offense` chain
early and sent `stance push their base`.

The acceptance NOTE came back:
`push gates not met (not consolidated: 1 of 12 outside squad 1, Hero 78%, gate is 80%, Priestess 68%)`.
I read it after the fact. Within ten seconds the sighting resolved as
**12, then 15** — 8 Footman, 3–5 Archer, Hero, Priestess. My ~6-unit sighting
was a fragment of a bigger force. I lost the mid fight, both heroes died at
t=299–320, and the army came home at 57% strength.

### Collapse (t=323–731)
At t=323 the `income_collapse` alarm fired: **the southwest mine was dry at
t≈320** — earlier than I had budgeted for, and I had no expansion started. My
workers auto-walked to the northwest mine 130 units away, so income effectively
went to zero for ~200 seconds while I scraped together 385g/205l for a TownHall.
Red arrived at my main at t=583 with 14–15 units against my 10 Footmen and no
hero, razed the Keep, the Blacksmith, the Shop and the Barracks by t=671.

I fell back to the new northwest hall, rebuilt a Barracks there, banked 2300
gold and queued six Footmen and a Hero revive — but Red's army followed within
40 seconds, killed the trickle as it spawned, razed the northern Barracks at
t=720 and killed six of my nine workers. At t=724 I held 1 Barracks-less hall,
3 workers, no army, against 15. I conceded.

### What lost it
1. **A 30-second late start** (ready timeout had already expired).
2. **Committing to a push on a stale, partial sighting.** The NOTE said the
   gates were not met and my intel was thin; the real force was 2.5x what I saw.
   That single fight cost 4 units and both heroes — 800g/200l of revival debt.
3. **No expansion before the home mine ran dry.** I never armed the `expand`
   recipe on `mine_dry`, which is exactly the rule that exists for this. By the
   time I reacted I could not pay for the hall, and I spent four whole cycles
   watching `blocked: cannot afford TownHall`.
4. Red played it straight and well: mass Footman with Archer support, both
   heroes alive, one decisive mid engagement, then an uninterrupted march
   through my base and a follow-up onto my expansion. No cleverness needed.

## Part 2 — The 2.1 scaffold as a tool

**The folded page.** Yes — this is the first thing that changed my behaviour.
`--doc` at ~75 lines was cheap enough that I ran it every single cycle for 40
cycles, and I never once fell back to `--digest`. But I have to be honest about
what I actually read: after the first three cycles I read the PROPERTIES block
(resources / squads / production / EVT) and stopped. I almost never read the
ACTIONS list, folded or not, because by then my play was doctrine-shaped —
triggers and stances I had already installed — and the thing I needed each cycle
was *state*, not vocabulary. The fold made the page affordable; it did not make
me read the bottom of it. The one place the actions list would have helped is
exactly where I lost: the `push` link carries the same gates as the NOTE, every
cycle, and I was not reading that far down the page.

**Acceptance NOTES.** One fired, on the `stance push` at t=291, and it was
correct on all three counts (not consolidated, Hero 78%, Priestess 68%). It did
**not** change my decision — I read it in the *next* cycle's digest, by which
time the fight was already going badly. That is the honest verdict: the NOTE
arrived at the right moment in the wire, and my loop read it one cycle too late
because I send-then-wait rather than read-the-echo-then-commit. A commander who
treated the echo of a commitment as a checkpoint (send push, read echo, decide
whether to abort *before* contact) would have got real value from it. The fix is
in my loop, not the scaffold.

**The playbook.** I declared `{"playbook":"standard-kingdom","focus":"economy"}`
in `bridge/blue/prefs.json` at t=36. I saw exactly one render of it — step 1/10
("Ten workers before anything clever") and it came up **INVALIDATED** on the
first cycle after I declared it, with the broken assumption printed in numbers:
`5/10 supply used with 8 more queued`. The exits were promoted to the top, and
exit 1 ("Farm first — buy the supply the pulse is about to eat") was precisely
the right move and precisely what my plan was already doing. So the INVALIDATED
interrupt fired, it was accurate, and it agreed with me. After that the PLAYBOOK
section fell off the top of my `head -16` window and I never read it again —
which is a real usage finding: **I truncated the page to 16 lines to keep cycles
cheap, and in doing so I cut off PLAYBOOK, ACTIONS and half the ALARM text.**
The page being affordable at 75 lines does not help if the commander's own loop
crops it to 16. The `why` sentences I did read were good — concrete, numeric,
non-preachy — but I read two of them all match.

**Focus.** `economy` was the right declaration for the first four minutes and
the wrong one from t=300 on, and I never rewrote the file. The brief explicitly
says a focus at t=60 and a different one at t=400 is a phase transition you can
name; I did not name mine. That is my omission, not the tool's.

**What is still missing.**
1. **A mine-depletion clock.** `income_collapse` fires when the mine is *already*
   dry. `mines[].remaining` is public, my workers' rate is knowable, and
   "your worked mine runs out in ~60s" is a fact, not advice. Every other
   deadline in this game is served early; this one is served late, and it decided
   my match.
2. **Sighting freshness on the digest line.** `enemy army spotted: ~6` and
   `enemy army of 15` are the same channel with a 3x difference and no confidence
   attached. The intel ledger has ages and the doc has them; the EVT line that
   actually reaches a truncating reader does not.
3. **The NOTE could name the abort.** It tells me the gates failed; it could
   print the one command that undoes the commitment (`{"type":"stance",...
   "turtle"}`), which is the thing I typed by hand one cycle later.
4. `harvest select:"idle workers"` repeatedly ordered workers that then read
   `Idle` again next snapshot, and `target_select:"nearest tree"` resolved to
   trees 60+ units away while closer ones existed in `trees_near`. I burned four
   cycles hand-plumbing worker ids because of it.
