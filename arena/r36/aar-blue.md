# AAR — Blue (Kingdom), arena r36

**Result: WIN by surrender at t≈343s** (game_over: Human, reason: surrender).

## Part 1 — The match

**Opening (t=0, one batch + ready).** 3 workers to gold, 2 to lumber, free Hero
queued, and four standing triggers armed before the clock ran: `worker-pulse`
(train Worker on idle hall), `supply-valve` (Farm on supply cap), `home-guard`
(turtle squad 1 on base attack), `hero-save` (hero retreats below 35%). Barracks
and a Farm ordered in the same batch. The first Farm was abandoned twice — site
clash at the auto-picked spot — fixed at t=20 with explicit coordinates.

**Key decisions with timestamps.**
- t=38: `army` plan armed — template Barracks→squad 1, 3 Footmen + 2 Archers,
  final step `stance secure center ford` gated on unit count. It block-retried
  through every "cannot afford" and completed at t=91 without another poll spent.
- t=91–124: squad 1 (5→7 units) held center ford; `tech` plan armed at t=114:
  upgrade to Keep → Sanctum → Sorcerers. Keep finished t=164.
- t=178: Priestess bought as second hero (400g/100l).
- t=222: worker-pulse retired at 11 workers (upkeep tax had started); armed the
  counter-punch trigger (`enemy_hero_down` → push their base).
- t=238: **the commit** — 14 units, full HP, and an intel ledger empty all
  match. Sent `stance push their base` and took the acceptance NOTE ("ledger
  empty — not the same as their having nothing") knowingly: the army was the scout.
- t=262: first contact revealed the whole story — 2 TownHalls, 1 Barracks,
  almost no army. Red had double-hall boomed with roughly one Footman and a Hero.
- t=306–326: our SW mine ran dry (income_collapse alarm) — mattered not at all,
  because at t=317 the counter-punch trigger fired: **we watched their hero die**
  and squad 1 re-committed automatically. One of their halls was already down.
- t=343: Red surrendered.

**What won it.** Doctrine + standing orders did nearly everything: the army plan
built the force, the template kept reinforcements enrolled, the secure stance
held mid (and vision) for free, and the push landed against an opponent who had
spent everything on economy and nothing on defense. Losses: 2 Footmen, 1 Archer.

**Opponent behavior.** Pure greed: two halls, farms, one Barracks, a lone
Footman, hero forward without support. No scouting pressure on us all game, no
harassment, no army at 4 minutes. When the punish arrived they had nothing, lost
the hero and a hall inside 60 seconds, and conceded — a correct concession.

## Part 2 — The scaffold in a cross-tier fight

**What I used.** The playbook (`standard-kingdom`) only as a checklist skim — I
went off-book in the first batch (its step 1 was the worker pulse; I sent the
pulse AND the hero AND the barracks at once, and the page immediately showed the
step INVALIDATED with the broken assumption in numbers, which was honest and
useful). The real scaffold load-bearers were: **plans** (army, tech — 2/2 slots,
both completed; block-retry on "cannot afford" is the best feature on the wire,
it turns an economy race into something you never poll for), **triggers** (7 of
8 slots at peak; hero-save and their-hero-down both fired and both mattered —
the counter-punch fired the winning order while I was reading a digest),
**templates + stances** (reinforcements inherited the push; I never issued one
unit-level combat order all match), and the **acceptance NOTE** on the blind
push, which is exactly the right shape: a fact, not a refusal.

**What their play said about their scaffold use.** Red looked like a commander
running the economy half of a playbook with no exit taken: double hall and worker
mass is a book line, but nothing ever answered `enemy_army_seen` — my 14-unit
squad crossed the map unopposed, which suggests no doorbell trigger, no
home-guard, possibly no polling on the alarm. Their hero met my army alone,
which suggests no hero-save either. Cross-tier, the visible difference was not
game knowledge but *standing-order coverage*: I had answers armed before the
questions; they answered nothing even after the question was on fire.

**What the document could serve at my tier that it does not.**
1. **A build-site sanity fact.** My first Farm died twice to "ground no longer
   clear" at the auto-picked site; the doc happily serves `region:our base,
   site:nearest legal site` with no hint that two builds in one batch can
   collide at the same nearest-legal-site. One line — "N other builds pending
   near this point" — would have saved 20 seconds and 2 events.
2. **An army-delta line in the digest.** The WIN line counts enemy production
   seen; nothing summarizes *my* strength vs the ledger's best guess ("your str
   2085 vs last-seen force ~2"). I computed it by hand from `intel.groups`
   every contact. At LLM latency that is the one number a commit decision needs.
3. **Idle-worker diagnosis.** `back-to-work` fired repeatedly while 3 workers
   sat idle (stranded at a dry mine, apparently failing to path to "nearest
   tree"). The digest counted them idle; nothing said *why the standing fix was
   not fixing them*. A `running_default`-style sentence on the idle count
   ("3 idle; your trigger back-to-work last reached 1 of them") would have
   surfaced the failure two minutes earlier.
4. **Trigger interaction preview.** worker-pulse + steady-footman + plans all
   drained the same bank; the errors channel showed the symptom ("cannot afford")
   one bounce at a time. A one-line spend-rate vs income fact would let a
   commander see the over-subscription instead of inferring it.

None of these are strategy advice; all are facts the engine already has, which
is exactly the doc's own standard.
