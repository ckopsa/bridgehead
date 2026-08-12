# After-Action Report — Arena r39, RED / Claude / kingdom, crossings

**Result:** Win by opponent surrender at t=320. My 21-unit army was parked inside their base; no enemy building was actually razed — the surrender came first.
Seat: bridge/red, model claude-opus-5, scaffold affordance-doc/2.2, playbook standard-kingdom (pre-declared).

## How it unfolded

- **t=0–20 — a bad start I caught early.** I readied immediately after the `--doc --all` read. My first batch queued workers, a Hero and a Barracks, but several commands were dropped (queue limits) and `"my base"` was not a place name — the error line taught me `our base`. Worse, the RUNWAY line read `gold 425 +0/min` with `workers 5 (idle 5)`: **workers do not auto-mine on this map**. That single RUNWAY number is what caught it at t=18; without it I'd have burned another minute at zero income. I sent `harvest select:workers target_select:"nearest mine"` and income went to 375/min.
- **t=20–130 — boom.** Triggers did the work: `steady-production` to 13 workers, `farms` on a repeat, `barracks` streaming Footmen from t=40. Second Barracks at ~t=121. Peak income 1481/min.
- **t=130 — the expansion, taken early on purpose.** RUNWAY read `mine 55% ≈ 2.6m at this rate`, which is a shorter clock than a hall's build time plus the walk. I did **not** wait for the alarm. I rewrote the pre-filled expand recipe's trigger from `mine_dry` to `game_time at:130` and retargeted it from `northwest mine` to `southeast mine` — SE ford is the mirror of NW and my army was already anchored there. The hall finished ~t=190; my home mine ran dry at t=270. Expanding on the runway rather than the alarm bought ~80 seconds of unbroken income.
- **t=184–198 — the one real loss.** I had squad 1 on `secure mid` for vision. Blue arrived with ~6 (5 Footman + Hero) into my 10. I won the trade on paper but **my Hero died at t=195**. My `hero-save` trigger was armed at `frac 0.35` and the Hero fell from 38% to dead faster than the 4 Hz watch could pull it. I re-armed it at `0.5` after reviving — correct lesson, wrong number the first time.
- **t=198–226 — reset.** Pulled back to the SE ford, revived the Hero (it kept L2), kept both Barracks running, added a Workshop.
- **t=226–320 — the push.** At 14 units I switched to `push → their base`. The squad repeatedly showed `gathering` and crawled, which the brief names exactly: a squad whose tail keeps being refilled faster than it closes. I diverted new production out of squad 1 and cleared a `reinforce` trigger I had armed (it was re-enrolling fresh home units into the pushing squad and re-triggering the gather every 20s — my own mistake, and the digest's `gathering` status is what exposed it). The squad then reached (-38,-37) at t=291, contacted 6 defenders at their base at t=296, and my Hero went L2→L4 on kills between t=299 and t=307. Blue surrendered at t=320.

## Opponent behavior

Blue played a similar one-base Footman-and-Hero opening, met me at the center ford around t=184 with ~6 units, won the hero kill, then did not scale: at t=296 they still had ~6 (4 Footman, 1 Archer, 1 Hero) and only 3 production buildings (2 Barracks, 1 TownHall). They never contested my SE expansion and never raided my economy. They conceded rather than fight 21 with 6.

## The three instrumented affordances, honestly

- **RUNWAY line — decisive, twice.** `+0/min` at t=18 is the only reason I found the idle-worker bug in the first 20 seconds instead of the first 90. `≈ 2.6m at this rate` at t=130 is what made me expand proactively.
- **`mine_depleting` alarm — never used it, and I bypassed it deliberately.** I tried to arm a trigger on `mine_depleting` at t=101 and the engine refused: it is an alarm, not a trigger `when` variant (valid variants do not include it). I never saw the alarm text fire in a digest either; the RUNWAY percentage had already moved me. Acting at 55% beat waiting for 30%.
- **Pre-filled expand recipe — used as a template, not as written.** Its shape (`build TownHall, select workers, nearest legal site`) I kept verbatim and it worked first try, with no worker id and no coordinate. Both of its *choices* I overrode: the `mine_dry` condition (too late) and the `northwest mine` region (wrong flank for my army). That is the right division of labour — the recipe saved me the syntax, not the judgement.

## Notable mistakes worth logging

1. `hero_below frac 0.35` is too late for a 320-HP melee hero in a 10-unit fight; 0.5 is the floor.
2. Arming an "enrol all army into the pushing squad" trigger actively sabotages a push. Reinforce in discrete waves via a second squad instead.
3. Trigger slots (8) filled up and silently rejected new arms — I had to clear spent ones (`workshop`, `expand2`) to make room.

No autopilot was used at any point.
