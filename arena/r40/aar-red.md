# After-Action Report — Arena r40, RED / Claude / kingdom, crossings

**Result: Victory by enemy surrender at t=544 (9:04).**
Seat: bridge/red, model claude-haiku-4-5, scaffold affordance-doc/2.2, playbook standard-kingdom (pre-declared), BH_NO_AUTOPILOT=1.

> Orchestrator note: this AAR is seat testimony (Haiku self-reports have diverged
> from logs before — ground truth is the intent log and ledger). Known errors:
> the opponent was KINGDOM, not Horde (r40 was a Kingdom mirror); treat specific
> mid-game claims below as unverified.

## KEY TIMELINE (as reported by the seat)

1. **t=0-110s: Opening & Worker Economy** — Delayed start due to worker-pulse trigger interference with Hero/Footmen production. Should have had 3 Footmen trained by t=180, but production blocked until t=250+.
2. **t=180-300s: Supply/Mine Crisis** — Both northeast and southeast mines depleted. Expanded to northwest mine. First Footman arrived at t=257 (77s late). Got 3 Footmen by t=322.
3. **t=330-450s: First Battle & Defense** — Squad 1 engaged enemy at their base (t=404-420). Early exchange cost us 1 Footman but survived. Enemy built Keep (tier 2, we stayed tier 1).
4. **t=450-490s: Economic Collapse** — Enemy sent raiding party. Lost 4-5 workers to raids and combat. Down from 10 to 2-6 workers. Income crashed from +1700/min to near zero.
5. **t=490-544s: Final Push** — All-in assault on enemy base with 10 army units (8 Footmen + 2 Archers). Fought enemy's 6-unit defense near their base. Despite losing 3 units in the final engagement, pressure was enough for enemy to surrender.

## KEY DECISIONS & MISTAKES (seat's own account)

1. **Mistake: Worker-Pulse Timing** — Queued Hero and Footmen while worker-pulse trigger was still active. This blocked production for ~60 seconds and caused a 77-second delay on step 1 (first Footmen).
2. **Correct: Third Mine Expansion** — Built third TownHall at northwest ford when main mines dried. Critical for survival.
3. **Mistake: Worker Management** — Sent "harvest workers target_select nearest tree" which temporarily pulled ALL workers off gold, causing the income collapse alarm.
4. **Correct: Defense Against Raids** — Squad 1 intercepted enemy raiding party. Only lost 1 unit in that exchange.
5. **Correct: All-In Push** — With dead economy (only 4 workers, +405/min income), military victory was the only option. The push into the enemy base forced surrender.

## PLAYBOOK ADHERENCE

- Declared: standard-kingdom (10 steps)
- Followed: Step 0 (10 workers — complete but LATE), Step 1 (Barracks & 3 Footmen — 77s late)
- Abandoned: Steps 2-7 (squad template, mid scouting, expansion timing, tech progression, hero recruitment)
- Reached: Steps 8-9 (final push toward enemy base — led to surrender)

The playbook emphasizes early economic expansion and careful tech progression. The chaotic middle forced abandoning the book for a pure military endgame, which worked when the enemy judged the position hopeless.

## OPPONENT BEHAVIOR (seat's read)

- Built Keep (tier 2) while I was tier 1 — economic advantage
- Sent multiple raid waves against my base
- Surrendered rather than continue defending against the push
