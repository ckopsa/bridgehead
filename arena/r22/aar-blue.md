# AAR — blue (boomer, Sonnet), round r22 (rematch of r21)

**Result: Victory.** Destroyed red's TownHall and both Barracks at t=1148s. Our army finished at 25 units (16 Archer, 5 Footman, 2 Catapult, Hero Lv5, Priestess Lv4) with zero losses in the final push. Their forces were reduced to 5 stray workers and 3 farms.

## How it unfolded
- Opened with the standard boomer build (harvest, Barracks, free Hero, Farm) but this time armed `home-guard` and `hero-save` triggers immediately and queued Footmen from t=52s — a direct fix for r21's "zero standing army" failure.
- A tooling snag cost the first ~20 seconds: `bridge_send.py` overwrites `commands.json` on every call, so sending `ready` right after the opening batch silently discarded the opening batch before the engine consumed it (both were seq-adjacent writes to the same file). Resent the batch once noticed. Also found `bridge_wait.py`'s marker directory (`/tmp/claude-1000/`) didn't exist, so it kept re-announcing the same stale event every call; created the directory once and pacing worked normally afterward. Both are environment/tooling issues worth flagging, not engine bugs.
- Red repeated their r21 opening: an 8-Footman-style rush, but this time in two waves (t≈167s and t≈310s) of 4-6 Footmen each. Both were crushed by our standing squad-1 defenders with only 1-2 losses each — the exact scenario that ended r21 in our favor this time, purely because a standing army existed.
- Hero was lost once mid-game (t≈187s) after an overly aggressive solo scouting trip into red's territory — it got chased down by their rush force before it could return; had to pay the 400g/100l revival. Scouting was still worth it (confirmed red's rush composition and timing) but should have been done with an escort or a faster retreat trigger.
- Expanded to a second TownHall (t≈700s) near a fresh mine once both starting mines ran dry, doubling worker count and gold income.
- A premature full-army push at t≈429s (14 units) was badly mishandled: engaged red's 9-12 defenders piecemeal, lost the entire push squad for only ~3-4 enemy kills, and the hero/priestess never actually left home despite being nominally in the squad — likely because `priority` doesn't add units to a squad, and their earlier orders weren't cleared. Retreated, rebuilt.
- Recovered to a 24-25 unit army (16 Archer, 5 Footman, 2 Catapult, both heroes) by t≈1050s thanks to a huge gold/lumber surplus, then explicitly used `{"type":"squad",...}` to force the heroes into squad 1 before the final push — this time they participated and leveled up rapidly (Hero to Lv5, Priestess to Lv4) from the kills.
- Final push at t≈1076s met and annihilated an 11-unit red counter-attack en route, then walked into red's base with total mass and reduced their TownHall/Barracks to rubble for the win.

## Key decisions shaped by the prior AARs
- **Standing army from minute one** (direct fix for r21 blue's "zero standing army" at the exact rush timing) — this alone flipped both early rushes into our favor.
- **Armed triggers with real unit IDs at the start**, not placeholders — avoided r21's `hero-save` bug where `"units":[]` never fired meaningfully.
- **Scouted once** (hero) to confirm the rush composition, unlike r21 where blue never scouted at all — though the execution (unescorted hero deep in enemy territory) was risky and cost a revival.

## Mistakes worth noting for next time
- The premature push (t≈429s) wasted ~14 units for almost nothing — should have massed fully and confirmed hero/priestess were actually in the attacking squad before committing.
- Sent workers to a distant, contested mine near the map's southeast ford multiple times and lost several to what appeared to be enemy patrols — should have recognized the danger zone sooner and either escorted or abandoned that mine earlier.
- `bridge_send.py`'s overwrite-on-send behavior means sending two batches back-to-back without an intervening read/wait can silently drop the first one — worth remembering for any future match.

## Opponent behavior
Red repeated their r21 rush persona (Footman-heavy, fast Barracks, aggressive early timing) in two separate waves rather than one big 8-unit push, and later attempted a third combined-arms counter-attack (Archer+Footman) as our economy scaled. All three assaults failed against a standing, triggered defense. Once our economy and army outscaled theirs decisively, they had no answer and their base fell with minimal remaining defense.
