# AAR — Blue (Human/kingdom seat), arena r25

**Result: LOSS by surrender at t=340s. game_over = "Claude" (Red), reason = surrender.**

## Part 1 — The match as I saw it

Opening plan (all armed at t=0 in one batch with ready): boomer economy plan
(3 gold / 2 lumber, hero + 2 workers, Barracks, Farm, second Barracks), a "war"
chain (turtle until 6 Footmen -> secure mid until 10 -> push their base), and
six triggers: home-guard, hero-save (0.35), supply-valve, worker/footman
production pumps, counter-punch on enemy_hero_down.

Key moments:
- t=4: the `unit_count ... below` field in my production triggers was rejected
  (engine wants `count`); I replaced them with repeating `game_time` pumps,
  which worked for the whole match.
- t=60-190: economy ran mostly on autopilot; recurring problem was workers
  going Idle after builds/retasks — I re-shepherded idle workers on almost
  every cycle. Also discovered queued build orders (Farm, Barracks #2, Sanctum)
  silently evaporate when the assigned worker is retasked before placement.
- t=~205: THE LOSING DECISION. My "war" chain advanced at 6 Footmen to
  "secure mid" — and walked hero + 5 footmen into the enemy's full 12-unit army
  (5 Archer, 5 Footman, Hero, Sorcerer) at center ford. Army wiped, hero dead
  inside one poll; hero-save at 0.35 did not get him out against focused fire.
  Losing the free first hero meant a 400g/100l revival I could never afford.
- t=298: enemy army of 12 arrived at my base with me holding 2 footmen and
  ~25 gold. Blacksmith, farms razed; CallToArms bought little; squad 0 wiped
  at t=308; Keep at 46% by t=324 with zero income possible under siege.
- t=340: surrendered — no army, no gold, no path back.

What lost it: (1) an aggression chain keyed on my own unit count with no
condition on enemy strength — "secure mid at 6 footmen" should have been gated
on intel (enemy_army_seen small, or at least scouted); (2) never scouting, so
the first sight of their 12-supply army was when it killed mine; (3) worker
idling / dropped build orders bled maybe 60-90s of income and delayed Sanctum,
so there was no second wave. Opponent played a clean one-base army timing and
hit exactly when my tech (Keep/Blacksmith/Sanctum) had spent the bank.

## Part 2 — The document as a tool

- **What I used:** the `--doc` view once at t=0, then `--digest` every cycle.
  Command-wise: ~6 form-filled templates from the doc (stance, trigger_set,
  plan_set shapes, build with `site: nearest legal site` default — that default
  is excellent), ~10 raw intents (harvest with explicit ids, cast, train), and
  the recipes as models (home-guard/hero-save copied nearly verbatim; the
  steady-production recipe copied verbatim and REJECTED — see below). Default
  confirms (silence) on maybe a third of cycles.
- **What changed decisions:** the DEFAULT line ("if you say nothing: ...") was
  the single most-read annotation and correctly told me when silence was safe.
  The alarm running-defaults were decisive at the end — "nothing recovers this
  automatically" on income_collapse and "no armed trigger covers a sighting"
  made the hopelessness legible and prompted the surrender. Build-kind
  affordability annotations ("cannot afford (55l short)") were used at t=0.
- **What misled me:** the `steady-production` recipe in the doc uses
  `unit_count(kind, below)` — the engine rejects `below` ("missing field
  count"). A recipe the doc itself prints should compile. This cost me a cycle
  and left me with a worse workaround (game_time pumps that fire forever and
  spam "cannot afford" errors).
- **Noise:** repeated trigger-fire EVT lines for the pumps drowned the event
  feed; "workers (idle N)" in the digest was essential but the doc never
  surfaces WHY workers went idle or that a pending build order was dropped on
  retask — that silent cancellation cost real money and deserves an event line.
- **What it should serve but does not:** (1) an intel line stronger than the
  win-condition scout nag — "enemy army last seen: size, where, when" in the
  digest every cycle, not only as an alarm when it is already at my base;
  (2) a warning annotation on plan steps whose advance is own-strength-only
  ("this step commits squad 0 with no enemy-strength gate") — that is exactly
  the mistake that lost this match; (3) idle-worker attribution.
