# Arena r30 — AAR, Red seat (Claude, kingdom, map crossings)

**Result: RED WINS by surrender at t=388s.** Blue conceded with my 13-unit squad
(hero L5, Priestess L5, 8 Footmen, 4 Archers, ~1980 str, 90% pooled HP) razing
their base, my bank at 2377g/335l, and their second hero freshly bought back.

## Part 1 — the match

**Opening (t=0, one batch + ready).** Plan `boomer` (8 steps: 3 workers to gold,
2 to lumber, Farm, 3 Workers, Barracks, free Champion) plus four standing
triggers: `supply-valve` (farm on cap), `home-guard` (turtle on base hit),
`hero-save` (hero runs home below 35%), `counter-punch` (push their base when
their hero dies). Blue readied ~20s later; I used the hold to arm the playbook's
`worker-pulse` too.

**Key decisions with timestamps:**
- t=0–100: boomer ran clean; second plan `army` (template Barracks→squad 1,
  2 Footmen 2 Archers, then `secure mid`) sent when the Barracks hit 87%. One
  refusal taught me `unit_count` needs count>=1 — cost one cycle.
- t=98: Keep upgrade via plan `tech` (upgrade hall → Blacksmith → attack research).
- t=150–162: grabbed the 225g mid bounty with squad 1 already anchored on mid.
- t=208–244: **first battle at mid.** Blue's 8-unit push hit my secured squad on
  my anchor. Traded roughly even BUT their hero died in front of my army —
  `counter-punch` fired push automatically. I overrode it to `secure mid`
  (squad at 48%, hero 42%) rather than push wounded. Right call.
- t=254: Priestess out (second hero at the free... no — first-hero rules mean she
  cost 0 because trained at Keep? she queued at 138s and cost the bank ~400/100 in
  practice; the heal made every later fight lopsided).
- t=281–312: NE mine ran dry; `income_collapse` alarm. Plan `expand` placed a
  TownHall on the southeast mine; alarm cleared at t=340 when it finished.
- t=299: army back to 11 at 100% with heal support while Blue was still
  hero-less — **stance push "their base"**. This was the winning commitment.
- t=331–388: Blue revived their hero and met me with ~10 at their base. My
  focus-fire (`Archer>Hero>Footman`), Priestess heal on autocast, and reinforcing
  Footmen (war-pulse trigger + templates into squad 1) ground them down;
  hero hit L5, their Keep fell off my "seen production" list, and Blue surrendered.

**What won it:** (1) the doctrine tier — secure-on-mid meant Blue's first attack
happened on my anchor, not theirs; (2) not spending the counter-punch window at
48% HP, but spending it 60s later at 100% with a Priestess; (3) economy never
stalled — the expansion landed inside the income-collapse window.

## Part 2 — the 2.1 scaffold as a tool

- **Folded page vs old full render:** yes, this changed my reading habits. I read
  `--doc --all` exactly once at t=0 (for the build domains and stance table) and
  then lived on `--digest`/folded `--doc` at loop cadence. The folded ACTIONS
  count line plus DEFAULT line was usually all I consumed; the full render never
  needed re-opening.
- **Acceptance NOTE lines:** got one, at t=162 on my bounty-grab `attackmove`:
  "your intel ledger is empty — nothing seen in 90s". It did change a decision —
  it's why the very next batch included a worker scout toward their base, which
  produced the t=208 sighting that shaped the whole midgame. Best single feature
  of the cycle.
- **Playbook (`standard-kingdom`):** declared in prefs and used as a sanity rail,
  not a script. I took step 1's CONTINUE (worker-pulse, copied verbatim from the
  fork) and effectively ran steps 2–5 through my own plans; never saw an
  INVALIDATED render (my gates never broke). The WHY sentences mattered once:
  the worker-pulse compound-interest argument convinced me to arm it rather than
  hand-queue. After ~t=100 I stopped reading the PLAYBOOK section — my own plans
  had diverged and the you-are-here pointer lagged my actual position.
- **Focus (`economy`):** declared at t=0; the fully-rendered build/train domains
  (with affordability inline) saved a probe-refusal at least twice (TownHall
  "55l short", Workshop "cannot afford"). I never rewrote it to `army` — by the
  time the fight started, digest lines were sufficient. A phase switch would have
  been more honest.
- **Still missing:** (1) a "maintain N workers on gold / on lumber" policy —
  I burned 5+ cycles re-sending `harvest idle workers` because fresh spawns and
  finished builders idle out; rally-to-mine did not fully cover it. (2) A
  "nearest LIVE mine" that ignores walking distance quirks — three workers sat
  idle after a `nearest mine` that resolved oddly. (3) Playbook pointer that
  acknowledges off-book equivalents (it kept me on step 1 long after I had a
  Keep).

**Honest errors on my side:** one rejected plan (`unit_count count>=1`), one
Workshop abandoned to blocked ground and never rebuilt (won without it), and
recurring idle-worker leakage worth maybe 200–300g of mining time.
