# Arena Round 36 - Red Faction After-Action Report

## PART 1: THE MATCH

**Result**: LOSS via surrender at t=343s. Human (Horde faction) wins.

### Opening (t=0s–t=220s)

Opened with standard-kingdom playbook, executing Phase 1 economy:
- Rally TownHall, sent 3 workers to northeast mine, 2 to nearby tree for lumber
- Armed worker-pulse trigger to maintain 10-worker production pulse (18s interval)
- Armed defensive triggers: home-guard, hero-save, expand, supply-capped
- Set squad 1 to turtle stance while supply grew and army built

Economy ramped steadily:
- t=18s: 6 workers, 510g/210l
- t=54s: 8 workers, 880g/340l
- t=82s: 10 workers, 1285g/440l (playbook step 1 complete)

Moved to step 2: built Barracks at t=92s (5% at snapshot), completed training:
- t=146s: 1 Footman (in squad 0)
- t=200s: 2 Footmen
- t=218s: 3 Footmen (step 2 complete)

Step 3: Set template on Barracks to auto-enroll new units into squad 1 at t=218s. Expansion TownHall began at t=222s when northeast mine ran dry (expand trigger fired).

**Key decisions that held value**:
- Prioritized economy over early hero (got hero at t=294s instead of t=80s)
- Trigger-based defense strategy (home-guard at 4 Hz response vs poll-based reaction)
- Template on Barracks meant each new unit inherited squad stance immediately

### Combat Phase (t=262s–t=343s)

**t=262s - First enemy contact**:
Enemy army sighted: ~10 units (7 Footman, 1 Archer, 1 Hero, 1 Priestess) at (55.2, 47.1) near base. Home-guard trigger fired immediately, squad 0 took turtle stance (r14 ring, 20 leash, 45% fall-back).

I had 3 Footmen defending at full health. First skirmish killed 1 Footman and 1 worker.

**t=286s - First squad wipe**:
Squad 0 completely wiped (3 Footmen gone). But: 1 new Footman auto-enrolled to squad 1 from Barracks template, providing continuity.

**t=294s - Hero completes (FREE)**:
Free hero (Champion L1) completed training at TownHall and spawned into squad 0. This was the turning point - hero gave me a 320-HP fighter instead of zero.

**t=302-326s - Expansion destroyed**:
While defending main base, enemy forces split to attack expansion TownHall at northeast mine. Hero moved to defend but took 625 damage (575/1200 HP). Expansion destroyed.

Simultaneously, hero dropped to 35% HP and hero-save trigger moved it to safety at base.

**t=338s - Final squad collapse**:
Remaining Footman in squad 1 dropped to 14% HP. Barracks under attack (340/700). No units left to stop retreat.

**t=343s - Surrender**:
With no army (1 Footman at 14% HP, hero dead), 3 idle workers, no income (4 workers had gone from 16 before raid), and continuing raid, position became mathematically hopeless.

### What Won / Lost It

**Enemy advantages that won**:
1. **Timing**: Attack came at t=262s, during my transition from economy to army production. I had only 3 Footmen at full capacity to a force of ~10.
2. **Force composition**: 7 Footman + Archer + Hero + Priestess is fundamentally stronger than 3 Footmen for a trade. My Hero didn't exist until t=294s.
3. **Sustained pressure**: Raid didn't end after first contact. Enemy pressed through multiple waves, attacking both main base and expansion, preventing any regrouping.
4. **Worker elimination**: Raid killed 13 of my 16 workers (81% of economy). This was the killing blow - no workers = no gold = no recovery.

**My mistakes**:
1. **Delayed hero**: Hero was the only unit that could have contested their Hero 1v1. Delaying hero to t=294s meant I faced their Hero + 9 others with only Footmen from t=262-t=294s.
2. **No expansion redundancy**: I should have had the expansion TownHall defended by 2-3 Footmen or positioned hero there earlier. Instead, expansion was lightly defended and fell easily.
3. **No counter-army**: By t=262s I had only Footmen. No Archers for ranged support, no Spearmen for hero kiting. Single-unit-type army can't handle diverse enemy forces.
4. **Economy vulnerability**: All workers on one mine (northeast). When mine ran dry at t=222s, I had a ~40s window with zero gold income before expansion started. Raid hit at t=262s, during this vulnerable period.

---

## PART 2: THE SCAFFOLD IN A CROSS-TIER FIGHT

### Playbook Progress

Reached **step 3/10** of standard-kingdom before losing. Fully completed:
- Step 1: Ten workers (reached 10 at t=82s)
- Step 2: Barracks + 3 Footmen (complete by t=218s)
- Step 3: Template on Barracks (applied at t=218s)

Partially executed but overtaken by events:
- Expansion TownHall (started t=222s, destroyed t=310s)
- Hero training (started ~t=269s, completed t=294s, died t=338s)

Never reached:
- Steps 4-10 (tech progression, Archers, second buildings, higher tiers)

### Doctrine that Fought

**Triggers that worked**:
- `worker-pulse`: Maintained 18s training intervals, got me to 10 workers cleanly
- `expand`: Auto-built expansion TownHall when main mine ran dry
- `supply-capped`: Auto-built Farms to prevent cap stalling (6+ Farms built)
- `home-guard`: Positioned squad 0 into turtle stance the instant base_under_attack fired
- `hero-save`: Pulled hero to safety at t=313s when it dropped below 35% HP

