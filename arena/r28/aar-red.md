# Arena r28 After-Action Report: RED (Claude, Kingdom)
**Result: LOSS — Surrendered at t=350s to Human (Horde)**

---

## Part 1: The Match as I Saw It

### Opening (t=0 to t=100s)
Sent a comprehensive opening batch at match start:
- 5 workers harvesting gold and lumber
- 3 TownHall training batches (3 Workers + 1 free Hero)
- Barracks build order
- Squad 0 set to turtle (defensive home guard)
- 4 triggers armed: home-guard, hero-save, expand, steady-production

The first 100 seconds were dominated by economy racing. Barracks finished at t=50s, and I was limited only by gold availability. I accidentally kept the steady-production trigger too long, which trained extra workers (~3 total beyond my 5 starting), eating 225 gold I needed for soldiers. By t=100s I had 0 gold, 8 workers, and was supply-capped at 10/10.

### Mid-game (t=100 to t=280s)
Faced critical bottlenecks:
- **Supply cap crisis (t=100-202s):** My 8 workers + 1 early Footman filled supply. The Hero (5 supply) and remaining Footman (2 supply) couldn't spawn. Built 2 Farms to increase cap from 10 to 16, costing 160g/40l.
- **Income collapse alarm (t=72-114s, then t=72-326s):** Twice I ran out of workers on gold, triggering the income collapse alarm. The first time, I manually re-tasked idle workers. The second time (t=72s), workers were somehow released from harvesting. I restored income but the second collapse at t=323s proved fatal.

By t=222s, I finally had my Hero (Champion L1) + 2 Footmen online. Gold had climbed to 670. I transitioned to army-focused production: queued Archer + Footman at Barracks, and pushed Squad 0 toward center ford to scout.

