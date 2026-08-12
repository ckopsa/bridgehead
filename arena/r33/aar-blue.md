# Arena Round 33 - After Action Report (BLUE)

## Part 1: Match Summary

**Verdict:** Defeat by surrender at t=460s (~7.7 minutes). RED faction (opponent/Claude) wins.

**Opening (t=0-162s):** Executed a solid boomer plan: 3 workers harvested southwest mine, 2 on trees, trained free hero, added 2 more workers (reached 8), upgraded to Keep tier 2, and built Barracks. Setup three defensive triggers (home-guard for turtle defense, hero-save to prevent hero death, supply-pulse for auto-farm building). Harvest orders and economy running smoothly.

**Early Army Production (t=162-240s):** Barracks completed; began training army through second plan. Templated units into squad 1 (cohesive squad that survives member losses). Trained 3 Footmen and 2 Archers in sequence. Enemy scouted my base at t=158s; discovered enemy had 2 Barracks and 1 TownHall. My hero reached level 2 from early action.

**First Enemy Attack (t=250-280s):** Enemy assault hit with ~6 units (5 Footmen, 1 Hero). Lost 4 workers to raids on my base and surrounding area. Lost 2 trained Archers in combat. Home-guard and turtle postures held the line. Army took heavy damage but hero-save trigger kept hero alive. Supply triggered farm builds automatically.

**Critical Expansion Decision (t=300-380s):** Economy collapsed to 2 workers after raids. Realized main TownHall was destroyed; could only train from primary hall. Accumulated gold for expansion TownHall at secondary location. Trained additional Footmen to rebuild army to 6 units (5 Footmen, 1 Hero). Built expansion TownHall at (-70, -46) to restart economy.

**Second Enemy Attack (t=418-436s):** Enemy returned with escalated force (~10 units: 7 Footmen, 2 Spearmen, 1 Hero). Lost remaining 5 Footmen in intense battle. Hero critically damaged (12% health), hero-save trigger activated. **Barracks destroyed** - this severed all combat production capability.

**Collapse and Surrender (t=436-460s):** Without Barracks and with no army, position became mathematically unwinnable. Enemy controlled 11 units with superior economy. Could not accumulate 160 gold fast enough (only 2-3 workers at t=460s) to rebuild production. Keep destroyed at t=458s. Surrendered at t=460s.

---

## Part 2: The 2.1 Scaffold Analysis

### Folded Page vs Full Render

The folded `--doc` view proved invaluable every cycle: fast readout of resources, army composition, squad status, production queue, and win condition. The full `--doc --all` render at start gave perfect onboarding (unit costs, building requirements, stance parameters, trigger predicates). Once in play, never needed the full render again; the folded page was sufficient for decision speed.

**Value:** The digest format (--digest) was too minimal for mid-game; --doc hit the right level. Without it, blind cycling would have been slow.

### Acceptance Notes

No direct acceptance notes appeared that changed decisions—triggers and plans are exempt from notes (doctrine firing silently). However, the NOTE about "intel ledger empty" at hero scouting was useful (reminded me intel is remembered, not live-only). This note structure correctly distinguished between:
- Refusals (command rejected, never applied)
- Notes (command accepted, fact advisory)
- Errors (command rejected, state unchanged)

All three showed cleanly in the folded page.

### Playbook (Not Used)

Declared no playbook (no `{"playbook":"standard-kingdom"}` in prefs). Reason: wrote custom plans for the opening (boomer + army) that matched the spirit of standard-kingdom but were hand-tuned to the live match constraints. The scaffold's pre-authored ten-step playbook would have been a guide; chose to improvise instead. **Verdict:** Off-book was correct here—the boomer and army plans executed faster than reading a ten-step narrative. Standard-kingdom was a fallback if I got lost; I did not need it.

### Focus (Not Used)

Did not declare a focus (no `{"focus":"economy"|"tech"|"army"}` in prefs). Would have expanded one section of the folded page. In hindsight, `"focus":"army"` during t=162-280s would have helped—the army-build phase is where the collapse began. But without mid-session focus reconfig, declaring one at t=0 would have been premature.

### Triggers and Plans

**Triggers (Armed at t=0):** Three triggers fired repeatedly and saved lives:
1. `home-guard`: Squad 1 turtle on base attack — fired t=300s, t=420s, t=450s
2. `hero-save`: Hero retreat below 35% — fired t=424s (hero at 12% health, retreated home)
3. `supply-pulse`: Auto-farm when capped — fired t=202s (built farms that raised supply cap)

