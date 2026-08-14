# After-action report — arena r42, RED (Claude, kingdom), crossings

**Result:** Win, `game_over_reason: surrender`, at game time 268s. My army (14 units, str 1567) was at (-36,-38), roughly two-thirds of the way into their half, pushing on their base when Blue conceded. No autopilot was used at any point.
Seat: bridge/red, model claude-opus-5, scaffold affordance-doc/2.2, playbook standard-kingdom (pre-declared).

## How it unfolded

- **t=0 (pre-ready):** Read `--doc --all`, sent `{"ready"}` immediately, then compiled the whole opening before the clock moved: all 5 workers to the nearest mine, Hero + 5 Workers queued at the hall, a Barracks and a Farm ordered, squad 0 stanced `turtle`, and three standing rules armed — `home-guard` (base_under_attack → turtle), `hero-save` (hero below 35% → run home), `expand` (mine_dry → TownHall at southeast mine).
- **t=35s:** first Barracks finished. Added `steady-prod` (repeat 25s, `idle barracks` → Footman) and later `archers` (repeat 30s → Archer). Those two triggers produced essentially my entire army without me spending a poll on it.
- **t=57s:** a Barracks build was abandoned — "ground was no longer clear when the worker arrived; nothing was spent." Re-issued at t=68s and it went up. That event line is the reason I noticed at all.
- **t=94–160s:** economy wobble. I over-committed workers to trees and gold income fell from ~650 to 468/min; the RUNWAY line is what caught it, and I pulled them back to the mine (income recovered to 1090/min by t=209). Later I had the opposite problem — lumber 190 vs the 205 a TownHall needs — and split three workers back onto trees. A `supply_capped` → build Farm trigger handled housing after that.
- **t=193s — the decisive read.** A lone worker I had sent to scout their base died there, but it bought the intel: "enemy army spotted: ~5 (2 Archer, 2 Footman, 1 Hero) near their base" while I had 8. That one line converted the match. At t=194s I flipped squad 1 from `secure mid` to `push their base`, re-rallied both Barracks to their base so reinforcements walked into the push, and set `priority: Archer, Hero, Building`.
- **t=210–225s:** they met me at the center ford, 5 v 9. I lost one Footman; my hero went L2 → L3 → L4 off the fight and the squad came out at ~68% and `pressing on`. That was the whole battle of the match.
- **t=243s:** ledger showed 3 enemy production buildings (2 Barracks, 1 TownHall). I started an expansion TownHall at the northeast mine (my main mine was at 39% with ~2.3 min of life) while the push kept walking.
- **t=268s:** Blue surrendered with my 14-unit squad closing on their base.

## Opponent behavior

Blue kept its army home through the first three minutes — the only sighting before the fight was 5 units sitting at their own base at t=193. They contested the center ford once with that same 5, lost the trade, and conceded rather than defend a base assault. They never raided my base or my expansion; `home-guard` and `hero-save` never fired.

## Vocabulary

Stances (`turtle` → `stage` → `secure` → `push` on one squad), triggers as production policy (`steady-prod`, `archers`, `farms`, plus armed-but-unused `expand`/`home-guard`/`hero-save`), `template`+`rally` on both Barracks so every new unit inherited squad 1's stance and walked to the front, and selectors (`my hall`, `idle barracks`, `all army`, `nearest mine`/`nearest tree`) almost everywhere instead of ids. Essentially no per-unit micro.

## Did the instrumentation change decisions?

Yes, three times. The **RUNWAY** line's income figure exposed the tree/gold misallocation at t=118 and the mine's 2.2-minute remaining life drove the expansion at t=243. The **build-abandoned events** caught both a blocked Barracks (t=56) and an unaffordable Farm (t=245) that would otherwise have been silent holes. The **recipes** in the affordance doc were the literal source of my three armed triggers — sent close to verbatim. The `train Priestess` refusal ("hero slots full (1/1 at tier 1) — upgrade a hall for another") was the one error that taught something new.

## Honest weakness

Gold- and lumber-starved for most of the midgame (`commit > income` on nearly every RUNWAY line) and never reached tier 2. Against an opponent who defended, that push might have stalled.
