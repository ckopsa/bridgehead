#!/usr/bin/env python3
"""Compile a natural-language directive into a batch of wc3clone Intent objects.

    intent_compile.py --seat bridge/red "hold the west, forage mid with cavalry"

WHAT THIS IS
------------
The game speaks exactly one language: `shared::Intent`, 29 verbs, documented in
docs/INTENT.md. A human's mouse compiles to it; a bridge commander's JSON *is*
it. This tool adds a third spelling of the same language — English — and it is
a TOOL, not an engine feature. Nothing here is natural-language processing.
It is a lookup table from the idioms that already appear in COMMANDER_BRIEF.md
and eight rounds of after-action reports to the verbs those idioms always meant.

The point is the shared vocabulary, not the parsing. A directive compiles to
Intent VALUES, never to JSON strings that hope to parse — and the game answers
back in `bridge/intent_log.jsonl` with one English sentence per intent, from
`Intent::sentence()`. That round trip is the confirmation dialogue:

    you:   "forage mid with the cavalry"
    tool:  {"type":"squad",...} {"type":"posture","id":2,...}
    game:  [ 91.6s] Claude/bridge: 3 units join squad 2
           [ 91.6s] Claude/bridge: squad 2 forages, mustering at (0.0, 0.0)

If the sentence is not what you meant, the compile was wrong, and you can see
that before the army moves.

TWO LAYERS, AND WHY
-------------------
1. A DETERMINISTIC PATTERN LAYER (below). Testable, reviewable, and boring on
   purpose: the same directive always produces the same intents, so a commander
   can rely on a phrase the way it relies on a hotkey.
2. AN LLM ESCAPE HATCH. Whatever layer 1 misses, a language model fills — and
   `--explain` prints the whole vocabulary with examples so it can self-serve.
   THIS FILE IS THE PROMPT: point a model at `intent_compile.py --explain`,
   give it the snapshot, and it writes the intents layer 1 could not.

Deterministic first. An idiom that earns its place here stops needing a model at
all, which is the direction this should keep moving.

CONDITIONALS ARE REAL NOW
-------------------------
"when my base is attacked, squad 1 defends our base" used to be the one thing
this tool structurally could not compile: the engine had no trigger system, so
the honest answer was to compile the ACTION, mark it deferred, and print the
command to run once the commander spotted the condition in `events`.

The engine has `trigger_set` now, so that whole paragraph is obsolete and the
clause compiles. A conditional becomes one trigger the engine watches at 4 Hz
and fires for you — which is the point, because the old advice priced every
reaction at one poll cycle, and a poll cycle for a language model is ten to
fifteen seconds.

    "when my base is attacked, squad 1 defends our base"
      -> {"type":"trigger_set","name":"base-attacked",
          "when":{"type":"base_under_attack"},
          "then":{"type":"posture","id":1,"posture":{...}}}

`when`/`if`/`once`/`after`/`as soon as` arm a ONCE trigger; `whenever` and
`every time` arm a REPEATING one with a cooldown. Name it yourself with a
trailing `as <name>`, or let the tool derive a stable one from the condition.

What it still will not do is guess at a condition it does not recognise: an
unparseable `when` clause is an error naming the predicates that exist, never
a plain order that quietly runs right now. An order that fires at the wrong
moment is the failure this tool exists to prevent, and it is worse when the
commander believes they armed a rule.

SEQUENCES ARE REAL TOO
----------------------
`then` is now a word the engine understands, so it is a word this tool
compiles. A directive whose clauses are joined by ", then" becomes ONE
`plan_set` — a named sequence the engine walks for you, submitting each step
when its turn comes:

    "build a barracks, then when we reach tier 2, build a sanctum,
     then train 3 sorcerers"
      -> {"type":"plan_set","name":"plan-build","steps":[
           {"intent":{"type":"build",...},
            "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}},
           {"intent":{"type":"build",...}},
           {"intent":{"type":"train",...}}]}

The grammar is the English one. A bare ", then" means "as soon as that lands".
A ", then when <condition>," is the same condition vocabulary triggers use, and
it attaches to the step BEFORE it — because that is what it governs: the plan
waits there until the condition holds. A ", then after 30s," is a fixed wait.

The comma is load-bearing and that is deliberate: "focus siege then heroes" is
a focus-fire chain, not a sequence, and splitting it would silently turn one
clause into two. Say ", then" when you mean a step.

A plan is once-through and bounded at 8 steps. Repetition is a trigger's job
(`whenever`), which is the other half of the same sentence.
"""

import argparse
import json
import math
import os
import re
import sys

# ---------------------------------------------------------------------------
# The world's fixed geography (shared.rs: HUMAN_BASE, CLAUDE_BASE, MAP_HALF)
# ---------------------------------------------------------------------------

MAP_HALF = 100.0
BASES = {"Human": (-70.0, -70.0), "Claude": (70.0, 70.0)}

# Squad 0 is the engine's auto-enroll pool (doctrine.rs::default_squad_autonomy).
# Allocating it would fight the engine for the same id, so we start at 1 and
# leave the floor alone.
FIRST_ALLOCATABLE_SQUAD = 1
# A directive re-issued next turn should re-target the squad it made last turn,
# not spawn a new one every cycle. Any live squad already holding this posture
# within this many world units of the named place is treated as "that one".
SQUAD_REUSE_RADIUS = 25.0
DEFAULT_DEFEND_RADIUS = 18.0
# shared.rs: MAX_PLAN_STEPS. Checked here rather than left to the engine so a
# too-long sequence is refused before it is sent, with advice attached.
MAX_PLAN_STEPS = 8

# --- unit selectors --------------------------------------------------------
# The nouns commanders actually use, mapped to snapshot `units[].kind` values.
KIND_WORDS = {
    "cavalry": ["Raider"],
    "raiders": ["Raider"],
    "raider": ["Raider"],
    "horse": ["Raider"],
    "siege": ["Catapult"],
    "catapults": ["Catapult"],
    "catapult": ["Catapult"],
    "footmen": ["Footman"],
    "footman": ["Footman"],
    "infantry": ["Footman", "Spearman"],
    "archers": ["Archer"],
    "archer": ["Archer"],
    "spearmen": ["Spearman"],
    "spearman": ["Spearman"],
    "knights": ["Knight"],
    "knight": ["Knight"],
    "gryphons": ["GryphonRider"],
    "gryphon": ["GryphonRider"],
    "air": ["GryphonRider"],
    "flyers": ["GryphonRider"],
    # "the hero" means every hero-CLASS unit, which is now more than one:
    # hero slots climb the hall ladder, so a Keep team fields a Champion AND a
    # Priestess. Group verbs want both; the verbs that need exactly one refuse
    # rather than pick (see `resolve_one_unit`).
    "hero": ["Hero", "Priestess"],
    "heroes": ["Hero", "Priestess"],
    "champion": ["Hero"],
    "priestess": ["Priestess"],
    # The Sorcerer is a CASTER but not a hero: no hero slot, no revival, no
    # levels. Keeping it out of "the hero" is the whole reason the word matters.
    "sorcerer": ["Sorcerer"],
    "sorcerers": ["Sorcerer"],
    "casters": ["Hero", "Priestess", "Sorcerer"],
    "workers": ["Worker"],
    "worker": ["Worker"],
    "peons": ["Worker"],
}
ARMY_WORDS = {"army", "everything", "everyone", "all", "all units", "the army", "troops"}
WORKER_KIND = "Worker"

# --- focus-fire classes ----------------------------------------------------
# Valid `priority` classes are fixed by shared.rs::TargetClass.
CLASS_WORDS = {
    "hero": "Hero",
    "heroes": "Hero",
    "archer": "Archer",
    "archers": "Archer",
    "footman": "Footman",
    "footmen": "Footman",
    "worker": "Worker",
    "workers": "Worker",
    "building": "Building",
    "buildings": "Building",
    "siege": "Siege",
    "catapult": "Siege",
    "catapults": "Siege",
    "cavalry": "Cavalry",
    "raider": "Cavalry",
    "raiders": "Cavalry",
}

# --- production ------------------------------------------------------------
# Which building trains what. `catalog.json` is authoritative and the tool
# reads it whenever the seat has one (see `Snapshot.trains`); this table is the
# offline fallback, and it is a fallback precisely because it goes stale — it
# claimed the Raider trained at the Workshop for exactly as long as that was
# true, and nothing here noticed when it moved to the Barracks.
FALLBACK_TRAINS = {
    "TownHall": ["Worker", "Hero", "Priestess"],
    "Keep": ["Worker", "Hero", "Priestess"],
    "Castle": ["Worker", "Hero", "Priestess"],
    "Barracks": ["Footman", "Archer", "Raider", "Spearman", "Knight"],
    "Workshop": ["Catapult", "GryphonRider"],
    "Sanctum": ["Sorcerer"],
}
UNIT_WORDS = {
    "worker": "Worker", "workers": "Worker", "peon": "Worker", "peons": "Worker",
    "footman": "Footman", "footmen": "Footman",
    "archer": "Archer", "archers": "Archer",
    "spearman": "Spearman", "spearmen": "Spearman",
    "knight": "Knight", "knights": "Knight",
    "catapult": "Catapult", "catapults": "Catapult",
    "raider": "Raider", "raiders": "Raider", "cavalry": "Raider",
    "gryphon": "GryphonRider", "gryphons": "GryphonRider",
    "gryphon rider": "GryphonRider", "gryphon riders": "GryphonRider",
    "hero": "Hero", "champion": "Hero", "priestess": "Priestess",
    "sorcerer": "Sorcerer", "sorcerers": "Sorcerer",
}

# The kinds that occupy a hero slot (shared.rs::is_hero_kind). The Sorcerer is
# deliberately absent: it casts, but it is not a hero.
HERO_KINDS = ("Hero", "Priestess")
# What a commander calls each hero class when disambiguating "the hero".
HERO_CLASS_WORD = {"Hero": "the champion", "Priestess": "the priestess"}
BUILDING_WORDS = {
    "farm": "Farm", "farms": "Farm",
    "barracks": "Barracks",
    "tower": "Tower", "towers": "Tower",
    "wall": "Wall", "walls": "Wall",
    "shop": "Shop",
    "sanctum": "Sanctum", "arcane sanctum": "Sanctum",
    "blacksmith": "Blacksmith", "forge": "Blacksmith",
    "workshop": "Workshop",
    "town hall": "TownHall", "townhall": "TownHall", "hall": "TownHall",
    "expansion": "TownHall",
}
RESEARCH_WORDS = {
    "attack": "attack", "damage": "attack", "weapons": "attack",
    "armor": "armor", "armour": "armor", "defense": "armor", "defence": "armor",
}
HALL_KINDS = ("TownHall", "Keep", "Castle")


# ---------------------------------------------------------------------------
# Snapshot
# ---------------------------------------------------------------------------


