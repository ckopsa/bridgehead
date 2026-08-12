# AAR — bridgehead arena round r34, BLUE seat

## Result: BLUE WIN (razed enemy production) — game over at t=387s

## Part 1: The Match

### Opening (t=0)
Sent one batch before ready:
- `worker-pulse` trigger: repeating (18s) Worker training at idle TownHall
- Trained the free Hero immediately
- `home-guard` recipe: squad 0 turtles on base_under_attack
- `hero-save` recipe: hero retreats to base below 30% hp
- `expand` recipe: build TownHall at nearest legal site when a mine runs dry
- `ready`

First error: the `home-guard` trigger_set was rejected because `then.squad`
was sent as `null` — the schema wants a concrete `u8`, not an omitted/null
field, despite the doc template showing `null` as a placeholder. Resent with
`"squad":0` and it went through fine at t~9s. Minor friction, cheap to fix.

### Early game (t=0-90s)
Forgot to harvest at the very start — workers sat idle for ~20s until an
"income collapse" alarm fired at t=20s (0 of 5 workers on gold). Sent
`{"type":"harvest","select":"workers","target_select":"nearest mine"}` and
income recovered by t=32s. This was the single biggest unforced economic
error of the match — the brief explicitly warns "harvest FIRST" and I still
missed it in the opening batch. Lesson logged for next time: harvest must be
in the READY batch, not step 2.

Built Farm, then Barracks (first Barracks attempt was abandoned — "ground no
longer clear when the worker arrived" — resent and it landed). Set up an
`army-pulse` trigger for Footmen once Barracks came up. Hero moved to mid
(t=73s) and got enrolled into squad 0, staged at "mid" as a forward
scout/rally point.

### Mid game (t=90-215s)
Economy scaled cleanly: worker-pulse and army-pulse kept producing without
per-cycle micro; each supply cap (16, 22, 28...) triggered a Farm build the
moment gold+lumber allowed it. Upgraded the hall to Keep at t=178s (had to
retry once — insufficient lumber the first attempt). By t=194s the army was
7 units / str 1160, well ahead of a scouted 5-6 unit enemy force near the
center ford.

First contact: pushed squad 0 to "their base" at t~200s. The fight cost the
hero heavily — dropped to 17% hp by t=213s (hero-save trigger fired
correctly and pulled it home) and the squad lost about half its strength
(str 1300 -> 456) in that first clash. Fell back to turtle-at-base to
recover.

### Regroup and second push (t=215-340s)
Healed to full over ~90s while continuing production (worker-pulse,
army-pulse, more Farms). By t=307s the army was back to 13-14 units /
str ~2000-2300 with the hero at level 2+ and full health — a much larger
force than the first attempt. Re-staged at mid, then pushed to "their base"
again at t~321s.

This second push won the exchange decisively: hero leveled 2->5 over the
engagement (kills), enemy's TownHall dropped out of the "seen production"
line by t=373s (implying it was razed), and by t=387s the match ended with
BLUE declared the winner for razing enemy production, while my own base,
economy (46/46 supply, 1470g/170l banked) and 14-unit army were still fully
intact.

### What won it
- A large economic lead built almost entirely off two repeating triggers
  (worker-pulse, army-pulse) plus reactive Farm-building on supply caps,
  which meant no cycles were spent hand-queuing units.
- Patience after the costly first push: retreating, healing, and re-massing
  before pushing again rather than trickling reinforcements into a losing
  fight.
- hero-save and home-guard both fired correctly and for free, without me
  having to hand-manage a single retreat.

### What could have gone better
- The initial harvest omission cost ~20 real seconds of zero income at the
  most compounding part of the game.
- Never got a second TownHall/expansion up (mine dried at t=322s, expand
  trigger needed a region and errored out silently in words; I never
  circled back to fix it because the war was already being won). Economy
  was carried entirely on one base's second mine + lumber the whole game.
- Never got a Workshop/siege unit or Tier-3 tech going; the win came from
  a straightforward mass-Footman/Hero push before any of that mattered.

## Part 2: The Opt-Out Playbook As A Tool

I opened with `standard-kingdom` declared in prefs.json and never edited or
disabled it — no focus, no playbook swap, no opt-out. It stayed present and
readable on every `--doc` page for the whole match.

**How far I got down the steps:** I saw step 1 ("Ten workers before anything
clever"), step 4 ("Be standing at mid by the fourth minute"), and step 5
("Take the second mine while the first still has gold in it") explicitly
render before the match ended. I never manually advanced or acknowledged a
step — the page auto-tracked progress from state facts (worker count,
army-at-mid, mine health) without my needing to tell it anything.

**Gates that held me honest:** Step 1's gate (`Worker 5/10` etc.) correctly
flagged as `NOT YET` early and then `INVALIDATED` once supply capped mid-way
through queuing — this is exactly the kind of fact-check I would have missed
by eye (I was mid-decision on Barracks/Keep at the time and the page caught
that the pulse's own assumption had broken).

**Exits taken:** None formally — I never sent an EXIT-labeled fork option
verbatim. But my actual play (Farm-before-more-workers when supply was
tight, taking the free Hero immediately) matched what the fork's EXIT
options recommended almost exactly, just arrived at independently rather
than by reading the fork and picking an option. In practice the playbook was
confirming decisions I'd already made rather than driving them.

**INVALIDATED renders:** Step 1 rendered INVALIDATED twice (at t=41s with
"5/10 supply used with 5 more queued", and again near t~148s with "17/22
supply used with 5 more queued") as the worker-pulse trigger kept trying to
overspend supply. Both times the fork correctly offered "Farm first" as the
lead EXIT, which is what I was already doing by the time I read it.

**Did the WHY sentences change decisions?** Marginally. The step-4 WHY
("both r27 seats played a whole 791-second game without meaningful contact
... a staged squad at mid is not an attack, it is the cheapest scout you
have") reinforced a decision I'd already made (staging the hero+squad at
mid around t=73s) rather than causing a new one — but it did make me more
comfortable holding a forward stage posture instead of turtling passively,
since the page named the specific failure mode (mutual passivity) I was
trying to avoid.

**prefs.json edits:** None. Left `{"playbook": "standard-kingdom"}` as-is
the entire match — no focus field added, no playbook switch, no opt-out.

**Acceptance NOTEs and remaining gaps:**
- One NOTE: "cmd 0: accepted; note: push gates not met (Hero 18%, gate is
  80%)" at t=213s — the push stance was accepted but flagged as gated on
  hero health, which was a useful signal that immediately preceded the
  hero-save trigger auto-pulling the hero out. This NOTE was informative
  after the fact but arrived at the same cycle as the crisis, not ahead of
  it — a bit late to change my decision that cycle.
- Missing/friction points: (1) `trigger_set` with a `null` field for an
  integer-typed slot (e.g. `then.squad`) is silently rejected with a
  generic "invalid type: null, expected u8" rather than the doc's own
  placeholder convention working; (2) the `expand` recipe's `then.region`
  needs an explicit region name or it fires and errors in words
  ("build needs x/z or a region name") — the recipe as documented doesn't
  pre-fill a sane default, so it fired uselessly once and I never got a
  working expansion going for the rest of the match; this is a real gap in
  the recipe's out-of-the-box usability that I'd flag as a builder-side fix.
