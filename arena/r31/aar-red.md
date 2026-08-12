# Arena r31 — RED (Claude/kingdom) after-action report

**Result: WIN — opponent surrendered at t≈731s.** Map `crossings`, doc_version
`affordance-doc/2.1`. Final state: 15 army units at 100% pooled HP (8 Footman,
5 Archer, Hero L7, Priestess L6) standing in the enemy's northwest-mine
expansion, having razed their Keep, their Workshop and every Barracks they
owned; 148g/620l banked, 8 workers, 3 Barracks + Keep standing at home.

## Part 1 — the match

### Opening (t=9, not t=0)
I arrived late. My first snapshot read `t=2s` with the event
`match start — 120s ready timeout expired without red blue; starting anyway`,
so the held-at-zero opening the brief describes had already lapsed for both
seats. I skipped the leisurely full `--doc --all` read-through of the map and
authored the whole opening in one batch at t≈9:

- 3 workers on the northeast mine, 2 on trees;
- an 8-step `plan_set boomer`: 2 Workers → free Hero → Farm → 2 Workers →
  Barracks → Worker, all in selectors (`my hall`, `workers`, `nearest legal
  site`), no ids beyond the five starting workers;
- three triggers armed before anything happened: `supply-valve`,
  `home-guard` (turtle squad 1 on `base_under_attack`), `hero-save` at 40%;
- `stance squad 1 turtle` + `template my TownHall squad 1` so every body ever
  produced inherited doctrine;
- `ready`.

The plan completed at t=27 — eight build-order commands for one poll. That is
the single largest thing that went right.

### Economy phase (t=30–280)
Pulse triggers replaced hand-queuing: `worker-pulse`, then `rax-pulse`
(Footman) and `archer-pulse` on `idle Barracks`. By t=185 I had 13 workers, two
Barracks, five Farms, and a 7-unit squad securing the center ford. Three
mistakes here, all mine:

1. **Repeated `build … region:"our base"` collisions.** Four separate
   `build abandoned: … the ground was no longer clear` lines. Nothing was spent,
   and the events told me so, but each one cost a farm's worth of tempo.
2. **An income collapse I caused myself (t=214).** Sending
   `harvest select:"workers" target_select:"nearest tree"` plus farm-builds put
   *zero* workers on gold. The alarm caught it — `none of your 14 workers is on
   gold — nothing recovers this automatically` — and I fixed it in one cycle.
   Without the alarm I would not have noticed for another two polls.
3. **A `plan tech` halt at t=220** on `cannot afford Keep`. I had let three
   Barracks eat the bank. I cleared and re-set the plan; the Keep finally went
   up at t=380.

### The failed first push (t=287–330)
At 14 units with upkeep already at 70% I committed: `stance squad 1 push
their base`. It was half right. I met their army in mid, lost four Archers and
a Footman, and the intel that came back was the real prize:
**5 enemy Footmen at (34,30) — inside my half — and 5 Footmen + Hero +
Priestess at their base with both heroes under 45%.** They had counter-raided
while I walked.

Key decision at t=329: I **cleared `home-guard`** rather than let it yank my
push home, then re-read the board and decided the push was too thin anyway
(squad at 54% HP, hero at 29% and running on the `hero-save` trigger) and
recalled to turtle. Mixed verdict: clearing home-guard was right (I did not
want a reflex deciding a strategic question), recalling was probably right,
but committing at 14 units with a 250s-stale ledger was not.

### The pivot that won it (t=478–557)
`the northeast mine your hall works has run dry` at t=478 — my only mine, gone,
with 36 gold in the bank and a 385g expansion hall unaffordable. I moved
workers to the southeast mine (150-unit round trip, no hall) and put the army
on `secure southeast mine`. Income was ~14 gold per 16 seconds. The expansion
plan blocked immediately on cost.

Then I read `mines[]` directly and found the fact that decided the match:
**both starting mines were dry, theirs included.** Neither side was going to
field another army. My 15 units at 100% HP were the last army I would ever
have, and every second of holding them cost 30% upkeep.

t=557: `plan_clear expand` + `stance squad 1 push their base`. All-in.

### The kill (t=586–731)
The squad crossed and pressed on. Barracks at t≈597, **their Keep at t≈610**,
Workshop next; heroes went L3→L7 and L1→L6 off building and unit kills while
staying at 100% HP — the push stance's 25% fallback and the hero-save trigger
never had to fire. At t=649 the ledger emptied and I had one remembered Barracks
at (-47,-85); I `region_set lastrax` and pushed it, found nothing, then guessed
their expansion was on the last live mine and sent `push northwest mine` at
t=683. It was: TownHall + 2 Barracks. Two Barracks fell, and at t=731 they
conceded with one TownHall left.

### What won it
The all-in read at t=557. The information that made it — both mines dry — was
public in `mines[]` and free to read; it turned "I am losing on economy" into
"there is no economy left to lose on, spend the army". Doctrine did the rest:
one `stance push` sentence fought a 150-second running battle across two enemy
bases with **three commands** from me in that whole span.

### What nearly lost it
The self-inflicted income collapse, the four abandoned farms, and the t=287
push into a stale ledger. Also: I let workers auto-path to the *southwest* mine
(enemy side) and lost five of them at their base, which I never noticed until
the loss lines appeared.