class Snapshot:
    """A read-only view of `state.json`, with the lookups the rules need.

    Everything here obeys fog: `units` and `bounties` hold only what this seat
    can see, and enemy buildings may be remembered ghosts. The compiler never
    works around that — a directive that names something the seat cannot see
    fails to resolve, which is the same answer the game would give.
    """

    def __init__(self, data, catalog=None):
        self.catalog = catalog or {}
        self.data = data or {}
        self.my_team = self.data.get("my_team", "Claude")
        self.units = self.data.get("units", [])
        self.buildings = self.data.get("buildings", [])
        self.squads = self.data.get("squads", [])
        self.mines = self.data.get("mines", [])
        self.bounties = self.data.get("bounties", [])
        self.map = self.data.get("map", {}) or {}
        self.chokes = self.map.get("chokes", []) or []

    @classmethod
    def load(cls, path, catalog=None):
        """Load a snapshot, and the seat's `catalog.json` beside it if present.

        The catalog is the game's own declaration of what exists — since
        bead/polish it carries each unit's `trained_at` and `class`
        transitively, i.e. it IS the tech tree. Reading it means new content is
        discoverable by this tool the same way it is discoverable by a
        commander: by reading, not by patching a table in here.
        """
        data = {}
        if path is not None:
            with open(path) as f:
                data = json.load(f)
            if catalog is None:
                beside = os.path.join(os.path.dirname(path) or ".", "catalog.json")
                catalog = beside if os.path.exists(beside) else None
        cat = None
        if catalog:
            try:
                with open(catalog) as f:
                    cat = json.load(f)
            except Exception:
                cat = None
        return cls(data, cat)

    @property
    def trains(self):
        """`{building kind: [unit ids]}` — from the catalog when we have one."""
        units = self.catalog.get("units") or []
        if not units:
            return FALLBACK_TRAINS
        table = {}
        for entry in units:
            trainer = entry.get("trained_at")
            if trainer:
                table.setdefault(trainer, []).append(entry["id"])
        # The hall ladder trains one roster; the catalog names only the lowest
        # rung, so an upgraded hall would otherwise train nothing.
        for rung in ("Keep", "Castle"):
            table.setdefault(rung, list(table.get("TownHall", [])))
        return table

    @property
    def target_classes(self):
        """Valid `priority` classes, from the catalog's `class` field."""
        found = {e.get("class") for e in (self.catalog.get("units") or [])}
        found.discard(None)
        return found or set(CLASS_WORDS.values())

    def heroes(self):
        """Living hero-CLASS units of ours. More than one, now that hero slots
        climb the hall ladder."""
        return [u for u in self.own_units() if u.get("kind") in HERO_KINDS]

    @property
    def enemy_team(self):
        return "Human" if self.my_team == "Claude" else "Claude"

    def my_base(self):
        halls = [b for b in self.own_buildings() if b.get("kind") in HALL_KINDS]
        if halls:
            home = BASES.get(self.my_team, (0.0, 0.0))
            halls.sort(key=lambda b: dist(tuple(b["pos"]), home))
            return tuple(halls[0]["pos"])
        return BASES.get(self.my_team, (0.0, 0.0))

    def their_base(self):
        halls = [b for b in self.enemy_buildings() if b.get("kind") in HALL_KINDS]
        if halls:
            away = BASES.get(self.enemy_team, (0.0, 0.0))
            halls.sort(key=lambda b: dist(tuple(b["pos"]), away))
            return tuple(halls[0]["pos"])
        return BASES.get(self.enemy_team, (0.0, 0.0))

    def own_units(self):
        return [u for u in self.units if u.get("team") == self.my_team]

    def own_buildings(self):
        return [b for b in self.buildings if b.get("team") == self.my_team]

    def enemy_buildings(self):
        return [b for b in self.buildings if b.get("team") != self.my_team]

    def finished(self, *kinds):
        return [
            b
            for b in self.own_buildings()
            if b.get("done") and (not kinds or b.get("kind") in kinds)
        ]

    def squad(self, sid):
        for s in self.squads:
            if s.get("id") == sid:
                return s
        return None


def dist(a, b):
    return math.hypot(a[0] - b[0], a[1] - b[1])


def parse_posture(text):
    """`"defend@(-6.0,-34.0)r=18"` -> `("defend", (-6.0, -34.0))`."""
    if not text:
        return None
    m = re.match(r"(\w+)@\(([-\d.]+),\s*([-\d.]+)\)", text)
    if m:
        return m.group(1), (float(m.group(2)), float(m.group(3)))
    m = re.match(r"escort:(\d+)", text)
    if m:
        return "escort", None
    return None


def clamp(pos):
    lim = MAP_HALF - 2.0
    return (max(-lim, min(lim, pos[0])), max(-lim, min(lim, pos[1])))


# ---------------------------------------------------------------------------
# Place resolution — the named geography a commander actually speaks
# ---------------------------------------------------------------------------

MID_WORDS = {"mid", "middle", "the middle", "centre", "center", "the centre",
             "the center", "midfield", "the midfield", "the mid"}
# The map's own compass, read off terrain.rs rather than assumed: the bases sit
# on the SW->NE diagonal (Human at (-70,-70) is the SOUTHWEST corner) and the
# "crossings" fords are named against it — "northwest ford" is at (-60, +60).
# So west is -x, north is +z, and the diagonals follow.
COMPASS = {
    "west": (-65.0, 0.0), "east": (65.0, 0.0),
    "north": (0.0, 65.0), "south": (0.0, -65.0),
    "northwest": (-60.0, 60.0), "northeast": (60.0, 60.0),
    "southwest": (-60.0, -60.0), "southeast": (60.0, -60.0),
}
# Words that mean "I am naming a gap in the terrain, not a direction".
CHOKE_NOUNS = {"ford", "fords", "choke", "chokepoint", "pass", "crossing",
               "crossings", "gap", "bridge"}
NOISE = {"the", "a", "an", "at", "on", "in", "our", "my", "their", "his", "her",
         "its", "of", "to"}


def _words(text):
    return [w for w in re.split(r"[^a-z0-9.+-]+", text.lower()) if w]


LITERAL_COORDS = re.compile(r"^\s*(?:at\s+)?\(?\s*-?\d+(?:\.\d+)?\s*[, ]\s*"
                            r"-?\d+(?:\.\d+)?\s*\)?\s*$")


def is_literal_coords(text):
    """Did the commander name exact ground, or an approximate landmark?

    It decides whether a build site may be nudged. "our base" is an anchor and
    moving twelve units off it is helpful; "(-40, 20)" is a decision, and
    quietly relocating it would be the tool overruling the commander.
    """
    return bool(text and LITERAL_COORDS.match(text.strip().lower()))


def resolve_place(text, snap):
    """A phrase -> `(x, z)`, or `None` if this seat cannot name that ground.

    Ordered from most specific to most general, and chokes deliberately beat
    the compass: on a map with fords, "hold the west" means the west ford, not
    an arbitrary point on the west side. On a map without them the compass
    anchor is the honest fallback.
    """
    if not text:
        return None
    raw = text.strip().lower().strip(".")

    # 1. Explicit coordinates, in every spelling a commander might type.
    m = re.search(r"\(?\s*(-?\d+(?:\.\d+)?)\s*[, ]\s*(-?\d+(?:\.\d+)?)\s*\)?\s*$", raw)
    if m and not re.search(r"[a-z]", raw[: m.start()].replace("at", "").strip()):
        return clamp((float(m.group(1)), float(m.group(2))))

    tokens = [w for w in _words(raw) if w not in NOISE]
    joined = " ".join(tokens)

    # 2. The contested middle — where the bounties spawn.
    if raw.strip() in MID_WORDS or joined in {"mid", "middle", "centre", "center", "midfield"}:
        return (0.0, 0.0)

    # 3/4. The two bases. "Their" wins over "our" only because the words differ.
    if any(w in tokens for w in ("enemy", "them", "theirs")) or "their" in raw:
        if any(w in tokens for w in ("base", "main", "hall", "townhall", "home")):
            return snap.their_base()
    if any(w in tokens for w in ("base", "home", "main")) and "enemy" not in tokens:
        if "their" in raw or "enemy" in raw:
            return snap.their_base()
        return snap.my_base()

    # 5. Resource nodes and treasure, BEFORE chokes: "the north mine" names a
    #    mine, and a fuzzy choke match on "north" would happily steal it.
    if "mine" in tokens or "mines" in tokens:
        target = pick_mine(tokens, snap)
        if target is not None:
            return tuple(target["pos"])
    if any(w in tokens for w in ("bounty", "bounties", "cache", "caches", "treasure")):
        if snap.bounties:
            me = snap.my_base()
            return tuple(min(snap.bounties, key=lambda b: dist(tuple(b["pos"]), me))["pos"])
        return (0.0, 0.0)  # none visible: hold the ground they spawn on
    if "expansion" in tokens:
        halls = [b for b in snap.own_buildings() if b.get("kind") in HALL_KINDS]
        home = BASES.get(snap.my_team, (0.0, 0.0))
        if len(halls) > 1:
            return tuple(max(halls, key=lambda b: dist(tuple(b["pos"]), home))["pos"])

    # 6. Named chokes. Requires either an exact word from the choke's name or
    #    an explicit choke noun ("the west ford"), so a bare direction stays a
    #    direction: "hold the west" is the west side, "hold the west ford" is
    #    the gap. Both are things a commander means, and they are not the same
    #    ground.
    named_choke = bool(set(tokens) & CHOKE_NOUNS)
    choke, score = match_choke(tokens, snap)
    if choke is not None and (named_choke or score >= 2):
        return tuple(choke["pos"])

    # 7. Compass anchors — "the west side of the map".
    for token in tokens:
        if token in COMPASS:
            return COMPASS[token]
    return None


def match_choke(tokens, snap):
    """Best-matching choke for a set of query words, plus its score.

    Scored rather than exact-matched, because the map names the gaps
    ("northwest ford") and commanders write "the west ford" or "the northwest
    crossing". An exact word is worth 2; a word contained in one (west inside
    northwest) is worth 1 — enough to pick the northwest ford over the centre
    one, not enough on its own to claim a bare direction.
    """
    best, best_score = None, 0
    for choke in snap.chokes:
        name_words = _words(choke.get("name", ""))
        score = 0
        for token in tokens:
            for word in name_words:
                if token == word:
                    score += 2
                elif len(token) > 3 and (token in word or word in token):
                    score += 1
        if score > best_score:
            best, best_score = choke, score
    return best, best_score


def pick_mine(tokens, snap):
    if not snap.mines:
        return None
    live = [m for m in snap.mines if m.get("remaining", 0) > 0] or snap.mines
    for token in tokens:
        if token in COMPASS:
            anchor = COMPASS[token]
            return min(live, key=lambda m: dist(tuple(m["pos"]), anchor))
    if "contested" in tokens or "middle" in tokens or "mid" in tokens:
        return min(live, key=lambda m: dist(tuple(m["pos"]), (0.0, 0.0)))
    if "their" in tokens or "enemy" in tokens:
        return min(live, key=lambda m: dist(tuple(m["pos"]), snap.their_base()))
    return min(live, key=lambda m: dist(tuple(m["pos"]), snap.my_base()))


