# Arena Round 17 — BLUE (Kingdom) After-Action Report

**Result: DEFEAT.** Keep destroyed at t=433s; I surrendered on the same tick the hall fell.
Match length: ~435s of game clock (~370s under my command; I took the seat at t=65s).

## How it unfolded

- **t=65-190s — the boomer opening worked.** One `plan_set` ("boomer", 8 steps) put 5 workers on
  gold/lumber, queued 4 more workers, and placed a Barracks and a Farm inside ~20 seconds of real
  time. That part of the round is a clean success for the plan vocabulary: eight commands, one poll.
- **t=138-190s — I was already behind on gold.** The Horde's mine drained ~1000 to my ~440 in the
  same window. They out-worked me early and I never closed it; I over-invested in lumber (400+ banked
  and idle) while gold-starved, and my `army` plan spent three straight cycles `blocked: cannot afford`.
- **t=187s — scouting cost me a worker and bought one true thing:** "~6 (3 Grunt, 3 Headhunter) near
  their base" at three minutes. That was an accurate warning I under-reacted to.
- **t=240-340s — MY DECISIVE ERROR.** I chained too many `bridge_wait` calls in one bash cycle and
  went ~100 game-seconds without issuing an order. I came back to **2280 unspent gold**, supply hard
  capped at 28/28 (so nothing could train even if I had ordered it), 8 army units, and 16 Horde
  units walking into my base. The game was decided in that gap, not in a fight.
- **t=343-347s — squad 1 (8 units) was wiped in four seconds** by 16. `home-guard` and `doorbell`
  fired correctly; there was simply nothing to bring home.
- **t=371-429s — collapse.** My mine went dry at the same moment (1965 gold, zero income). The
  revived Hero spawned into the enemy army and died twice for 650g total. Barracks, then Blacksmith,
  then the Keep fell to a 20-stack (9 Grunt / 8 Headhunter / 3 Shaman). CallToArms fired off a
  trigger as designed and made no difference at 20-to-0.

## What fighting the Horde was like

- **Grunts are not the surprise; HEADHUNTERS are.** The final stack was 8 Headhunters to 9 Grunts —
  their "line units are cheap and tanky" reputation hid the fact that their cheap RANGED unit (85g/20l,
  vs my Archer at 90g/30l) is what actually kills things. A Grunt wall with a Headhunter back rank is
  the same shape as Footman+Archer but arrives sooner because Grunts cost 125 to my Footman's 135
  and Impalers/Headhunters cost less lumber than Archers. **Lumber is the Kingdom's tax and the Horde
  barely pays it.** That is why I sat on 400 idle lumber and no gold while they massed.
- **They never split and never raided.** Zero harassment of my workers all game. One Peon scouted mid
  at ~170s, then nothing until a single 16-unit ball at 340s and a 20-unit ball at 429s. It was one
  timing attack, reinforced, aimed at my production. That is a very punishing pattern against a
  commander whose polling cadence has gaps.
- **Shamans arrived with the second wave (3 of them)** and I never got a fight long enough to see
  Bloodlust matter. My units died too fast to observe it.
- **The tankiness is real:** 2 stray Grunts chewed a 700 HP Barracks down to 22 HP while my 8-unit
  squad was dying elsewhere, and they survived a Tower.
- **My Spearman counter never got to exist.** Their Wolfriders are Fortress-gated, they never built
  any, and the anti-cavalry insurance I planned for was answering a question they didn't ask.

## Vocabularies used, and whether they carried weight

- **PLANS — yes, biggest win.** `boomer` (8 steps) and `army` did in one command what would have cost
  me six polls. The blocked-never-skipped semantics are exactly right: `blocked: cannot afford Footman
  (135g 0l)` told me my economy was wrong faster than any snapshot reading would have. The one sharp
  edge: a blocked step **wakes `bridge_wait` every 2 seconds**, which turned my event loop into
  spam and pushed me toward the long multi-wait batches that lost me the game.
- **TRIGGERS — armed 5, all fired correctly, none saved me.** `home-guard`, `doorbell`
  (`enemy_army_seen size 5 within 40s`), `ford-watch` (`enemy_in mid`), `hero-save`, and a `militia`
  rule casting CallToArms on `base_under_attack`. Every one fired on the right condition at the right
  second. They are reaction, not force: a rule that recalls a squad cannot recall a squad that is
  8-vs-16. `doorbell` on the intel ledger is the best predicate in the set and I should have wired it
  to *production* and *retreat*, not to a defend posture.
- **REGIONS — used lightly (`home-front`), and correctly.** `posture defend region:home-front` reads
  well in the log and re-aims from one `region_set`. I did not get the game far enough for the
  ford-holding payoff.
- **INTEL — the ledger worked exactly as documented.** `enemy army spotted: ~20 (9 Grunt, 8 Headhunter,
  3 Shaman)` from a dead scout's memory is the single most useful line the bridge printed all match.

## Top 3 complaints

1. **A blocked plan step turns `bridge_wait` into a fire hose.** "cannot afford X" is the *expected*
   state of a plan — the brief says so — yet it wakes me every ~2s with the identical error. That
   punished me for using the feature well, and it directly caused the 100-second blind window that
   lost the game. Blocked-on-affordability should be a quiet retry, or wake at most once per step.
2. **Nothing warns you that you are supply-capped or income-dead.** I hit 28/28 supply and 2280 banked
   gold and a dry mine simultaneously, and the snapshot reported all three as plain numbers among
   thirty other plain numbers. There are trigger predicates for `mine_dry` and `unit_count` but none
   for "supply blocked" or "gold above N" — the two facts that describe a commander who has stopped
   playing. Give me `supply_capped` and `gold_above` as predicates and my plan could have unstuck itself.
3. **Reviving a hero rallies it into the meat grinder.** Both my Champions died within 20 seconds of
   spawning, 650 gold, because a newly-trained unit inherits the Barracks/Keep rally and the squad
   template — which at that moment pointed at a fight. A hero should spawn *held* at its hall, or
   `train` should take a rally override. Related: `template` doctrine applied to a 400g hero is a
   footgun with no warning.

## Honest bottom line

I lost this on tempo and attention, not on the matchup. The Horde played one clean timing push with a
cheap ranged core; I answered it with a good opening, correct doctrine, and a 100-second gap in the
middle where I banked 2280 gold and built nothing. The vocabularies did their jobs. The commander did not.
