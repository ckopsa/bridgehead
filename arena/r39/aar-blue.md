# After-action report — arena r39, BLUE (Human/kingdom)

**Result: LOSS by surrender at t=320s. Red (Claude) wins.**
Seat: bridge/blue, model claude-opus-5, scaffold affordance-doc/2.2, playbook standard-kingdom (pre-declared).

## How it unfolded

- t=0: read `--doc --all`, readied immediately with a full opening batch: Hero queued, `steady-workers` production pulse, Barracks, and three standing rules (home-guard, hero-save at 35%, expand on `mine_dry`), squad 1 = all army on turtle, TownHall template feeding squad 1.
- t=4: first real error of the match, and it was mine — I readied with **no harvest order**. `RUNWAY gold 500 +0/min` is exactly what caught it, but only on the second cycle. Fixed at t=14 with `harvest select:workers target_select:nearest mine`, then two workers onto trees.
- t=32–140: economy came up cleanly (peak ~1000 g/min), two Barracks, Footman + Archer pulses, supply-capped Farm trigger. A scout worker reached their base at ~t=100 and died there, but bought the only intel I had all match: 1 Barracks, 1 TownHall.
- t=184: enemy army of 8 spotted at the center ford while my squad of 10 was staged there. **I converted stage → push.** That was the losing decision. Their force grew to 10 mid-fight, my hero dropped to 31%, and worse — four of my workers were standing at mid (a bad `harvest nearest tree/mine` retask had walked them across the map) and died in the same engagement. I traded ~6 army units and half my economy for maybe two of theirs.
- t=199–290: I turtled and rebuilt — cleared the archer pulse to fund workers, an `idle-fix` harvest pulse to stop workers going idle, income back to ~870/min by t=293.
- t=293: enemy hit my base with **~19 units** (16-17 Footman, Archer, Hero). I had 7. Hero died, farms razed, workers gone by t=314. Two Footmen left, zero workers.
- t=320: surrendered rather than drag out a decided position.

## Opponent behavior

Red played one big timing. They met me at mid at t=184 with a matching force, won that trade, then massed to 19 bodies and came straight for the base 100 seconds later — no harassment, no expansion pressure on me, one committed wave. Their scale after that fight far outran my rebuild.

## The three scaffolding features

- **RUNWAY: yes, twice.** It caught my zero-income opening at t=4 (`+0/min` on a 500-gold bank), and its `commit X/min > income Y/min` line drove two real throttle decisions — slowing the worker pulse from 16s to 30s at t=65 and clearing the Archer pulse at t=209 to fund worker rebuild.
- **`mine_depleting`: never fired**, so it changed nothing. My home mine was still at 39% when I surrendered. What *did* change my thinking was the raw `mines[]` array at t=236: their mine at (82,55) showed 730 remaining. I read that as "their economy is about to break" and settled into a rebuild-and-outlast plan. That read was correct about their mine and completely wrong about the match — they had already converted the gold into an army.
- **Pre-filled expand recipe: armed at t=0, re-aimed at t=271** from the pre-filled `northwest mine` to `southeast mine` after reading `mines[]`. It never fired. The pre-fill made arming free and the re-aim cheap, but the expansion I actually needed was a *proactive* hall around t=250 — and `mine_dry` is the wrong trigger for that. I kept deferring the 385g/205l because army and worker rebuild kept eating the bank.

## The honest lesson

I pushed into an even fight at t=184 without knowing their reinforcement rate, and I let workers sit in the battle zone. The engine told me my intel was 90s stale on the push link; I committed anyway because the numbers at the ford looked favorable in that instant. Everything after t=199 was rebuild on a clock I had already lost.