# ---------------------------------------------------------------------------
# Unit selection
# ---------------------------------------------------------------------------


def resolve_units(text, snap, default="army"):
    """A phrase like "the cavalry" / "squad 2" / "everything" -> a list of ids.

    Own units only, always: the compiler validates ownership anyway
    (intent.rs), but a selector that could name an enemy unit would produce
    errors instead of an empty selection, and an empty selection is the more
    honest report.
    """
    phrase = (text or default).strip().lower()
    tokens = [w for w in _words(phrase) if w not in NOISE]
    mine = snap.own_units()

    m = re.search(r"squad\s*(\d+)", phrase)
    if m:
        sid = int(m.group(1))
        return [u["id"] for u in mine if u.get("squad") == sid]

    if phrase in ARMY_WORDS or any(w in ARMY_WORDS for w in tokens) or not tokens:
        return [u["id"] for u in mine if u.get("kind") != WORKER_KIND]

    kinds = []
    for token in tokens:
        kinds.extend(KIND_WORDS.get(token, []))
    if not kinds:
        # An unrecognised noun must not silently become "the whole army" —
        # that is how a directive moves things it never named.
        return None
    return [u["id"] for u in mine if u.get("kind") in kinds]


def resolve_one_unit(text, snap, role, result, clause, default="hero"):
    """Exactly one unit, or a refusal that says how to say it unambiguously.

    Hero slots climb the hall ladder, so a Keep team fields a Champion AND a
    Priestess and "the hero" stops naming one thing. The verbs that take a
    LIST (retreat, focus, leash, autocast, squad) are unaffected — both heroes
    is a fine answer to "autocast at 3". The verbs that take exactly ONE unit
    are not, and this is where they land.

    Picking the first, or the nearest, would be this tool guessing at the one
    place a guess is unrecoverable: an escort posture aimed at the wrong hero
    sends the army to the wrong side of the map, and it does it silently. So
    it refuses, and names the two words that resolve it. Refusing here is the
    same rule as an unresolvable place — say what you meant.
    """
    ids = resolve_units(text, snap, default=default)
    if ids is None:
        result.fail(clause, f"cannot resolve units {text!r}")
        return None
    if not ids:
        result.fail(clause, f"no unit matches {text or default!r}")
        return None
    if len(ids) == 1:
        return ids[0]
    named = {u["id"]: u for u in snap.own_units()}
    classes = [named[i]["kind"] for i in ids if i in named]
    if all(c in HERO_KINDS for c in classes) and len(set(classes)) == len(classes):
        options = " or ".join(sorted(HERO_CLASS_WORD[c] for c in classes))
        result.fail(clause, f"{text or default!r} is ambiguous — you have "
                            f"{len(ids)} heroes; say {options}")
    else:
        result.fail(clause, f"{text or default!r} names {len(ids)} units and "
                            f"{role} takes exactly one")
    return None


# ---------------------------------------------------------------------------
# Compilation context
# ---------------------------------------------------------------------------


class Result:
    def __init__(self):
        self.intents = []
        self.notes = []      # (clause, "ok", human summary)
        self.deferred = []   # (clause, why, suggested follow-up)
        self.errors = []     # (clause, reason)

    def ok(self, clause, summary):
        self.notes.append((clause, summary))

    def fail(self, clause, reason):
        self.errors.append((clause, reason))


class Ctx:
    def __init__(self, snap, result):
        self.snap = snap
        self.result = result
        self.used_squads = {s.get("id") for s in snap.squads if s.get("id") is not None}
        self.assigned = {}
        # Two `build` clauses in one directive used to pick the same default
        # site and the same nearest worker, so the second silently replaced the
        # first — one building instead of two, no error anywhere. A batch is
        # applied in order against a world that has not moved yet, so the tool
        # has to remember what it already spent.
        # Seeded with everything already standing: a default site that lands
        # on your own farm is refused by intent.rs as "site is blocked", which
        # is a correct error and a useless one — the commander said "build a
        # farm", not "build a farm exactly there". Found live: the second
        # turn's `build a workshop` picked the spot the first turn's farm had
        # taken, and the batch came back rejected.
        self.claimed_sites = [tuple(b["pos"]) for b in snap.own_buildings()]
        self.busy_workers = set()

    def claim_site(self, pos):
        """A free-enough site near `pos`, given what this batch already took."""
        step = 8.0
        for ring in range(12):
            for dx, dz in ((0, 0), (1, 0), (0, 1), (1, 1), (-1, 0), (0, -1),
                           (-1, 1), (1, -1), (-1, -1)):
                candidate = clamp((pos[0] + dx * ring * step,
                                   pos[1] + dz * ring * step))
                if all(dist(candidate, taken) >= step for taken in self.claimed_sites):
                    self.claimed_sites.append(candidate)
                    return candidate
        self.claimed_sites.append(pos)
        return pos

    def squad_for(self, key, same_job):
        """A squad id for one directive clause, stable across re-issues.

        `same_job(posture_string)` decides whether a live squad is already
        doing this clause's work. When one is, the clause re-targets it instead
        of allocating a new one — a commander that repeats a standing directive
        every cycle keeps one squad rather than shredding its army into a fresh
        one per turn.
        """
        if key in self.assigned:
            return self.assigned[key]
        for s in self.snap.squads:
            if s.get("id") is None or not same_job(s.get("posture")):
                continue
            self.assigned[key] = s["id"]
            return s["id"]
        sid = FIRST_ALLOCATABLE_SQUAD
        taken = self.used_squads | set(self.assigned.values())
        while sid in taken:
            sid += 1
        self.used_squads.add(sid)
        self.assigned[key] = sid
        return sid


def posture_clause(ctx, clause, word, place_text, who_text, extra=None, radius=None):
    """The shared body of hold / push / forage: squad, then posture.

    Two intents, because that is what the language has: membership and purpose
    are separate verbs, and the intent log reads as two sentences. Exactly what
    the human's doctrine card submits when `[I][W]` is pressed on a selection
    that is not already one squad (docs/INTENT.md).
    """
    snap = ctx.snap
    pos = resolve_place(place_text, snap)
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {place_text!r}")
        return []
    ids = resolve_units(who_text, snap)
    if ids is None:
        ctx.result.fail(clause, f"cannot resolve units {who_text!r}")
        return []

    def same_job(posture):
        parsed = parse_posture(posture)
        return (parsed is not None and parsed[0] == word
                and parsed[1] is not None
                and dist(parsed[1], pos) <= SQUAD_REUSE_RADIUS)

    sid = ctx.squad_for((word, round(pos[0]), round(pos[1])), same_job)
    out = []
    if ids:
        out.append({"type": "squad", "units": ids, "id": sid})
    posture = dict(extra or {})
    posture["type"] = word
    posture["x"], posture["z"] = round(pos[0], 1), round(pos[1], 1)
    if word == "defend":
        posture["radius"] = float(radius if radius is not None else DEFAULT_DEFEND_RADIUS)
    out.append({"type": "posture", "id": sid, "posture": posture})
    ctx.result.ok(
        clause,
        f"squad {sid} {word}s ({pos[0]:.1f}, {pos[1]:.1f}) with {len(ids)} unit(s)",
    )
    return out


# ---------------------------------------------------------------------------
# The pattern layer
# ---------------------------------------------------------------------------
#
# Each rule is (name, regex, handler). Handlers return a list of Intent dicts
# and report what they did (or why they could not) on `ctx.result`. Order
# matters: the first regex that matches a clause wins, so put the specific
# forms above the general ones.

WITH = r"(?:\s+(?:with|using)\s+(?P<who>.+?))?"
RULES = []


def rule(name, pattern):
    def wrap(fn):
        RULES.append((name, re.compile(pattern, re.I), fn))
        return fn

    return wrap


@rule("squad-posture",
      r"^squad\s*(?P<sid>\d+)\s+(?P<verb>defends?|holds?|guards?|pushes|push|"
      r"attacks?|strikes?|forages?|hunts?)\s+(?P<place>.+?)"
      r"(?:\s+(?:at|within)\s+(?:radius\s+)?(?P<radius>\d+))?$")
def _squad_posture(m, ctx, clause):
    """"squad 1 defends our base" — a posture on a squad NAMED by the commander.

    Above `hold`/`push` in the table because it is the specific form: those
    rules allocate a squad and enrol units into it, which is what you want when
    you say "hold the ford with the cavalry" and exactly what you do NOT want
    when you have already built squad 1 and are talking about it.

    It emits ONE intent, which is also what makes it the natural action half of
    a trigger: "when my base is attacked, squad 1 defends our base" defers a
    single posture rather than a membership change the commander never asked to
    postpone.
    """
    word = {"defend": "defend", "defends": "defend",
            "hold": "defend", "holds": "defend",
            "guard": "defend", "guards": "defend",
            "push": "push", "pushes": "push",
            "attack": "push", "attacks": "push",
            "strike": "push", "strikes": "push",
            "forage": "forage", "forages": "forage",
            "hunt": "forage", "hunts": "forage"}[m.group("verb").lower()]
    pos = resolve_place(m.group("place"), ctx.snap)
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {m.group('place')!r}")
        return []
    sid = int(m.group("sid"))
    posture = {"type": word, "x": round(pos[0], 1), "z": round(pos[1], 1)}
    if word == "defend":
        radius = m.group("radius")
        posture["radius"] = float(radius) if radius else float(DEFAULT_DEFEND_RADIUS)
    ctx.result.ok(clause, f"squad {sid} {word}s ({pos[0]:.1f}, {pos[1]:.1f})")
    return [{"type": "posture", "id": sid, "posture": posture}]


@rule("trigger-clear",
      r"^(?:clear|disarm|cancel|drop|remove|forget)\s+(?:the\s+)?"
      r"(?:trigger\s+(?P<name>[A-Za-z0-9][A-Za-z0-9_-]*)|"
      r"(?P<all>all\s+triggers|every\s+trigger|triggers|my\s+triggers))$")
def _trigger_clear(m, ctx, clause):
    """Disarm one rule, or the whole slate."""
    if m.group("name"):
        ctx.result.ok(clause, f"clear trigger {m.group('name')!r}")
        return [{"type": "trigger_clear", "name": m.group("name")}]
    ctx.result.ok(clause, "clear every trigger")
    return [{"type": "trigger_clear"}]


@rule("hold", r"^(?:hold|defend|guard|garrison|sit\s+on)\s+(?P<place>.+?)"
              + WITH + r"(?:\s+(?:at|within)\s+(?:radius\s+)?(?P<radius>\d+))?$")
def _hold(m, ctx, clause):
    radius = float(m.group("radius")) if m.group("radius") else None
    return posture_clause(ctx, clause, "defend", m.group("place"), m.group("who"),
                          radius=radius)


