# After-action report — arena r41, RED (Claude/kingdom), crossings

**Result: loss by my own surrender at t=1270s**, with 0 army, 2 workers, a dry mine and +20 gold/min against an intact opponent. No autopilot was used at any point.
Seat: bridge/red, model claude-opus-5, scaffold affordance-doc/2.2, NO playbook (ki4i pair, bookless half).

**How I structured the game.** I leaned almost entirely on the *standing-policy* vocabulary rather than per-cycle micro: squads + `stance` for doctrine, and `trigger_set` as a production engine (`workers`, `army`, `army2` as `game_time`-pulse producers; `idlefix` to re-task idle workers; `home-guard`, `hero-save`, `supply`, `expand` as contingencies). I declared no playbook. The intent was a low-cadence, policy-driven game where the engine kept buildings busy between my polls. That part worked — the trigger log shows continuous production without my transcribing it.

**How it unfolded.**
- t=0: readied immediately after the `--doc --all` read. Opening batch queued the free Hero, a Barracks and a Farm, and armed home-guard/hero-save.
- **First mistake, t=0–26: I did not order harvesting in my opening batch.** Five workers stood idle for ~26 seconds and both buildings were refused for blocked sites because I omitted `"site":"nearest legal site"`. The engine even raised an `income collapse` alarm before I noticed. Fixed at t=27.
- t=27–200: economy recovered strongly (+1030/min peak, 11 workers), Barracks up, footman/archer mix flowing via triggers.
- **t=245: Blue attacked my base with 9 units at the exact moment I had moved squad 0 out to `secure` the southeast ford.** Home-guard pulled them back and I held, killing the raid — hero back to full, army 13/str 1778 by t=315. But I lost farms and several workers.
- **t=361 decision: I read that as a won defense and counter-pushed at their base** (cleared home-guard, `push` on their base). It was wrong. The doc warned me — the push gate reported "Hero 11%" — my hero was already hurt from the defense and hero-save yanked it home mid-march. Squad 0 met their force near the center ford, lost the archers, and the hero died. From t=417 I was down to 5 footmen and 6 workers.
- t=470–580: rebuilt workers, but my harvest orders had quietly drifted — lumber sat at 210 and gold income read 190/min with 9 workers until a blanket `harvest workers → nearest mine` at t=578 restored 1030/min. That lost window is what killed me.
- **t=630: the home mine went dry with 442 gold banked.** My `expand` trigger fired but no hall went up, and my explicit `build TownHall` at northeast mine never started either — by the time I re-sent it at t=704 I had 42 gold and could never again reach the 385 needed. Income was permanently 20/min.
- t=704 onward: with no path to an expansion I threw everything at their base. The army died at their base around t=1122; they finished my workers. Zero army, two workers, no gold.
- t=1270: surrendered rather than let a decided position run.

**Opponent behavior.** Blue played a clean two-Barracks aggressive game: a ~9-unit footman/archer/hero timing attack at t=245, an intact counterattack force that beat my counter-push at the fords at ~t=400, and follow-up pressure on my workers. I only ever saw their base once (2 Barracks + 1 TownHall). They never let me stabilize a second mine.

**Key lessons, honestly.** (1) Harvest belongs in the first batch — the brief says so explicitly and I skipped it. (2) `site: "nearest legal site"` is not the default unless you send it. (3) The affordance doc *told me* the push gate was unmet at t=417 and I pushed anyway. (4) The single biggest error was strategic, not tactical: I never took a second mine while I had 442 gold and a 1030/min economy — I spent it on units. On crossings, the expansion is the game.
