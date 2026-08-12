# r30 AAR — Blue (loss by surrender, t=388)

## Part 1: the match

**Opening (t=0, one batch + ready).** 3 workers to gold, 2 to lumber, a 6-step
`boomer` plan (Farm → free Hero → Barracks → workers to 8), six triggers
(supply-valve, home-guard, hero-save 35%, steady-footman pulse, doorbell at
enemy_army_seen 6, counter-punch), squad 0 turtle. Prefs declared
`playbook: standard-kingdom, focus: army`.

**Key decisions and timestamps.**
- t=27: Farm and Barracks builds collided ("ground no longer clear") — both
  abandoned. Re-issued Barracks manually at t=~35; ~20s of tempo lost.
- t=105: Keep started; t=145 done; Blacksmith and 2nd Barracks up. Economy and
  production automation (footman pulse 20s, archer pulse 45s, worker plan)
  ran cleanly — the machine trained ~1 unit per pulse without me touching it.
- t=110: worker scout died at mid, revealing their 4-unit early push. Correct
  spend of 75g.
- t=206: with 8 army I took `secure center ford`. t=212 doorbell (threshold 6)
  fired and recalled me — I overrode it, RAISED the doorbell to 10, and
  re-secured the ford. **This was the losing decision chain.**
- t=233-244: ford skirmish went badly — lost 2 archers, 2 footmen, the Hero
  AND the fresh Priestess (she spawned and walked into the fight at 88%).
  Their army was 8 and concentrated; mine arrived strung out.
- t=264: revived Hero; by t=320 I was 12 strong at 100% and full bank.
- t=330: I re-secured the ford AGAIN with 13 — and their full 11-unit army
  (which I had not seen for 90s; intel was stale, exactly what the push-gate
  numbers warn about) caught my column strung out on the march at t=333-347.
  13 army became 2 in ~15 seconds. Hero died a second time.
- t=345: home mine ran dry the same moment — no expansion trigger was armed
  (I read the `recipe:expand` form early and never sent it; that was a real
  miss). Gold income went to zero.
- t=347-378: their army entered the base; CallToArms militia (my `muster`
  trigger + manual cast) converted the workers and they died with the army —
  7 workers became 1. Keep 46% and falling, 0 gold, 2 footmen.
- t=380: surrendered. Keep was destroyed at t=386 anyway; the concession only
  shortened a decided game.

**What lost it.** Twice I marched a defend-shaped army across open ground at a
ford against a concentrated opponent with fresher intel, and twice the engine
had told me the exact risk (stale intel, doorbell recall) and I overrode it.
The opponent played one concentrated blob and timed both strikes on my march.
Secondary: never arming `expand` meant the dry mine at t=345 was terminal
instead of a bump.

## Part 2: the 2.1 scaffold as a tool

- **Folded page vs old full render:** yes, I actually read actions at loop
  cadence this round. `--doc --all` once at t=0 (~650 lines), then `--digest`
  most cycles and `--doc --prefs` when deciding — the folded page's DEFAULT
  line and EVT tail carried 90% of each decision. The full render would never
  have been re-read; the fold is what made re-reading real.
- **Acceptance NOTE lines:** none changed a decision — I never sent a bare
  `stance push` into gates, so I never earned one. The equivalent facts
  reached me instead through the doorbell trigger firing and the NOT READY
  push line's "intel ledger is EMPTY / stale" — which I read and overrode,
  to my cost. The information channel worked; the commander didn't listen.
- **Playbook:** declared `standard-kingdom` and an INVALIDATED render DID
  fire at t=28 (step 1 "ten workers" broken by supply pressure) with exits
  first — its exit 1 (Farm first) was already my plan step, which was a nice
  confirmation, and the WHY sentences on the exits are genuinely the best
  prose on the page. After the opening I stopped reading the playbook
  section; my own plans/triggers were the real spine. A mid-game re-render
  ("you are here: step 6, eyes on mid") might have argued me out of the
  fatal ford marches — I never looked.
- **Focus:** declared `army`; it expanded stance/train/recipe forms in full,
  which I used exactly twice (reading Priestess price/slots, and the stance
  domains). Cheap, mildly useful, never misleading.
- **Errors as teachers:** the build-site collision event ("nothing was
  spent") and the idle-worker-carrying diagnosis were both readable straight
  off the digest — good.
- **Still missing:** (1) a "maintain N workers on gold/lumber" policy — I
  spent ~6 polls re-sending harvest for one idle carrying worker; (2) a
  march-safety fact on `secure`/`move` links, e.g. "this anchor is 60 units
  from the squad; enemy last seen on the path 8s ago" — both my army wipes
  happened in transit, and nothing on the page prices transit; (3) the
  doorbell/`enemy_army_seen` trigger recalling a squad I had deliberately
  committed felt like fighting my own doctrine — a "suppress trigger X for
  60s" verb would say what I meant.

## Verdict
Loss by surrender at t=388 (Keep razed at t=386 regardless). Economy and
production automation were excellent; the match was lost to two strung-out
marches against a concentrated enemy with better intel discipline, plus a
never-armed expansion trigger meeting a dry mine at the worst moment.