@rule("commit", r"^(?:push|attack|strike|press|commit|engage|go|all\s*in)$")
def _commit(m, ctx, clause):
    """A bare offensive verb with no object.

    "commit" has one meaning in a game won by razing production buildings, and
    a commander under time pressure types the short form. Spelling it out here
    rather than rejecting it is what lets a deferred conditional ("strike when
    their hero falls") hand back a command that actually runs.
    """
    return posture_clause(ctx, clause, "push", "their base", None)


@rule("push", r"^(?:push|attack|strike|press|hit|assault|advance\s+on|go\s+for|"
              r"break|siege|raze)\s+(?:into\s+|on\s+|at\s+)?(?P<place>.+?)" + WITH + r"$")
def _push(m, ctx, clause):
    return posture_clause(ctx, clause, "push", m.group("place"), m.group("who"))


@rule("forage", r"^(?:forage|hunt|contest)\s+"
                r"(?:the\s+)?(?:bounties\s+|caches\s+|treasure\s+)?"
                r"(?:at|around|in|on)?\s*(?P<place>.+?)" + WITH + r"$")
def _forage(m, ctx, clause):
    return posture_clause(ctx, clause, "forage", m.group("place"), m.group("who"))


@rule("escort", r"^(?:escort|bodyguard|protect|babysit)\s+(?P<who_target>.+?)" + WITH + r"$")
def _escort(m, ctx, clause):
    snap = ctx.snap
    target = resolve_one_unit(m.group("who_target"), snap, "escort",
                              ctx.result, clause)
    if target is None:
        return []
    escorts = resolve_units(m.group("who"), snap)
    if escorts is None:
        ctx.result.fail(clause, f"cannot resolve units {m.group('who')!r}")
        return []
    escorts = [i for i in escorts if i != target]
    # An escort has no ground position, so "already doing this job" means
    # "already escorting this exact unit" — the wire form is `escort:<id>`.
    sid = ctx.squad_for(("escort", target), lambda p: p == f"escort:{target}")
    out = []
    if escorts:
        out.append({"type": "squad", "units": escorts, "id": sid})
    out.append({"type": "posture", "id": sid, "posture": {"type": "escort", "unit": target}})
    ctx.result.ok(clause, f"squad {sid} escorts unit {target} with {len(escorts)} unit(s)")
    return out


@rule("squad-retask", r"^squad\s+(?P<id>\d+)\s+(?:should\s+)?"
                      r"(?:(?P<verb>holds?|defends?|pushe?s?|attacks?|forages?|strikes?)\s+)?"
                      r"(?:to|at|on|into)?\s*(?P<place>.+?)$")
def _squad_retask(m, ctx, clause):
    snap = ctx.snap
    sid = int(m.group("id"))
    pos = resolve_place(m.group("place"), snap)
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {m.group('place')!r}")
        return []
    verb = (m.group("verb") or "").lower()
    if verb.startswith(("hold", "defend")):
        word = "defend"
    elif verb.startswith("forage"):
        word = "forage"
    elif verb:
        word = "push"
    else:
        # No verb given: keep doing whatever this squad is already for, and
        # only move the objective. Re-pointing a squad is the commonest
        # mid-match adjustment and it should not silently change its job.
        existing = parse_posture((snap.squad(sid) or {}).get("posture"))
        word = existing[0] if existing and existing[0] != "escort" else "push"
    posture = {"type": word, "x": round(pos[0], 1), "z": round(pos[1], 1)}
    if word == "defend":
        posture["radius"] = DEFAULT_DEFEND_RADIUS
    ctx.result.ok(clause, f"squad {sid} re-pointed: {word} ({pos[0]:.1f}, {pos[1]:.1f})")
    return [{"type": "posture", "id": sid, "posture": posture}]


@rule("stand-down", r"^(?:stand\s+down|disband|clear)\s*(?:squad\s+(?P<id>\d+))?$")
def _stand_down(m, ctx, clause):
    sid = int(m.group("id")) if m.group("id") else 0
    ctx.result.ok(clause, f"squad {sid} stands down")
    return [{"type": "posture", "id": sid}]


@rule("retreat", r"^(?:retreat|fall\s*back|withdraw|pull\s*(?:out|back)|break\s*off|bail)"
                 r"(?:\s+(?:at|below|under|when))?\s*(?P<pct>\d+)\s*%?"
                 r"(?:\s+to\s+(?P<place>.+?))?" + WITH + r"$")
def _retreat(m, ctx, clause):
    snap = ctx.snap
    ids = resolve_units(m.group("who"), snap)
    if ids is None:
        ctx.result.fail(clause, f"cannot resolve units {m.group('who')!r}")
        return []
    if not ids:
        ctx.result.fail(clause, "no matching units to give a retreat policy")
        return []
    pos = resolve_place(m.group("place"), snap) if m.group("place") else snap.my_base()
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {m.group('place')!r}")
        return []
    frac = round(int(m.group("pct")) / 100.0, 3)
    ctx.result.ok(clause, f"{len(ids)} unit(s) fall back to "
                          f"({pos[0]:.1f}, {pos[1]:.1f}) below {frac:.0%}")
    return [{"type": "retreat", "units": ids, "below": frac,
             "x": round(pos[0], 1), "z": round(pos[1], 1)}]


@rule("focus", r"^(?:focus|target|prioriti[sz]e|kill|snipe)\s+(?P<classes>.+?)" + WITH + r"$")
def _focus(m, ctx, clause):
    snap = ctx.snap
    parts = re.split(r"\s*(?:>|then|before)\s*", m.group("classes").strip(), flags=re.I)
    known = snap.target_classes
    classes = []
    for part in parts:
        for token in _words(part):
            name = CLASS_WORDS.get(token)
            # The catalog names every class that exists; a word this tool knows
            # but the loaded build does not is not a class here.
            if name and name in known and name not in classes:
                classes.append(name)
    if not classes:
        ctx.result.fail(clause, f"no valid target class in {m.group('classes')!r}")
        return []
    ids = resolve_units(m.group("who"), snap)
    if ids is None:
        ctx.result.fail(clause, f"cannot resolve units {m.group('who')!r}")
        return []
    if not ids:
        ctx.result.fail(clause, "no matching units to give a focus order")
        return []
    ctx.result.ok(clause, f"{len(ids)} unit(s) focus {' > '.join(classes)}")
    return [{"type": "priority", "units": ids, "classes": classes}]


@rule("leash", r"^(?:leash|tether|anchor|chain)\s+(?P<who>.+?)\s+(?:to|at|on)\s+"
               r"(?P<place>.+?)(?:\s+within\s+(?P<r>\d+))?$")
def _leash(m, ctx, clause):
    snap = ctx.snap
    ids = resolve_units(m.group("who"), snap)
    if not ids:
        ctx.result.fail(clause, f"cannot resolve units {m.group('who')!r}")
        return []
    pos = resolve_place(m.group("place"), snap)
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {m.group('place')!r}")
        return []
    radius = float(m.group("r")) if m.group("r") else 20.0
    ctx.result.ok(clause, f"{len(ids)} unit(s) leashed to "
                          f"({pos[0]:.1f}, {pos[1]:.1f}) r{radius:.0f}")
    return [{"type": "leash", "units": ids, "x": round(pos[0], 1),
             "z": round(pos[1], 1), "radius": radius}]


@rule("autocast", r"^(?:auto-?cast|auto-?slam)\s*(?:at\s+)?(?P<n>\d+)\+?" + WITH + r"$")
def _autocast(m, ctx, clause):
    snap = ctx.snap
    ids = resolve_units(m.group("who"), snap, default="hero")
    if not ids:
        ctx.result.fail(clause, "no hero to give an auto-cast rule")
        return []
    n = int(m.group("n"))
    ctx.result.ok(clause, f"{len(ids)} caster(s) auto-cast at {n}+ enemies")
    return [{"type": "autocast", "units": ids, "min_enemies": n}]


@rule("buy", r"^(?:buy|purchase)\s+(?:a\s+|an\s+|the\s+)?(?P<item>.+?)"
             r"(?:\s+for\s+(?P<who>.+?))?$")
def _buy(m, ctx, clause):
    snap = ctx.snap
    shops = [b for b in snap.finished("Shop") if b.get("sells")]
    if not shops:
        ctx.result.fail(clause, "no finished Shop of yours to buy from")
        return []
    shop = shops[0]
    want = "".join(_words(m.group("item")))
    match = None
    for entry in shop.get("sells", []):
        if "".join(_words(entry.get("id", ""))) == want:
            match = entry
            break
    if match is None:
        shelf = ", ".join(e.get("id", "?") for e in shop.get("sells", []))
        ctx.result.fail(clause, f"{m.group('item')!r} is not on the shelf ({shelf})")
        return []
    if match.get("locked"):
        ctx.result.fail(clause, f"{match['id']} is locked (needs tier {match.get('tier')})")
        return []
    # `buy` gained an optional `hero`, because a team can field two of them and
    # only one inventory can take the item. With one hero the field stays
    # absent — the historical shape, and the game infers the only candidate.
    # With two, "buy a potion" is a question, not an order.
    buyer = None
    if m.group("who") or len(snap.heroes()) > 1:
        buyer = resolve_one_unit(m.group("who"), snap, "buy", ctx.result, clause)
        if buyer is None:
            return []
    intent = {"type": "buy", "shop": shop["id"], "item": match["id"]}
    if buyer is not None:
        intent["hero"] = buyer
    ctx.result.ok(clause, f"buy {match['id']} at shop {shop['id']}"
                          + (f" for hero {buyer}" if buyer else ""))
    return [intent]


@rule("use-item", r"^use\s+(?:the\s+)?(?:item\s+)?(?:in\s+)?"
                  r"(?:slot\s+)?(?P<slot>[012])(?:\s+for\s+(?P<who>.+?))?$")
def _use_item(m, ctx, clause):
    snap = ctx.snap
    holder = None
    if m.group("who") or len(snap.heroes()) > 1:
        holder = resolve_one_unit(m.group("who"), snap, "use_item", ctx.result, clause)
        if holder is None:
            return []
    intent = {"type": "use_item", "slot": int(m.group("slot"))}
    if holder is not None:
        intent["hero"] = holder
    ctx.result.ok(clause, f"use item in slot {m.group('slot')}"
                          + (f" (hero {holder})" if holder else ""))
    return [intent]


@rule("tier-up", r"^(?:tier\s*up|upgrade\s+(?:the\s+)?(?:hall|town\s*hall|keep)|"
                 r"go\s+(?:tier\s*)?(?:2|3|two|three)|t(?:2|3))$")
def _tier_up(m, ctx, clause):
    halls = [b for b in ctx.snap.finished(*HALL_KINDS)]
    if not halls:
        ctx.result.fail(clause, "no finished hall to upgrade")
        return []
    hall = max(halls, key=lambda b: b.get("tier", 1))
    ctx.result.ok(clause, f"upgrade {hall['kind']} {hall['id']} to its next tier")
    return [{"type": "upgrade", "building": hall["id"]}]


