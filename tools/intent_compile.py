#!/usr/bin/env python3
"""Compile a natural-language directive into a batch of wc3clone Intent objects.

    intent_compile.py --seat bridge/red "hold the west, forage mid with cavalry"

WHAT THIS IS
------------
The game speaks exactly one language: `shared::Intent`, 25 verbs, documented in
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

WHAT IT DELIBERATELY WILL NOT DO
--------------------------------
Conditionals. "strike when their hero falls" has no verb in the language,
because the engine has no trigger system — doctrine is the only thing that acts
on its own, and it reacts to health, range and treasure, not to arbitrary
events. Rather than invent a trigger or silently drop the condition, the tool
compiles the ACTION, reports it as deferred, and hands back the exact command
to run when the commander sees the condition in the event feed. Refusing to
guess is a feature: the alternative is an army that attacks at the wrong moment
and a log that says the commander ordered it.
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
    "hero": ["Hero", "Priestess"],
    "champion": ["Hero"],
    "priestess": ["Priestess"],
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
# Which building trains what. The catalog is authoritative (`catalog.json`);
# this is the shortcut that lets "train 3 footmen" pick a building by itself.
TRAINS = {
    "TownHall": ["Worker", "Hero", "Priestess"],
    "Keep": ["Worker", "Hero", "Priestess"],
    "Castle": ["Worker", "Hero", "Priestess"],
    "Barracks": ["Footman", "Archer", "Spearman", "Knight"],
    "Workshop": ["Catapult", "Raider", "GryphonRider"],
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
}
BUILDING_WORDS = {
    "farm": "Farm", "farms": "Farm",
    "barracks": "Barracks",
    "tower": "Tower", "towers": "Tower",
    "wall": "Wall", "walls": "Wall",
    "shop": "Shop",
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

    def __init__(self, data):
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
    def load(cls, path):
        if path is None:
            return cls({})
        with open(path) as f:
            return cls(json.load(f))

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
    target_ids = resolve_units(m.group("who_target"), snap, default="hero")
    if not target_ids:
        ctx.result.fail(clause, f"no unit matches {m.group('who_target')!r}")
        return []
    target = target_ids[0]
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
    classes = []
    for part in parts:
        for token in _words(part):
            if token in CLASS_WORDS and CLASS_WORDS[token] not in classes:
                classes.append(CLASS_WORDS[token])
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


@rule("buy", r"^(?:buy|purchase)\s+(?:a\s+|an\s+|the\s+)?(?P<item>.+?)$")
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
    ctx.result.ok(clause, f"buy {match['id']} at shop {shop['id']}")
    return [{"type": "buy", "shop": shop["id"], "item": match["id"]}]


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
    producers = [b for b in snap.own_buildings()
                 if b.get("done") and kind in TRAINS.get(b.get("kind"), [])]
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
# Driver
# ---------------------------------------------------------------------------

CONDITIONAL = re.compile(r"^(?P<action>.+?)\s+(?:when|if|once|after|as\s+soon\s+as)\s+"
                         r"(?P<cond>.+?)$", re.I)


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
        for clause in split_clauses(directive):
            cond = CONDITIONAL.match(clause)
            if cond:
                # The action may be perfectly compilable; the trigger is not
                # expressible. Say so, and hand back the command to run later.
                action = cond.group("action").strip()
                probe = Result()
                probe_ctx = Ctx(snap, probe)
                probe_ctx.used_squads = set(ctx.used_squads)
                trial = compile_clause(action, probe_ctx)
                suggestion = action if trial else None
                result.deferred.append((clause, cond.group("cond").strip(), suggestion))
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
    buy <item>                                 -> buy at your Shop
    scout <place> [with <units>]               -> attack-move your cheapest eyes
    surrender / autopilot [off]

  UNITS:  the army (default) | cavalry | siege | footmen | archers | spearmen
          knights | gryphons | the hero | workers | squad <n> | everything
  PLACES: mid | our base | their base | <choke name, e.g. "the west ford">
          the west/east/north/south | the north mine | the contested mine
          the nearest bounty | our expansion | explicit "(-40, 20)"
  CLASSES (focus): Hero Archer Footman Worker Building Siege Cavalry

  Clauses split on commas, semicolons and newlines — not on "and".

WHAT IT WILL NOT DO, AND WHAT TO DO INSTEAD
  Conditionals ("strike when their hero falls") have no verb in this language:
  the engine has no trigger system, and doctrine reacts only to health, range
  and treasure. The tool compiles the action, marks it deferred, and prints the
  command to run when you see the condition in the event feed. Watch `events`
  (bridge_wait.py wakes you on them), then run the printed command.

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
        print(f"  deferred {clause!r:<44} -> no trigger verb exists; watch for "
              f"{cond!r} in `events`, then run:", file=stream)
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