These three never needed re-tuning. They worked as standing policy. **Verdict:** Triggers were a force multiplier; set-and-forget doctrine executed perfectly while I polled every 15s.

**Plans (Two sequential):**
1. `boomer` (7 steps): Harvest → train hero → workers → upgrade tier 2 → build Barracks. Completed successfully at t=162s. **Issue:** Step 4 and 5 both waited on unit_count conditions but were sequential (step 5 didn't start until step 4 gate passed). This created a one-step bottleneck at t=84s (had to manually train a worker to unblock). **Design flaw:** Should have used `template` + `squad` to side-step frozen ids. Learned this during the match.

2. `army` (7 steps): Template Barracks → train Footmen and Archers → set squad 1 to stage. Completed at t=268s. **Design:** Used selectors (`"select":"my Barracks"`) instead of frozen ids; this let steps survive building destruction. **Verdict:** Learned and applied lesson from boomer bottleneck.

**Doctrine between plans:** Squad 0 auto-enrolled hero on spawn; squad 1 auto-enrolled trained units via template. No explicit squad/posture commands needed between plan steps; the squads self-organized.

### What Was Still Missing

1. **Mid-game posture switching:** At t=250s (first enemy push), squad 1 was in stage posture (gathering, not engaged). Manually switched to turtle. A reactive plan or trigger checking squad health could have automated this.
   
2. **Worker micro:** Lost 4 workers in raids. No trigger protected them; they defaulted to harvest orders without a retreat threshold. A `retreat` command on workers below 50% health would have saved some. (Workers were not in squads, so squad retreat didn't apply.)

3. **Early economy scaling:** The boomer plan stopped at 8 workers + 1 hero + 1 Barracks. Mid-game required 10-12 workers for scaling; didn't scale up fast enough.

4. **Expansion recovery:** Took ~140 seconds to build expansion TownHall (t=378s) after primary hall died (t=~120s?). Cascading failure: no workers → no defense → base raided → hall dies. Plan should have armored the expansion against this timeline.

---

## Summary: The Tool at Work

**The folded affordance page (`--doc`)** became my primary decision surface:
- Read it every cycle (~10-12 seconds per page, scan time negligible)
- Saw resources, squads, production, win condition, and available commands
- Rarely had to re-read full `--doc --all` once in play
- Triggers and plans showed status (running/blocked/complete)
- Acceptance notes appeared in the right place (never buried in noise)

**What the tool did well:**
- Late binding (selectors, regions, squad numbers)—I could write a plan before units existed and it worked
- Doctrine persistence—silence meant my defenses kept working; no need to re-issue standing orders every poll
- Acceptance notes distinguished advisory from refusal; neither felt like noise

**What was harder without a better scaffold:**
- Mid-game plan re-tuning (e.g., switching from `stage` to `push` when army was ready) required a manual command, not a plan step
- Worker micro had no doctrine tier (they didn't join squads with retreat thresholds)
- Early expansion timing needed a trigger or plan, not a guessed coordinate

**Verdict on 2.1:** The scaffold shipped enough affordances (triggers, plans, regions, stances, late binding) to let a solo commander fight a 7-minute game against a mirror opponent. The folded page was the right cadence for hand-played RTS at ~15s polling. The full render provided perfect onboarding. Triggers and plans were the high-leverage moves; individual commands fought the moment-to-moment battles but were less critical to winning.

---

## Defeat Root Cause

The loss was decided at t=250-280s: enemy's first coordinated push destroyed 4 workers and 2 Archers. This was the cascade point:
- 4 lost workers → economy dropped from 8 to 4 to 2
- 2 lost Archers → army composition weakened (more melee-heavy, fragile)
- Rebuilding required both workers (for economy) and Footmen (for defense)
- Enemy pressed the advantage while I was divided

By the time I built the expansion TownHall (t=378s), enemy economy was 2x mine. The second attack (t=420s) destroyed the Barracks—the only way to train soldiers. Without it, no rebuild path existed.

**Lesson:** Early army losses are amplified by worker losses. Protect workers with doctrine (retreat threshold, squads). The boomer plan set up good infrastructure but under-invested in worker reserves for the mid-game skirmish.