@rule("research", r"^research\s+(?P<what>[a-z]+)$")
def _research(m, ctx, clause):
    ladder = RESEARCH_WORDS.get(m.group("what").lower())
    if ladder is None:
        ctx.result.fail(clause, f"no research ladder called {m.group('what')!r}"
                                f" (attack, armor)")
        return []
    forges = ctx.snap.finished("Blacksmith")
    if not forges:
        ctx.result.fail(clause, "no finished Blacksmith")
        return []
    ctx.result.ok(clause, f"research {ladder} at Blacksmith {forges[0]['id']}")
    return [{"type": "research", "building": forges[0]["id"], "upgrade": ladder}]


def building_kind_of(word):
    """A commander's word for a building -> its snapshot `kind`."""
    word = re.sub(r"\s+", " ", word.strip().lower())
    return BUILDING_WORDS.get(word) or word.title().replace(" ", "")


@rule("template", r"^(?:the\s+)?(?P<b>barracks|workshop|town\s*hall|hall|keep|castle)\s+"
                  r"(?:units\s+)?(?:join|joins|go\s+to|report\s+to)\s+squad\s+(?P<id>\d+)$")
def _template(m, ctx, clause):
    kind = building_kind_of(m.group("b"))
    # "hall" means whichever rung of the hall ladder is standing.
    kinds = HALL_KINDS if kind == "TownHall" else (kind,)
    candidates = ctx.snap.finished(*kinds)
    if not candidates:
        ctx.result.fail(clause, f"no finished {kind}")
        return []
    sid = int(m.group("id"))
    ctx.result.ok(clause, f"{len(candidates)} {kind}(s) stamp squad {sid} on every unit")
    return [{"type": "template", "building": b["id"], "squad": sid} for b in candidates]


@rule("rally", r"^rally\s+(?:the\s+)?(?P<b>[a-z ]+?)\s+(?:to|at|on)\s+(?P<place>.+?)$")
def _rally(m, ctx, clause):
    kind = building_kind_of(m.group("b"))
    kinds = HALL_KINDS if kind == "TownHall" else (kind,)
    producers = ctx.snap.finished(*kinds)
    if not producers:
        ctx.result.fail(clause, f"no finished {kind}")
        return []
    pos = resolve_place(m.group("place"), ctx.snap)
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {m.group('place')!r}")
        return []
    ctx.result.ok(clause, f"{len(producers)} {kind}(s) rally to "
                          f"({pos[0]:.1f}, {pos[1]:.1f})")
    return [{"type": "rally", "building": b["id"],
             "x": round(pos[0], 1), "z": round(pos[1], 1)} for b in producers]


@rule("train", r"^(?:train|make|build|queue|add)\s+(?:(?P<n>\d+)\s+)?"
               r"(?:a\s+|an\s+|the\s+)?(?:more\s+)?(?P<unit>[a-z ]+?)s?$")
def _train(m, ctx, clause):
    snap = ctx.snap
    name = m.group("unit").strip().lower()
    kind = UNIT_WORDS.get(name) or UNIT_WORDS.get(name + "s")
    if kind is None:
        return None  # not a unit: let the `build` rule have this clause
    trains = snap.trains
    producers = [b for b in snap.own_buildings()
                 if b.get("done") and kind in trains.get(b.get("kind"), [])]
    if not producers:
        ctx.result.fail(clause, f"no finished building of yours trains {kind}")
        return []
    n = int(m.group("n") or 1)
    out = []
    # Spread across producers by current queue length: two barracks should
    # build in parallel, not queue seven deep behind one.
    load = {b["id"]: len(b.get("queue", [])) for b in producers}
    for _ in range(n):
        target = min(producers, key=lambda b: load[b["id"]])
        load[target["id"]] += 1
        out.append({"type": "train", "building": target["id"], "unit": kind})
    ctx.result.ok(clause, f"train {n}x {kind} across {len(producers)} building(s)")
    return out


@rule("build", r"^(?:build|place|put\s+(?:up|down)|wall)\s+(?:(?P<n>\d+)\s+)?"
               r"(?:a\s+|an\s+|the\s+)?(?P<kind>[a-z ]+?)s?"
               r"(?:\s+(?:at|on|near|by)\s+(?P<place>.+?))?$")
def _build(m, ctx, clause):
    snap = ctx.snap
    name = m.group("kind").strip().lower()
    kind = BUILDING_WORDS.get(name) or BUILDING_WORDS.get(name + "s")
    if kind is None:
        ctx.result.fail(clause, f"no building called {name!r}")
        return []
    pos = resolve_place(m.group("place"), snap) if m.group("place") else None
    if pos is None:
        # Unspecified site: beside the hall, offset toward the middle of the
        # map — the direction a human's placement ghost drifts, and the only
        # one guaranteed not to be off the edge behind the base.
        base = snap.my_base()
        pos = clamp((base[0] + (-12.0 if base[0] > 0 else 12.0),
                     base[1] + (-12.0 if base[1] > 0 else 12.0)))
    if not is_literal_coords(m.group("place")):
        pos = ctx.claim_site(pos)
    workers = [u for u in snap.own_units()
               if u.get("kind") == WORKER_KIND and u["id"] not in ctx.busy_workers]
    if not workers:
        ctx.result.fail(clause, "no free worker to build with")
        return []
    builder = min(workers, key=lambda u: dist(tuple(u["pos"]), pos))
    ctx.busy_workers.add(builder["id"])
    ctx.result.ok(clause, f"worker {builder['id']} builds {kind} at "
                          f"({pos[0]:.1f}, {pos[1]:.1f})")
    return [{"type": "build", "worker": builder["id"], "kind": kind,
             "x": round(pos[0], 1), "z": round(pos[1], 1)}]


@rule("harvest", r"^(?:harvest|mine|gather|work|chop)\s+(?P<what>gold|lumber|wood|trees?|"
                 r"the\s+mine|mines?)" + WITH + r"$")
def _harvest(m, ctx, clause):
    snap = ctx.snap
    what = m.group("what").lower()
    selected = resolve_units(m.group("who"), snap, default="workers")
    if selected is None:
        ctx.result.fail(clause, f"cannot resolve units {m.group('who')!r}")
        return []
    # Only workers can gather — intent.rs rejects anyone else with an error per
    # unit, so filtering here turns a wall of errors into an empty selection.
    chosen = set(selected)
    workers = [u["id"] for u in snap.own_units()
               if u.get("kind") == WORKER_KIND and u["id"] in chosen]
    if not workers:
        ctx.result.fail(clause, "no workers")
        return []
    if "lumber" in what or "wood" in what or "tree" in what:
        trees = snap.data.get("trees_near", [])
        if not trees:
            ctx.result.fail(clause, "no trees in `trees_near`")
            return []
        home = snap.my_base()
        node = min(trees, key=lambda t: dist(tuple(t["pos"]), home))["id"]
        label = "lumber"
    else:
        target = pick_mine([], snap)
        if target is None:
            ctx.result.fail(clause, "no gold mine in the snapshot")
            return []
        node = target["id"]
        label = "gold"
    ctx.result.ok(clause, f"{len(workers)} worker(s) harvest {label} at node {node}")
    return [{"type": "harvest", "units": workers, "target": node}]


@rule("scout", r"^(?:scout|probe|peek\s+at|look\s+at|check)\s+(?P<place>.+?)" + WITH + r"$")
def _scout(m, ctx, clause):
    snap = ctx.snap
    pos = resolve_place(m.group("place"), snap)
    if pos is None:
        ctx.result.fail(clause, f"cannot resolve place {m.group('place')!r}")
        return []
    ids = resolve_units(m.group("who"), snap) if m.group("who") else None
    if not ids:
        # Cheapest eyes on the map first (COMMANDER_BRIEF: raiders see 24).
        for preference in ("Raider", "Archer", "Footman", "Worker"):
            ids = [u["id"] for u in snap.own_units() if u.get("kind") == preference]
            if ids:
                break
        ids = ids[:1]
    if not ids:
        ctx.result.fail(clause, "no unit available to scout with")
        return []
    ctx.result.ok(clause, f"{len(ids)} unit(s) attack-move to "
                          f"({pos[0]:.1f}, {pos[1]:.1f}) to look")
    return [{"type": "attackmove", "units": ids,
             "x": round(pos[0], 1), "z": round(pos[1], 1)}]


@rule("surrender", r"^(?:surrender|concede|resign|gg|i\s+concede)$")
def _surrender(m, ctx, clause):
    ctx.result.ok(clause, "surrender the match")
    return [{"type": "surrender"}]


@rule("autopilot", r"^autopilot(?:\s+(?P<off>off|on))?$")
def _autopilot(m, ctx, clause):
    on = (m.group("off") or "on").lower() != "off"
    ctx.result.ok(clause, f"autopilot {'on' if on else 'off'}")
    return [{"type": "autopilot", "on": on}]


# ---------------------------------------------------------------------------
# Triggers: the `when` half of the language
# ---------------------------------------------------------------------------
#
# A conditional compiles to ONE `trigger_set`: the engine watches the predicate
# at 4 Hz and submits the action itself. That is the whole reason this section
# exists — the old advice ("watch `events`, then send the command") priced every
# reaction at a poll cycle, and a poll cycle for a model is ten to fifteen
# seconds.
#
# Predicates are `shared::TriggerWhen`, and the list is short on purpose: each
# one is answerable from state the engine already keeps. A condition outside it
# is DEFERRED with the old advice rather than guessed at — see `parse_when`.

# Connectors that introduce a condition, and whether they mean "keep watching".
# `whenever` / `every time` are the English for a repeating rule, and treating
# them as synonyms of `when` would silently disarm a rule the commander expects
# to keep working.
ONCE_WORDS = ("when", "if", "once", "after", "as soon as")
REPEAT_WORDS = ("whenever", "every time", "each time", "any time")
CONNECTORS = "|".join(w.replace(" ", r"\s+") for w in REPEAT_WORDS + ONCE_WORDS)

# Default cooldown for a repeating trigger, in game seconds. Long enough that a
# rule cannot re-fire inside one engagement, short enough to answer the next
# one. Override with a trailing "every 90s".
DEFAULT_REPEAT_S = 45.0

# "<action> when <cond>" — the trailing form, matched per clause.
CONDITIONAL = re.compile(rf"^(?P<action>.+?)\s+(?P<conn>{CONNECTORS})\s+"
                         r"(?P<cond>.+?)$", re.I)
# "when <cond>, <action>" — the LEADING form, matched against the whole
# directive BEFORE clause splitting, because the comma in it is the joint of one
# sentence rather than a separator between two.
LEADING_CONDITIONAL = re.compile(rf"^(?P<conn>{CONNECTORS})\s+(?P<cond>.+?)\s*,\s*"
                                 r"(?P<action>.+)$", re.I)
# An explicit name: "... as home-guard" / "... call it home-guard".
NAMED = re.compile(r"^(?P<rest>.+?)\s+(?:as|named|call\s+it|calling\s+it)\s+"
                   r"(?P<name>[A-Za-z0-9][A-Za-z0-9_-]{0,23})$", re.I)
