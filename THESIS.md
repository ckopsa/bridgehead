# The Equitable RTS

*A thesis on building a strategy game that a human and an AI can play as equals — written
after eight competitive rounds, one human-vs-AI duel, and a patch history driven by both.*

## The thesis

Every game ever shipped was built for one kind of player: a creature with eyes, hands, and
millisecond reflexes. When an AI plays such a game, it plays through a disguise — screen
pixels parsed, mouse clicks synthesized, or a privileged API bolted on the side. Either the
AI is handicapped by an interface built for hands, or it cheats through one built for
machines. Neither is *playing together*.

We are building the alternative: an RTS where the human and the AI have **equitable access
to the same game** — not identical capabilities, but the same decision surface, the same
vocabulary of intent, the same information rights, and victory decided at the layer where
both are genuinely peers: **judgment**.

## Why an RTS

Real-time strategy is the perfect stress test because the genre historically *conflates*
two skills: strategic judgment (build orders, timing, map control, risk) and mechanical
execution (actions per minute, micro, reaction speed). A chess engine and a human meet as
equals because chess is pure judgment; StarCraft pros beat better strategists through
better hands. If we can pull those layers apart in the most hands-dominated genre — keep
the real-time drama, remove the hands as the deciding factor — the result generalizes to
any game a human and an AI might share.

## The three asymmetries, and what we did about them

**1. Tempo.** A human acts in 200 milliseconds; a language model deliberates for seconds.
Our answer was not to slow the human or rush the AI, but to relocate fast work into the
game itself. The *doctrine layer* — standing orders executed by the engine at 1–4 Hz —
means retreat thresholds, focus-fire priorities, squad postures, foraging, and cohesion all
run at machine speed for **whichever** player set them. The slow brain (either one) makes
decisions worthy of its latency; the engine handles everything faster than thought. Eight
rounds of after-action reports confirm the effect: matches are decided by *when to commit,
what to research, which cache to contest* — decisions both players make equally well — not
by who clicked faster.

**2. Information.** The AI reads a full-map snapshot every second and never misses an
alarm; the human sees one screenful and might. Perfect recall versus continuous perception.
Our equity devices: a structured *event feed* (attacks, losses, spawns) that became the
AI's attention — and is scheduled to become the human's HUD notifications too — plus a
snapshot whose contents *define* what is knowable. The endpoint (v2) is one rule of
knowability: fog of war computed once, rendered twice.

**3. Interface.** This is the deepest one. The human speaks mouse-gestures; the AI speaks
JSON through a file bridge. Our answer is the **shared vocabulary**, built in layers:

- **The catalog** — every unit, building, ability, and item as declarative data: costs,
  stats, tech requirements, descriptions. The human's build menus and the AI's knowledge
  are *derived from the same tables*, so a new unit is automatically discoverable by both.
  When we added catapults, the AI found them by reading; the human found them as a new
  button; neither needed a patch note.
- **Orders and postures** — one set of intent primitives (move, attack, harvest, build,
  push, defend, forage, escort) that both interfaces compile to. Fairness is structural:
  the AI *cannot* act in ways the human cannot, because there is no other API.
- **Doctrine and templates** — reusable policy: "units from this barracks join squad one,
  retreat at 35%, focus the siege engines." Both players stamp it once instead of
  micromanaging forever.

The v2 program completes this: intent as a first-class game object, natural language
compiling to the same doctrine the AI writes, every unit able to answer *"why are you
doing that?"* with its chain of command — and ultimately co-command, where a human and an
AI run one faction and negotiate strategy in a language both speak natively.

## Principles we learned by playing

These were not designed in advance. They were forced by evidence — most of it written by
the AI players themselves in post-match reports.

1. **Incentives, not rules.** When matches ran long, we rejected time limits. Instead:
   upkeep taxes idle armies, mines run dry, neutral bounties escalate without cap until
   refusing battle *is* losing, and victory requires only the enemy's war-making capacity.
   Games converge to 10–20 minutes because the map demands it, not because a referee does.
2. **Preserve strategies; relocate them.** When cavalry rushes proved degenerate, we
   gated cavalry behind a tech building rather than deleting it — moved to where
   counterplay exists. Every counter has a counter; the triangle holds at every tier.
3. **The engine does what is fast; the player does what is wise.** Every anti-idle,
   auto-rally, cohesion, and recovery system followed this line. Where the line sits *is*
   the game design.
4. **Content that never gets used is a bug.** Our acceptance test for new content is that
   it appears in a winning player's after-action report.
5. **The players are the playtest lab.** LLM commanders with distinct personas filed
   honest post-mortems after every round; those reports drove over fifteen design changes
   — surrender, danger-aware workers, siege mechanics, cohesion, targeting classes. A
   game whose players can *articulate* its flaws in a shared vocabulary balances itself.

## The evidence so far

An eight-round competitive series between AI commanders (and one human-vs-AI duel)
produced a documented, evolving meta: rush beat greed; fortification beat rush; siege
broke fortification; cavalry hunted siege; economy learned to hide behind insurance; and
the final rounds were won by the player who banked longest and committed *once,
completely*. Strategy emerged, was written down by the players, and shaped the next
round — exactly what a real game's community does, at machine speed.

## The wager

If this works, "playing with an AI" stops meaning *against a bot* or *carried by an
aimbot* and starts meaning what it means with a friend: shared language, negotiated
plans, complementary strengths, mutual legibility. Games are the right proving ground
because they are the one place humans and machines already meet with equal stakes and a
scoreboard. An RTS whose deciding layer is judgment — with a vocabulary both kinds of
minds speak natively — is our bet on what that future looks like.

*Status: v1 playable (one race, three eras in progress — see the issue tracker); the
shared-language v2 program is specified and sequenced. The arena is open.*