## Part 2 — the 2.1 scaffold as a tool

**The folded page vs the old full render.** Yes — this is the first thing that
actually changed my behaviour. I read `--doc --all` once (~300 lines before I
cut it off, at t=2 when there was no clock), then ran the folded `--doc` on
every one of ~30 cycles. But the honest accounting is narrower than the design
hopes: what I read at loop cadence was almost always the *first 16-20 lines* —
PROPERTIES/DIGEST, ALARM, EVT, DEFAULT — and I piped the rest through `sed -n
'1,18p'`. So the fold made the page cheap enough to run every cycle, and then I
still did not read the ACTIONS section most cycles. The fold solved the page's
cost; it did not solve my triage. What I *did* read every cycle was the digest,
and the digest is where every decision came from. Meanwhile I hand-wrote raw
JSON (rung 4) for essentially every command — because I knew the verbs from the
brief and the affordance line is longer to copy than the intent is to write.
The document's real value to me was **facts I could not compute** (slot
pressure, `unlocked`, the domain lists at t=0), not its pre-written commands.

**Acceptance NOTE lines.** Two fired, both on `stance push` with a stale
ledger: `last enemy sighting 250s stale, threshold is 45s` (t=586) and `85s
stale` (t=712). Neither changed a decision, and both times the note was
*correct and I was right anyway* — the t=586 one was the all-in, taken
deliberately with no intel because the alternative was decaying to a score
loss. What the notes did do is confirm the thing I had reasoned to: that I was
committing blind. If a note had fired at t=287 (my *bad* push) it would have
said the same words about the same condition and I would have ignored it the
same way, because the flaw there was army size, not staleness. The gate that
would have helped me at t=287 was `push_min_units`/consolidation, and it did
not trip — my squad was consolidated and above six. So: notes were accurate,
zero-cost, and non-decisive.

**The playbook.** I declared `{"playbook":"standard-kingdom","focus":"army"}`
in `bridge/red/prefs.json` at t=9 and left it there all match. It rendered
step 1/10 ("Ten workers before anything clever") and — usefully — rendered it
**INVALIDATED on my very first post-opening cycle**, with the broken assumption
in numbers: `5/10 supply used with 10 more queued`. That was a real interrupt
and it was right: my `boomer` plan had queued past my supply cap, and the exits
were re-ordered to put "Farm first — buy the supply the pulse is about to eat"
on top. I did exactly that exit (two farms, seq=2) — though I would like to
claim I read the fork and chose it, and the truth is I had already decided from
the digest's `supply 5/10` and the playbook agreed with me one line later.
Its other exit, "Take the free hero now", was already step 3 of my plan.
After that first invalidation I stopped reading the PLAYBOOK block, because my
own plan had diverged (three Barracks, no expansion, an all-in) and a pointer
that says "you are here" on a plan I am not running is noise. **The WHY
sentences did matter, once**: "a capped hall trains nothing at any price" is
the sentence that made supply feel urgent rather than cosmetic.

**Focus.** Declared `army`. It expanded the five `stance:squad-N:*` links in
full every cycle — which is the section I needed *least*, because I knew the
five stances from the brief and their `why` lines were all "squad 0 has no
members" for the first three minutes. Declaring `economy` for the first four
minutes and `army` after would have been the right call, and the fact that a
focus is a rewritable file is the feature I under-used. My focus never moved,
so it recorded nothing about my phase transition — which is exactly the
measurement the design says it is for.

**What is still missing.**
1. **A `mines[]` line in the digest.** The single most decision-relevant fact
   in this match — "both starting mines are dry" — required me to drop out of
   the view and write a Python one-liner. `mines` is public, unfiltered, and
   cheap: one line of `remaining` per mine belongs beside RESOURCES. The
   `income_collapse` alarm told me *my* mine was dry; nothing told me theirs
   was, and that asymmetry was the whole game.
2. **A "less than" predicate.** There is `unit_count(kind, count)` meaning
   *at least*, so a worker pulse cannot say "until 14". I armed
   `unit_count Worker >= 1` as a permanent-true clock and then had to
   hand-clear it. `game_time` as a repeating clock is the same hack.
3. **`build` region collisions.** Four `build abandoned: the ground was no
   longer clear` events in one match, all from repeated
   `region:"our base" site:"nearest legal site"`. `nearest legal site` does not
   consider *other builds in flight or queued this cycle* the way `build`'s
   worker selector already considers claimed builders. The same rule that fixed
   the builder should fix the site.
4. **Ghost-building staleness in the WIN line.** `raze their production: none
   seen yet` at t=683 after razing four buildings was technically honest and
   strategically misleading — I had to grep `buildings[]` for `last_seen` to
   find the survivor at (-47,-85). A "1 remembered at (-47,-85), 33s ago" on the
   WIN line would have saved me a cycle and a Python call.
5. **Nothing in the scaffold priced upkeep against time.** I sat at 70% upkeep
   from t=246 to t=557 — 300 game seconds of taxed income for an army that was
   doing nothing. `upkeep 70%` is printed; what it *costs per second* is not,
   and that number is what should have pushed me to commit ~200 seconds earlier
   than I did.