# An explicit cooldown: "... every 90s" / "... every 2 minutes".
EVERY = re.compile(r"^(?P<rest>.+?)\s+every\s+(?P<n>\d+(?:\.\d+)?)\s*"
                   r"(?P<unit>s|sec|secs|seconds|m|min|mins|minutes)?$", re.I)


def _seconds(n, unit):
    """A duration a commander typed. Bare numbers are seconds, like every other
    time in this game (`Bounty.expires_at`, `GameEvent.t`)."""
    n = float(n)
    return n * 60.0 if unit and unit.lower().startswith("m") else n


def parse_when(text):
    """A condition phrase -> a `TriggerWhen` dict, or None if it is outside the
    predicate vocabulary.

    None is not an error: the caller DEFERS instead, because a commander whose
    condition this cannot express is better served by the old watch-the-feed
    advice than by a rule that fires on something else. The predicates are
    deliberately few — every one of them is answerable from state the engine
    already keeps, which is what keeps them cheap enough to evaluate at 4 Hz.
    """
    t = " ".join(text.strip().lower().rstrip(".").split())
    theirs = bool(re.search(r"\b(their|enemy|enemies|hostile|his|her|its)\b", t))

    # --- the base is being hit -------------------------------------------
    if (not theirs and re.search(r"\b(base|town|hall|home|expansion)\b", t)
            and re.search(r"\b(attack|attacked|attacking|raid|raided|hit|damaged|"
                          r"burning|threatened)\b", t)):
        return {"type": "base_under_attack"}

    # --- THEIR hero is down -----------------------------------------------
    # This branch is the one that used to be a deferral, and the distinction it
    # rests on is worth keeping in view. There is still no reading of an enemy
    # hero's HEALTH — "strike when their hero is below 30%" is as unanswerable
    # as it ever was, because no human can select an enemy hero and read a
    # number off it. What changed is that whether you WATCHED IT DIE is a fact
    # a human plainly has, and the intel ledger now keeps it. So the honest
    # predicate is not "their hero is hurt" but "their hero is believed dead",
    # and that is what these words compile to.
    #
    # Above the `not theirs` block so "their hero falls" cannot fall through
    # into a rule about OUR hero — which is the silent wrong order this whole
    # tool exists to avoid, and the reason the sentence was refused for so
    # long.
    if theirs and re.search(r"\b(hero(?:es)?|champion|priestess)\b", t):
        if re.search(r"\b(falls?|fell|dies?|died|dying|down|dead|killed|"
                     r"slain|drops?|goes\s+down)\b", t):
            when = {"type": "enemy_hero_down"}
            # Name the class only if they named it. "their hero falls" means
            # whichever one — a commander who says "hero" is not asking to be
            # told they should have said "champion".
            for word, kind in (("champion", "Hero"), ("priestess", "Priestess")):
                if re.search(rf"\b{word}\b", t):
                    when["class"] = kind
                    break
            return when

    # --- a hero is dying --------------------------------------------------
    # OURS. `their hero is below 40%` still reaches no branch and still defers:
    # enemy hero health is not knowable, and answering it with our own number
    # would be a rule that fires on the wrong army.
    if not theirs:
        m = re.search(r"\bhero(?:es)?\b.*?(?:below|under|at)\s*(?P<pct>\d+)\s*%", t)
        if m:
            return {"type": "hero_below",
                    "frac": round(int(m.group("pct")) / 100.0, 3)}
        if re.search(r"\bhero(?:es)?\b.*\b(falls?|dying|dies|in\s+trouble|"
                     r"nearly\s+dead|low|hurt|wounded)\b", t):
            # A named default rather than a refusal: 35% is the threshold the
            # human's own [V] Fall back preset uses, so "my hero is in trouble"
            # means the same number in both interfaces.
            return {"type": "hero_below", "frac": 0.35}

    # --- a squad is breaking ---------------------------------------------
    m = re.search(r"\bsquad\s*(?P<sid>\d+)\b.*?(?:below|under|at)\s*(?P<pct>\d+)\s*%", t)
    if m:
        return {"type": "squad_below", "id": int(m.group("sid")),
                "frac": round(int(m.group("pct")) / 100.0, 3)}

    # --- treasure ---------------------------------------------------------
    # Above the sighting branch, not below it: "a bounty APPEARS" shares its
    # verb with "cavalry appears", and the noun is the unambiguous half.
    if re.search(r"\b(bounty|bounties|cache|caches|treasure|chest)\b", t):
        return {"type": "bounty_spawned"}

    # --- a body of their troops, from the ledger --------------------------
    # Above the sighting branch because "army" is a noun the sighting branch
    # cannot resolve to a class — it would return None and defer a sentence
    # that is now perfectly expressible.
    #
    # The difference from `enemy_sighted` is memory, and it is worth stating
    # because the two English sentences look alike. `enemy_sighted` is true
    # only while eyes are ON them; `enemy_army_seen` reads the intel ledger, so
    # it stays true after the scout that found them is killed — which is
    # exactly what the scout was killed to prevent.
    if re.search(r"\b(army|armies|force|forces|host|warband|stack|group|"
                 r"doomstack|deathball)\b", t):
        m = re.search(r"(?P<n>\d+)", t)
        if m:
            when = {"type": "enemy_army_seen", "size": int(m.group("n"))}
            # "in the last 30 seconds" / "within 20s" — an optional bound on
            # how stale the observation may be.
            w = re.search(r"(?:within|in\s+the\s+last|no\s+older\s+than)\s+"
                          r"(?P<w>\d+)\s*(?P<unit>s|secs?|seconds?|m|mins?|minutes?)\b", t)
            if w:
                secs = int(w.group("w"))
                if w.group("unit").startswith("m"):
                    secs *= 60
                when["within_s"] = float(secs)
            return when

    # --- eyes on the enemy ------------------------------------------------
    rest = None
    m = re.search(r"\b(?:i|we)\s+(?:see|sight|spot|find)\s+(?P<rest>.+)$", t)
    if m:
        rest = m.group("rest")
    elif re.search(r"\b(sighted|spotted|seen|appears?|arrives?|shows?\s+up)\b", t):
        m = re.match(r"^(?P<rest>.+?)\s+(?:is|are)?\s*(?:sighted|spotted|seen|"
                     r"appears?|arrives?|shows?\s+up)\b", t)
        if m:
            rest = m.group("rest")
    if rest is not None:
        rest = rest.strip()
        count = 1
        cm = re.match(r"^(?P<n>\d+)\s*(?:or\s+more\s+)?(?P<what>.*)$", rest)
        if cm:
            count, rest = int(cm.group("n")), cm.group("what").strip()
        what = re.sub(r"^(?:their|the|a|an|any|some|enemy|hostile)\s+", "", rest).strip()
        what = re.sub(r"^(?:their|enemy|hostile)\s+", "", what).strip()
        if what in ("", "units", "unit", "them", "anything", "enemies", "enemy",
                    "troops", "army", "something"):
            return {"type": "enemy_sighted", "count": count}
        cls = CLASS_WORDS.get(what) or CLASS_WORDS.get(what.rstrip("s"))
        if cls:
            return {"type": "enemy_sighted", "class": cls, "count": count}
        return None

    # --- the gold runs out ------------------------------------------------
    if re.search(r"\bmines?\b", t) and re.search(
            r"\b(dry|dries|empty|empties|exhausted|depleted|out)\b", t):
        return {"type": "mine_dry"}

    # --- teching up -------------------------------------------------------
    reached = re.search(r"\b(reach|reached|reaches|hit|hits|get|got|gets|have|has|"
                        r"finish|finished|am|are|is)\b", t)
    m = re.search(r"\b(?:tier|t)\s*(?P<t>[123])\b", t)
    if m and reached:
        return {"type": "tier_reached", "tier": int(m.group("t"))}
    if reached and re.search(r"\bkeep\b", t):
        return {"type": "tier_reached", "tier": 2}
    if reached and re.search(r"\bcastle\b", t):
        return {"type": "tier_reached", "tier": 3}

    # --- army size --------------------------------------------------------
    m = re.search(r"\b(?:i|we)\s+(?:have|has|field|fields|reach|reaches|get|gets)\s+"
                  r"(?P<n>\d+)\s+(?P<kind>[a-z ]+)$", t)
    if m:
        word = m.group("kind").strip()
        kind = UNIT_WORDS.get(word) or UNIT_WORDS.get(word.rstrip("s"))
        if kind:
            return {"type": "unit_count", "kind": kind, "count": int(m.group("n"))}
        return None

    # --- the clock --------------------------------------------------------
    if re.search(r"\b(clock|game\s+time|minutes?|seconds?|secs?)\b", t):
        m = re.search(r"(?P<n>\d+(?:\.\d+)?)\s*(?P<unit>s|sec|secs|seconds|m|min|"
                      r"mins|minutes)?\b", t)
        if m:
            unit = m.group("unit")
            if not unit and re.search(r"\bminutes?\b", t):
                unit = "m"
            return {"type": "game_time",
                    "at": round(_seconds(m.group("n"), unit), 1)}

    return None


def name_for(when):
    """The auto-derived trigger name.

    Deterministic and short (the engine caps a name at 24 bytes), so re-issuing
    the same directive next cycle REPLACES the rule in place instead of spending
    another of the eight slots. A commander that wants its own label says
    `... as home-guard`.
    """
    kind = when["type"]
    if kind == "base_under_attack":
        return "base-attacked"
    if kind == "hero_below":
        return f"hero-{int(round(when['frac'] * 100))}"
    if kind == "squad_below":
        return f"sq{when['id']}-{int(round(when['frac'] * 100))}"
    if kind == "enemy_sighted":
        what = when.get("class", "enemy").lower()
        n = when.get("count", 1)
        return f"{what}-seen" if n <= 1 else f"{n}-{what}-seen"
    if kind == "enemy_army_seen":
        return f"army-{when.get('size', 1)}"
    if kind == "enemy_hero_down":
        # The class when they named one, so a rule about their Champion and a
        # rule about their Priestess do not overwrite each other. Spelled with
        # the CLASS word rather than the wire kind: `Hero` would auto-name to
        # `hero-down`, one character away from `hero_below`'s `hero-35`, and
        # the two are about opposite armies.
        cls = when.get("class")
        if cls:
            return {"Hero": "champion-down"}.get(cls, f"{cls.lower()}-down")
        return "their-hero-down"
    if kind == "bounty_spawned":
        return "bounty-up"
    if kind == "mine_dry":
        return "mine-dry"
    if kind == "tier_reached":
        return f"tier{when['tier']}"
    if kind == "unit_count":
        return f"{when['kind'].lower()}-{when['count']}"
    if kind == "game_time":
        return f"t{int(when['at'])}"
    return "trigger"