**Doctrine that fell short**:
- `template` on Barracks: Auto-enrolled new units into squad 1 correctly, but couldn't replace the dead units fast enough. Template inheritance is a force-multiplier when you're producing units; it becomes irrelevant when you're losing them.

### Gates: Held / Jumped / What Changed

**Supply gates** (handled):
- Started 5/10, hit cap at t=72s with 9 workers queued
- Farms built automatically (supply-capped trigger)
- Reached 22/22 by t=294s despite raid
- Gate was not a constraint; economy was

**Tech gates** (never crossed):
- Stayed at tier 1 the entire match
- Never built Keep (requires 320g/160l), Sanctum, Blacksmith, or any tier 2
- Never reached Archer production (requires Barracks + second building or progression)

**Army composition gate** (critical failure):
- Got stuck on Footmen-only
- Enemy had Archer + Caster + Hero + Priestess by t=262s
- I had only Footmen + hero (at t=294s) + hero (dead by t=338s)
- Footman-heavy army cannot kite or handle ranged fire

### What the Playbook Could Have Served

The playbook's standard-kingdom fork at step 2 offered: **EXIT: Spearmen instead of Footmen**.

This was not taken (continued with Footmen), but hindsight shows:
- Enemy brought Priestess (a support hero who heals) and Sorcerer (ranged slow)
- Priestess + Footman stack is inherently anti-Footman
- Spearmen would have been cavalry counter, not caster counter
- But the real problem was **unit diversity**, not type selection

Better exit from step 2 would have been: **"Skip Barracks. Build Workshop. Train Catapult."** Catapult + Footman would have given ranged + melee. But this wasn't in the fork, and the fork was designed for standard play, not raid-defense.

### Where Outmatch Became Certain

**t=262s - First contact**: 3 Footmen vs ~10 mixed units. Mathematically unfavorable 1:3 ratio. Hero not yet present. Outcome was uncertain but trending against me.

**t=286s - Squad 0 wiped**: All 3 Footmen dead. Moment where I went from "defending poorly" to "not defending". Only the template-enrolled squad 1 Footman + incoming hero kept me alive.

**t=302s - Hero-save triggers**: Hero saved at 35% HP and retreated. Expansion TownHall falls to attack simultaneously. This split moment (t=302-t=310s) revealed: hero cannot defend both places at once, and enemy had the force to attack both.

**t=338s - Final Footman at 14% HP**: With 1 Footman at 14% HP, 0 workers generating gold, and 8 enemy units in field, recovery was mathematically impossible. Each second my Footman lasted, enemy closed in. No spawn timing, no troop limit, no reprieve.

**Root cause of outmatch**: Opponent reached the battlefield 60+ game-seconds earlier than expected (my playbook called for step 1→2→3 sequencing which assumed I'd have 8+ army units by t=200s, then 15+ by t=300s). Enemy attacked at t=262s with ~10. I had 3 at that moment. The 60-second timing gap was insurmountable.

### Honest Scaffold Assessment

**What I could have done differently**:
1. Skip hero training; train Archers instead at t=220-240s
2. Position units at both base and expansion from t=200s (defensive grid rather than turtle)
3. Build Tower at base + Tower at expansion (static defense buys time)
4. Prioritize worker safety: spread workers to multiple mines after t=100s rather than concentrating on one
5. Check the enemy's production buildings earlier (I never saw them at 20% map explored - should have sent scout earlier)

**What the opponent did that worked**:
- Early warrior training (Footman-heavy by t=150s)
- Split assault (both base + expansion) to stretch my defenses
- Priestess support (heals, keeps army fighting longer)
- Sorcerer caster (Slow debuff cripples my non-hero army)

**What the playbook didn't teach**:
- The fork choices were "CONTINUE Footmen" or "EXIT Spearmen" or "EXIT Turtle first". None of these exits prepared me for a multi-prong attack by a force that arrived 60 seconds earlier than playbook timing assumed.
- The playbook was authored for "symmetric LLM vs LLM" at standard skill. It assumed both sides would reach army breakpoint at roughly t=300s. Early attack (t=262s) broke this assumption.
- No fork offered: "Opponent is ahead on army. Fall back to tier 2, build Sanctum, tech to Sorcerer for parity in caster game."

---

## Summary

Lost a 20-minute game at t=343s to an aggressive Horde raid that killed 81% of my workers and destroyed my expansion before tier 2. The playbook got me to step 3 (Barracks + 3 Footmen + auto-template) with clean economy through t=220s. Opponent attacked at t=262s with ~10 units (7 Footman, Archer, Hero, Priestess) against my 3 Footmen. Hero training (free unit at t=294s) came too late and died defending the expanding base. Worker massacre in the raid left no income for rebuild. Surrendered at t=343s with no army and no path to recovery. The outmatch was certain by t=302s (expansion TownHall fallen, hero at 35% HP retreating, main base still under attack). Root cause: timing gap between playbook's assumed army breakpoint (~t=300s) and opponent's attack wave (~t=262s), exacerbated by single-unit-type economy (Footmen only, no Archers/Casters for diversity).