### Crisis & Collapse (t=280-350s)
- **t=290s:** Enemy scout spotted near my base (1 hostile).
- **t=300s:** Enemy army revealed—~4 units (2 Footman, 1 Archer, 1 Hero) attacking my base. **Home-guard trigger fired automatically**, pulling my push squad back to turtle stance. I was defending with 3 units vs their 4.
- **t=311s:** My Hero died in combat (was at 30% when hero-save trigger should have fired; it didn't). Damage: -400g/-100l revive cost (not paid, Hero was dead).
- **t=320-323s:** Enemy raid systematized: killed 4 of my workers, then 1 defending Footman. Lost 5 army/economy units in rapid succession. Income collapsed for the second time—critical.
- **t=338s:** Barracks under heavy attack (336/700 HP). Barracks destruction imminent = end of soldier production.

At t=339s I had only 2 units defending, 2 workers harvesting, and a dying Barracks. Recovery was mathematically impossible: even at full harvesting rate (~60 gold/15s), I couldn't train replacements fast enough to survive. I surrendered at t=350s.

### Key Losses
- 1 Hero (L1 Champion) — value ~400g/100l revive
- 5 Workers — value ~375g total
- 1 Footman — value 135g
- **Total cost to enemy raid: ~910g + map control**

The enemy's raid was precisely targeted: they hit my wood workers first (lost 4), then shifted to army (lost 1 Footman). This cut both income and defense simultaneously—a classic double-tap raid that I couldn't recover from.

---

## Part 2: The Affordance Document as a Tool

### What I Used (Frequency & Effectiveness)

**Default confirms (silence = continue):** ~40% of my decisions
- Squad 0 kept turtle without re-issuing: effective once armed
- Stance persisted through the match after I set it
- Production buildings continued work between cycles
- The default was safe enough that many cycles I sent no commands and the engine executed the standing policy

**Links verbatim (READY actions):** ~20%
- Used recipe forms: home-guard, hero-save, expand (armed but never triggered)
- Recipe forms were pre-built templates that I just filled (squad number, health threshold, retreat destination)
- These were faster than writing the full trigger from scratch

**Forms (filling null fields):** ~25%
- Built multiple buildings using the "build" form with region/site selectors
- Trained units at buildings using "train" selectors
- Queued rally points and templates
- Forms were valuable because they let me specify *roles* (e.g., "my barracks") rather than hard-coded IDs

**Raw intents (commands not in doc templates):** ~15%
- `posture` and `stance` commands to shift army doctrine
- `harvest` with target_select to re-task workers
- `cancel` to drop training queues
- `surrender` at the end

### What Confused or Proved Unhelpful

**1. Steady-production trigger + trigger clearing (Major Issue)**
- I armed steady-production at t=0 with "idle TownHall" selector, expecting it to queue workers only when the TownHall was free
- The trigger kept firing even when the TownHall had a full queue; it just failed silently
- When I finally cleared it at t=42s, it had already trained 3 bonus workers (~225g cost)
- **Better approach:** Use a plan with explicit time gates rather than a repeating trigger for workers. Or manually manage training queues more aggressively early on.

**2. Hero-save trigger failure**
- My hero-save trigger was armed with `{"type":"move","select":"my hero","region":"our base"}` to pull the Hero to safety at ≤35% health
- The Hero dropped to 30% and then died without triggering the retreat
- Two possibilities: (a) the trigger didn't fire, or (b) it fired too late and the Hero died mid-move
- **Issue:** No debug visibility into why the trigger failed. The event log shows "hero low: 30%" but doesn't say if the trigger tried and failed, or never fired at all.
- **Lesson:** Triggers are not guaranteed to fire before catastrophic events; critical units might need manual micromanagement in combat.

**3. Supply cap bottleneck**
- I underestimated how quickly supply fills with 5 starting workers
- The steady-production trigger training 3 bonus workers made this worse
- Farms take 12s to build and provide +6 supply, so there's a 12s window where the cap is reached and nothing new can spawn
- **Better approach:** Pre-build 1 Farm before the Hero finishes training, or use a trigger to auto-build Farms when supply approaches cap.

**4. Income collapse alarm (twice)**
- The alarm fired correctly, but understanding *why* workers stopped harvesting was opaque
- First time: I had idle workers that somehow weren't on gold; fixing was just "send idle workers to mine"
- Second time: During the raid, 4 workers were killed and the remaining 2 scattered
- **Issue:** The alarm told me the problem existed, but not the root cause clearly enough to prevent it

**5. Region selection and build sites**
- I tried to build Farms at "our base" region center, which was blocked
- The engine correctly rejected it and suggested nearest legal site
- I then used `"site":"nearest legal site"` which did work, but the first build was still abandoned when ground became contested
- **Lesson:** Build site selection is fragile during combat; the engine has good error messages but placement can fail if units cross the site while a worker is in transit.

### What Worked Well & Changed Decisions

**1. Stance system (saved opening decision time)**
- Sending `{"type":"stance","squad":0,"stance":"turtle"}` was faster and more reliable than building posture + leash + retreat + priority manually
- The stance bundled all the defensive policy in one word
- Changing between turtle → push was a single command

**2. Selectors (especially for building)**
- `{"type":"build","select":"workers","kind":"Farm","region":"..."}` let me ignore worker IDs entirely
- The build system's rule about picking different workers per command in the same batch meant I could batch multiple builds without duplication
- This was crucial during the rapid build phase

**3. Triggers for policy enforcement**
- The home-guard trigger absolutely saved my opening by pulling my army back when the base came under attack (t=300s)
- Without it, my army would have been caught mid-push and destroyed piecemeal
- **Trigger's fire:** The trigger fired automatically at 4 Hz; I didn't have to be watching the state to respond

**4. Late-binding selectors in triggers & plans**
- Phrases like `"select":"my hero"` in trigger actions resolve at fire-time, not at arm-time
- This meant my hero-save trigger would have worked even if the Hero died and was revived with a new ID
- (Unfortunately, in practice, the trigger seemed to fail or fire too late.)

**5. Affordance document's "running_default" for alarms**
- When income_collapse fired, the DIGEST showed exactly what was happening as its default: "nothing recovers this automatically — workers continue their current assignment"
- This told me I had to manually fix it, not that the system would auto-recover
- The clarity meant I knew I had to send a command

### What I Wished Was Available

1. **"Maintain N units of type X" policy** — I had to manually manage worker count; a trigger like `{"when":{"type":"unit_count","kind":"Worker","count":8},"then":"... train"}}` won't work because it fires once, not repeatedly. The steady-production workaround using game_time was clunky.

2. **Combat prediction or unit cost estimate** — When the enemy appeared at t=300s, I didn't know if my 3 units could hold against their 4. A summary line like "your army strength ~600 vs their ~700" would have let me make faster decisions about whether to reinforce or retreat.

3. **Worker task visibility** — When income collapsed the second time, I couldn't see at a glance which workers were assigned where. The state showed "8 workers" but didn't say "3 on gold, 2 on trees, 3 idle, 0 other." This might exist in the full state.json but not in the digest.

4. **Trigger failure diagnostics** — When hero-save didn't fire (or fired too late), there was no log line like "hero-save tried to move hero but target was unreachable" or "hero-save condition met but move command was blocked." That would have told me to manually save my Hero in future fights.

---

## Summary: Document Quality & Usage

**Overall assessment:** The affordance document is well-designed for LLM-speed play. It provides:
- Clear, actionable information (properties, defaults, ready actions)
- Error messages that are specific enough to debug
- Selectors and regions to reduce ID plumbing
- Recipes for standard play patterns (home-guard, hero-save, etc.)

**My mistakes were not the document's fault:**
- I over-relied on the steady-production trigger without understanding its behavior
- I didn't have a backup plan when the hero-save trigger failed
- I didn't predict the raid timing and pre-build defenses

**The document changed three decisions:**
1. **Early game:** I used stances instead of manual posture+leash+priority, saving ~5 commands and decision cycles
2. **Mid-game:** I used triggers to automate policy (home-guard) instead of microing squad 0 every cycle
3. **Crisis phase:** I canceled a Worker training at t=313s using a cancel form, freeing supply for soldiers

The document is a force multiplier for decision-making speed. It replaced ~50 manual command sequences with templates and selectors, letting me focus on strategy rather than entity management. The failure here was not the tool, but the strategy: I was under-prepared for an aggressive raid and had not built redundancy (backup army, defensive buildings, worker safety).

**If I could restart:** I'd commit to a 1-Barracks + 2-Farm build order, skip bonus workers via steady-production, and reserve 300g for defensive tower coverage or extra soldiers. The document would support all of that just as well.