def compile_conditional(conn, cond_text, action_text, clause, ctx):
    """One conditional -> the intents it means.

    The action is compiled by the ORDINARY rules against the ordinary snapshot,
    so a trigger can say anything the language can say. When the action compiles
    to several intents — "hold the ford with the cavalry" is membership *and*
    purpose — the leading ones are emitted NOW and the LAST becomes the deferred
    action. That split is the honest reading of the sentence: who is in the
    squad is a fact you establish today; what the squad does when the base burns
    is the part that waits.
    """
    conn_norm = " ".join(conn.lower().split())
    repeat = DEFAULT_REPEAT_S if conn_norm in REPEAT_WORDS else None

    # Cooldown first, then the name: both are trailing modifiers, and "... as
    # home-guard every 2 minutes" puts them in that order because it is the
    # order a person says them in.
    name = None
    m = EVERY.match(action_text)
    if m:
        action_text = m.group("rest").strip()
        repeat = _seconds(m.group("n"), m.group("unit"))
    m = NAMED.match(action_text)
    if m:
        action_text, name = m.group("rest").strip(), m.group("name").strip()

    when = parse_when(cond_text)

    # Compile the action against a throwaway context first: if the condition
    # turns out to be inexpressible we must not have spent a squad id or a build
    # site on a batch we are not going to send.
    probe = Result()
    probe_ctx = Ctx(ctx.snap, probe)
    probe_ctx.used_squads = set(ctx.used_squads)
    probe_ctx.assigned = dict(ctx.assigned)
    probe_ctx.claimed_sites = list(ctx.claimed_sites)
    probe_ctx.busy_workers = set(ctx.busy_workers)
    trial = compile_clause(action_text, probe_ctx)

    if when is None:
        # The condition is outside the predicate vocabulary. The old advice is
        # still the right advice for exactly this case, so it survives here
        # rather than being deleted along with the rest of it.
        ctx.result.deferred.append((clause, cond_text.strip(),
                                    action_text if trial else None))
        return []
    if not trial:
        reason = probe.errors[0][1] if probe.errors else "no pattern matched"
        ctx.result.fail(clause, f"the condition is fine, but the action "
                                f"{action_text!r} did not compile ({reason})")
        return []
    # A trigger may not arm or disarm a trigger — the engine refuses it, and
    # emitting it anyway would mean learning that from an error channel a turn
    # later. It is the line between doctrine and a scripting language, and it
    # is what makes the cap of eight an actual bound rather than a starting
    # balance.
    if any(i["type"] in ("trigger_set", "trigger_clear") for i in trial):
        ctx.result.fail(clause, "a trigger cannot arm or clear another trigger "
                                "— triggers are doctrine, not a scripting language")
        return []

    # The action really is being compiled now, so its claims are real claims.
    ctx.used_squads = probe_ctx.used_squads
    ctx.assigned = probe_ctx.assigned
    ctx.claimed_sites = probe_ctx.claimed_sites
    ctx.busy_workers = probe_ctx.busy_workers

    setup, action = trial[:-1], trial[-1]
    name = name or name_for(when)
    trigger = {"type": "trigger_set", "name": name, "when": when, "then": action}
    if repeat is not None:
        trigger["repeat"] = repeat
    cadence = f"repeating every {repeat:g}s" if repeat is not None else "once"
    ctx.result.ok(clause,
                  f"trigger {name!r} ({cadence}): when {when['type']} -> {action['type']}"
                  + (f", plus {len(setup)} intent(s) sent now" if setup else ""))
    return setup + [trigger]


# ---------------------------------------------------------------------------
# Sequences: "X, then Y, then Z" -> one plan
# ---------------------------------------------------------------------------
#
# One `plan_set`, because the engine walks a plan itself: the whole sequence is
# handed over once and each step is submitted when its turn comes. The old
# alternative was to send step 1 and remember to send step 2 next cycle, which
# for a language model prices a five-step build order at five polls of nothing
# but transcription.

# The joint. A COMMA (or semicolon) is required before `then`, and that is the
# whole disambiguation: "focus siege then heroes" is a focus-fire chain that
# lives inside ONE clause, and treating its `then` as a step boundary would
# turn one correct order into two wrong ones. A commander who means a step
# types the comma, which is what they were going to type anyway.
PLAN_JOINT = re.compile(r"[,;]\s*then\s+", re.I)

# "after 30s, <action>" — the fixed-wait step introducer, matched on a part
# after the joint has split it off.
AFTER_STEP = re.compile(r"^after\s+(?P<n>\d+(?:\.\d+)?)\s*"
                        r"(?P<unit>s|sec|secs|seconds|m|min|mins|minutes)?\s*,\s*"
                        r"(?P<action>.+)$", re.I)
# "when <cond>, <action>" — the same leading-conditional shape the trigger
# layer uses, re-read here as an ADVANCE condition rather than as a trigger.
WHEN_STEP = re.compile(rf"^(?:{CONNECTORS})\s+(?P<cond>.+?)\s*,\s*(?P<action>.+)$", re.I)


def plan_name_for(steps):
    """The auto-derived plan name.

    Deterministic and short, for the reason `name_for` is: re-issuing the same
    directive next cycle must REPLACE the plan rather than spend the other of
    the two slots. Named after what the sequence starts by doing, which is how
    a commander refers to it anyway ("my build order", "the push").
    """
    if not steps:
        return "plan"
    return f"plan-{steps[0]['intent']['type'].replace('_', '-')}"[:24]


def compile_plan(parts, directive, ctx):
    """A ", then"-joined directive -> one `plan_set`.

    Each part compiles through the ORDINARY rules, so a plan step can say
    anything the language can say. A part that compiles to several intents (a
    `hold` is membership AND purpose) contributes them as consecutive steps,
    because that is what it means — and it is honest about the step budget it
    just spent.

    The condition on a part attaches to the PREVIOUS step's `advance`: "X, then
    when we reach tier 2, Y" means the plan sits on X until tier 2. That is the
    only reading of the sentence, and getting it backwards would make a plan
    wait for a condition after it had already acted on it.
    """
    name = None
    steps = []
    for i, raw in enumerate(parts):
        part = raw.strip().rstrip(".")
        advance = None
        m = AFTER_STEP.match(part)
        if m:
            advance = {"type": "after",
                       "secs": round(_seconds(m.group("n"), m.group("unit")), 1)}
            part = m.group("action").strip()
        else:
            m = WHEN_STEP.match(part)
            if m:
                when = parse_when(m.group("cond"))
                if when is None:
                    ctx.result.fail(directive,
                                    f"step {i + 1}: {m.group('cond').strip()!r} is not a "
                                    f"condition the engine can watch — see --explain "
                                    f"for the list")
                    return []
                advance = {"type": "when", "when": when}
                part = m.group("action").strip()
        # A trailing "as <name>" on the LAST part names the whole plan, the
        # same modifier a trigger takes and in the same position.
        nm = NAMED.match(part)
        if nm:
            part, name = nm.group("rest").strip(), nm.group("name").strip()

        if advance is not None:
            if not steps:
                ctx.result.fail(directive,
                                "a plan cannot open with a condition — say "
                                "\"when <cond>, <action>\" for a trigger, or put the "
                                "condition on a later step")
                return []
            # It governs the step BEFORE it: the plan waits there.
            steps[-1]["advance"] = advance

        out = compile_clause(part, ctx)
        if out is None:
            ctx.result.fail(directive, f"step {i + 1}: {part!r} did not compile — "
                                       f"see --explain")
            return []
        # A plan may not set a plan; the engine refuses it and learning that
        # from an error channel a turn later is exactly the round trip this
        # tool exists to save.
        if any(x["type"] in ("plan_set", "plan_clear") for x in out):
            ctx.result.fail(directive, "a plan step cannot set or clear a plan — "
                                       "plans are doctrine, not a scripting language")
            return []
        steps.extend({"intent": x} for x in out)

    if not steps:
        return []
    if len(steps) > MAX_PLAN_STEPS:
        ctx.result.fail(directive,
                        f"that is {len(steps)} steps and the engine takes "
                        f"{MAX_PLAN_STEPS} (some clauses cost two — \"hold X with Y\" "
                        f"is membership and purpose). Split it into two plans")
        return []

    name = name or plan_name_for(steps)
    ctx.result.ok(directive,
                  f"plan {name!r}: {len(steps)} steps, "
                  + " then ".join(s["intent"]["type"] for s in steps))
    return [{"type": "plan_set", "name": name, "steps": steps}]


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def split_clauses(text):
    """One directive -> its clauses.

    Commas, semicolons and newlines separate; " and " deliberately does not,
    because it appears inside single clauses ("hold the ford and the mine")
    far more often than between them. For a focus-fire chain use `>`:
    "focus siege > heroes".

    Two commas are NOT separators, both found by writing directives rather than
    by imagining them: the one inside a bracketed coordinate, `(-40, 20)`, and
    the one between two bare numbers, `-40, 20`. Splitting either produced a
    build order at a completely different place plus an unparseable fragment —
    a wrong order that still looked like a successful compile, which is the
    worst failure this tool can have.
    """
    clauses, current, depth = [], [], 0
    for i, ch in enumerate(text):
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth = max(0, depth - 1)
        if ch in ",;\n" and depth == 0 and not _between_numbers(text, i):
            clauses.append("".join(current))
            current = []
            continue
        current.append(ch)
    clauses.append("".join(current))
    return [c.strip() for c in clauses if c.strip()]


def _between_numbers(text, i):
    """Is `text[i]` a comma sitting between two numbers, as in `-40, 20`?"""
    before = text[:i].rstrip()
    after = text[i + 1:].lstrip()
    return bool(re.search(r"\d$", before) and re.match(r"-?\d", after))


def compile_clause(clause, ctx):
    text = clause.strip().rstrip(".")
    for name, pattern, handler in RULES:
        m = pattern.match(text)
        if not m:
            continue
        out = handler(m, ctx, clause)
        if out is None:
            continue  # the handler declined; keep looking
        return out
    return None


def compile_directives(directives, snap):
    """Compile every directive into one batch of Intent values."""
    result = Result()
    ctx = Ctx(snap, result)
    for directive in directives:
        # A ", then" chain is matched against the WHOLE directive, ahead of
        # everything else, for the same reason the leading conditional is: its
        # commas are the joints of one sentence rather than separators between
        # independent orders, and `split_clauses` would shred it.
        chain = PLAN_JOINT.split(directive.strip().rstrip("."))
        if len(chain) > 1:
            result.intents.extend(compile_plan(chain, directive.strip(), ctx))
            continue
        # The LEADING conditional is matched against the whole directive, before
        # `split_clauses` ever sees it. "when my base is attacked, squad 1
        # defends our base" has exactly one comma and it is the joint of one
        # sentence — splitting there produced a dangling "when ..." fragment and
        # an order that ran immediately, which is the worst of both.
        lead = LEADING_CONDITIONAL.match(directive.strip().rstrip("."))
        if lead:
            result.intents.extend(compile_conditional(
                lead.group("conn"), lead.group("cond"), lead.group("action").strip(),
                directive.strip(), ctx))
            continue
        for clause in split_clauses(directive):
            cond = CONDITIONAL.match(clause.strip().rstrip("."))
            if cond:
                result.intents.extend(compile_conditional(
                    cond.group("conn"), cond.group("cond"),
                    cond.group("action").strip(), clause, ctx))
                continue
            out = compile_clause(clause, ctx)
            if out is None:
                result.fail(clause, "no pattern matched — see --explain, "
                                    "or write the intents directly")
            else:
                result.intents.extend(out)
    return result


