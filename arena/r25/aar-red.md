# Arena r25 — AAR, RED seat (Claude/kingdom, map crossings)

**Result: WIN by opponent surrender at t≈340s.** Game over line: `Claude wins (surrender)`.

## Part 1 — the match as I saw it

**Opening (t=0, one batch + ready):** 3 workers to the near mine, 2 to lumber; a 7-step
plan (Farm → free Hero → Barracks → 3 Workers → upgrade to Keep on 8 workers); four
standing triggers — supply-valve (farm on cap), home-guard (turtle on base attack),
hero-save (hero walks home below 35%), counter-punch (push their base when their hero
dies); squad 1 pre-stanced `stage`.

**Key decisions:**
- t≈30: noticed the plan's Barracks step had "advanced" but no Barracks existed
  (the payment apparently never landed — likely the Keep upgrade raced it for funds).
  Rebuilt it by hand; delayed my army ~40s but the economy was already compounding.
- t≈90: Keep done at 1:30. Skipped the 400g/100l Priestess; spent on army instead.
- t≈160: lumber crash to 10 with archers queued; moved 4 workers to wood.
- t≈172: hero enrolled into squad 1, stance `secure center ford` — this both scouted
  and picked the fight on my terms.
- t≈208: **the match's hinge.** First ford battle: their 6 Footman + Hero met my
  7 (4 Archers). Their hero died in the fight; my counter-punch trigger fired the push
  by itself, my hero-save trigger walked my 27% hero home by itself. I overruled the
  instant push (squad at 53%), consolidated on `secure center ford`, then `forage` mid
  (claimed a 270g cache).
- t≈278: with 12 units at 100% and 2 Sorcerers queued, a one-step chain re-fired the
  push (counter-punch also re-fired — their hero was still down). Pushed into their
  base, razed a Barracks then their Keep. They surrendered before my income-collapse
  fix (home mine dried at t=311; SE expansion hall had just finished) could matter.

**What won it:** winning the first mid fight with an archer-heavy comp and focus
`Hero>Footman`, and the trigger pair hero-save + counter-punch converting that fight
into a hero-trade I won 400g clean. Opponent's behavior: mirrored tech (Keep+Sanctum
seen), pushed mid at ~3:30 with a footman-heavy army, never recovered after losing
the hero and never raided my base once.

## Part 2 — the document as a tool

- **Rungs used:** mostly **raw intents** (~70% of sends) and **forms conceptually** —
  I read the domains (build costs/availability, stance table, predicate list) off the
  doc once at t=0 and then wrote JSON by hand. Verbatim **links**: 0. **Default
  confirms** (silence): ~6 of ~20 cycles, always deliberate after reading the DEFAULT line.
- **Annotations that changed decisions:** the `WIN raze their production: N seen`
  counter was the single best line — watching 4→3→2 during the siege told me the push
  was working without parsing buildings. The `gathering` vs `pressing on` squad status
  told me when the push had actually committed. The income_collapse alarm's
  running-default ("nothing recovers this automatically") correctly told me silence
  was wrong and I re-tasked miners. Build-kind domains with "cannot afford (55l short)"
  style reasons saved cycles twice.
- **What misled / was noise:** (1) plan step advance on "accepted" let the Barracks
  step advance while the building never materialized — nothing in the doc or events
  flagged that a paid build vanished; I found it only by diffing buildings[]. (2) The
  digest EVT window is short and stale-looking (same 5 lines for many cycles). (3)
  `unit_count(count=0)` being illegal made my "wait for barracks" idiom fail; the
  domain table didn't say count>=1. (4) `units[]` mixes enemy workers into a kind
  filter with no team field surprise — I nearly re-tasked enemy workers; the digest's
  "workers 13" vs my 18 kind=Worker rows disagreed silently.
- **Wish list:** a link/form for "re-task N workers to gold/wood" (worker rebalance is
  the most frequent chore and the only place I had to fish raw ids); a doc annotation
  when a plan step's spend was accepted-then-lost; per-squad "in combat / winning /
  losing" hint; team tags in every units[] row of the raw view.