# ---------------------------------------------------------------------------
# --explain: the vocabulary, for a commander or a model
# ---------------------------------------------------------------------------

EXPLAIN = """\
intent_compile.py — English -> wc3clone Intent objects

USAGE
  intent_compile.py --seat bridge/red "hold the west, forage mid with cavalry"
  intent_compile.py --seat bridge/red --send "push their base"
  intent_compile.py --state state.json --json "retreat at 35%"   # array on stdout

  --seat DIR    read DIR/state.json (and, with --send, write DIR/commands.json)
  --state FILE  read a snapshot directly
  --send        write the batch to the seat with the next seq, like bridge_send.py
  --out FILE    write {"seq":N,"commands":[...]} to FILE
  --json        print only the JSON array (for piping)
  --explain     print this

WHAT THE PATTERN LAYER UNDERSTANDS
  Standing doctrine (these are what win matches — they act between your turns):
    hold <place> [with <units>] [within <r>]   -> squad + posture defend
    push <place> [with <units>]                -> squad + posture push
       (also: attack / strike / press / hit / assault / advance on / raze)
    forage <place> [with <units>]              -> squad + posture forage
    escort <unit> [with <units>]               -> squad + posture escort
    squad <n> <place>                          -> re-point squad n, keeping its job
    squad <n> holds|pushes|forages <place>     -> re-point and change its job
    stand down [squad <n>]                     -> clear a posture
    retreat at <p>% [to <place>] [with <units>]-> retreat policy
    focus <class> [> <class> ...]              -> focus-fire priority
    leash <units> to <place> [within <r>]      -> leash policy
    autocast at <n>                            -> hero auto-cast rule
    <building> units join squad <n>            -> doctrine template
    rally <building> to <place>                -> rally point for new units

  Economy and production:
    harvest gold|lumber [with <units>]         -> harvest
    build <kind> [at <place>]                  -> build (nearest worker)
    train [n] <unit>                           -> train, spread across producers
    tier up                                    -> upgrade your top hall
    research attack|armor                      -> research at a Blacksmith
    buy <item> [for the champion|priestess]    -> buy at your Shop
    use slot <0|1> [for the champion]          -> consume an inventory item
    scout <place> [with <units>]               -> attack-move your cheapest eyes
    surrender / autopilot [off]

  UNITS:  the army (default) | cavalry | siege | footmen | archers | spearmen
          knights | gryphons | sorcerers | workers | squad <n> | everything
          the hero  -- every hero-CLASS unit. Hero slots climb the hall ladder,
          so a Keep team has TWO; verbs taking a list get both, and the verbs
          taking exactly one (escort, buy, use) refuse and ask for
          "the champion" or "the priestess" rather than guess.
  PLACES: mid | our base | their base | <choke name, e.g. "the west ford">
          the west/east/north/south | the north mine | the contested mine
          the nearest bounty | our expansion | explicit "(-40, 20)"
  CLASSES (focus): Hero Archer Footman Worker Building Siege Cavalry

  Clauses split on commas, semicolons and newlines — not on "and".

PLANS — "X, then Y, then Z" (the engine walks the sequence for you)
  A ", then"-joined directive becomes ONE plan_set: a named sequence the engine
  steps through, submitting each step when its turn comes. Once through, never
  looping; at most 8 steps, at most 2 plans running.

    build a barracks, then train 4 footmen
    build a barracks, then when we reach tier 2, build a sanctum,
        then train 3 sorcerers
    push mid, then after 60s, push their base
    hold the west ford, then when I see 3 siege, retreat at 40%   as opener

  , then                -> next step as soon as this one is accepted
  , then when <cond>,   -> wait here until <cond> (the trigger vocabulary)
  , then after <n>s,    -> wait here for <n> seconds
  ... as <name>         -> name the plan (else a stable one is derived)

  THE COMMA MATTERS. "focus siege then heroes" is a focus chain in ONE clause;
  "focus siege, then push mid" is two steps. Say ", then" when you mean a step.

  A step's units are frozen when you set the plan, so a step cannot name
  soldiers you do not have yet. Name a SQUAD instead and let the squad fill up:
    "the barracks units join squad 2, then when I have 8 footmen,
     squad 2 pushes their base"

TRIGGERS — "when X, Y" (the engine watches it for you, at 4 Hz)
  A conditional compiles to one `trigger_set`. The engine evaluates the
  predicate every 250ms and submits the action itself, so a reaction costs you
  nothing per poll. Max 8 armed triggers; re-using a name replaces that rule.
    "when my base is attacked, squad 1 defends our base"
    "pull back when my hero drops below 30%"
    "whenever a bounty appears, forage mid with the cavalry"   (repeating)
    "when we reach tier 2, build a workshop as tech-up"        (your own name)
    "when the clock passes 6 minutes, push their base every 90s"
    "clear all triggers" / "disarm trigger home-guard"     -> trigger_clear
  when / if / once / after / as soon as   -> fires ONCE, then disarms
  whenever / every time / each time       -> REPEATS (45s cooldown by default)
  "... as <name>"   names it;   "... every 90s"   sets the cooldown.

  THE ELEVEN PREDICATES (this is the whole list — `shared::TriggerWhen`):
    my base is attacked                 any of your buildings damaged (last 8s)
    my hero drops below 40%             any living hero of yours under that
    squad 2 drops below 50%             that squad's POOLED health under that
    I see 3 or more siege               enemy units you can SEE right now
    an enemy army of 6 is spotted       6+ enemy troops in your INTEL ledger,
                                        which outlives the scout that found
                                        them; add "within 30s" to require a
                                        fresh sighting rather than a remembered
                                        one. Workers do not count as an army.
    their hero falls                    an enemy hero you WATCHED DIE and have
                                        not seen alive since; name the champion
                                        or the priestess to mean just one
    a bounty appears                    a cache you can see is on the map
    my mine runs dry                    a dry gold mine near one of your halls
    we reach tier 2                     your tech tier (1/2/3)
    we have 8 footmen                   your living count of one unit kind
    the clock passes 6 minutes          game time

  If your condition is not one of these the tool says so and falls back to the
  old advice — watch `events` (bridge_wait.py wakes you on them) and send the
  action yourself. It will not silently pick a predicate that is nearly right:
  "strike when their hero is below 30%" still defers, because nothing reads an
  ENEMY hero's health — you cannot select one, so no number about it is
  knowable — and answering it with your own is a different order. "when their
  hero FALLS" is a different question, and that one compiles: whether you
  watched it die is a fact you have.

  A trigger's action is any intent, but exactly ONE. When your phrasing needs
  two ("hold the ford with the cavalry" is membership AND purpose), the
  membership is sent now and the purpose is what waits.

IF A PHRASE IS NOT HERE (the escape hatch)
  You are a language model with the snapshot in front of you. Write the intents
  yourself and send them with tools/bridge_send.py. The full 25-verb schema is
  docs/INTENT.md; every verb's shape is in COMMANDER_BRIEF.md's command
  reference. This tool is a convenience over that schema, never a gate in front
  of it — anything it can express you can also write by hand, and anything it
  cannot, you still can.

  The confirmation loop is the same either way: the game logs one English
  sentence per intent to bridge/intent_log.jsonl (`Intent::sentence()`), and
  every unit reports its own reason in the snapshot as `units[].why`
  ("posture:push sq1", "order:move by bridge t=123"). Compile, send, then read
  those two back. If the sentence is wrong, the compile was wrong.
"""


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def next_seq(seat):
    """The same rule bridge_send.py uses: one past whatever anyone last set."""
    seq = 0
    for name, key in (("state.json", "seq_applied"), ("commands.json", "seq")):
        try:
            with open(os.path.join(seat, name)) as f:
                seq = max(seq, json.load(f).get(key, 0))
        except Exception:
            pass
    return seq + 1


def report(result, stream):
    for clause, summary in result.notes:
        print(f"  ok       {clause!r:<44} -> {summary}", file=stream)
    for clause, cond, suggestion in result.deferred:
        # `deferred` now means one specific thing: the ACTION compiles but the
        # CONDITION is outside `TriggerWhen`. The old watch-the-feed advice is
        # exactly right for that case and nothing else, so it lives here.
        print(f"  deferred {clause!r:<44} -> no predicate matches {cond!r} "
              f"(see --explain for the eleven); watch `events` for it, then run:",
              file=stream)
        print(f"           intent_compile.py --seat <SEAT> --send "
              f"{suggestion!r}" if suggestion else
              "           (and write the intents by hand — see --explain)",
              file=stream)
    for clause, reason in result.errors:
        print(f"  FAILED   {clause!r:<44} -> {reason}", file=stream)


def main(argv=None):
    ap = argparse.ArgumentParser(add_help=True, description=__doc__.split("\n")[0])
    ap.add_argument("directives", nargs="*", help="natural-language directives")
    ap.add_argument("--seat", help="seat directory (reads <seat>/state.json)")
    ap.add_argument("--state", help="path to a state.json")
    ap.add_argument("--out", help="write the {seq, commands} batch here")
    ap.add_argument("--send", action="store_true",
                    help="write <seat>/commands.json with the next seq")
    ap.add_argument("--json", action="store_true", help="print only the JSON array")
    ap.add_argument("--explain", action="store_true", help="print the vocabulary")
    args = ap.parse_args(argv)

    if args.explain:
        print(EXPLAIN)
        return 0
    if not args.directives:
        ap.error("give at least one directive, or --explain")

    state_path = args.state or (os.path.join(args.seat, "state.json") if args.seat else None)
    if state_path and not os.path.exists(state_path):
        print(f"intent_compile: no snapshot at {state_path}", file=sys.stderr)
        return 2
    snap = Snapshot.load(state_path)
    result = compile_directives(args.directives, snap)

    if not args.json:
        report(result, sys.stderr)

    payload = json.dumps(result.intents, indent=None if args.json else 2)
    if args.send:
        if not args.seat:
            ap.error("--send needs --seat")
        batch = {"seq": next_seq(args.seat), "commands": result.intents}
        tmp = os.path.join(args.seat, "commands.tmp")
        with open(tmp, "w") as f:
            json.dump(batch, f)
        os.replace(tmp, os.path.join(args.seat, "commands.json"))
        print(f"sent seq={batch['seq']} ({len(result.intents)} intents) to {args.seat}",
              file=sys.stderr)
    elif args.out:
        with open(args.out, "w") as f:
            json.dump({"seq": 1, "commands": result.intents}, f, indent=2)
        print(f"wrote {len(result.intents)} intents to {args.out}", file=sys.stderr)
    else:
        print(payload)

    # Non-zero only when nothing at all compiled: a partially understood
    # directive still moves the army, exactly as a partially valid bridge batch
    # does (docs/INTENT.md: errors are appended, not fatal).
    return 0 if result.intents or result.deferred else 1


if __name__ == "__main__":
    sys.exit(main())
