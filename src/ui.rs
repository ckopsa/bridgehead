//! ui.rs — player controls & HUD.
//!
//! Owns: `Selected` marker, selection rings, right-click context orders,
//! building placement ghost, command hotkeys/buttons, control groups, and the
//! bevy_ui HUD: a top resource bar plus a classic WC3-style bottom console
//! (minimap | selection panel | command card), the drag rectangle, the
//! game-over banner, the top-right alert stack, and the top-left co-command
//! proposal panel.
//!
//! The alert stack renders `shared::GameEvents` — the very buffer bridge.rs
//! serializes for an external commander, filtered to `Team::Human` the same way
//! the bridge filters to its seat. One producer, two renderers: whatever the
//! machine is told about the match, the player is told too. Space (or a click
//! on a row) sends the camera to where the news came from.
//!
//! The proposal panel is the human's half of co-command (copilot.rs): a
//! co-commander's pending directives, with the reason it gave and the compiled
//! English of every command, answered with `[Enter]` / `[Backspace]` or the
//! per-card buttons. Left, not right, and on its own clock — see the comment
//! above `PROP_SLOTS`.
//!
//! All world picking is analytic: the cursor is projected onto the Y=0 plane
//! with `shared::cursor_to_ground` and distance-tested in XZ against entity
//! positions. No physics, no mesh raycasts.
//!
//! UI-mutation note (Bevy B0001): every system mutates UI nodes through a
//! single `&mut Node` query keyed by the `El` enum (and a single
//! `&mut BackgroundColor` / `&mut Text` query), so no two mutable queries in a
//! system can ever alias. The minimap systems use `With`/`Without` marker
//! filters to stay provably disjoint.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::{PrimaryWindow, SystemCursorIcon};
use bevy::winit::cursor::CursorIcon;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use crate::command::{CommandLink, PendingOrder};
use crate::copilot::{Copilot, CopilotSet, ProposalVerdict, VetoReason, PROPOSAL_TTL};
// The hotkey registry (hotkeys.rs) is the single table every binding on this
// file's cards comes from. Nothing here writes a `KeyCode` literal for a card
// button any more: `hotkeys::key(Hk::Whatever)` yields the key and
// `hotkeys::key_caption` yields the tile caption, so a caption cannot drift
// from the key that fires it.
use crate::hotkeys::{self, Action as Hk};
use crate::intent::IntentApply;
use crate::shared::*;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Click radius (XZ) for units.
const UNIT_PICK_RADIUS: f32 = UNIT_RADIUS + 0.1;
/// Click radius for tree resource nodes.
const TREE_PICK_RADIUS: f32 = 2.0;
/// Click radius for gold mines.
const MINE_PICK_RADIUS: f32 = 3.5;
/// Minimum pixel travel before a left-drag becomes a rubber-band box.
const DRAG_THRESHOLD: f32 = 8.0;
/// Maximum queued units per production building.
const MAX_QUEUE: usize = 7;

/// Leash radius written by the [G Guard] command-card toggle.
const GUARD_RADIUS: f32 = 18.0;
/// HP fraction at which the [V Fallback] toggle breaks a unit off.
const FALLBACK_FRAC: f32 = 0.35;
/// Enemies inside a caster's ability radius before [T Auto-Cast] fires.
const AUTOCAST_MIN_ENEMIES: u32 = 3;
/// Ability slots the doctrine card offers an auto-cast toggle for. Three is
/// every ability any caster in the game has (the Champion's two, plus the
/// probe-only third), and three is exactly what the card has room for once the
/// postures, the parameterised pair and the page toggle have taken their slots.
const MAX_AUTOCAST_SLOTS: usize = 3;
// The keys for those per-ability toggles live in the registry as
// `Hk::AutoCastSlot(n)` — Z/X/C, which are the item and Blacksmith letters on
// the ORDERS card. Legal because the orders card and the doctrine card are
// different contexts (see `hotkeys::Ctx`), and now checked there rather than
// argued for here.

/// Retreat thresholds the doctrine card's [F] steps through, in order. The
/// coarse [V] toggle writes `FALLBACK_FRAC` and nothing else; this is the
/// parameterised form, and closing exactly this gap (an on/off switch where
/// the bridge gets a number) is what docs/TEMPO.md §2.0 asked for.
const FALLBACK_STEPS: [f32; 3] = [0.25, 0.35, 0.50];
/// Leash radii the doctrine card's [G] steps through. `GUARD_RADIUS` is the
/// middle rung, so the quick preset and the parameterised control agree.
const LEASH_STEPS: [f32; 3] = [10.0, 18.0, 30.0];
/// ---- Free entry ----------------------------------------------------------
///
/// The presets above are three rungs; `Intent::Retreat`/`Intent::Leash` carry
/// an arbitrary float, and a bridge commander types whatever it likes. That
/// gap is the last place in the doctrine vocabulary where the two seats are not
/// equal — not in what can be SAID, but in what can be said PRECISELY. These
/// increments close it: `[-]`/`[=]` walk the retreat threshold and `[[]`/`[]]`
/// walk the leash radius, one increment per press, over the whole legal range.
///
/// Keys rather than a text field because this is a game with a command card,
/// and a modal number box in the middle of a fight is not an affordance, it is
/// an interruption. `-`/`=` and `[`/`]` are the only adjacent free pairs left
/// on the keyboard (every letter is a command hotkey somewhere in this file),
/// they are unshifted, and they read as "less/more" without a legend.
const FALLBACK_NUDGE: f32 = 0.05;
/// Lowest threshold worth expressing. Nudging below it turns the policy off,
/// which is the same place the [F] cycle wraps to — one concept, one exit.
const FALLBACK_MIN: f32 = 0.05;
/// A unit that retreats above this is retreating at full health.
const FALLBACK_MAX: f32 = 0.95;
const LEASH_NUDGE: f32 = 2.0;
const LEASH_MIN: f32 = 2.0;
/// A leash wider than the map's half-width is not a leash.
const LEASH_MAX: f32 = 60.0;

/// Move `current` one increment and clamp, or turn the policy off.
///
/// From "off" the first press lands on `start` (the middle preset) rather than
/// at the bottom of the range: the player pressing `[=]` on an unleashed
/// selection means "start leashing", and starting at radius 2 would be a
/// technically-correct answer to a question nobody asked. Nudging below `lo`
/// returns `None` — the same "off" the cycle wraps to, so the two controls
/// agree about what the bottom of the scale is.
fn nudge_value(current: Option<f32>, up: bool, step: f32, start: f32, lo: f32, hi: f32) -> Option<f32> {
    let Some(current) = current else {
        return Some(start);
    };
    let next = current + if up { step } else { -step };
    (next >= lo - 1e-4).then(|| next.min(hi))
}

/// The corner badge on a selection tile: the squad id, or nothing at all.
///
/// Deliberately not "-" or "0" for a unit in no squad. The badge exists to make
/// a MIXED selection legible at a glance, and a grid where every tile carries a
/// mark is a grid where no mark stands out.
fn squad_badge(squad: Option<u8>) -> String {
    squad.map(|id| id.to_string()).unwrap_or_default()
}

/// Render a doctrine number without lying about it: whole values lose the
/// decimal point, everything else keeps one digit. A caption that rounded 37.5
/// to 38 would show a threshold no unit on the field is using.
fn trim_num(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{:.0}", v)
    } else {
        format!("{:.1}", v)
    }
}

/// Radius written by the doctrine card's Defend posture.
const DEFEND_RADIUS: f32 = 22.0;

/// The name the human's one trigger preset is armed under. A fixed name rather
/// than a generated one is what makes the tile a TOGGLE: pressing it twice
/// clears the rule it just set instead of arming a second copy, and re-pressing
/// it after moving the army replaces the rule in place without spending another
/// of the team's eight slots.
const HOME_GUARD: &str = "home-guard";
/// Radius of the Defend posture the home guard falls back to. Wider than
/// `DEFEND_RADIUS` because it has to cover a base rather than a chokepoint.
const HOME_GUARD_RADIUS: f32 = 26.0;
/// Cooldown between home-guard fires. A base is raided more than once a match,
/// and the rule must survive the first harassing Raider.
const HOME_GUARD_COOLDOWN: f32 = 30.0;

/// Is `name` armed for the human right now?
///
/// Spent once-triggers do not count: the tile is a toggle and its lit state has
/// to mean "this rule will fire", not "this rule exists".
fn has_trigger(triggers: &Triggers, name: &str) -> bool {
    triggers
        .get(Team::Human)
        .iter()
        .any(|t| t.name.as_str() == name && t.armed)
}

/// The selection panel's one-line trigger readout: every rule this team has
/// armed, with its state. The human's whole "list" view, and deliberately one
/// line — eight short names fit, and a panel that grew a scrolling list would
/// be building the authoring UI this bead explicitly deferred.
///
/// Empty string when there are none, which is how every other optional line in
/// this panel disappears.
fn trigger_line(triggers: &Triggers, now: f32) -> String {
    let armed = triggers.get(Team::Human);
    if armed.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = armed
        .iter()
        .map(|t| match t.status(now) {
            "armed" => t.name.as_str().to_string(),
            other => format!("{} ({other})", t.name),
        })
        .collect();
    format!("Triggers: {}", parts.join("  "))
}

/// The panel's region readout: every circle THIS team has named, with its
/// radius.
///
/// Own regions only. The map's built-ins are on the map itself — a line that
/// recited `our base, their base, mid, four mines` every frame would be a
/// readout of things that cannot change, crowding out the one list that does.
/// Empty string when there are none, like every other optional line here.
fn region_line(regions: &Regions) -> String {
    let named = regions.get(Team::Human);
    if named.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = named
        .iter()
        .map(|r| format!("{} r{}", r.name, trim_num(r.radius)))
        .collect();
    format!("Regions: {}", parts.join("  "))
}

/// The name the next `[M]` click will give its circle: the lowest free
/// `mark N`.
///
/// The human has no text entry, and building one would be a keyboard-capture
/// mode inside a game whose every other gesture is a click — so the engine
/// names the region and the player renames nothing. `mark 1`..`mark 8` is a
/// vocabulary a person can hold, it matches `MAX_REGIONS_PER_TEAM`, and it is
/// spellable by the seat that most wants to read it: a co-commander sharing
/// this team sees `mark 2` in its snapshot and can say `defend mark 2` back.
///
/// `None` when all eight are taken — the tile greys out rather than silently
/// stealing a name the player is using.
fn mark_number(regions: &Regions) -> Option<usize> {
    (1..=MAX_REGIONS_PER_TEAM).find(|n| {
        let name = format!("mark {n}");
        !regions
            .get(Team::Human)
            .iter()
            .any(|r| normalize_place(&r.name) == normalize_place(&name))
    })
}

/// The same answer, spelled. One of the two is always derived from the other,
/// so they cannot disagree about which slot is next.
fn next_mark_name(regions: &Regions) -> Option<String> {
    mark_number(regions).map(|n| format!("mark {n}"))
}
/// Radius a `[M]` mark gets before anybody tunes it. `DEFEND_RADIUS`, and not
/// by coincidence: the commonest thing to do with a marked circle is defend it,
/// and a mark whose ring did not match the ring a squad would hold there would
/// be a picture of the wrong decision.
const REGION_MARK_RADIUS: f32 = DEFEND_RADIUS;
/// One `,`/`.` press, in world units. Two nav cells — the smallest change that
/// is visible on a 100px minimap.
const REGION_NUDGE: f32 = 4.0;

/// The selection panel's one-line plan readout: every plan this team has, where
/// it is, and whether it is in trouble.
///
/// **The asymmetry is deliberate and it is the same one triggers made.** The
/// human gets a STATUS display and no authoring UI. That is not a seat the
/// engine treats differently — `plan_set` is one verb in one language and a
/// human's `intent_compile.py` sentence compiles to exactly the JSON a bridge
/// commander writes. It is a rendering decision, which is the one place
/// docs/INTENT.md permits the two seats to differ: a person at a keyboard
/// *already has* sequencing — they press the keys in order, at 200ms each, and
/// a mouse-driven step editor would be strictly slower than the thing it
/// automates. What the person does NOT have is a way to see a sequence their
/// co-commander set running unattended, and that is this line.
///
/// A blocked or halted plan shows its reason inline, truncated, because the
/// whole failure design is that its owner never has to go correlate a status
/// against an error channel.
fn plan_line(plans: &Plans) -> String {
    let mine = plans.get(Team::Human);
    if mine.is_empty() {
        return String::new();
    }
    /// How much of a refusal fits on a HUD line before it starts pushing the
    /// other plan off the end. Enough for "not enough gold (need 160…".
    const WHY_MAX: usize = 34;
    let parts: Vec<String> = mine
        .iter()
        .map(|p| {
            let head = format!("{} {}/{}", p.name, p.step_no(), p.steps.len());
            match &p.state {
                PlanState::Running => head,
                PlanState::Done => format!("{head} (done)"),
                PlanState::Blocked(why) | PlanState::Halted(why) => {
                    let word = if matches!(p.state, PlanState::Blocked(_)) {
                        "blocked"
                    } else {
                        "halted"
                    };
                    let short: String = if why.chars().count() > WHY_MAX {
                        why.chars().take(WHY_MAX).collect::<String>() + "…"
                    } else {
                        why.clone()
                    };
                    format!("{head} ({word}: {short})")
                }
            }
        })
        .collect();
    format!("Plans: {}", parts.join("  "))
}

/// What the player believes about the enemy's heroes, as one line.
///
/// The human half of the snapshot's `intel.heroes`, and it must stay the same
/// claim: **one rule of knowability, rendered twice**. So it reads the same
/// `FogGrid::hero_intel()` the bridge serialises, and it says the same three
/// words — a class never seen is omitted here exactly as it is reported
/// `"unknown"` there, because a HUD line listing every class the player has
/// never met would be an enemy roster nobody scouted.
///
/// What it deliberately cannot say is a level. A human cannot select an enemy
/// hero (the pickers are own-team only), so there is no gesture that would
/// ever have shown them one, and printing it here would hand the keyboard an
/// information right the wire does not have — the asymmetry backwards.
///
/// Empty string when nothing has been seen, which is how every other optional
/// line in this panel disappears.
fn enemy_hero_line(grid: &FogGrid, now: f32) -> String {
    let parts: Vec<String> = grid
        .hero_intel()
        .iter()
        .filter(|h| h.status != HeroStatus::Unknown)
        .map(|h| {
            let age = h.t_seen.map_or(String::new(), |t| {
                format!(" {:.0}s ago", (now - t).max(0.0))
            });
            match h.status {
                // No age on a death: you watched it happen, and "seen dying
                // 40s ago" invites the reader to wonder whether it has since
                // stopped being dead. It has not; it has possibly been
                // revived, and that is a different sentence the moment
                // anybody sees it.
                HeroStatus::SeenDying => format!("{} down", kind_name(h.kind)),
                _ => format!("{} alive{age}", kind_name(h.kind)),
            }
        })
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    format!("Their heroes: {}", parts.join("   "))
}

/// Highest squad id a human gesture will ever mint. Matches the three control
/// groups; a bridge commander may use any id it likes.
const MAX_UI_SQUAD: u8 = 3;

/// Step a value through `steps` and back to "off": `None -> steps[0] -> … ->
/// steps[n-1] -> None`. `eq` decides which rung the current value is on, so a
/// value a bridge commander wrote (37.5) lands on the next rung above it
/// rather than resetting the cycle.
fn cycle_step(current: Option<f32>, steps: &[f32]) -> Option<f32> {
    let Some(current) = current else {
        return steps.first().copied();
    };
    steps.iter().copied().find(|s| *s > current + 1e-3)
}

const TOP_BAR_H: f32 = 34.0;
/// Uniform gap between console zones and the console edge.
const PAD: f32 = 8.0;
/// Minimap is a square of at most this many logical pixels — the size it wants
/// and, on any window this HUD was designed for, the size it gets. What it
/// actually measures is `HudLayout::minimap_px`; see `hud_layout`.
///
/// The bottom console no longer has a constant of its own. It used to be a
/// flat 200px, which was two pixels short of the minimap it contained, and
/// deriving its height from its contents is what fixed that.
const MINIMAP_PX: f32 = 184.0;

/// How close to a minimap click an own hall must stand to be the hall that
/// click NAMED, in world units. A hall's footprint is about seven minimap
/// pixels, which is a hard thing to hit under pressure; halls stand far enough
/// apart that this radius still cannot mean two of them at once, and the
/// nearest wins if it ever does.
const MINIMAP_HALL_PICK: f32 = 10.0;

/// Selection cards: 2 rows of 6.
const MAX_CARDS: usize = 12;
const CARD_PX: f32 = 44.0;
const CARD_GAP: f32 = 5.0;

/// Command card: 4x3 grid.
///
/// It was 3x3 until the Blacksmith became the eighth placeable building: a
/// worker card is `[A] [S]` plus one entry per buildable kind, so eight kinds
/// need ten slots and the ninth-and-tenth would have been dropped by the
/// `truncate` at the end of `command_entries` — silently, which is the worst
/// way for a building to become unbuildable.
///
/// It grew a COLUMN rather than a row because the console is height-bound
/// (the console is ~202px and three rows of 52 plus gaps and margins already
/// spend 184 of it); a fourth row would not fit, and a fourth column costs
/// 52px of the selection panel's flex-grow width, which has it to spare.
const CMD_SLOTS: usize = 12;
const CMD_PX: f32 = 52.0;
const CMD_GAP: f32 = 6.0;
const CMD_COLS: f32 = 4.0;

const CONSOLE_BG: Color = Color::srgb(0.07, 0.08, 0.11);
const PANEL_BG: Color = Color::srgba(0.04, 0.05, 0.08, 0.86);
const EDGE: Color = Color::srgb(0.26, 0.28, 0.36);
const SLOT_BG: Color = Color::srgb(0.13, 0.15, 0.20);
const BAR_BG: Color = Color::srgb(0.16, 0.17, 0.21);
const MINIMAP_BG: Color = Color::srgb(0.06, 0.10, 0.07);
/// Bounty-cache minimap dot: brighter and whiter than the gold-mine dot so the
/// two never read as the same thing.
const BOUNTY_DOT: Color = Color::srgb(1.0, 0.93, 0.55);

/// Alert stack: how many notifications are on screen at once. Small on purpose
/// — the point is to catch an eye, and a wall of text catches nothing.
const NOTIF_SLOTS: usize = 6;
const NOTIF_W: f32 = 322.0;
/// Minimum row height; a long message wraps and grows past it.
const NOTIF_ROW_H: f32 = 24.0;
const NOTIF_GAP: f32 = 4.0;
/// Real seconds a notification stays on screen, and how much of that tail it
/// spends fading. Real, not game, time: `WC3_SPEED` accelerates the war, not
/// the eye reading about it.
const NOTIF_LIFETIME: f32 = 9.0;
const NOTIF_FADE: f32 = 2.5;
const NOTIF_FONT: f32 = 13.0;
/// Cap on how much of a narrow window the stack may take. A tiling WM can hand
/// the game a window slimmer than `NOTIF_W`, and a fixed-width stack pinned to
/// the right edge then runs off the left one.
///
/// 0.52, down from the 0.9 that only considered the stack on its own. The two
/// floating panels live in opposite top corners and 0.9 + `PROP_MAX_FRAC`'s
/// 0.44 is 134% of the window: below about 620px wide the alert stack grew
/// left across the proposal panel and the player was reading two things
/// printed on top of each other, with `notif_rect` and `prop_rect` both
/// claiming the same clicks. 0.52 + 0.44 = 0.96 can never overlap, and at any
/// width at or above 620 neither cap binds, so nothing about the sizes this
/// HUD was designed at changes.
const NOTIF_MAX_FRAC: f32 = 0.52;
/// Height budgeted per row when hit-testing (see `notif_rect`). A row is one
/// line of text most of the time, but a long message in a narrow window wraps,
/// and no analytic guess can know which. Two lines' worth means the stack
/// occasionally swallows a click just under it — much better than leaking one
/// through as a stray move order on the battlefield behind.
const NOTIF_ROW_HIT_H: f32 = NOTIF_ROW_H + NOTIF_FONT + NOTIF_GAP;
/// Jump the camera to the newest alert, then the one before it, and so on.
/// Space is free: letters are command hotkeys, arrows pan, `.` cycles workers.
const NOTIF_FOCUS_KEY: KeyCode = hotkeys::FOCUS_ALERT;

// --- Co-command: the proposal panel ----------------------------------------
//
// Where it sits is a design decision, not a layout accident. The alert stack
// owns the top-right and is built for news: six rows that fade after nine
// seconds. A proposal is not news, it is an OPEN QUESTION with a twenty-second
// clock, and putting it in a stack that expires on a different timer would
// mean the thing you must answer scrolls away under the thing you need not.
//
// So: top-left, the last large area the HUD does not own (the minimap is
// bottom-left, the console bottom, the resource bar a thin strip above). The
// ARRIVAL is still announced in the alert stack, which is where the player's
// eye already goes — transient notice on the right, standing decision on the
// left. The visual vocabulary is the alert stack's, because it should read as
// the same HUD: `PANEL_BG`, a coloured spine down the left edge, and amber for
// "something of yours is affected", which is what amber already means here.

/// One card per possible pending proposal; `copilot::MAX_PENDING` is the cap
/// that makes the pool exact rather than a guess.
const PROP_SLOTS: usize = crate::copilot::MAX_PENDING;
const PROP_W: f32 = 360.0;
/// Same narrow-window guard as the alert stack.
const PROP_MAX_FRAC: f32 = 0.44;
const PROP_FONT: f32 = 13.0;
const PROP_HEAD_FONT: f32 = 12.0;
const PROP_GAP: f32 = 6.0;
/// Most sentences printed per card before the rest are summarised. Bounds the
/// card's height, which is what makes `prop_rect` able to hit-test it.
const PROP_MAX_SENTENCES: usize = 4;
/// Height budgeted per card when hit-testing. Generous for the same reason
/// `NOTIF_ROW_HIT_H` is: a card's real height depends on how much of the note
/// wraps, no analytic guess can know, and swallowing a click just under the
/// panel is far better than leaking one through as a stray order. Raised by
/// one line's worth when the veto legend became its own line on the top card.
const PROP_CARD_HIT_H: f32 = 148.0;
/// Approve / veto the TOP pending proposal — index 0 of `Copilot::pending`,
/// which copilot.rs keeps in answer order (urgent first, then oldest). Both
/// keys are free: every letter is a command hotkey, Space focuses alerts, `.`
/// cycles workers.
const PROP_APPROVE_KEY: KeyCode = hotkeys::APPROVE_PROPOSAL;
const PROP_VETO_KEY: KeyCode = hotkeys::VETO_PROPOSAL;
/// The co-commander's colour. Deliberately NOT one of the three severity
/// colours: "my partner said this" must never read as "the game says this".
const PROP_ACCENT: Color = Color::srgb(0.68, 0.63, 1.0);
/// An urgent proposal's spine and headline. The alert stack's Warning amber —
/// the one exception to the rule above, and it earns it: urgency is a claim
/// about the GAME ("this window closes"), not about who is speaking, so it is
/// the one part of a card that should read in the HUD's own vocabulary.
/// `severity_color(EventSeverity::Warning)` is not const-callable, so the
/// value is written once here and checked against it by
/// `an_urgent_card_wears_the_warning_tint`.
const PROP_URGENT: Color = Color::srgb(1.0, 0.86, 0.35);

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        // The hotkey table is checked before a single system is registered: a
        // debug build refuses to start a match whose command card has two
        // buttons on one letter. Release builds skip it — the same table is
        // proved by `hotkeys::the_registry_has_no_collision_in_any_card_context`
        // in CI, and a shipped binary should not pay for the walk.
        if let Err(clash) = hotkeys::validate() {
            debug_assert!(false, "hotkey registry: {clash}");
            error!("hotkey registry: {clash}");
        }
        app.init_resource::<UiState>()
            .init_resource::<Notifications>()
            .init_resource::<AlertPings>()
            .init_resource::<HudLayout>()
            // `setup_fog` after `setup_ui`: it parents the minimap's fog layer
            // to the `MinimapRoot` that `setup_ui` spawns.
            .add_systems(
                Startup,
                (setup_ui, setup_hover, setup_fog, setup_alert_cues).chain(),
            )
            .add_systems(
                Update,
                // Two groups, each internally chained and the pair chained to
                // each other — one flat tuple would be 22 systems and Bevy's
                // tuple impls stop at 20. The split is where it reads best:
                // everything that *takes input* first, everything that *draws
                // the result* second.
                (
                    (
                        // First in the chain, ahead of every reader: the
                        // minimap's pixel size is an input to hit-testing a
                        // click on it, and a frame that hit-tested against last
                        // frame's size would put a camera jump in the wrong
                        // place on the frame a window is resized.
                        apply_hud_layout,
                        minimap_static_markers,
                        // Fog first, so everything downstream in this chain —
                        // the pickers, the minimap, the hover ring — reads the
                        // same visibility the player is looking at.
                        apply_fog_visibility,
                        // Immediately after the hider, because they are two
                        // halves of one answer: `apply_fog_visibility` decides
                        // what is on screen at all, `apply_fog_tint` decides
                        // how brightly what remains is lit.
                        apply_fog_tint,
                        update_fog_overlay,
                        sync_building_ghosts,
                        sync_intel_markers,
                        surrender_hotkey,
                        screenshot_hotkey,
                        scheduled_screenshots,
                        command_input,
                        panel_clicks,
                        // Before `minimap_input`: both write `CameraFocus` and
                        // terrain.rs honours the last one, so a live minimap
                        // drag outranks a Space press from earlier in the frame.
                        notification_input,
                        control_groups,
                        minimap_input,
                        left_mouse,
                        right_mouse,
                    )
                        .chain(),
                    (
                        // Grouped, not listed: the two armed-gesture previews
                        // read the cursor and write nothing else anybody here
                        // reads, so their order relative to each other is
                        // genuinely free — which is exactly what a nested tuple
                        // says, while keeping the group's place in the chain.
                        (update_ghost, update_posture_marker),
                        // Nested with the rally flag rather than listed: this
                        // tuple is at Bevy's 20-element ceiling, and the two
                        // draw unrelated standing facts whose relative order is
                        // genuinely free.
                        (update_rally_flag, update_region_rings),
                        // Chain of Command feedback. Like every system here it
                        // runs before the compiler, so a click gets its marker
                        // on the next frame rather than this one — 16ms, which
                        // is the difference between "acknowledged instantly"
                        // and "acknowledged instantly" as far as a player is
                        // concerned, and is what keeps the whole UI one ordered
                        // chain instead of two.
                        update_link_rings,
                        update_transit_markers,
                        hover_feedback,
                        sync_selection_rings,
                        update_minimap,
                        (update_minimap_bounties, update_minimap_regions),
                        update_notifications,
                        // After the drain that fills the ping list, so a ring
                        // is on screen the same frame its alert row is.
                        update_minimap_pings,
                        update_hud,
                    )
                        .chain(),
                )
                    // Every gesture system here submits intents; the compiler
                    // runs after all of them, so a click is compiled in the
                    // frame it happened rather than the next one.
                    // That is exactly `SimSet::Input`'s slot — after the fog
                    // recompute, before `SimSet::Intent` — so the set name now
                    // carries what these two clauses used to.
                    .chain()
                    .in_set(SimSet::Input)
                    .before(IntentApply)
                    .after(FogSet),
            )
            // Co-command's two systems sit OUTSIDE the input chain above,
            // which is at Bevy's 20-system tuple ceiling — and they want
            // different ordering anyway. `proposal_input` only has to beat
            // `CopilotSet`, so a verdict given this frame is resolved this
            // frame and its orders compile with the rest of the frame's
            // intents. `update_proposals` is the one HUD system that must run
            // AFTER the frame's decisions: a proposal the player just answered
            // should leave the screen immediately, not flicker one more time.
            .add_systems(
                Update,
                (
                    proposal_input.in_set(SimSet::Input).before(CopilotSet),
                    // `SimSet::Cosmetic`, not `Input`: this one must run after
                    // `CopilotSet`, and `CopilotSet` is downstream of `Input`,
                    // so filing it with its sibling would be a cycle. It draws
                    // and nothing else, which is what `Cosmetic` is for.
                    update_proposals.in_set(SimSet::Cosmetic).after(CopilotSet),
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Module-private state
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
struct UiState {
    /// Screen position where the left button went down (None = no press pending).
    drag_start: Option<Vec2>,
    /// True once the press has travelled past `DRAG_THRESHOLD`.
    dragging: bool,
    /// `A` pressed: next left click issues an attack-move.
    attack_move_armed: bool,
    /// Building placement mode.
    placement: Option<BuildingKind>,
    /// Workers already handed a segment of the CURRENT wall chain. A Build
    /// order overwrites the previous one, so a wall line has to be spread
    /// round-robin over the selected workers instead of piled on the nearest
    /// one. Cleared whenever placement mode is entered or cancelled.
    wall_chain: Vec<Entity>,
    /// Control groups 1..3 — and, since `control_groups` submits the `squad`
    /// verb, the same sets doctrine.rs executes postures for.
    groups: HashMap<u8, Vec<Entity>>,
    /// Which MODE of the command card is showing ([I] toggles).
    page: CardPage,
    /// Which OVERFLOW page within that mode is showing ([Tab] walks them).
    ///
    /// Separate from `page` because the two mean different things: `page` picks
    /// a vocabulary, this picks a slice of one. Clamped by `paginate` every
    /// frame, so a selection that shrinks under the player cannot leave the card
    /// showing nothing.
    card_page: usize,
    /// A posture waiting for the player to click its point on the ground.
    /// Same shape as `placement`: an armed mode the next left-click consumes.
    posture_place: Option<PostureArm>,
    /// A TARGETED cast waiting for the player to click where it goes — the
    /// third user of the same press-then-click vocabulary building placement
    /// taught and postures borrowed. A `Caster`-geometry ability never arms
    /// anything and fires on the key press exactly as it always did.
    cast_place: Option<CastArm>,
    /// True while `[M]` has armed the region marker: the next left-click on
    /// the ground names a circle at that point. The fifth user of the
    /// press-then-click vocabulary building placement taught.
    region_place: bool,
    /// The radius the next marked region gets, in world units, or `None` for
    /// [`REGION_MARK_RADIUS`]. Free-entry, not a preset ladder — `,`/`.` tune
    /// it, and it persists between marks so a player who wants three 30-unit
    /// circles sets the size once. `Option` so the first nudge lands on the
    /// default rather than on zero, which is what `nudge_value` already does
    /// for the other two numeric parameters on this page.
    region_radius: Option<f32>,
    /// A teleport item waiting for the player to click WHICH hall it goes to.
    /// Armed only when there are two or more to choose between; see
    /// [`TeleportArm`].
    teleport_place: Option<TeleportArm>,
    /// Round-robin cursor for the idle-worker hotkey.
    idle_cursor: usize,
    /// Left button went down inside the minimap and is still held.
    minimap_drag: bool,
    /// Entity represented by selection card i (refreshed by `update_hud`).
    card_entities: Vec<Entity>,
    /// Command bound to command-card button i (refreshed by `update_hud`).
    card_actions: Vec<CmdAction>,
    /// Number of live queue tiles (refreshed by `update_hud`).
    queue_len: usize,
    /// Number of visible alert rows (refreshed by `update_notifications`), so
    /// `cursor_over_hud` can tell a click on a notification from a world click
    /// without walking the UI tree.
    notif_rows: usize,
    /// Number of visible co-command proposal cards (refreshed by
    /// `update_proposals`), for the same reason `notif_rows` exists: a click
    /// on "Approve" must not also be a move order on the ground behind it.
    prop_cards: usize,
    /// The hero the player most recently had selected — the Shop's customer.
    ///
    /// A Shop's buy card is drawn for a BUILDING selection, so at the moment of
    /// buying there is no hero selected to read the intent off. With one hero
    /// per team that never mattered; with a Champion and a Priestess it decides
    /// whose bag the potion lands in, so the UI remembers the last hero the
    /// player looked at and sells to that one. Cleared to the team default when
    /// the remembered hero dies.
    last_hero: Option<Entity>,
}

#[derive(Resource)]
struct UiAssets {
    ring_mesh: Handle<Mesh>,
    /// A *thin* ring, for circles drawn at map scale. `ring_mesh`'s band is
    /// 16% of its radius, which reads as a donut once the radius is a command
    /// node's 30 world units rather than a unit's 1.1.
    hairline_mesh: Handle<Mesh>,
    ring_mat: Handle<StandardMaterial>,
    ghost_ok: Handle<StandardMaterial>,
    ghost_bad: Handle<StandardMaterial>,
    /// docs/TEMPO.md §4 — the circle inside which your orders are free.
    node_ring_mat: Handle<StandardMaterial>,
    /// A region this team named: warmer and slightly louder than the map's own
    /// places, because it is the one of the two the player chose and can move.
    region_mine_mat: Handle<StandardMaterial>,
    /// A built-in place. The quietest thing on the ground — it is on screen for
    /// the whole match on every map, and a permanent fact has to be ignorable.
    region_map_mat: Handle<StandardMaterial>,
    /// An order still travelling, in the rally flag's gold: both mean "this is
    /// where a thing you said is going to happen".
    transit_mat: Handle<StandardMaterial>,
}

#[derive(Resource)]
struct HoverAssets {
    friendly: Handle<StandardMaterial>,
    hostile: Handle<StandardMaterial>,
    resource: Handle<StandardMaterial>,
}

/// Marker on the flat green ring entity parented to a selected entity.
#[derive(Component)]
struct SelectionRing;

/// Back-pointer on a selected entity to its ring child.
#[derive(Component)]
struct HasRing(Entity);

/// The single translucent placement footprint.
#[derive(Component)]
struct Ghost;

/// The single translucent disc under the cursor while a squad posture is armed
/// and waiting for its ground click.
///
/// Building placement has had a ghost since the first version, and arming a
/// posture is the same gesture with the same two steps — but it showed nothing,
/// so "where exactly is the Defend circle going to sit" was answerable only
/// after the click, by reading the sentence in the log. The disc is drawn at
/// the posture's real radius, so a Defend ring you place is the ring you get.
#[derive(Component)]
struct PostureMarker;

/// The rubber-band selection rectangle UI node.
#[derive(Component)]
struct DragRect;

/// One pooled alert row (index 0 = topmost = newest) and the text inside it.
/// Deliberately *not* `El`/`Slot`: those exist so `update_hud` can hold all of
/// its mutable `Node`/`Text` access in one query, and the alert stack is driven
/// by a different clock from different data. Its own markers keep the two
/// systems from ever fighting over the same node.
#[derive(Component)]
struct NotifRow(usize);

#[derive(Component)]
struct NotifText(usize);

/// The "[Space] focus" footer under the stack; hidden when the stack is empty.
#[derive(Component)]
struct NotifHint;

/// The single world-space ring shown under whatever the cursor would pick.
/// One pooled proposal card, by queue index (0 = oldest = the hotkeys' target).
#[derive(Component)]
struct PropCard(usize);

/// Which line of which card a text node is. One marker component with a part
/// tag rather than three marker types, because three `&mut Text` queries in one
/// system would need three mutually-exclusive `Without` filters to satisfy
/// Bevy B0001 — and one query with a discriminant is the same information
/// without the aliasing puzzle.
#[derive(Component, Clone, Copy)]
struct PropText {
    card: usize,
    part: PropPart,
}

#[derive(Clone, Copy, PartialEq)]
enum PropPart {
    /// `#3   copilot   14s   [Enter] approve  [Bksp] veto`
    Head,
    /// The co-commander's stated reason.
    Note,
    /// The compiled sentences, then any conflict tags.
    Body,
}

/// One of a card's two buttons. `approve` false is the veto.
#[derive(Component, Clone, Copy)]
struct PropBtn {
    card: usize,
    approve: bool,
}

#[derive(Component)]
struct HoverRing;

/// The single pooled rally-point banner, moved to the rally location of the
/// one selected production building (hidden otherwise).
#[derive(Component)]
struct RallyFlag;

/// One pooled ring showing the free radius of an own command node
/// (docs/TEMPO.md §3). Drawn only while `WC3_COMMAND_LATENCY` is on, so with
/// the feature off not one of these entities is ever spawned.
#[derive(Component)]
struct LinkRing;

/// One pooled ring drawing a named region on the ground. Own regions and the
/// map's built-ins share the pool and are told apart by material, not by
/// component: they are the same kind of object, and the difference the player
/// needs is "mine" versus "the map's", which is a colour.
#[derive(Component)]
struct RegionRing;

/// One pooled marker at the destination of a selected unit's in-transit order,
/// closing as the order arrives. This is the countdown: the ring's radius is
/// `ready_at - now` made visible, so an order in flight looks like an order in
/// flight rather than like the game ignoring the click.
#[derive(Component)]
struct TransitRing;

/// The bottom console strip. `apply_hud_layout` owns its height.
#[derive(Component)]
struct ConsoleRoot;

/// Container of the minimap; all markers are absolute children of it.
#[derive(Component)]
struct MinimapRoot;

/// A pooled, per-frame-updated minimap dot (units & buildings).
#[derive(Component)]
struct MinimapMarker;

/// A minimap dot spawned once at startup (trees & gold mines).
#[derive(Component)]
struct MinimapStatic;

/// A pooled minimap dot for a bounty cache. Its own tiny pool rather than a
/// share of `MinimapMarker`'s: there are never more than a handful, and they
/// pulse on their own clock instead of tracking a unit.
#[derive(Component)]
struct MinimapBounty;

/// A named place's outline on the minimap. Pooled, never despawned — the same
/// contract every other dynamic minimap marker keeps.
#[derive(Component)]
struct MinimapRegion;

/// The camera-viewport outline drawn on the minimap.
#[derive(Component)]
struct MinimapViewport;

/// Every HUD node whose layout/colour is refreshed each frame. One enum keeps
/// all mutable `Node` / `BackgroundColor` access inside a single query.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum El {
    SinglePane,
    MultiPane,
    Portrait,
    HpFill,
    /// Wrapper around the hero-only XP + mana bars (hidden for everything else).
    HeroBars,
    XpFill,
    ManaFill,
    ProgWrap,
    ProgFill,
    Card(usize),
    CardHp(usize),
    QueueTile(usize),
    CmdBtn(usize),
}

/// Every HUD text, matched by slot.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Resources,
    Supply,
    Hints,
    Banner,
    BannerSub,
    PortraitLetter,
    Name,
    Hp,
    Stats,
    Extra,
    /// Hero inventory line ("[1] Potion  [2] -"); empty for everything else.
    Items,
    /// One-line doctrine summary of the selection (empty = no policies).
    Doctrine,
    /// The selection's answer to "why are you doing that?" — verbatim the same
    /// string the bridge reads from the snapshot's `units[].why`.
    Why,
    /// What reaching the selection costs, and what is already on its way —
    /// the HUD's half of the snapshot's `units[].link` / `units[].pending`
    /// (docs/TEMPO.md §4). Empty string whenever the mechanic is off.
    Link,
    /// Every trigger this team has armed, and its state. Empty until the
    /// player (or their co-commander) arms one.
    Triggers,
    /// The region readout, directly under the triggers line. Together they are
    /// the two halves of standing policy the human can otherwise only infer
    /// from circles on the map: what will fire, and where the ground is.
    Regions,
    /// Every plan this team has, where it is, and why it stopped if it did.
    /// Empty until one is set. See `plan_line` for why the human gets a status
    /// readout and no authoring UI.
    Plans,
    /// What we believe about the enemy's heroes — the HUD's rendering of the
    /// snapshot's `intel.heroes`. Empty until one has been laid eyes on, so a
    /// match where neither hero has been met looks exactly as it always did.
    EnemyHeroes,
    /// Top bar: how much of this army is inside its own chain of command.
    /// Empty string whenever the mechanic is off.
    Coverage,
    Overflow,
    CardLetter(usize),
    /// Squad badge in the corner of a selection tile — the digit `Ctrl+N` and
    /// the bridge's `squad` verb both write. Empty for a unit in no squad and
    /// for every building.
    ///
    /// A multi-select used to be an anonymous grid of initials: the doctrine
    /// card would say "squad 1" (the first unit's), the player would set a
    /// posture, and two of the six tiles would quietly not be in it. The badge
    /// is how a mixed selection becomes visible BEFORE the order, which is the
    /// only time it can still be fixed.
    CardSquad(usize),
    QueueLetter(usize),
    CmdKey(usize),
    CmdLabel(usize),
    CmdCost(usize),
    /// The overflow-page indicator under the command card.
    CmdPage,
}

// ---------------------------------------------------------------------------
// Commands (shared by hotkeys and command-card buttons)
// ---------------------------------------------------------------------------

// The command card used to carry a four-writer `CardActions` bundle —
// `CastAbility`, `BuyItem`, `UseItem`, `UpgradeBuilding` — field for field
// identical to the one bridge.rs carried. Both are gone: the card emits
// `SubmitIntent` and nothing else, and intent.rs owns the four writers. Two
// interfaces that had independently converged on the same bundle are exactly
// the duplication the intent layer exists to remove, and dropping it also
// bought `command_input` three parameters of headroom against Bevy's ceiling.

/// The command card has two pages. Page one is the classic RTS card — build,
/// train, cast, the four coarse doctrine toggles. Page two is doctrine proper:
/// squad postures, parameterised retreat/leash, and the production template.
///
/// A page rather than more keys, because the 3x3 card was already full and the
/// hotkey landscape already crowded; and a page rather than a separate screen,
/// because a posture has to be issued in the middle of a fight, next to the
/// selection it is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum CardPage {
    #[default]
    Orders,
    Doctrine,
}

/// The four postures, as the card offers them. `SquadPosture` carries a point
/// (or a unit) that the player has not chosen yet when the button is pressed,
/// so the button names only the kind and the click supplies the rest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PostureKind {
    Defend,
    Push,
    Forage,
    /// Screen one of our own units. Arms a click like the others — the click
    /// picks a UNIT rather than a point.
    Escort,
}

impl PostureKind {
    fn label(self) -> &'static str {
        match self {
            PostureKind::Defend => "Defend",
            PostureKind::Push => "Push",
            PostureKind::Forage => "Forage",
            PostureKind::Escort => "Escort",
        }
    }
    /// True when the gesture is "press, then click one of our units".
    ///
    /// Escort used to be issued outright at press time, aimed at the team's
    /// lowest-entity-id living hero, because the hero was the only escortee the
    /// UI could name. `PostureIntent::Escort` has always taken any own unit —
    /// a commander could screen a Catapult, a Priestess, or the one Worker
    /// walking out to expand — so the human seat was strictly less expressive
    /// than the bridge on this one verb. Arming a unit click closes it, and
    /// costs the player nothing: escorting the hero is now "press R, click the
    /// hero", one click more than before and the only click that was ever
    /// ambiguous.
    fn needs_unit(self) -> bool {
        matches!(self, PostureKind::Escort)
    }
}

/// A posture button waiting for its ground click. The squad is resolved at
/// press time (not at click time) so the sentence the player is composing
/// cannot change under them if the selection does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PostureArm {
    squad: u8,
    kind: PostureKind,
}

/// One `cast` sentence, whatever supplied the aim. Written once because the
/// hotkey, the command-card button and the armed click all compose the same
/// object — and because it is exactly what a bridge commander types, which is
/// the claim docs/INTENT.md makes about every gesture in this file.
fn cast_here(caster: Entity, slot: usize, aim: Option<CastTarget>) -> Intent {
    let (x, z, target) = match aim {
        Some(CastTarget::Point(p)) => (Some(p.x), Some(p.z), None),
        Some(CastTarget::Unit(e)) => (None, None, Some(intent_id(e))),
        None => (None, None, None),
    };
    Intent::Cast {
        hero: intent_id(caster),
        ability: Some(AbilitySelector::Index(slot)),
        x,
        z,
        target,
    }
}

/// A targeted-cast button waiting for its click. Like [`PostureArm`], the
/// casters are resolved at PRESS time rather than at click time: the sentence
/// the player is composing must not change under them if a unit dies or the
/// selection shifts between the key and the click.
#[derive(Clone, PartialEq, Eq, Debug)]
struct CastArm {
    /// Everyone who will cast — one `Intent::Cast` each, all at the same aim.
    casters: Vec<Entity>,
    /// Slot, because the UI is index-native and the hotkey IS the slot.
    slot: usize,
    /// Ability name, for the hint line the player reads while aiming.
    name: &'static str,
    /// True for `AbilityTarget::Unit`: the click names a unit, not ground.
    wants_unit: bool,
}

/// A teleport item waiting for the player to say WHICH hall.
///
/// The fourth user of the press-then-click vocabulary building placement
/// taught, and the first whose click names a *building* rather than a point or
/// a unit. Like [`CastArm`], the hero and the slot are resolved at PRESS time:
/// the bag the player is spending from must not change under them between the
/// key and the click.
///
/// It only ever exists with **two or more** standing halls. With one hall
/// there is no choice to make, so the key fires the scroll outright with no
/// destination at all — a ceremony that always has exactly one answer is a
/// tax, not a decision, and the nearest-hall default already says it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct TeleportArm {
    /// Whose bag, named explicitly for the reason `UseSlot` always named it:
    /// reading "the team's hero" at click time would show the Priestess's
    /// scroll and burn the Champion's.
    hero: Entity,
    slot: usize,
    /// Item name, for the hint line the player reads while choosing.
    name: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CmdAction {
    AttackMove,
    Stop,
    Place(BuildingKind),
    Train(UnitKind),
    /// Cast ability slot N of every selected own hero. One entry per UNLOCKED
    /// slot, so a hero with two spells gets two buttons and two hotkeys.
    CastHero(usize),
    /// Cast ability slot N of the single selected own building.
    CastBuilding(usize),
    /// Buy a consumable at the single selected own Shop, for the team's hero.
    Buy(ItemId),
    /// Convert the single selected own building into its next tier in place.
    /// Carries the RESULT, so the button can name what you get.
    Upgrade(BuildingKind),
    /// Start the next rung of a team-wide research ladder at the single
    /// selected own forge. Carries the LADDER, not the level — the level is
    /// always current+1 and resolving it here would let a stale card (these
    /// are rebuilt from last frame's layout) buy the wrong rung.
    Research(ResearchKind),
    /// Consume the selected own hero's inventory slot.
    UseSlot(usize),
    /// Doctrine: toggle `LeashPolicy` on the whole own-unit selection.
    ToggleGuard,
    /// Doctrine: toggle `RetreatPolicy` on the whole own-unit selection.
    ToggleFallback,
    /// Doctrine: advance the `TargetPriority` preset by one step.
    CyclePriority,
    /// Doctrine: toggle `AutoCastPolicy` slot 0 on every selected own hero.
    /// The quick switch; page two has one of these per ability.
    ToggleAutoCast,
    /// Doctrine (page two): toggle the auto-cast rule for ONE ability slot.
    ToggleAutoCastSlot(usize),

    // --- page two: the doctrine card -------------------------------------
    /// Flip between the orders card and the doctrine card.
    TogglePage,
    /// Set the selection's squad posture. Ground-pointed kinds arm a click.
    SetPosture(PostureKind),
    /// Clear the selection's squad posture (membership survives).
    ClearPosture,
    /// Step the retreat threshold: off -> 25% -> 35% -> 50% -> off. The
    /// parameterised form of [V], and the reason it exists: the bridge sends a
    /// number, so the human must be able to choose one too.
    CycleFallback,
    /// Step the leash radius: off -> 10 -> 18 -> 30 -> off.
    CycleLeash,
    /// Nudge the retreat threshold by one increment (`true` = up). The presets
    /// are a fast path, not the vocabulary: the wire carries any float and so
    /// must the human.
    NudgeFallback(bool),
    /// Nudge the leash radius by one increment (`true` = up).
    NudgeLeash(bool),
    /// Step the selected building's template squad: none -> 1 -> 2 -> 3 -> none.
    TemplateSquad,
    /// Step the selected building's template retreat threshold.
    TemplateFallback,
    /// Step the selected building's template focus-fire preset.
    TemplatePriority,
    /// Toggle auto-cast in the selected building's template (heroes only, but
    /// a hall is exactly where a hero is trained).
    TemplateAutoCast,
    /// Remove the selected building's template entirely.
    TemplateClear,
    /// Arm or clear the `home-guard` trigger for the selection's squad.
    ToggleHomeGuard,
    /// Arm the region marker: the next ground click names a circle.
    MarkRegion,
    /// Forget every region this team named.
    ClearRegions,
    /// Free-entry radius for the next mark; `true` is bigger.
    NudgeRegion(bool),
}

// ---------------------------------------------------------------------------
// Doctrine: what the player can set from the command card
// ---------------------------------------------------------------------------

/// Focus-fire presets cycled by the [P] button. The player never edits the
/// class list directly — one button walks these four states.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum PrioPreset {
    #[default]
    None,
    HuntHero,
    KillArchers,
    Siege,
    /// Shoot the air first. The bridge can already write `prio: ["Air", ..]`
    /// by hand; without this the human has no way to say the same thing, and
    /// "the AI cannot act in ways the human cannot" has to run both
    /// directions.
    AntiAir,
}

impl PrioPreset {
    /// None -> Hunt Hero -> Kill Archers -> Siege -> Anti-Air -> None.
    fn next(self) -> Self {
        match self {
            PrioPreset::None => PrioPreset::HuntHero,
            PrioPreset::HuntHero => PrioPreset::KillArchers,
            PrioPreset::KillArchers => PrioPreset::Siege,
            PrioPreset::Siege => PrioPreset::AntiAir,
            PrioPreset::AntiAir => PrioPreset::None,
        }
    }
    /// Button caption; the empty state just names the button.
    fn label(self) -> &'static str {
        match self {
            PrioPreset::None => "Priority",
            PrioPreset::HuntHero => "Hunt Hero",
            PrioPreset::KillArchers => "Kill Archers",
            PrioPreset::Siege => "Siege",
            PrioPreset::AntiAir => "Anti-Air",
        }
    }
    /// Compact tag for the selection-panel doctrine line.
    fn tag(self) -> &'static str {
        match self {
            PrioPreset::None => "",
            PrioPreset::HuntHero => "HuntHero",
            PrioPreset::KillArchers => "KillArchers",
            PrioPreset::Siege => "Siege",
            PrioPreset::AntiAir => "AntiAir",
        }
    }
    /// The class list written to `TargetPriority` (empty = remove it).
    fn classes(self) -> &'static [TargetClass] {
        match self {
            PrioPreset::None => &[],
            PrioPreset::HuntHero => &[
                TargetClass::Hero,
                TargetClass::Archer,
                TargetClass::Footman,
                TargetClass::Worker,
                TargetClass::Building,
            ],
            PrioPreset::KillArchers => &[
                TargetClass::Archer,
                TargetClass::Hero,
                TargetClass::Footman,
                TargetClass::Worker,
                TargetClass::Building,
            ],
            PrioPreset::Siege => &[TargetClass::Building],
            PrioPreset::AntiAir => &[
                TargetClass::Air,
                TargetClass::Archer,
                TargetClass::Hero,
                TargetClass::Footman,
                TargetClass::Worker,
            ],
        }
    }
    /// Which preset a live `TargetPriority` reads as (its first class decides,
    /// so a list the bridge wrote still maps onto the nearest preset).
    fn of(first: Option<TargetClass>) -> Self {
        match first {
            Some(TargetClass::Hero) => PrioPreset::HuntHero,
            Some(TargetClass::Archer) => PrioPreset::KillArchers,
            Some(TargetClass::Building) => PrioPreset::Siege,
            Some(TargetClass::Air) => PrioPreset::AntiAir,
            _ => PrioPreset::None,
        }
    }
}

/// The doctrine components of one selected own unit, read out of the ECS.
#[derive(Clone, Copy, Default)]
struct UnitDoctrine {
    /// Leash radius, when the unit has a `LeashPolicy`.
    leash: Option<f32>,
    /// Retreat threshold, when the unit has a `RetreatPolicy`.
    retreat: Option<f32>,
    prio: PrioPreset,
    autocast: bool,
    /// Which ability SLOTS carry an auto-cast rule. `AutoCastPolicy` has been
    /// per-slot since abilities v2; the card only ever read "is the component
    /// there at all", which is why a hero with two spells had one toggle and no
    /// way to say which spell it meant.
    autocast_slots: [bool; MAX_AUTOCAST_SLOTS],
    hero: bool,
    /// The caster's kind, so the card can name its abilities. `None` for
    /// anything with no ability list.
    caster: Option<UnitKind>,
    /// Squad membership — the same handle `Ctrl+N` and the bridge's `squad`
    /// verb write, and the thing a posture is about.
    squad: Option<u8>,
}

impl UnitDoctrine {
    fn read(
        leash: Option<&LeashPolicy>,
        retreat: Option<&RetreatPolicy>,
        prio: Option<&TargetPriority>,
        autocast: Option<&AutoCastPolicy>,
        // The unit's kind, which is what decides whether it has anything to
        // auto-cast at all. Any caster counts, not just a hero — the
        // Sorcerer's whole doctrine is its auto-cast toggle.
        kind: UnitKind,
        squad: Option<&SquadId>,
    ) -> Self {
        let abilities = abilities_of_unit(kind);
        let mut autocast_slots = [false; MAX_AUTOCAST_SLOTS];
        for (i, flag) in autocast_slots.iter_mut().enumerate() {
            *flag = autocast.is_some_and(|p| p.min_enemies_for(i).is_some_and(|n| n > 0));
        }
        UnitDoctrine {
            leash: leash.map(|l| l.radius),
            retreat: retreat.map(|r| r.below_frac),
            prio: PrioPreset::of(prio.and_then(|p| p.0.first().copied())),
            autocast: autocast.is_some(),
            autocast_slots,
            hero: !abilities.is_empty(),
            caster: (!abilities.is_empty()).then_some(kind),
            squad: squad.map(|s| s.0),
        }
    }
}

/// Aggregate doctrine of the current own-unit selection. `command_input` and
/// `update_hud` each build one from the same entity-index-sorted list, so the
/// captions, the highlight and the executed toggle always agree.
#[derive(Clone, Copy, Default)]
struct DoctrineState {
    units: usize,
    /// Selected units that HAVE an ability — heroes and Sorcerers alike. Named
    /// for the only thing it decides: whether the [T Auto-Cast] toggle is on
    /// the card at all.
    heroes: usize,
    leashed: usize,
    leash_radius: f32,
    fallback: usize,
    fallback_frac: f32,
    autocast: usize,
    /// Casters carrying an auto-cast rule for each ability slot.
    autocast_slots: [usize; MAX_AUTOCAST_SLOTS],
    /// Kind of the FIRST selected caster — whose ability list names the
    /// per-ability toggles. A mixed Champion+Sorcerer selection therefore shows
    /// the Champion's spells; the intent it submits names a SLOT, which is what
    /// `AutoCastPolicy` stores, so the Sorcerer's slot 0 is set alongside the
    /// Champion's. Naming the slot after one caster's ability is a caption
    /// problem, not a correctness one, and the alternative (refusing to show
    /// the toggle for mixed selections) removes a control for no gain.
    caster: Option<UnitKind>,
    /// Preset of the FIRST selected unit (lowest entity index).
    prio: PrioPreset,
    /// Squad of the FIRST selected unit — the squad a posture gesture is about.
    squad: Option<u8>,
    /// How many of the selection are in that squad (the rest are elsewhere or
    /// nowhere, which is what makes a posture gesture also submit `squad`).
    in_squad: usize,
}

impl DoctrineState {
    /// `sorted` must be ordered by entity index — that fixes "the first unit".
    fn of(sorted: &[UnitDoctrine]) -> Self {
        let first_squad = sorted.iter().find_map(|u| u.squad);
        let mut s = DoctrineState {
            units: sorted.len(),
            prio: sorted.first().map(|u| u.prio).unwrap_or_default(),
            squad: first_squad,
            caster: sorted.iter().find_map(|u| u.caster),
            ..default()
        };
        for u in sorted {
            if u.hero {
                s.heroes += 1;
                for (slot, set) in u.autocast_slots.iter().enumerate() {
                    if *set {
                        s.autocast_slots[slot] += 1;
                    }
                }
            }
            if u.squad.is_some() && u.squad == first_squad {
                s.in_squad += 1;
            }
            if let Some(r) = u.leash {
                if s.leashed == 0 {
                    s.leash_radius = r;
                }
                s.leashed += 1;
            }
            if let Some(f) = u.retreat {
                if s.fallback == 0 {
                    s.fallback_frac = f;
                }
                s.fallback += 1;
            }
            if u.autocast {
                s.autocast += 1;
            }
        }
        s
    }
    /// "All or most of them carry it" — the button-highlight rule.
    fn most(count: usize, total: usize) -> bool {
        count > 0 && count * 2 >= total
    }
    fn guard_active(&self) -> bool {
        Self::most(self.leashed, self.units)
    }
    fn fallback_active(&self) -> bool {
        Self::most(self.fallback, self.units)
    }
    fn autocast_active(&self) -> bool {
        Self::most(self.autocast, self.heroes)
    }
    /// "Most of the selected casters auto-cast ability slot N."
    fn autocast_slot_active(&self, slot: usize) -> bool {
        Self::most(
            self.autocast_slots.get(slot).copied().unwrap_or(0),
            self.heroes,
        )
    }
    /// Radius the current selection is leashed at, for the [G] cycle. `None`
    /// means "no leash", which is the first rung.
    fn leash_value(&self) -> Option<f32> {
        (self.leashed > 0).then_some(self.leash_radius)
    }
    /// Threshold the current selection falls back at, for the [F] cycle.
    fn fallback_value(&self) -> Option<f32> {
        (self.fallback > 0).then_some(self.fallback_frac)
    }
    /// Compact panel line; empty when the selection carries no policy at all.
    /// A trailing `xN` marks a policy only part of the selection has.
    /// `posture` is the live `SquadOrders` entry for `self.squad`, rendered by
    /// the caller (which is the only place with the resource).
    fn line(&self, posture: Option<&str>) -> String {
        let tally = |count: usize, total: usize| {
            if count < total {
                format!(" x{}", count)
            } else {
                String::new()
            }
        };
        let mut parts: Vec<String> = Vec::new();
        // Squad first: it is the handle everything else on the doctrine card
        // hangs off, and the one thing the human could not name at all before.
        if let Some(squad) = self.squad {
            parts.push(format!(
                "squad {}{}",
                squad,
                tally(self.in_squad, self.units)
            ));
            if let Some(posture) = posture {
                parts.push(posture.to_string());
            }
        }
        if self.leashed > 0 {
            parts.push(format!(
                "guard({:.0}){}",
                self.leash_radius,
                tally(self.leashed, self.units)
            ));
        }
        if self.fallback > 0 {
            parts.push(format!(
                "fallback({:.0}%){}",
                self.fallback_frac * 100.0,
                tally(self.fallback, self.units)
            ));
        }
        if self.prio != PrioPreset::None {
            parts.push(format!("prio:{}", self.prio.tag()));
        }
        if self.autocast > 0 {
            parts.push(format!("autocast{}", tally(self.autocast, self.heroes)));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("Doctrine: {}", parts.join("   "))
        }
    }
}

/// Read every selected own unit's doctrine, sorted by entity index so both
/// consumers agree on which unit is "first".
fn sorted_doctrine(mut list: Vec<(u32, UnitDoctrine)>) -> Vec<UnitDoctrine> {
    list.sort_by_key(|(i, _)| *i);
    list.into_iter().map(|(_, d)| d).collect()
}

/// A live `SquadPosture`, in the panel's compact shorthand. The point is
/// spelled out because a posture *is* its point — "defend" alone tells the
/// player nothing about which ground they told the squad to hold.
/// **What reaching this selection costs**, for the info panel — the HUD's half
/// of the snapshot's `units[].link` and `units[].pending` (docs/TEMPO.md §4,
/// follow-up 7). Without a readout like this the mechanic is indistinguishable
/// from input lag, which is exactly why the feature ships default-off.
///
/// Tallied like [`why_line`] and for the same reason, but sorted **worst
/// first**: the number that decides whether to reach for a unit at all is the
/// slowest one in the selection, not the typical one.
///
/// `in_transit` carries the link each already-travelling order is paying. The
/// panel reports that, not the time remaining — the countdown belongs to the
/// closing ring on the ground ([`update_transit_markers`]), and a number that
/// ticks in the corner of the screen is a worse answer to "is my order lost?"
/// than a marker at the place the order is going.
fn link_line(mut links: Vec<f32>, in_transit: Vec<f32>) -> String {
    if links.is_empty() {
        return String::new();
    }
    links.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let mut tally: Vec<(String, usize)> = Vec::new();
    for l in links {
        let key = format!("{l:.1}s");
        match tally.last_mut() {
            Some((seen, n)) if *seen == key => *n += 1,
            _ => tally.push((key, 1)),
        }
    }
    let shown: Vec<String> = tally
        .iter()
        .take(2)
        .map(|(k, n)| if *n > 1 { format!("{k} x{n}") } else { k.clone() })
        .collect();
    let more = if tally.len() > 2 {
        format!("  (+{} more)", tally.len() - 2)
    } else {
        String::new()
    };
    let mut out = format!("Link: {}{more}", shown.join("   "));
    if !in_transit.is_empty() {
        let worst = in_transit.iter().copied().fold(0.0_f32, f32::max);
        out.push_str(&format!(
            "   ·   {} in transit ({worst:.1}s)",
            in_transit.len()
        ));
    }
    out
}

/// **How much of this army is inside its own chain of command**, for the top
/// bar. A standing fact rather than an alert, and the one number that tells a
/// player whether their next click will be answered at once.
///
/// Empty — and so invisible — whenever the mechanic is off, which is what makes
/// a flag-off match pixel-identical to v1.
fn coverage_line(on: bool, nodes: usize, covered: usize, total: usize) -> String {
    if !on {
        return String::new();
    }
    let plural = if nodes == 1 { "" } else { "s" };
    format!("Chain: {nodes} node{plural} · {covered}/{total} in reach")
}

/// The selection's answer to "why are you doing that?", for the info panel.
///
/// Every string here is byte-identical to what the same unit reports in the
/// bridge snapshot's `units[].why`. That is the whole point: introspection is
/// part of the decision surface, and a human who cannot ask what their army is
/// doing is not playing the same game as a commander who can read it.
///
/// One unit answers for itself. Several answer as a tally — "why is my army
/// doing that" is usually the question of whether it is doing ONE thing, so a
/// single line means the group is coherent and two mean it has split.
fn why_line(answers: Vec<String>) -> String {
    if answers.is_empty() {
        return String::new();
    }
    let mut answers = answers;
    answers.sort();
    let mut tally: Vec<(String, usize)> = Vec::new();
    for a in answers {
        match tally.last_mut() {
            Some((seen, n)) if *seen == a => *n += 1,
            _ => tally.push((a, 1)),
        }
    }
    if tally.len() == 1 {
        return format!("Why: {}", tally[0].0);
    }
    // Commonest first: the majority reason is the one that describes the army.
    tally.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let shown: Vec<String> = tally
        .iter()
        .take(2)
        .map(|(reason, n)| format!("{reason} x{n}"))
        .collect();
    match tally.len().saturating_sub(2) {
        0 => format!("Why: {}", shown.join("   ")),
        rest => format!("Why: {}   (+{rest} more)", shown.join("   ")),
    }
}

fn posture_tag(posture: &SquadPosture) -> String {
    match posture {
        SquadPosture::Defend { pos, radius } => {
            format!("defend({:.0},{:.0} r{:.0})", pos.x, pos.z, radius)
        }
        SquadPosture::Push { pos } => format!("push({:.0},{:.0})", pos.x, pos.z),
        SquadPosture::Escort { .. } => "escort".to_string(),
        SquadPosture::Forage { muster } => {
            format!("forage(muster {:.0},{:.0})", muster.x, muster.z)
        }
    }
}

/// Which button on the doctrine card a live posture corresponds to, so the
/// card can show what the squad is currently doing.
fn posture_kind(posture: &SquadPosture) -> PostureKind {
    match posture {
        SquadPosture::Defend { .. } => PostureKind::Defend,
        SquadPosture::Push { .. } => PostureKind::Push,
        SquadPosture::Escort { .. } => PostureKind::Escort,
        SquadPosture::Forage { .. } => PostureKind::Forage,
    }
}

/// The `DoctrineTemplate` of the one selected building, read out of
/// the ECS. `capable` is false when the selection is not a single own finished
/// building with a training queue — i.e. when intent.rs would reject a
/// `template` for it, so the card refuses to offer one.
#[derive(Clone, Copy, Default)]
struct TemplateView {
    capable: bool,
    squad: Option<u8>,
    retreat: Option<f32>,
    prio: PrioPreset,
    autocast: bool,
}

impl TemplateView {
    fn read(capable: bool, template: Option<&DoctrineTemplate>) -> Self {
        let t = template.cloned().unwrap_or_default();
        TemplateView {
            capable,
            squad: t.squad,
            retreat: t.retreat.map(|r| r.below_frac),
            prio: PrioPreset::of(t.priority.as_ref().and_then(|p| p.first().copied())),
            autocast: t.autocast.is_some_and(|n| n > 0),
        }
    }
    fn is_empty(&self) -> bool {
        self.squad.is_none()
            && self.retreat.is_none()
            && self.prio == PrioPreset::None
            && !self.autocast
    }
    /// Panel line for a selected building — the building-side twin of
    /// `DoctrineState::line`.
    fn line(&self) -> String {
        if !self.capable || self.is_empty() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(squad) = self.squad {
            parts.push(format!("squad {squad}"));
        }
        if let Some(frac) = self.retreat {
            parts.push(format!("fallback({:.0}%)", frac * 100.0));
        }
        if self.prio != PrioPreset::None {
            parts.push(format!("prio:{}", self.prio.tag()));
        }
        if self.autocast {
            parts.push("autocast".to_string());
        }
        format!("Trains with: {}", parts.join("   "))
    }
}

struct CmdEntry {
    action: CmdAction,
    /// The key that fires this tile, from `hotkeys::REGISTRY` and nowhere else.
    /// The tile's caption is derived from it at draw time by
    /// `hotkeys::key_caption`, so there is no second copy to drift.
    key: KeyCode,
    label: String,
    /// Small cost caption under the label ("135g", "40mp", ...).
    cost: String,
    /// Gold/lumber gate for the "can't afford" tint, when the action has one.
    afford: Option<(u32, u32)>,
    /// False = drawn dark (ability on cooldown or out of mana).
    enabled: bool,
    /// True = drawn highlighted like an armed Attack (a doctrine toggle that
    /// is currently switched on).
    active: bool,
    /// True = a tech requirement is unmet. The entry stays visible (so the
    /// player can see what unlocks it) but is drawn dark, its cost line reads
    /// "needs <Building>", and neither the hotkey nor the button does anything.
    locked: bool,
}

impl CmdEntry {
    fn plain(action: CmdAction, key: KeyCode, label: &str) -> Self {
        CmdEntry {
            action,
            key,
            label: label.to_string(),
            cost: String::new(),
            afford: None,
            enabled: true,
            active: false,
            locked: false,
        }
    }
    /// Mark a doctrine toggle as currently switched on.
    fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
    /// Tech gate: dark + inert while `completed` (the team's finished
    /// buildings) does not satisfy `reqs`. Replaces the cost caption with the
    /// first missing prerequisite so the card explains itself.
    fn requires(mut self, reqs: &[BuildingKind], completed: &[BuildingKind]) -> Self {
        if !requirements_met(reqs, completed.iter().copied()) {
            let missing = reqs
                .iter()
                // Tier-aware like `requirements_met`, so a card never reads
                // "needs Keep" to a player who is standing on a Castle.
                .find(|r| !completed.iter().any(|owned| building_satisfies(*owned, **r)))
                .copied()
                .unwrap_or(BuildingKind::TownHall);
            self.locked = true;
            self.enabled = false;
            self.cost = format!("needs {}", building_name(missing));
        }
        self
    }
    fn priced_as_building(self, kind: BuildingKind) -> Self {
        let s = building_stats(kind);
        self.priced(s.cost_gold, s.cost_lumber)
    }
    fn priced(mut self, gold: u32, lumber: u32) -> Self {
        // "0g" is a price tag that reads like a bug. Only one thing in the
        // game is actually free — your first hero of each class — and a button
        // that says so is the whole point of the change: the player who never
        // clicked the 400g hero has to be able to see, without doing any
        // arithmetic, that it now costs nothing.
        self.cost = if gold == 0 && lumber == 0 {
            "Free".to_string()
        } else if lumber > 0 {
            format!("{}g {}l", gold, lumber)
        } else {
            format!("{}g", gold)
        };
        self.afford = Some((gold, lumber));
        self
    }
}

/// One castable ability of the current selection, ready for the card.
#[derive(Clone, Copy)]
struct AbilitySlot {
    /// Slot in the caster's `abilities_of_*` list — the cast selector.
    index: usize,
    def: AbilityDef,
    /// Off cooldown and (heroes) affordable.
    ready: bool,
    /// Seconds of cooldown left, for the caption.
    cooldown: f32,
}

/// What the current selection can do about heroes, abilities and items.
#[derive(Clone, Default)]
struct HeroCmds {
    /// Hero training offered by the selected town hall.
    train: Option<HeroTrain>,
    /// The selected own hero's UNLOCKED abilities, in slot order.
    abilities: Vec<AbilitySlot>,
    /// The single selected own completed building's unlocked abilities.
    building_abilities: Vec<AbilitySlot>,
    /// The single selected own completed Shop's buy state.
    shop: Option<ShopState>,
    /// The single selected own completed building's available tier-up, when it
    /// has one and is not already converting: `(result kind, gold, lumber)`.
    upgrade: Option<(BuildingKind, u32, u32)>,
    /// Inventory of the selected own hero (all None when none is selected).
    items: [Option<ItemId>; 2],
    /// Research buttons for the single selected own completed forge, in ladder
    /// order. Empty for every other building.
    research: Vec<ResearchCmd>,
}

/// One research button's state. Everything the card needs to decide what the
/// button says and whether it does anything.
#[derive(Clone, Copy)]
struct ResearchCmd {
    kind: ResearchKind,
    /// Levels the TEAM already holds — not a property of this forge.
    level: u32,
    /// Cost and duration of the next rung; `None` at the cap.
    next: Option<ResearchStep>,
    /// Fraction complete, when THIS forge is working on THIS ladder.
    in_progress: Option<f32>,
    /// This forge is busy with the OTHER ladder. One job per forge: the answer
    /// to "I want both at once" is a second Blacksmith.
    blocked: bool,
}

/// The research buttons offered by the single selected building.
///
/// A free function taking plain values because `HeroCmds` is assembled twice —
/// once in `command_input` to dispatch a key press, once in `update_hud` to
/// draw the card — and a button whose enabled-ness is computed by two copies of
/// the same logic is a button that will eventually disagree with its own
/// hotkey. Every other field of `HeroCmds` is duplicated at both sites; this
/// one is not.
fn research_cmds(
    kind: BuildingKind,
    done: bool,
    levels: ResearchState,
    active: Option<&Researching>,
) -> Vec<ResearchCmd> {
    if !done {
        return Vec::new();
    }
    building_researches(kind)
        .iter()
        .map(|&k| ResearchCmd {
            kind: k,
            level: levels.level(k),
            next: levels.next_step(k),
            in_progress: active.filter(|a| a.kind == k).map(|a| {
                ((a.total - a.remaining) / a.total.max(0.001)).clamp(0.0, 1.0)
            }),
            blocked: active.is_some_and(|a| a.kind != k),
        })
        .collect()
}

/// Player-facing name for a research ladder — short, because it has to fit a
/// 52px tile. `ResearchKind::label` is the long form the event feed uses.
fn research_name(kind: ResearchKind) -> &'static str {
    match kind {
        ResearchKind::Attack => "Attack",
        ResearchKind::Armor => "Armor",
    }
}

/// The hall's hero button(s). Hero slots scale with the hall ladder
/// (`shared::hero_slots`: 1 at TownHall, 2 at Keep, 3 at Castle) and classes
/// must be distinct, so the card no longer offers "the" hero — it offers one
/// button per class and decides each independently:
///
///   * class already standing or already queued -> **hidden** (there is
///     nothing to buy);
///   * class dead but recorded -> shown as **"Revive"** at the revival price,
///     which is the only price a hero ever has;
///   * class never fielded -> shown at **"Free"**;
///   * ...and any of those is **greyed** when every slot is spoken for, so a
///     tier-1 player can SEE the Priestess they would get by teching up rather
///     than discovering her existence in the catalog.
#[derive(Clone, Default)]
struct HeroTrain {
    /// Hero classes alive on the field or sitting in a queue.
    held: Vec<UnitKind>,
    /// The team's live tier — what `hero_slot_check` reads the ceiling from.
    tier: TechTier,
    /// The same numbers, kept for the greyed button's "slots 1/2" caption.
    slots: u32,
    used: u32,
    /// Per class: `(kind, gold, lumber, is_revival)`.
    costs: Vec<(UnitKind, u32, u32, bool)>,
}

impl HeroTrain {
    /// `(gold, lumber, label, enabled)` for one hero class, or `None` when the
    /// button should not appear at all.
    fn offer(&self, kind: UnitKind) -> Option<(u32, u32, &'static str, bool)> {
        let verdict = hero_slot_check(&self.held, kind, self.tier);
        if verdict == HeroSlotVerdict::DuplicateClass {
            return None;
        }
        let (_, gold, lumber, revival) = *self.costs.iter().find(|(k, ..)| *k == kind)?;
        let label = if revival { "Revive" } else { unit_name(kind) };
        Some((gold, lumber, label, verdict == HeroSlotVerdict::Ok))
    }
}

/// Build the hero card state for one team from the world.
fn hero_train_state(
    records: &HeroRecords,
    tier: TechTier,
    held: Vec<UnitKind>,
) -> HeroTrain {
    HeroTrain {
        tier,
        slots: hero_slots(tier),
        used: held.len() as u32,
        costs: ALL_UNIT_KINDS
            .iter()
            .copied()
            .filter(|k| is_hero_kind(*k))
            .map(|k| {
                let (gold, lumber, _) = hero_train_cost(records, Team::Human, k);
                (k, gold, lumber, records.get(Team::Human, k).is_some())
            })
            .collect(),
        held,
    }
}

/// Why a Shop's buy buttons are (not) usable. The Shop sells to the team's
/// hero wherever it stands — WC3 sells to whoever walks up, we sell to the one
/// hero the team is allowed to have.
#[derive(Clone, Copy, Default)]
struct ShopState {
    /// There is a living hero to buy for.
    hero: bool,
    /// ...and THAT hero — the customer the card is drawn for, i.e. the last
    /// hero the player selected, not merely "the team's hero" — has a free
    /// inventory slot. With a Champion and a Priestess both alive, asking the
    /// wrong one would grey the button while the buyer had room, or offer it
    /// while the buyer was full.
    room: bool,
    /// The team's tech tier — the shelf is tiered, so a Shop built at T1 shows
    /// the T2 banner and the T3 scroll as locked rather than hiding them. A
    /// player has to be able to see what climbing the ladder buys.
    tier: TechTier,
}

/// Every placeable building, in card order, with the key that places it.
///
/// Both facts come from `hotkeys::REGISTRY`: the `Hk::Build(kind)` rows ARE the
/// card's left-to-right order, and the key beside each one is the binding. A new
/// `BuildingKind` therefore declares its position and its letter once, in one
/// table, and everything else — cost, name, tech gating — still comes from the
/// shared tables.
///
/// The old version of this function carried a thirty-line proof that [C] and
/// [M] were free on a worker's card because building-ability letters never
/// share a card with build buttons. The proof was right; it is now
/// `hotkeys::validate()`, which checks it for every card rather than for the
/// two the author thought of.
fn build_cards(race: Race) -> Vec<(BuildingKind, KeyCode)> {
    hotkeys::build_order(race)
}

/// Ability button caption: the ability's own name, plus the countdown while it
/// is cooling down. Works for hero and building casters alike.
fn ability_label(def: &AbilityDef, cooldown: f32) -> String {
    if cooldown > 0.0 {
        format!("{} {:.0}s", def.name, cooldown.ceil())
    } else {
        def.name.to_string()
    }
}

/// The read-only side of the command card, bundled so `command_input` keeps
/// headroom against Bevy's parameter ceiling (the same reason `CardActions`
/// existed before intent.rs took the writers). `squads` is here because the
/// doctrine page has to show what a squad is *currently* doing before it can
/// offer to change it, and `research`/`researching` because a forge's buttons
/// have to show what the TEAM has already bought before offering the next rung.
///
/// `update_hud` takes the whole bundle too, rather than `TechTiers`,
/// `SquadOrders` and `AbilityCooldowns` loose. That is two parameters cheaper
/// than spelling them out — which is what buys the room for the research reads
/// — and it removes a second, drifting copy of the same lookups: the card the
/// player SEES and the card the keyboard DISPATCHES against are now computed
/// from one set of facts.
#[derive(SystemParam)]
struct CastLookup<'w, 's> {
    tiers: Res<'w, TechTiers>,
    squads: Res<'w, SquadOrders>,
    /// The team's armed triggers. Here rather than as its own parameter
    /// because `command_input` and `update_hud` both sit on Bevy's
    /// 16-parameter ceiling and both already share this bundle — and because
    /// squads and triggers are the same kind of thing (standing policy the
    /// engine executes), read by the same two systems for the same reason.
    triggers: Res<'w, Triggers>,
    /// Rides along with `triggers` for the same reason and on the same rule:
    /// named ground is standing policy, read by the same two systems that read
    /// the other two kinds.
    regions: Res<'w, Regions>,
    /// And the sequenced kind, read by the same system for the same reason.
    /// Needs no clock: a plan's state is a fact it already carries, not a
    /// cooldown to be computed against now.
    plans: Res<'w, Plans>,
    /// Rides along with `triggers` because it is only ever read to answer a
    /// question about them: is this repeating rule still inside its cooldown?
    clock: Res<'w, Time>,
    cooldowns: Query<'w, 's, &'static AbilityCooldowns>,
    /// The team's completed research levels — what a research button reads to
    /// decide whether it is buyable, in progress, or already at the cap.
    research: Res<'w, TeamResearch>,
    /// Forges mid-job, looked up by entity like `cooldowns`.
    researching: Query<'w, 's, &'static Researching>,
    /// Who each team is playing. The build card is the player's OWN roster, so
    /// this is read once per frame and handed to `command_entries`. It rides in
    /// this bundle rather than as its own parameter for the reason the doc
    /// above gives: both consumers are on the 16-parameter ceiling.
    races: Res<'w, Races>,
}

/// Every UNLOCKED ability of a caster, priced and cooled, ready for the card.
/// The unlock test and the readiness test are shared.rs's, so a button is lit
/// exactly when combat.rs would honour the cast — no second opinion here.
fn ability_slots(
    list: &'static [AbilityDef],
    ctx: UnlockCtx,
    hero: Option<&Hero>,
    cooldowns: Option<&AbilityCooldowns>,
) -> Vec<AbilitySlot> {
    unlocked_abilities(list, ctx)
        .into_iter()
        .map(|index| AbilitySlot {
            index,
            def: list[index],
            ready: ability_ready(&list[index], hero, cooldowns, index),
            cooldown: cooldowns.map_or(0.0, |c| c.remaining(index)),
        })
        .collect()
}

/// The contextual command set for the current selection — the WHOLE of it, in
/// priority order and of any length. Both the keyboard and the command card
/// drive off this list, so a click and a key press run the exact same code path.
///
/// Layout per selection type (the card shows `CMD_SLOTS` = 4x3 at a time; see
/// `paginate` for what happens past that):
///   worker(s)            A S | B F H O L K N C M | I ‖ G V P
///   worker(s) + hero     ...the same, then R Y D, Z X, T on the overflow page
///   fighters             A S | G V P | I                    (6)
///   hero                 A S R | Z X (carried items) | G V P T | I  (<=12)
///   town hall            Q(Worker) W/E(hero class) C(CallToArms) U | I
///   barracks             Q(Footman) W(Archer) E(Raider) R(Spearman) T(Knight) | I  (6)
///   workshop             Q(Catapult) W(Gryphon Rider) | I     (3)
///   shop                 Q W E R T — the shelf, five rungs (`Hk::ShopSlot`)
///   blacksmith           Q(Attack) W(Armor)                  (2)
///
/// **Nothing is dropped any more.** This function used to end in a `truncate` to
/// `CMD_SLOTS` with a hand-written order of sacrifice — [P Priority] yields
/// first, then [V Fallback], then [G Guard] — because the card physically could
/// not show more than twelve tiles. That truncate once silently ate the eighth
/// build card, which is the worst possible way for a building to become
/// unbuildable, and the ninth building spent the last of the budget: a worker
/// card kept no quick toggle at all. Paging replaces the whole mechanism. The
/// list comes back complete and in priority order, `paginate` decides which
/// slice is on screen, and the overflow is one [Tab] away instead of gone.
///
/// The old priority order survives as ORDER, which is all it ever really was:
/// orders, then builds (never yield — a greyed [K Workshop] is how the player
/// learns what unlocks it, and it is the only route to a building at all), then
/// the hero's own buttons, then the quick doctrine toggles, which are the things
/// that land on page two when something has to. The page toggle is last and is
/// pinned to every page by `paginate`.
///
/// Abilities and items are generic: the hero button reads `ability_of_unit`, so
/// a Champion shows [R Slam 40mp] and a Priestess [R Heal 45mp] with no code
/// here naming either; the building button reads `ability_of_building`, which
/// only the TownHall answers today ([C CallToArms]).
fn command_entries(
    page: CardPage,
    // The selecting team's RACE — which build buttons a worker draws. It is a
    // parameter rather than a lookup because this whole function is pure and
    // the tests call it with no World; a race read from a resource here would
    // put the build card out of their reach.
    race: Race,
    own_units: usize,
    has_worker: bool,
    // (kind, completed) of the only selected building, when it is the whole selection.
    single_building: Option<(BuildingKind, bool)>,
    hero: HeroCmds,
    card: DoctrineCard,
    // Completed buildings the player owns — the tech gate for build entries.
    completed: &[BuildingKind],
) -> Vec<CmdEntry> {
    let doc = card.doc;
    if page == CardPage::Doctrine {
        return doctrine_entries(own_units, card);
    }
    let mut out: Vec<CmdEntry> = Vec::new();

    if own_units > 0 {
        out.push(CmdEntry::plain(
            CmdAction::AttackMove,
            bind(Hk::AttackMove),
            "Attack",
        ));
        out.push(CmdEntry::plain(CmdAction::Stop, bind(Hk::Stop), "Stop"));
    }

    // Builds first: they never yield (see above), so a worker selection always
    // shows the classic layout even when a hero got caught in the drag box.
    if has_worker {
        for (kind, key) in build_cards(race) {
            out.push(
                CmdEntry::plain(CmdAction::Place(kind), key, building_name(kind))
                    // ...after `priced`: an unmet requirement takes the cost line.
                    .priced_as_building(kind)
                    .requires(building_requires(kind), completed),
            );
        }
    }

    // The hero's abilities, whichever class it is and however many it has
    // unlocked: one button per slot, in slot order, on [R] [Y] [D].
    for slot in &hero.abilities {
        let Some(key) = hotkeys::key(Hk::HeroAbility(slot.index)) else {
            continue;
        };
        let mut entry = CmdEntry::plain(
            CmdAction::CastHero(slot.index),
            key,
            &ability_label(&slot.def, slot.cooldown),
        );
        // A mana-less caster (the Sorcerer pays only a cooldown) would
        // otherwise advertise "0mp", which reads as a bug rather than a
        // design. Its readiness is already in the label's cooldown timer.
        if slot.def.mana_cost > 0.0 {
            entry.cost = format!("{:.0}mp", slot.def.mana_cost);
        }
        // A targeted ability's cost line says what the key will ASK FOR,
        // exactly as a posture's does — the player has to learn "press, then
        // click" once, and then it is the same gesture everywhere. The reach
        // is on the line too, because the click that lands outside it is
        // refused rather than obeyed.
        if let Some(range) = slot.def.target.range() {
            entry.cost = format!(
                "{} <= {range:.0}",
                if slot.def.target.wants_unit() {
                    "click a unit"
                } else {
                    "click ground"
                }
            );
        }
        entry.enabled = slot.ready;
        out.push(entry);
    }

    // Carried consumables: one button per filled slot, so an empty inventory
    // costs the card nothing.
    for slot in 0..hero.items.len() {
        let (Some(key), Some(Some(item))) = (
            hotkeys::key(Hk::ItemSlot(slot)),
            hero.items.get(slot).copied(),
        ) else {
            continue;
        };
        out.push(CmdEntry::plain(
            CmdAction::UseSlot(slot),
            key,
            item_name(item),
        ));
    }

    if own_units == 0 {
        if let Some((kind, true)) = single_building {
            for (i, unit) in trainable(kind).iter().enumerate() {
                let Some(key) = hotkeys::key(Hk::TrainSlot(i)) else {
                    continue;
                };
                if is_hero_kind(*unit) {
                    // One button per hero class, each decided on its own —
                    // hidden while that hero is alive or queued, "Revive" once
                    // it has a record, greyed while every slot is taken.
                    let Some(train) = hero.train.as_ref() else {
                        continue;
                    };
                    let Some((gold, lumber, label, enabled)) = train.offer(*unit) else {
                        continue;
                    };
                    let mut entry = CmdEntry::plain(CmdAction::Train(*unit), key, label)
                        .priced(gold, lumber);
                    if !enabled {
                        entry.enabled = false;
                        entry.cost = format!("slots {}/{}", train.used, train.slots);
                    }
                    out.push(entry);
                } else {
                    let s = unit_stats(*unit);
                    out.push(
                        CmdEntry::plain(CmdAction::Train(*unit), key, unit_name(*unit))
                            .priced(s.cost_gold, s.cost_lumber)
                        // No unit has a tech requirement today; wiring it here
                        // means the first one that does is gated for free.
                        .requires(unit_requires(*unit), completed),
                    );
                }
            }

            // The building's own active abilities (TownHall: Call to Arms).
            for slot in &hero.building_abilities {
                let Some(key) = hotkeys::key(Hk::BuildingAbility(slot.index)) else {
                    continue;
                };
                let mut entry = CmdEntry::plain(
                    CmdAction::CastBuilding(slot.index),
                    key,
                    &ability_label(&slot.def, slot.cooldown),
                );
                entry.enabled = slot.ready;
                out.push(entry);
            }

            // Research. [Q]/[W] by ladder index, reusing the production
            // letters exactly as the Shop's buy buttons do: a Blacksmith trains
            // nothing, so Q and W are free on its card and the player's muscle
            // memory for "first button on a building" carries over intact.
            //
            // The button is inert in three different ways, and says which:
            // already at the cap, this forge working on this ladder, or this
            // forge working on the other one. All three read as dark tiles; the
            // cost caption is what distinguishes them, because a player who
            // pressed [Q] and got nothing deserves to be told why.
            for (i, r) in hero.research.iter().enumerate() {
                let Some(key) = hotkeys::key(Hk::TrainSlot(i)) else {
                    continue;
                };
                let mut entry = CmdEntry::plain(
                    CmdAction::Research(r.kind),
                    key,
                    // The level shown is the one this button BUYS, so the card
                    // reads as a purchase rather than as a status line.
                    &match (r.in_progress, r.next) {
                        (Some(_), _) => format!("{} {}", research_name(r.kind), r.level + 1),
                        (None, Some(step)) => {
                            format!("{} {}", research_name(r.kind), step.level)
                        }
                        (None, None) => {
                            format!("{} {}", research_name(r.kind), RESEARCH_MAX_LEVEL)
                        }
                    },
                );
                match (r.in_progress, r.next) {
                    (Some(frac), _) => {
                        entry.enabled = false;
                        entry.cost = format!("{:.0}%", frac * 100.0);
                    }
                    (None, None) => {
                        entry.enabled = false;
                        entry.cost = "maxed".to_string();
                    }
                    (None, Some(step)) => {
                        entry = entry.priced(step.cost_gold, step.cost_lumber);
                        if r.blocked {
                            entry.enabled = false;
                            entry.cost = "forge busy".to_string();
                        }
                    }
                }
                out.push(entry);
            }

            // Tier up in place, on [U] (`Hk::TierUp`) because it is the letter
            // that says what it does; the card has room here because a hall
            // spends at most four slots on training and Call to Arms.
            if let Some((to, gold, lumber)) = hero.upgrade {
                out.push(
                    CmdEntry::plain(
                        CmdAction::Upgrade(to),
                        bind(Hk::TierUp),
                        &format!("Upgrade: {}", building_name(to)),
                    )
                    .priced(gold, lumber),
                );
            }

            // A Shop sells to the team's one hero: dark without a hero, with a
            // full inventory, or with an empty purse.
            if let Some(shop) = hero.shop {
                for (i, item) in ALL_ITEMS.iter().enumerate() {
                    let Some(key) = hotkeys::key(Hk::ShopSlot(i)) else {
                        continue;
                    };
                    let def = item_def(*item);
                    let unlocked = item_unlocked(*item, shop.tier);
                    // A locked rung says WHAT IT COSTS in tech, not just that
                    // it is dark: "Banner T2" is a build order, "Banner"
                    // greyed out is a mystery.
                    let caption = if unlocked {
                        item_name(*item).to_string()
                    } else {
                        format!("{} {}", item_name(*item), def.tier.name())
                    };
                    let mut entry = CmdEntry::plain(CmdAction::Buy(*item), key, &caption)
                        .priced(def.cost_gold, 0);
                    entry.enabled = shop.hero && shop.room && unlocked;
                    out.push(entry);
                }
            }
        }
    }

    // --- doctrine toggles (appended last, so they are what pages) ----------
    //
    // They used to be DROPPED here, in a documented order of sacrifice, because
    // the card could not grow. Appending them last preserves the same priority —
    // a worker card still opens on the classic build layout, and the toggles are
    // what lands on the overflow page — without the part where a control
    // vanished. Every one of them also has a page-two equivalent behind [I],
    // which is why they were the right things to push down in the first place.
    if own_units > 0 {
        out.push(
            CmdEntry::plain(CmdAction::ToggleGuard, bind(Hk::QuickGuard), "Guard")
                .active(doc.guard_active()),
        );
        out.push(
            CmdEntry::plain(
                CmdAction::ToggleFallback,
                bind(Hk::QuickFallback),
                "Fallback",
            )
            .active(doc.fallback_active()),
        );
        out.push(
            CmdEntry::plain(
                CmdAction::CyclePriority,
                bind(Hk::QuickPriority),
                doc.prio.label(),
            )
            .active(doc.prio != PrioPreset::None),
        );
        if doc.heroes > 0 {
            out.push(
                CmdEntry::plain(
                    CmdAction::ToggleAutoCast,
                    bind(Hk::QuickAutoCast),
                    "Auto-Cast",
                )
                .active(doc.autocast_active()),
            );
        }
    }

    // The mode toggle, always last and never dropped: `paginate` pins it to the
    // final slot of EVERY page, so [I] is in the same place whatever the card is
    // showing. It is the only route to postures and templates.
    if own_units > 0 || card.tmpl.capable {
        out.push(CmdEntry::plain(
            CmdAction::TogglePage,
            bind(Hk::ModeToggle),
            "Doctrine",
        ));
    }
    out
}

/// The key bound to an action that MUST be bound.
///
/// Every call site here names a fixed action ("attack-move", "the mode
/// toggle"), not an indexed slot, so a missing binding is a broken registry
/// rather than a card that ran out of rungs — `hotkeys::validate` and
/// `no_action_is_registered_twice` both fail loudly first. The fallback keeps a
/// release build drawing a card instead of panicking mid-match; it is
/// unreachable in any tested configuration.
fn bind(action: Hk) -> KeyCode {
    match hotkeys::key(action) {
        Some(key) => key,
        None => {
            debug_assert!(false, "{action:?} has no binding in hotkeys::REGISTRY");
            KeyCode::Escape
        }
    }
}

/// One page of the command card: the tiles to draw, and how many pages there
/// are in total.
struct CardPageView<'a> {
    tiles: Vec<&'a CmdEntry>,
    /// Zero-based, already clamped into range.
    page: usize,
    /// Always at least 1.
    pages: usize,
}

/// Slice a card's entries into the `CMD_SLOTS` tiles that page `page` shows.
///
/// # Paging semantics
///
/// The card pages in two different senses, and they get deliberately different
/// rules. Both are validated (`hotkeys::validate`, and the systematic card test
/// in this file).
///
/// **Modes** — the orders card and the doctrine card, flipped with [I]. These
/// are different vocabularies for the same selection, so the MODE IS PART OF THE
/// CONTEXT and keys repeat across it on purpose: [G] is Guard on the orders card
/// and the leash radius on the doctrine card, [T] is Auto-Cast and Stand Down.
/// The mode toggle is pinned to the last slot of every page and the hint line
/// names the mode you are on, which is what makes the repeat legible.
///
/// **Overflow pages** — one vocabulary that ran out of tiles, walked with [Tab].
/// These are the SAME context, so every key across them is unique, and because
/// they are, *a hotkey stays live on every overflow page*: `command_input`
/// dispatches against the whole entry list, not against the visible slice. Only
/// the tiles move; the keyboard never does. That is the whole reason to prefer
/// overflow paging to the old order-of-sacrifice truncation — a player who has
/// learned [K Workshop] never has to know which page it is on.
///
/// The pinned mode toggle costs one slot per page, leaving eleven for content:
/// exactly the budget a nine-building worker card already spent (`A S` + nine
/// builds), so today's cards page identically to how they looked before.
fn paginate(entries: &[CmdEntry], page: usize) -> CardPageView<'_> {
    // The trailing mode toggle is pinned rather than paged. It is emitted last
    // by `command_entries`/`doctrine_entries` precisely so it can be peeled off
    // here without a second list.
    let pinned = entries
        .last()
        .filter(|e| e.action == CmdAction::TogglePage)
        .is_some();
    let content = &entries[..entries.len() - usize::from(pinned)];
    let per_page = CMD_SLOTS - usize::from(pinned);
    let pages = content.len().div_ceil(per_page).max(1);
    let page = page.min(pages - 1);
    let mut tiles: Vec<&CmdEntry> = content
        .iter()
        .skip(page * per_page)
        .take(per_page)
        .collect();
    if pinned {
        tiles.extend(entries.last());
    }
    CardPageView { tiles, page, pages }
}

/// The page indicator drawn under the card: empty while everything fits, so the
/// HUD says nothing about a mechanism the player is not currently using.
///
/// It names the MODE as well as the page number, because the mode is part of the
/// context (see `paginate`) and a player looking at [Q Defend] should be able to
/// see, without pressing anything, that they are on the doctrine card.
fn card_page_label(mode: CardPage, page: usize, pages: usize) -> String {
    if pages <= 1 {
        return String::new();
    }
    let name = match mode {
        CardPage::Orders => "Orders",
        CardPage::Doctrine => "Doctrine",
    };
    format!(
        "{name} {}/{}   [{}] more",
        page + 1,
        pages,
        hotkeys::key_caption(hotkeys::NEXT_CARD_PAGE)
    )
}

/// Everything page two draws itself from. Bundled so `command_entries` keeps
/// one argument for "the doctrine situation" instead of five.
#[derive(Clone, Copy, Default)]
struct DoctrineCard {
    doc: DoctrineState,
    /// Live posture of `doc.squad`, when that squad has an entry in
    /// `SquadOrders` — i.e. when doctrine.rs is actually executing something.
    posture: Option<PostureKind>,
    tmpl: TemplateView,
    /// Is the `home-guard` trigger armed right now? The tile is a toggle, so
    /// this is what decides whether pressing it arms or clears.
    home_guard: bool,
    /// The number of the next free `mark N`, or `None` when all
    /// `MAX_REGIONS_PER_TEAM` are taken. A number rather than the name so this
    /// struct stays `Copy` — `next_mark_name` spells it out at the two places
    /// that need the string.
    region_mark: Option<usize>,
    /// How many regions this team has named. Decides whether the clear tile is
    /// worth offering.
    region_count: usize,
    /// Radius the next mark gets, already defaulted.
    region_radius: f32,
    /// Is the marker armed, waiting for a ground click?
    region_armed: bool,
}

/// Page two: the doctrine card. This is the half of docs/TEMPO.md §2.0 that
/// the bridge had and the human did not — squads with postures, a retreat
/// threshold and a leash radius the player chooses rather than accepts, and a
/// production template. Every button submits an intent that a commander could
/// have typed, and the log cannot tell which happened.
///
///   units selected      Q Defend  W Push  E Forage | R Escort  T Stand Down
///                       F Fall back%  G Guard r  P Priority
///                       Z/X/C Auto <ability>, one per slot | I Orders  (<=12)
///   production building Q Squad  W Fall back%  E Priority  R Auto-cast
///                       T Clear | I Orders                              (6)
///
/// Every posture arms a click, exactly like building placement does:
/// Defend/Push/Forage want a point, Escort wants one of our own units.
/// `[-]/[=]` and `[[]/[]]` nudge the two numbers and are raw keys with no tile
/// — see `FALLBACK_NUDGE` for why they are keys rather than buttons.
fn doctrine_entries(own_units: usize, card: DoctrineCard) -> Vec<CmdEntry> {
    let doc = card.doc;
    let mut out: Vec<CmdEntry> = Vec::new();

    if own_units > 0 {
        for (kind, action) in [
            (PostureKind::Defend, Hk::PostureDefend),
            (PostureKind::Push, Hk::PosturePush),
            (PostureKind::Forage, Hk::PostureForage),
            (PostureKind::Escort, Hk::PostureEscort),
        ] {
            let mut entry =
                CmdEntry::plain(CmdAction::SetPosture(kind), bind(action), kind.label())
                    .active(card.posture == Some(kind));
            entry.cost = if kind.needs_unit() {
                "click a unit".to_string()
            } else {
                "click ground".to_string()
            };
            out.push(entry);
        }
        let mut stand_down =
            CmdEntry::plain(CmdAction::ClearPosture, bind(Hk::StandDown), "Stand Down");
        stand_down.enabled = card.posture.is_some();
        out.push(stand_down);

        // The parameterised pair. Captions name the CURRENT value, like [P]
        // does, so the card always reads as state rather than as a verb — and
        // the cost line names the nudge keys, which are the only doctrine
        // controls with no tile of their own.
        let fallback = doc.fallback_value();
        let mut fallback_entry = CmdEntry::plain(
            CmdAction::CycleFallback,
            bind(Hk::CycleFallback),
            &match fallback {
                // One decimal, not zero: `[-]`/`[=]` move in 5-point steps but
                // a commander can send 37.5, and a caption that rounded it
                // would show a number the unit is not actually using.
                Some(frac) => format!("Fall back {}%", trim_num(frac * 100.0)),
                None => "Fall back".to_string(),
            },
        )
        .active(fallback.is_some());
        fallback_entry.cost = "- / = tune".to_string();
        out.push(fallback_entry);

        let leash = doc.leash_value();
        let mut leash_entry = CmdEntry::plain(
            CmdAction::CycleLeash,
            bind(Hk::CycleLeash),
            &match leash {
                Some(r) => format!("Guard r{}", trim_num(r)),
                None => "Guard".to_string(),
            },
        )
        .active(leash.is_some());
        leash_entry.cost = "[ / ] tune".to_string();
        out.push(leash_entry);
        out.push(
            CmdEntry::plain(
                CmdAction::CyclePriority,
                bind(Hk::CyclePriority),
                doc.prio.label(),
            )
            .active(doc.prio != PrioPreset::None),
        );

        // One auto-cast toggle per ability the selected casters have. Page
        // one's [T] is the quick switch for slot 0 and stays exactly that; a
        // Champion who has learned Warcry can only be told to auto-cast it
        // here, because slot 0 is the only slot [T] can name.
        if let Some(caster) = doc.caster {
            for (slot, def) in abilities_of_unit(caster)
                .iter()
                .enumerate()
                .take(MAX_AUTOCAST_SLOTS)
            {
                let Some(key) = hotkeys::key(Hk::AutoCastSlot(slot)) else {
                    continue;
                };
                let mut entry = CmdEntry::plain(
                    CmdAction::ToggleAutoCastSlot(slot),
                    key,
                    &format!("Auto {}", def.name),
                )
                .active(doc.autocast_slot_active(slot));
                // The threshold is the rule: "auto-cast" with no number is a
                // caster that fires at one enemy, and this card has always
                // meant three.
                entry.cost = format!("{AUTOCAST_MIN_ENEMIES}+ foes");
                out.push(entry);
            }
        }

        // **The trigger preset.** One tile, and it is a toggle for the same
        // reason [G] Guard is one: the human's fast path is a switch, and the
        // parameterised form of the same statement lives on the wire and in
        // tools/intent_compile.py. Pressed, it arms `home-guard` — when any of
        // our buildings takes damage, this squad falls back and defends the
        // nearest hall. Pressed again, it clears it.
        //
        // This is a PRESET, not an authoring surface, and the asymmetry is
        // real and documented (docs/INTENT.md § Triggers): a commander can
        // write any of thirteen predicates against any of the 29 verbs, and the
        // human at the keyboard gets one canned rule plus a readout. What
        // closes most of the gap is that the English compiler speaks the same
        // sentences to the same wire.
        let mut guard = CmdEntry::plain(
            CmdAction::ToggleHomeGuard,
            bind(Hk::HomeGuard),
            "Home guard",
        )
        .active(card.home_guard);
        guard.cost = if card.home_guard { "armed".into() } else { "trigger".into() };
        out.push(guard);
    } else if card.tmpl.capable {
        let t = card.tmpl;
        out.push(
            CmdEntry::plain(
                CmdAction::TemplateSquad,
                bind(Hk::TemplateSquad),
                &match t.squad {
                    Some(id) => format!("Squad {id}"),
                    None => "Squad".to_string(),
                },
            )
            .active(t.squad.is_some()),
        );
        out.push(
            CmdEntry::plain(
                CmdAction::TemplateFallback,
                bind(Hk::TemplateFallback),
                &match t.retreat {
                    Some(frac) => format!("Fall back {:.0}%", frac * 100.0),
                    None => "Fall back".to_string(),
                },
            )
            .active(t.retreat.is_some()),
        );
        out.push(
            CmdEntry::plain(
                CmdAction::TemplatePriority,
                bind(Hk::TemplatePriority),
                t.prio.label(),
            )
            .active(t.prio != PrioPreset::None),
        );
        out.push(
            CmdEntry::plain(
                CmdAction::TemplateAutoCast,
                bind(Hk::TemplateAutoCast),
                "Auto-cast",
            )
            .active(t.autocast),
        );
        let mut clear = CmdEntry::plain(
            CmdAction::TemplateClear,
            bind(Hk::TemplateClear),
            "Clear Doctrine",
        );
        clear.enabled = !t.is_empty();
        out.push(clear);
    }

    // Pinned last, like the orders card's — `paginate` puts it in the final
    // slot of every page, so [I] is always the way back however deep the
    // doctrine card gets.
    // --- territory ---------------------------------------------------------
    //
    // Deliberately OUTSIDE the selection branches above: naming ground is about
    // the ground, not about who is standing on it, and a player who has to
    // select a footman before they may mark a ford would rightly ask why.
    //
    // These two are the human's whole authoring surface for regions, and unlike
    // the home-guard preset it is not a canned sentence — the player picks the
    // point and the radius, and only the NAME is chosen for them, because there
    // is no text entry anywhere in this HUD. `mark 3` is a poorer name than
    // `north-pass`, and it is a real name: it round-trips through the wire, the
    // snapshot and a co-commander's directive unchanged.
    let mut mark = CmdEntry::plain(
        CmdAction::MarkRegion,
        bind(Hk::MarkRegion),
        &match card.region_mark {
            Some(n) => format!("Mark {n} r{}", trim_num(card.region_radius)),
            None => "Mark region".to_string(),
        },
    )
    .active(card.region_armed);
    mark.enabled = card.region_mark.is_some();
    mark.cost = if card.region_mark.is_some() {
        "; / ' size".to_string()
    } else {
        format!("{MAX_REGIONS_PER_TEAM} named")
    };
    out.push(mark);

    let mut forget = CmdEntry::plain(
        CmdAction::ClearRegions,
        bind(Hk::ClearRegions),
        "Forget marks",
    );
    forget.enabled = card.region_count > 0;
    forget.cost = format!("{} named", card.region_count);
    out.push(forget);

    out.push(CmdEntry::plain(
        CmdAction::TogglePage,
        bind(Hk::ModeToggle),
        "Orders",
    ));
    out
}

/// The doctrine components each command-card toggle writes. Kept next to
/// `command_entries` so the button and its effect stay in step.
fn priority_component(preset: PrioPreset) -> Option<TargetPriority> {
    let classes = preset.classes();
    (!classes.is_empty()).then(|| TargetPriority(classes.to_vec()))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn unit_name(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Worker => "Worker",
        UnitKind::Footman => "Footman",
        UnitKind::Archer => "Archer",
        UnitKind::Hero => "Champion",
        UnitKind::Catapult => "Catapult",
        UnitKind::Raider => "Raider",
        UnitKind::Priestess => "Priestess",
        UnitKind::Spearman => "Spearman",
        UnitKind::Sorcerer => "Sorcerer",
        UnitKind::Knight => "Knight",
        UnitKind::GryphonRider => "Gryphon",
        UnitKind::Peon => "Peon",
        UnitKind::Grunt => "Grunt",
        UnitKind::Headhunter => "Headhunter",
        UnitKind::Wolfrider => "Wolfrider",
        UnitKind::Impaler => "Impaler",
        UnitKind::Demolisher => "Demolisher",
        UnitKind::Shaman => "Shaman",
        UnitKind::Warchief => "Warchief",
        UnitKind::FarSeer => "Far Seer",
        UnitKind::Wyvern => "Wyvern",
    }
}

/// Short, card-sized item name ("Potion" reads better on a 52px button than
/// the catalog id "HealingPotion").
fn item_name(id: ItemId) -> &'static str {
    match id {
        ItemId::HealingPotion => "Potion",
        ItemId::TownPortal => "Portal",
        ItemId::BootsOfSpeed => "Boots",
        ItemId::BannerOfCommand => "Banner",
        ItemId::ScrollOfMassTeleport => "MassTP",
    }
}

/// Display name (spaced, player-facing) — deliberately not
/// `shared::building_name`, which is the catalog id ("TownHall").
fn building_name(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::TownHall => "Town Hall",
        BuildingKind::Barracks => "Barracks",
        BuildingKind::Farm => "Farm",
        BuildingKind::Tower => "Tower",
        BuildingKind::Wall => "Wall",
        BuildingKind::Workshop => "Workshop",
        BuildingKind::Shop => "Shop",
        BuildingKind::Blacksmith => "Blacksmith",
        BuildingKind::Keep => "Keep",
        BuildingKind::Castle => "Castle",
        BuildingKind::Sanctum => "Sanctum",
        BuildingKind::Stronghold => "Stronghold",
        BuildingKind::WarCamp => "War Camp",
        BuildingKind::Burrow => "Burrow",
        BuildingKind::Watchtower => "Watchtower",
        BuildingKind::SpiritLodge => "Spirit Lodge",
        BuildingKind::WarMill => "War Mill",
        BuildingKind::Fortress => "Fortress",
        BuildingKind::Hold => "Hold",
    }
}

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

fn resource_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Gold => "Gold",
        ResourceKind::Lumber => "Lumber",
    }
}

fn dist_xz(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

/// Where the cursor sits on the horizontal plane a given unit occupies.
///
/// The camera looks down at a fixed pitch, so the y=0 point under the cursor
/// and the point at the unit's own height are offset by `height / tan(pitch)`
/// — over 4 world units for a flyer, against a pick radius of ~1.4. Testing an
/// airborne unit against the ground projection would make it impossible to
/// click, select, right-click or hover: a unit a bridge commander could order
/// and a human could not. Everything picks against its own plane instead,
/// which also quietly sharpens picking for ordinary ground units (whose body
/// centres are ~1.4 up, not at zero).
fn pick_point_for(ray: Option<Ray3d>, ground: Vec3, target_y: f32) -> Vec3 {
    ray.and_then(|r| ray_at_height(r, target_y)).unwrap_or(ground)
}

fn clamp_to_map(p: Vec3) -> Vec3 {
    Vec3::new(
        p.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
        0.0,
        p.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
    )
}

/// Snap a footprint centre so its edges land on nav-cell boundaries.
fn snap_footprint(p: Vec3, size: f32) -> Vec3 {
    let half = size * 0.5;
    let sx = ((p.x - half) / CELL).round() * CELL + half;
    let sz = ((p.z - half) / CELL).round() * CELL + half;
    Vec3::new(sx, 0.0, sz)
}

fn placement_valid(nav: &NavGrid, econ: &Economy, kind: BuildingKind, pos: Vec3) -> bool {
    let stats = building_stats(kind);
    nav.rect_is_free(pos, stats.size) && econ.can_afford(stats.cost_gold, stats.cost_lumber)
}

/// Cursor sitting on top of a HUD panel? Then it isn't a world click. The bar
/// and the console are fixed strips; the alert stack floats, so it only counts
/// while it actually has rows in it.
fn cursor_over_hud(cursor: Vec2, window: &Window, ui: &UiState, hud: &HudLayout) -> bool {
    if cursor.y < TOP_BAR_H || cursor.y > window.height() - hud.console_h {
        return true;
    }
    notif_rect(window, ui.notif_rows).is_some_and(|r| r.contains(cursor))
        || prop_rect(window, ui.prop_cards).is_some_and(|r| r.contains(cursor))
}

// --- Responsive console ----------------------------------------------------
//
// The console used to be 200 fixed pixels holding a 184px minimap with an 8px
// margin and a 1px border on each side. 184 + 16 + 2 is 202, so the minimap's
// bottom edge was two pixels below the console it lives in and `overflow:
// clip()` quietly ate them — the map's south edge and its border were missing
// at EVERY window size, and nobody noticed because a missing 2px border looks
// like a design choice.
//
// A tiling WM makes that worse rather than differently: it hands the game
// whatever the tile is, and a 200px console is a third of a 600px-tall window.
// So the console is no longer a number. It is **derived from what it has to
// hold**, and the two numbers below are the things that have to hold.

/// The minimap's chrome: `PAD` above and below, a 1px border each side. What
/// the console needs *beyond* the map itself.
const MINIMAP_CHROME: f32 = 2.0 * PAD + 2.0;
/// Never shrink the map past the point where a dot is a dot.
const MINIMAP_MIN_PX: f32 = 120.0;
/// …and never let it eat a narrow window's console. At 800 wide this is 224,
/// so it binds only well below the sizes this was tuned for.
const MINIMAP_MAX_W_FRAC: f32 = 0.28;
/// The height of the command card's own column: three rows of tiles, their
/// gaps, the overflow-page line, and the margins. This is a **floor** on the
/// console — the card's tiles are a fixed grid, so a console shorter than this
/// clips buttons, which is the one failure that costs you the game rather than
/// just looking wrong.
const CMD_PAGE_LINE_H: f32 = 14.0;
/// How much of a short window the console may claim before it starts giving
/// height back. A third is already a lot of a 600px screen.
const CONSOLE_MAX_H_FRAC: f32 = 0.34;

/// What the console and minimap measure at this window size.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
struct HudLayout {
    console_h: f32,
    minimap_px: f32,
}

impl Default for HudLayout {
    fn default() -> Self {
        // The full-size answer, so anything constructed before the first
        // `apply_hud_layout` (setup, a test app) is already consistent.
        hud_layout(1280.0, 800.0)
    }
}

/// The whole responsive rule, as one pure function of the window size — which
/// is what makes it testable without a compositor. Three clauses, in priority
/// order:
///
/// 1. The command card's grid never clips. It is a fixed number of fixed
///    tiles, and a half-drawn build menu is a broken game rather than an ugly
///    one, so its height is a hard floor on the console.
/// 2. Below that floor the console takes at most `CONSOLE_MAX_H_FRAC` of the
///    window, so a short tile keeps a battlefield.
/// 3. The minimap is square, fits inside whatever height clauses 1-2 left, and
///    is additionally capped by width and by a legibility minimum.
fn hud_layout(window_w: f32, window_h: f32) -> HudLayout {
    let card_h = 3.0 * CMD_PX + 2.0 * CMD_GAP + CMD_PAGE_LINE_H + 2.0 * PAD;
    let want = MINIMAP_PX + MINIMAP_CHROME;
    // `max(card_h)` twice over: once so a short window cannot squeeze below the
    // floor, once so the fraction cannot either.
    let console_h = want.min((window_h * CONSOLE_MAX_H_FRAC).max(card_h)).max(card_h);
    let minimap_px = (console_h - MINIMAP_CHROME)
        .min(window_w * MINIMAP_MAX_W_FRAC)
        .clamp(MINIMAP_MIN_PX, MINIMAP_PX);
    HudLayout {
        console_h,
        minimap_px,
    }
}

/// Recompute on resize and push the two numbers into the nodes that wear them.
///
/// Written every frame rather than on a resize event on purpose: the nodes are
/// only *touched* when a value actually changes (Bevy's change detection then
/// does the rest), and a resize event that arrives while the window is being
/// dragged between tiles is exactly the event most likely to be missed.
fn apply_hud_layout(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut hud: ResMut<HudLayout>,
    mut console: Query<&mut Node, (With<ConsoleRoot>, Without<MinimapRoot>, Without<MinimapFog>)>,
    mut minimap: Query<&mut Node, (With<MinimapRoot>, Without<MinimapFog>)>,
    mut minimap_fog: Query<&mut Node, With<MinimapFog>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let next = hud_layout(window.width(), window.height());
    if *hud == next {
        return;
    }
    *hud = next;
    for mut node in &mut console {
        node.height = Val::Px(next.console_h);
    }
    for mut node in &mut minimap {
        node.width = Val::Px(next.minimap_px);
        node.height = Val::Px(next.minimap_px);
    }
    // The fog layer is an absolutely-positioned child that must cover the map
    // exactly; it does not inherit a size it was given in pixels.
    for mut node in &mut minimap_fog {
        node.width = Val::Px(next.minimap_px);
        node.height = Val::Px(next.minimap_px);
    }
}

/// Screen-space rectangle of the proposal panel, or `None` when nothing is
/// pending. Analytic like `notif_rect`, and generous for the same reason — see
/// `PROP_CARD_HIT_H`. Top-LEFT, so it never overlaps the alert stack's rect and
/// the two hit tests stay independent.
fn prop_rect(window: &Window, cards: usize) -> Option<Rect> {
    if cards == 0 {
        return None;
    }
    let width = PROP_W.min(window.width() * PROP_MAX_FRAC);
    let top = TOP_BAR_H + PAD;
    Some(Rect::new(
        PAD,
        top,
        PAD + width,
        top + cards as f32 * (PROP_CARD_HIT_H + PROP_GAP),
    ))
}

/// Screen-space rectangle of the alert stack, or `None` when it is empty.
/// Analytic like `minimap_rect`: the stack is pinned to the top-right corner
/// under the resource bar, so its extent follows from the row count. Width
/// mirrors `notif_width` and height is deliberately generous — see
/// `NOTIF_ROW_HIT_H`.
fn notif_rect(window: &Window, rows: usize) -> Option<Rect> {
    if rows == 0 {
        return None;
    }
    let width = notif_width(window);
    let top = TOP_BAR_H + PAD;
    let left = (window.width() - PAD - width).max(0.0);
    Some(Rect::new(
        left,
        top,
        left + width,
        top + rows as f32 * NOTIF_ROW_HIT_H,
    ))
}

/// How wide the alert stack actually renders: `NOTIF_W`, unless the window is
/// too narrow to give it that much. Must agree with the `max_width` the stack
/// node carries, or the hit rect and the pixels drift apart.
fn notif_width(window: &Window) -> f32 {
    NOTIF_W.min(window.width() * NOTIF_MAX_FRAC)
}

/// Screen-space rectangle of the minimap. The console is a strip pinned to the
/// bottom of the window and the minimap sits at its top-left with a `PAD`
/// margin, so the rect is exact without touching layout internals — given the
/// same `HudLayout` the nodes were sized from.
fn minimap_rect(window: &Window, hud: &HudLayout) -> Rect {
    let top = window.height() - hud.console_h + PAD;
    Rect::new(PAD, top, PAD + hud.minimap_px, top + hud.minimap_px)
}

/// World XZ -> minimap pixel offset, for a map `px` on a side. +X is right,
/// +Z is up (matching the default camera view: the Human base ends up
/// bottom-left).
fn world_to_minimap(p: Vec3, px: f32) -> Vec2 {
    Vec2::new(
        (p.x + MAP_HALF) / (2.0 * MAP_HALF) * px,
        (MAP_HALF - p.z) / (2.0 * MAP_HALF) * px,
    )
}

/// Minimap pixel offset -> world XZ.
fn minimap_to_world(uv: Vec2, px: f32) -> Vec3 {
    Vec3::new(
        uv.x / px * 2.0 * MAP_HALF - MAP_HALF,
        0.0,
        MAP_HALF - uv.y / px * 2.0 * MAP_HALF,
    )
}

fn hp_color(frac: f32) -> Color {
    if frac > 0.5 {
        Color::srgb(0.30, 0.80, 0.35)
    } else if frac > 0.25 {
        Color::srgb(0.90, 0.76, 0.25)
    } else {
        Color::srgb(0.88, 0.25, 0.22)
    }
}

/// Can the player see that spot right now? The one question every picker and
/// every marker in this file asks before letting the interface acknowledge an
/// enemy. Always the Human grid — this file only ever renders for the human,
/// even when a script is driving that faction.
fn fog_sees(fog: &FogGrids, pos: Vec3) -> bool {
    fog.get(Team::Human).sees(pos)
}

/// Shift a colour toward white. A NEGATIVE amount darkens instead, which is
/// how remembered enemy structures are drawn — hence the clamp at both ends.
fn lighten(c: Color, amount: f32) -> Color {
    let s = c.to_srgba();
    Color::srgb(
        (s.red + amount).clamp(0.0, 1.0),
        (s.green + amount).clamp(0.0, 1.0),
        (s.blue + amount).clamp(0.0, 1.0),
    )
}

/// Replace (or extend) the selection.
fn apply_selection(
    commands: &mut Commands,
    currently_selected: &Query<Entity, With<Selected>>,
    new: &[Entity],
    additive: bool,
) {
    if !additive {
        for e in currently_selected.iter() {
            commands.entity(e).try_remove::<Selected>();
        }
    }
    for &e in new {
        commands.entity(e).try_insert(Selected);
    }
}

/// Issue a Move / AttackMove to a group with the usual formation spread.
// ---------------------------------------------------------------------------
// Speaking the shared language
//
// Every gesture below compiles to a `shared::Intent` and submits it. The UI no
// longer writes `Order`s, doctrine components, training queues, rally points
// or ability/upgrade events itself — intent.rs does, from the same values a
// bridge commander sends as JSON. What is left here is the *gesture*: deciding
// which units a right-click meant, which worker is nearest the build site,
// what "guard" implies as an anchor and a radius, which ability slot a hotkey
// names. That translation is the human interface's real job, and the sentence
// it produces is indistinguishable from the AI's.
// ---------------------------------------------------------------------------

/// Name a group of entities in the shared language.
fn ids(group: &[Entity]) -> Vec<IntentId> {
    group.iter().copied().map(intent_id).collect()
}

/// Submit one intent on behalf of the player at the keyboard.
fn say(submissions: &mut EventWriter<SubmitIntent>, intent: Intent) {
    submissions.write(SubmitIntent::ui(Team::Human, intent));
}

/// Move / AttackMove for a group. intent.rs applies the formation spread and
/// the map clamp, so a mouse drag and a bridge `move` land in the same shape.
fn ground_intent(
    submissions: &mut EventWriter<SubmitIntent>,
    group: &[Entity],
    ground: Vec3,
    attack_move: bool,
) {
    if group.is_empty() {
        return;
    }
    let units = ids(group);
    let (x, z) = (ground.x, ground.z);
    say(
        submissions,
        if attack_move {
            Intent::AttackMove {
                units,
                x: Some(x),
                z: Some(z),
                // A mouse click names ground, never a name. The region form is
                // for sentences; a gesture already knows exactly where it
                // pointed, and re-deriving a name for it would be the UI
                // guessing at what the player meant.
                region: None,
            }
        } else {
            Intent::Move {
                units,
                x: Some(x),
                z: Some(z),
                region: None,
            }
        },
    );
}

/// The second half of a posture gesture: an armed button plus the point the
/// player clicked becomes one `posture` sentence. A pure function so the click
/// handler and the tests take the same path, and so "what does clicking here
/// mean" is answerable without a window.
///
/// `None` for Escort, which names a unit rather than a point — see
/// `posture_unit_intent`, its twin for the other kind of click.
fn posture_intent(arm: PostureArm, ground: Vec3) -> Option<Intent> {
    let p = clamp_to_map(ground);
    let posture = match arm.kind {
        PostureKind::Defend => PostureIntent::Defend {
            x: Some(p.x),
            z: Some(p.z),
            region: None,
            radius: Some(DEFEND_RADIUS),
        },
        PostureKind::Push => PostureIntent::Push {
            x: Some(p.x),
            z: Some(p.z),
            region: None,
        },
        PostureKind::Forage => PostureIntent::Forage {
            x: Some(p.x),
            z: Some(p.z),
            region: None,
        },
        PostureKind::Escort => return None,
    };
    Some(Intent::Posture {
        id: arm.squad,
        posture: Some(posture),
    })
}

/// The unit-clicking half of the same gesture. `None` for every posture that
/// wants a point, so the two functions are total together and neither can be
/// called for the wrong kind of click by accident.
///
/// The caller is responsible for having picked one of OUR units: intent.rs
/// re-checks ownership anyway (`unit N not found/not yours`), but a UI that
/// let you click an enemy and then quietly did nothing would be worse than one
/// that never offered the click.
fn posture_unit_intent(arm: PostureArm, target: Entity) -> Option<Intent> {
    match arm.kind {
        PostureKind::Escort => Some(Intent::Posture {
            id: arm.squad,
            posture: Some(PostureIntent::Escort {
                unit: intent_id(target),
            }),
        }),
        _ => None,
    }
}

/// Pull the entities out of a `(Entity, UnitKind, carrying)` selection slice.
fn entities_of(sel: &[(Entity, UnitKind, bool)]) -> Vec<Entity> {
    sel.iter().map(|(e, _, _)| *e).collect()
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn text_bundle(s: &str, size: f32, color: Color, slot: Slot) -> impl Bundle {
    (
        Text::new(s),
        TextFont {
            font_size: size,
            ..default()
        },
        TextColor(color),
        slot,
    )
}

fn setup_ui(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // --- 3D helper assets -------------------------------------------------
    let ring_mesh = meshes.add(Torus::new(0.84, 1.0));
    // Same unit circle, drawn as a hairline: at a command node's radius the
    // 16%-of-radius band above would be a 5-unit-wide donut over the base.
    let hairline_mesh = meshes.add(Torus::new(0.985, 1.0));
    let node_ring_mat = materials.add(StandardMaterial {
        // Deliberately quiet: this is a standing fact about the map, not an
        // alert. It is on screen for the whole match, so it has to be
        // ignorable — docs/TEMPO.md §4 asks for feedback, not for decoration.
        base_color: Color::srgba(0.42, 0.72, 1.0, 0.30),
        emissive: LinearRgba::new(0.06, 0.16, 0.30, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    // The two region materials. Both hairlines, both unlit, both blended — what
    // separates them is hue and alpha, so "which of these circles did I draw?"
    // is answerable at a glance without a legend.
    let region_mine_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.78, 0.35, 0.50),
        emissive: LinearRgba::new(0.32, 0.22, 0.05, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let region_map_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.72, 0.78, 0.86, 0.16),
        emissive: LinearRgba::new(0.06, 0.07, 0.09, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let transit_mat = materials.add(StandardMaterial {
        // The rally flag's gold, on purpose: the player already reads that
        // colour as "somewhere I told something to go".
        base_color: Color::srgba(1.0, 0.84, 0.20, 0.55),
        emissive: LinearRgba::new(0.55, 0.42, 0.05, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let ring_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 1.0, 0.35),
        emissive: LinearRgba::new(0.1, 0.6, 0.15, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let ghost_ok = materials.add(StandardMaterial {
        base_color: Color::srgba(0.25, 1.0, 0.35, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let ghost_bad = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.25, 0.2, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(ghost_ok.clone()),
        Transform::from_xyz(0.0, -50.0, 0.0),
        Visibility::Hidden,
        Ghost,
    ));

    // --- pending-posture disc (one pooled entity, like the ghost) ----------
    let posture_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.42, 0.68, 1.0, 0.30),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    commands.spawn((
        // A unit-radius disc lying in the XZ plane, scaled per posture. Same
        // shape language as the selection ring, so "a circle on the ground"
        // keeps meaning "an area, not a thing".
        Mesh3d(meshes.add(Circle::new(1.0))),
        MeshMaterial3d(posture_mat.clone()),
        Transform::from_xyz(0.0, -50.0, 0.0)
            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        Visibility::Hidden,
        PostureMarker,
    ));

    // --- rally banner (one pooled entity: pole + pennant) -----------------
    let rally_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.84, 0.20),
        emissive: LinearRgba::new(0.55, 0.42, 0.05, 1.0),
        unlit: true,
        ..default()
    });
    let pole_mesh = meshes.add(Cylinder::new(0.11, 3.2));
    let flag_mesh = meshes.add(Cuboid::new(1.15, 0.66, 0.06));
    commands
        .spawn((
            RallyFlag,
            Transform::from_xyz(0.0, -50.0, 0.0),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((
                Mesh3d(pole_mesh),
                MeshMaterial3d(rally_mat.clone()),
                Transform::from_xyz(0.0, 1.6, 0.0),
            ));
            p.spawn((
                Mesh3d(flag_mesh),
                MeshMaterial3d(rally_mat),
                Transform::from_xyz(0.62, 2.75, 0.0)
                    .with_rotation(Quat::from_rotation_z(-0.14)),
            ));
        });

    commands.insert_resource(UiAssets {
        ring_mesh,
        hairline_mesh,
        ring_mat,
        ghost_ok,
        ghost_bad,
        node_ring_mat,
        region_mine_mat,
        region_map_mat,
        transit_mat,
    });

    // --- Top resource bar --------------------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Px(TOP_BAR_H),
                align_items: AlignItems::Center,
                column_gap: Val::Px(24.0),
                padding: UiRect::horizontal(Val::Px(14.0)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|p| {
            p.spawn(text_bundle(
                "Gold: 0   Lumber: 0",
                18.0,
                Color::srgb(1.0, 0.86, 0.35),
                Slot::Resources,
            ));
            p.spawn(text_bundle(
                "Supply: 0/0",
                18.0,
                Color::WHITE,
                Slot::Supply,
            ));
            // Node coverage. Empty — and therefore invisible, the bar being
            // left-packed — for every match played with the feature off.
            p.spawn(text_bundle(
                "",
                16.0,
                Color::srgb(0.55, 0.78, 1.0),
                Slot::Coverage,
            ));
        });

    // --- Bottom console ----------------------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                // Resized by `apply_hud_layout`; this is the full-size answer
                // so the first frame is already right.
                height: Val::Px(HudLayout::default().console_h),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                border: UiRect::top(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(CONSOLE_BG),
            BorderColor(EDGE),
            ConsoleRoot,
        ))
        .with_children(|console| {
            spawn_minimap(console);
            spawn_selection_panel(console);
            spawn_command_card(console);
        });

    // --- Rubber-band rectangle (hidden until dragging) ---------------------
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(0.0),
            height: Val::Px(0.0),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.35, 0.95, 0.45, 0.15)),
        BorderColor(Color::srgba(0.45, 1.0, 0.55, 0.85)),
        DragRect,
    ));

    // --- Game-over banner (empty text == invisible) ------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(10.0),
                ..default()
            },
            ZIndex(10),
        ))
        .with_children(|p| {
            p.spawn(text_bundle("", 72.0, Color::WHITE, Slot::Banner));
            p.spawn(text_bundle(
                "",
                26.0,
                Color::srgb(0.85, 0.85, 0.9),
                Slot::BannerSub,
            ));
        });

    spawn_notifications(&mut commands);
    spawn_proposals(&mut commands);
}

/// The co-commander's pending directives: a fixed pool of cards in the
/// top-left, hidden until a partner asks for something. Pooled and mutated in
/// place like every other refreshed node in this file.
fn spawn_proposals(commands: &mut Commands) {
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(PAD),
            top: Val::Px(TOP_BAR_H + PAD),
            width: Val::Px(PROP_W),
            max_width: Val::Percent(PROP_MAX_FRAC * 100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(PROP_GAP),
            ..default()
        })
        .with_children(|panel| {
            for i in 0..PROP_SLOTS {
                panel
                    .spawn((
                        Node {
                            width: Val::Percent(100.0),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(3.0),
                            padding: UiRect::axes(Val::Px(9.0), Val::Px(6.0)),
                            // The same severity spine the alert stack uses,
                            // in the co-commander's own colour.
                            border: UiRect::left(Val::Px(3.0)),
                            display: Display::None,
                            ..default()
                        },
                        BackgroundColor(PANEL_BG),
                        BorderColor(PROP_ACCENT),
                        PropCard(i),
                    ))
                    .with_children(|card| {
                        card.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: PROP_HEAD_FONT,
                                ..default()
                            },
                            TextColor(PROP_ACCENT),
                            PropText {
                                card: i,
                                part: PropPart::Head,
                            },
                        ));
                        card.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: PROP_FONT,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            PropText {
                                card: i,
                                part: PropPart::Note,
                            },
                        ));
                        card.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: PROP_FONT,
                                ..default()
                            },
                            TextColor(Color::srgb(0.72, 0.76, 0.86)),
                            PropText {
                                card: i,
                                part: PropPart::Body,
                            },
                        ));
                        card.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(6.0),
                            margin: UiRect::top(Val::Px(3.0)),
                            ..default()
                        })
                        .with_children(|btns| {
                            for approve in [true, false] {
                                btns.spawn((
                                    Button,
                                    Node {
                                        padding: UiRect::axes(Val::Px(9.0), Val::Px(3.0)),
                                        border: UiRect::all(Val::Px(1.0)),
                                        ..default()
                                    },
                                    BackgroundColor(SLOT_BG),
                                    BorderColor(if approve {
                                        PROP_ACCENT
                                    } else {
                                        EDGE
                                    }),
                                    PropBtn { card: i, approve },
                                ))
                                .with_children(|b| {
                                    b.spawn((
                                        Text::new(if approve {
                                            "Approve"
                                        } else {
                                            "Veto"
                                        }),
                                        TextFont {
                                            font_size: PROP_HEAD_FONT,
                                            ..default()
                                        },
                                        TextColor(if approve {
                                            PROP_ACCENT
                                        } else {
                                            Color::srgb(0.72, 0.74, 0.80)
                                        }),
                                    ));
                                });
                            }
                        });
                    });
            }
        });
}

/// The alert stack: a fixed pool of rows in the top-right corner, hidden until
/// something happens. Pooled and mutated in place like every other refreshed
/// node in this file — nothing here is ever spawned or despawned mid-match.
///
/// Top-right is the only large piece of screen the HUD does not already own:
/// the resource bar is a thin strip above it, the console and minimap are at
/// the bottom. It sits under the game-over banner's `ZIndex(10)` so a finished
/// match is never obscured by news about it.
fn spawn_notifications(commands: &mut Commands) {
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            right: Val::Px(PAD),
            top: Val::Px(TOP_BAR_H + PAD),
            width: Val::Px(NOTIF_W),
            // A tiling window manager will happily hand this game a window
            // narrower than the stack; without the cap the rows run off the
            // left edge and the messages lose their first few words.
            max_width: Val::Percent(NOTIF_MAX_FRAC * 100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            row_gap: Val::Px(NOTIF_GAP),
            ..default()
        })
        .with_children(|stack| {
            for i in 0..NOTIF_SLOTS {
                stack
                    .spawn((
                        Button,
                        Node {
                            width: Val::Percent(100.0),
                            min_height: Val::Px(NOTIF_ROW_H),
                            align_items: AlignItems::Center,
                            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
                            // A severity-coloured spine down the left edge: the
                            // part you read from the corner of your eye.
                            border: UiRect::left(Val::Px(3.0)),
                            display: Display::None,
                            ..default()
                        },
                        BackgroundColor(PANEL_BG),
                        BorderColor(EDGE),
                        NotifRow(i),
                    ))
                    .with_children(|row| {
                        row.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: NOTIF_FONT,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            NotifText(i),
                        ));
                    });
            }
            stack.spawn((
                Text::new(""),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.60, 0.70)),
                NotifHint,
            ));
        });
}

fn spawn_minimap(console: &mut ChildSpawnerCommands) {
    console
        .spawn((
            Node {
                width: Val::Px(HudLayout::default().minimap_px),
                height: Val::Px(HudLayout::default().minimap_px),
                margin: UiRect::all(Val::Px(PAD)),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(MINIMAP_BG),
            BorderColor(EDGE),
            MinimapRoot,
        ))
        .with_children(|m| {
            // Camera viewport outline (transparent, 1px white border).
            m.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(0.0),
                    height: Val::Px(0.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                BorderColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
                // Above the fog layer — where the camera is looking is never
                // hidden from the player.
                ZIndex(3),
                MinimapViewport,
            ));
        });
}

fn spawn_selection_panel(console: &mut ChildSpawnerCommands) {
    console
        .spawn(Node {
            flex_grow: 1.0,
            flex_basis: Val::Px(0.0),
            min_width: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            margin: UiRect::all(Val::Px(PAD)),
            row_gap: Val::Px(4.0),
            // This panel is the console's only *elastic* zone, so it is the
            // only one whose contents can be forced past their box: squeeze the
            // width and the idle hint wraps from two lines to nine, which at
            // 560px runs out of the bottom of the window. `hud_layout` cannot
            // help — the overflow is text reflow, not geometry — so the panel
            // owns its own edge. Nothing is lost that was ever visible: the
            // clip only bites once the line has already fallen off the screen.
            overflow: Overflow::clip(),
            ..default()
        })
        .with_children(|c| {
            // ---- single-entity pane ----
            c.spawn((
                Node {
                    display: Display::None,
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(10.0),
                    ..default()
                },
                El::SinglePane,
            ))
            .with_children(|s| {
                // Portrait tile.
                s.spawn((
                    Node {
                        width: Val::Px(64.0),
                        height: Val::Px(64.0),
                        flex_shrink: 0.0,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border: UiRect::all(Val::Px(2.0)),
                        ..default()
                    },
                    BackgroundColor(SLOT_BG),
                    BorderColor(EDGE),
                    El::Portrait,
                ))
                .with_children(|q| {
                    q.spawn(text_bundle("", 30.0, Color::WHITE, Slot::PortraitLetter));
                });

                // Details column.
                s.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(3.0),
                    flex_grow: 1.0,
                    min_width: Val::Px(0.0),
                    ..default()
                })
                .with_children(|d| {
                    d.spawn(text_bundle("", 20.0, Color::WHITE, Slot::Name));
                    d.spawn(text_bundle(
                        "",
                        14.0,
                        Color::srgb(0.82, 0.88, 0.95),
                        Slot::Hp,
                    ));
                    // HP bar.
                    d.spawn((
                        Node {
                            width: Val::Px(220.0),
                            height: Val::Px(10.0),
                            ..default()
                        },
                        BackgroundColor(BAR_BG),
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            BackgroundColor(hp_color(1.0)),
                            El::HpFill,
                        ));
                    });
                    // Hero-only XP (purple) + mana (blue) bars.
                    d.spawn((
                        Node {
                            display: Display::None,
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(3.0),
                            ..default()
                        },
                        El::HeroBars,
                    ))
                    .with_children(|h| {
                        for (el, color) in [
                            (El::XpFill, Color::srgb(0.62, 0.40, 0.92)),
                            (El::ManaFill, Color::srgb(0.30, 0.55, 0.95)),
                        ] {
                            h.spawn((
                                Node {
                                    width: Val::Px(220.0),
                                    height: Val::Px(7.0),
                                    ..default()
                                },
                                BackgroundColor(BAR_BG),
                            ))
                            .with_children(|b| {
                                b.spawn((
                                    Node {
                                        width: Val::Percent(0.0),
                                        height: Val::Percent(100.0),
                                        ..default()
                                    },
                                    BackgroundColor(color),
                                    el,
                                ));
                            });
                        }
                    });
                    d.spawn(text_bundle(
                        "",
                        13.0,
                        Color::srgb(0.74, 0.80, 0.90),
                        Slot::Stats,
                    ));
                    d.spawn(text_bundle(
                        "",
                        13.0,
                        Color::srgb(0.95, 0.85, 0.45),
                        Slot::Extra,
                    ));
                    // Hero inventory (Z / X use the two slots).
                    d.spawn(text_bundle(
                        "",
                        13.0,
                        Color::srgb(0.70, 0.90, 0.80),
                        Slot::Items,
                    ));
                    // Build / training progress bar.
                    d.spawn((
                        Node {
                            display: Display::None,
                            width: Val::Px(220.0),
                            height: Val::Px(9.0),
                            ..default()
                        },
                        BackgroundColor(BAR_BG),
                        El::ProgWrap,
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Node {
                                width: Val::Percent(0.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.35, 0.72, 0.95)),
                            El::ProgFill,
                        ));
                    });
                    // Queue tiles.
                    d.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(4.0),
                        ..default()
                    })
                    .with_children(|r| {
                        for i in 0..MAX_QUEUE {
                            r.spawn((
                                Button,
                                Node {
                                    display: Display::None,
                                    width: Val::Px(26.0),
                                    height: Val::Px(26.0),
                                    align_items: AlignItems::Center,
                                    justify_content: JustifyContent::Center,
                                    border: UiRect::all(Val::Px(1.0)),
                                    ..default()
                                },
                                BackgroundColor(SLOT_BG),
                                BorderColor(EDGE),
                                El::QueueTile(i),
                            ))
                            .with_children(|t| {
                                t.spawn(text_bundle(
                                    "",
                                    13.0,
                                    Color::WHITE,
                                    Slot::QueueLetter(i),
                                ));
                            });
                        }
                    });
                });
            });

            // ---- multi-selection cards ----
            c.spawn((
                Node {
                    display: Display::None,
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    align_content: AlignContent::FlexStart,
                    column_gap: Val::Px(CARD_GAP),
                    row_gap: Val::Px(CARD_GAP),
                    max_width: Val::Px(6.0 * (CARD_PX + CARD_GAP)),
                    ..default()
                },
                El::MultiPane,
            ))
            .with_children(|m| {
                for i in 0..MAX_CARDS {
                    m.spawn((
                        Button,
                        Node {
                            display: Display::None,
                            width: Val::Px(CARD_PX),
                            height: Val::Px(CARD_PX),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BackgroundColor(SLOT_BG),
                        BorderColor(EDGE),
                        El::Card(i),
                    ))
                    .with_children(|card| {
                        card.spawn(text_bundle("", 20.0, Color::WHITE, Slot::CardLetter(i)));
                        // Squad badge, top-right. Absolute so it sits over the
                        // centred initial instead of pushing it around, and the
                        // same blue the doctrine summary line uses — squad is
                        // one idea and it should have one colour.
                        card.spawn((
                            text_bundle(
                                "",
                                11.0,
                                Color::srgb(0.62, 0.80, 1.0),
                                Slot::CardSquad(i),
                            ),
                            Node {
                                position_type: PositionType::Absolute,
                                right: Val::Px(2.0),
                                top: Val::Px(0.0),
                                ..default()
                            },
                        ));
                        card.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                bottom: Val::Px(0.0),
                                width: Val::Percent(100.0),
                                height: Val::Px(3.0),
                                ..default()
                            },
                            BackgroundColor(hp_color(1.0)),
                            El::CardHp(i),
                        ));
                    });
                }
            });

            c.spawn(text_bundle(
                "",
                14.0,
                Color::srgb(0.80, 0.86, 0.95),
                Slot::Overflow,
            ));
            // Standing orders of the selection; empty (and so invisible) until
            // at least one policy is set. Lives outside the single/multi panes
            // so it shows for both.
            c.spawn(text_bundle(
                "",
                13.0,
                Color::srgb(0.62, 0.80, 1.0),
                Slot::Doctrine,
            ));
            // "Why are you doing that?", under the standing orders that are
            // usually the answer. Dimmer than the doctrine line because it is
            // a readout of the engine's reasoning, not a setting to change.
            c.spawn(text_bundle(
                "",
                12.0,
                Color::srgb(0.70, 0.70, 0.78),
                Slot::Why,
            ));
            // Directly under `Why`, and for the same reason it sits there: a
            // unit's reason and the cost of changing it are one thought.
            c.spawn(text_bundle(
                "",
                12.0,
                Color::srgb(0.55, 0.78, 1.0),
                Slot::Link,
            ));
            // The team's armed rules. Below the selection's own lines because
            // it is the one entry in this panel that is not about the
            // selection: a trigger belongs to the faction, and it stays on
            // screen when nothing at all is selected.
            c.spawn(text_bundle(
                "",
                12.0,
                Color::srgb(0.85, 0.72, 0.40),
                Slot::Triggers,
            ));
            // Directly beneath the rules, in the same colour: both are standing
            // policy the engine runs without anybody watching, and a player
            // scanning for "what is the game doing on my behalf" should find
            // them as one block rather than two.
            c.spawn(text_bundle(
                "",
                12.0,
                Color::srgb(0.85, 0.72, 0.40),
                Slot::Plans,
            ));
            // Then the ground those two are written against. Amber — the
            // colour the marks are drawn in on both maps, so the readout and
            // the circle are visibly the same object.
            c.spawn(text_bundle(
                "",
                12.0,
                Color::srgb(1.0, 0.78, 0.35),
                Slot::Regions,
            ));
            // What we know of THEIR heroes. Same argument as the trigger line
            // — it belongs to the faction rather than to the selection — and
            // the same self-hiding empty string. Amber rather than the
            // trigger line's gold: this is the one line in the panel that is
            // about the opponent, and it should not read as something of ours.
            c.spawn(text_bundle(
                "",
                12.0,
                Color::srgb(0.88, 0.60, 0.42),
                Slot::EnemyHeroes,
            ));
            c.spawn(text_bundle(
                "Left-click / drag to select.",
                13.0,
                Color::srgb(0.62, 0.92, 0.68),
                Slot::Hints,
            ));
        });
}

fn spawn_command_card(console: &mut ChildSpawnerCommands) {
    // A column: the 4x3 grid, then the overflow-page indicator under it. The
    // indicator is a text line rather than a thirteenth tile because a tile
    // would cost a slot on every card, including the eleven-in-twelve that never
    // page — and the whole reason paging exists is that slots are the scarce
    // thing. Three 52px rows plus gaps and the two PAD margins spend 184px, and
    // an 11px line fits in the `CMD_PAGE_LINE_H` budgeted after it — which is
    // exactly the sum `hud_layout` uses as the console's floor.
    console
        .spawn(Node {
            width: Val::Px(CMD_COLS * CMD_PX + (CMD_COLS - 1.0) * CMD_GAP),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            margin: UiRect::all(Val::Px(PAD)),
            ..default()
        })
        .with_children(|col| {
            spawn_command_grid(col);
            col.spawn(text_bundle(
                "",
                11.0,
                // The doctrine blue: paging is a fact about the card, in the
                // same voice the card's other state lines use.
                Color::srgb(0.62, 0.80, 1.0),
                Slot::CmdPage,
            ));
        });
}

fn spawn_command_grid(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            width: Val::Px(CMD_COLS * CMD_PX + (CMD_COLS - 1.0) * CMD_GAP),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_content: AlignContent::FlexStart,
            column_gap: Val::Px(CMD_GAP),
            row_gap: Val::Px(CMD_GAP),
            ..default()
        })
        .with_children(|g| {
            for i in 0..CMD_SLOTS {
                g.spawn((
                    Button,
                    Node {
                        display: Display::None,
                        width: Val::Px(CMD_PX),
                        height: Val::Px(CMD_PX),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(SLOT_BG),
                    BorderColor(EDGE),
                    El::CmdBtn(i),
                ))
                .with_children(|b| {
                    b.spawn(text_bundle("", 17.0, Color::WHITE, Slot::CmdKey(i)));
                    b.spawn(text_bundle(
                        "",
                        9.0,
                        Color::srgb(0.85, 0.90, 0.98),
                        Slot::CmdLabel(i),
                    ));
                    b.spawn(text_bundle(
                        "",
                        9.0,
                        Color::srgb(1.0, 0.86, 0.35),
                        Slot::CmdCost(i),
                    ));
                });
            }
        });
}

// ---------------------------------------------------------------------------
// Command input: hotkeys AND command-card buttons (identical code paths)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn command_input(
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    keys: Res<ButtonInput<KeyCode>>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    game_over: Res<GameOver>,
    cast: CastLookup,
    mut focus: EventWriter<CameraFocus>,
    mut submissions: EventWriter<SubmitIntent>,
    pressed_buttons: Query<(&Interaction, &El), Changed<Interaction>>,
    selected: Query<Entity, With<Selected>>,
    sel_units: Query<
        (
            Entity,
            &Unit,
            &Team,
            &Transform,
            Option<&Hero>,
            Option<&LeashPolicy>,
            Option<&RetreatPolicy>,
            Option<&TargetPriority>,
            Option<&AutoCastPolicy>,
            Option<&Inventory>,
            Option<&SquadId>,
        ),
        With<Selected>,
    >,
    // Read-only now: the training queue is pushed by intent.rs, never here.
    sel_buildings: Query<
        (
            Entity,
            &Building,
            &Team,
            Option<&TrainingQueue>,
            Option<&UnderConstruction>,
            // Per-ability cooldowns are read through `CastLookup` by entity, so
            // this query stays free of them.
            Option<&Upgrading>,
            Option<&DoctrineTemplate>,
        ),
        With<Selected>,
    >,
    // Read-only: the team's heroes (anywhere on the map) are the Shop's
    // customers, Escort's target, and — with `SquadId` — the census that tells
    // a posture gesture which squad ids are already spoken for.
    all_units: Query<(
        Entity,
        &Unit,
        &Team,
        &Order,
        &Transform,
        Option<&Inventory>,
        Option<&SquadId>,
    )>,
    // Read-only: the fallback rally looks for the nearest own town hall, and
    // the hero-slot tally has to see EVERY own queue, not just the selection.
    all_buildings: Query<(
        &Building,
        &Team,
        &Transform,
        Has<UnderConstruction>,
        Option<&TrainingQueue>,
    )>,
) {
    if game_over.winner.is_some() {
        return;
    }

    // Escape cancels every transient mode, innermost first, and finally backs
    // out of the doctrine page — so Escape always means "one step out".
    if keys.just_pressed(hotkeys::CANCEL) {
        // Innermost first, and a hall-pick is the innermost thing there is: it
        // is armed from a card that is itself only reachable with a hero
        // selected, and backing out of it must not also drop the hero.
        if ui.teleport_place.is_some() {
            ui.teleport_place = None;
        } else if ui.cast_place.is_some() {
            ui.cast_place = None;
        } else if ui.placement.is_some() {
            ui.placement = None;
            ui.wall_chain.clear();
        } else if ui.posture_place.is_some() {
            ui.posture_place = None;
        } else if ui.region_place {
            ui.region_place = false;
        } else if ui.attack_move_armed {
            ui.attack_move_armed = false;
        } else {
            ui.page = CardPage::Orders;
            // ...and back to the first page of it: "one step out" should not
            // leave the player looking at an overflow page of a card they just
            // asked to leave.
            ui.card_page = 0;
        }
        ui.dragging = false;
        ui.drag_start = None;
        return;
    }

    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    // --- idle worker cycling (not a command-card entry) -------------------
    if !ctrl && keys.just_pressed(hotkeys::IDLE_WORKER) {
        let idle: Vec<(Entity, Vec3)> = all_units
            .iter()
            .filter(|(_, u, t, o, _, _, _)| {
                **t == Team::Human && is_worker_kind(u.kind) && matches!(o, Order::Idle)
            })
            .map(|(e, _, _, _, tf, _, _)| (e, tf.translation))
            .collect();
        if !idle.is_empty() {
            let (pick, pos) = idle[ui.idle_cursor % idle.len()];
            ui.idle_cursor = ui.idle_cursor.wrapping_add(1);
            apply_selection(&mut commands, &selected, &[pick], false);
            focus.write(CameraFocus { pos });
            return;
        }
    }

    // --- what can this selection do? --------------------------------------
    let own_units: Vec<(Entity, Vec3)> = sel_units
        .iter()
        .filter(|(_, _, t, _, _, _, _, _, _, _, _)| **t == Team::Human)
        .map(|(e, _, _, tf, _, _, _, _, _, _, _)| (e, tf.translation))
        .collect();
    // The same selection, named the way the shared language names units.
    let own_ids = || -> Vec<IntentId> { own_units.iter().map(|(e, _)| intent_id(*e)).collect() };
    let has_worker = sel_units
        .iter()
        .any(|(_, u, t, ..)| *t == Team::Human && is_worker_kind(u.kind));
    // Every selected own CASTER — anything whose kind has an ability list.
    // Heroes qualify through their tables like everything else, and so does
    // the Sorcerer, which carries no `Hero` component at all.
    let own_casters: Vec<(Entity, UnitKind, Option<Hero>, Inventory)> = sel_units
        .iter()
        .filter(|(_, u, t, ..)| **t == Team::Human && !abilities_of_unit(u.kind).is_empty())
        .map(|(e, u, _, _, h, _, _, _, _, inv, _)| {
            (e, u.kind, h.copied(), inv.copied().unwrap_or_default())
        })
        .collect();
    // Doctrine of the own-unit selection, in a stable order.
    let doc = DoctrineState::of(&sorted_doctrine(
        sel_units
            .iter()
            .filter(|(_, _, t, ..)| **t == Team::Human)
            .map(|(e, u, _, _, _, leash, retreat, prio, autocast, _, squad)| {
                (
                    e.index(),
                    UnitDoctrine::read(
                        leash,
                        retreat,
                        prio,
                        autocast,
                        u.kind,
                        squad,
                    ),
                )
            })
            .collect(),
    ));

    let mut b_iter = sel_buildings.iter();
    // The one selected own building: its kind, whether it is finished, its
    // entity (buy/cast target) and its ability cooldown, if it has one.
    let single = match (b_iter.next(), b_iter.next()) {
        (Some((e, b, t, _, uc, up, _)), None) if *t == Team::Human => {
            Some((e, b.kind, uc.is_none(), up.is_some()))
        }
        _ => None,
    };
    let single_building = single.map(|(_, kind, done, _)| (kind, done));
    // The template side of the doctrine card. intent.rs accepts a `template`
    // only for an own, finished, unit-producing building, so the card offers
    // one under exactly that condition and never logs a rejected intent.
    let mut t_iter = sel_buildings.iter();
    let single_template = match (t_iter.next(), t_iter.next()) {
        (Some((_, b, t, queue, uc, _, tmpl)), None) => TemplateView::read(
            *t == Team::Human
                && uc.is_none()
                && queue.is_some()
                && !trainable(b.kind).is_empty(),
            tmpl,
        ),
        _ => TemplateView::default(),
    };

    // Every hero class this team is holding: alive anywhere on the map, or
    // sitting in ANY of its queues (not just the selected building's — two
    // halls each queuing a Priestess is exactly the case the slot rule is
    // for). Same tally the bridge snapshot and economy.rs's pay-point compute.
    let mut held_heroes: Vec<UnitKind> = all_units
        .iter()
        .filter(|(_, u, t, ..)| **t == Team::Human && is_hero_kind(u.kind))
        .map(|(_, u, ..)| u.kind)
        .collect();
    for (_, team, _, _, queue) in all_buildings.iter() {
        if *team != Team::Human {
            continue;
        }
        held_heroes.extend(
            queue
                .into_iter()
                .flat_map(|q| q.queue.iter().copied())
                .filter(|k| is_hero_kind(*k)),
        );
    }
    // The team's hero wherever it stands — the Shop's default customer and the
    // Escort posture's target. With two heroes possible this is the first one
    // found; `buy`/`use_item` name a hero explicitly when the player has one
    // SELECTED (see `shop_customer`).
    // The team's heroes by entity id, so "the default hero" is the same stable
    // lowest-id tie-break intent.rs documents rather than query order.
    let mut own_hero_list: Vec<(Entity, Inventory)> = all_units
        .iter()
        .filter(|(_, u, t, ..)| **t == Team::Human && is_hero_kind(u.kind))
        .map(|(e, _, _, _, _, inv, _)| (e, inv.copied().unwrap_or_default()))
        .collect();
    own_hero_list.sort_by_key(|(e, _)| *e);
    let team_hero: Option<(Entity, Inventory)> = own_hero_list.first().copied();
    // Remember the hero the player is looking at, and forget one that died.
    if let Some((e, ..)) = own_casters.iter().find(|(_, k, ..)| is_hero_kind(*k)) {
        ui.last_hero = Some(*e);
    }
    if ui.last_hero.is_some_and(|e| !own_hero_list.iter().any(|(h, _)| *h == e)) {
        ui.last_hero = None;
    }
    // Who a Shop sells to: the last hero the player selected, else the default.
    let shop_customer: Option<(Entity, Inventory)> = ui
        .last_hero
        .and_then(|e| own_hero_list.iter().find(|(h, _)| *h == e).copied())
        .or(team_hero);
    let hero_train = hero_train_state(&records, cast.tiers.get(Team::Human), held_heroes);
    let hero_cmds = HeroCmds {
        train: Some(hero_train.clone()),
        abilities: own_casters
            .first()
            .map(|(entity, kind, hero, _)| {
                ability_slots(
                    abilities_of_unit(*kind),
                    UnlockCtx::new(hero.map_or(0, |h| h.level), cast.tiers.get(Team::Human)),
                    hero.as_ref(),
                    cast.cooldowns.get(*entity).ok(),
                )
            })
            .unwrap_or_default(),
        building_abilities: single
            .filter(|(_, _, done, _)| *done)
            .map(|(entity, kind, _, _)| {
                ability_slots(
                    abilities_of_building(kind),
                    UnlockCtx::building(cast.tiers.get(Team::Human)),
                    None,
                    cast.cooldowns.get(entity).ok(),
                )
            })
            .unwrap_or_default(),
        shop: single.and_then(|(_, kind, done, _)| {
            (done && kind == BuildingKind::Shop).then(|| ShopState {
                // Room is asked of the hero who will actually receive the item.
                hero: shop_customer.is_some(),
                room: shop_customer.is_some_and(|(_, inv)| inv.0.iter().any(|s| s.is_none())),
                tier: cast.tiers.get(Team::Human),
            })
        }),
        upgrade: single.and_then(|(_, kind, done, upgrading)| {
            (done && !upgrading)
                .then(|| upgrade_cost(kind).zip(building_upgrades_to(kind)))
                .flatten()
                .map(|((gold, lumber, _), to)| (to, gold, lumber))
        }),
        items: own_casters.first().map(|(_, _, _, inv)| inv.0).unwrap_or_default(),
        research: single
            .map(|(entity, kind, done, _)| {
                research_cmds(
                    kind,
                    done,
                    cast.research.get(Team::Human),
                    cast.researching.get(entity).ok(),
                )
            })
            .unwrap_or_default(),
    };

    // Completed own buildings = the tech state every build entry is gated on.
    let completed: Vec<BuildingKind> = all_buildings
        .iter()
        .filter(|(_, t, _, under, _)| **t == Team::Human && !under)
        .map(|(b, _, _, _, _)| b.kind)
        .collect();

    // Which squad the doctrine page is about, and what it is already doing.
    let card = DoctrineCard {
        doc,
        posture: doc
            .squad
            .and_then(|s| cast.squads.0.get(&(Team::Human, s)))
            .map(posture_kind),
        tmpl: single_template,
        home_guard: has_trigger(&cast.triggers, HOME_GUARD),
        region_mark: mark_number(&cast.regions),
        region_count: cast.regions.get(Team::Human).len(),
        region_radius: ui.region_radius.unwrap_or(REGION_MARK_RADIUS),
        region_armed: ui.region_place,
    };
    let race = cast.races.get(Team::Human);
    let entries = command_entries(
        ui.page,
        race,
        own_units.len(),
        has_worker,
        single_building,
        hero_cmds,
        card,
        &completed,
    );

    // --- collect this frame's commands ------------------------------------
    //
    // Dispatched against the WHOLE entry list, not the visible page. Overflow
    // pages are one vocabulary split across two screens (see `paginate`), so a
    // key means the same thing on every one of them and works from any of them:
    // a player who has learned [K Workshop] never has to know which page the
    // tile is on. Only the tiles page; the keyboard does not.
    let mut actions: Vec<CmdAction> = Vec::new();
    if !ctrl {
        for entry in &entries {
            if entry.locked {
                continue;
            }
            if keys.just_pressed(entry.key) {
                actions.push(entry.action);
            }
        }
    }
    // [Tab] walks the overflow pages of the current mode. A raw key with no
    // tile, for the same reason the nudges are: it navigates the menu rather
    // than being an item on it, and the indicator under the card names it.
    if !ctrl && keys.just_pressed(hotkeys::NEXT_CARD_PAGE) {
        let pages = paginate(&entries, 0).pages;
        if pages > 1 {
            ui.card_page = (ui.card_page + 1) % pages;
        }
    }
    // [I] is a raw hotkey as well as a button, and stays one: the mode toggle
    // is now pinned to every page so the tile is always there, but the doctrine
    // page is the only route to postures and templates and a route that depends
    // on a tile being drawn is one a future card can close.
    //
    // The old gate here was `a selection, or a production building` — because
    // every tile on page two was about one or the other. Territory broke that:
    // `Mark region` and `Forget marks` are about the GROUND, and they are the
    // human's only authoring surface for regions, so a page that refused to
    // open with nothing selected would put them behind a footman for no
    // reason. The page now always has something on it, so the gate is gone.
    if !ctrl
        && keys.just_pressed(bind(Hk::ModeToggle))
        && !actions.contains(&CmdAction::TogglePage)
    {
        actions.push(CmdAction::TogglePage);
    }
    // The free-entry nudges, doctrine page only, raw keys only — they are
    // deliberately NOT card entries. The card is a menu of what you can do; the
    // nudge is a refinement of a value the card already shows, and the two
    // captions say which keys do it. Adding four more tiles for it would have
    // pushed the card past `CMD_SLOTS` and silently dropped the page toggle.
    if !ctrl && ui.page == CardPage::Doctrine && !own_units.is_empty() {
        for (key, action) in [
            (bind(Hk::NudgeFallbackDown), CmdAction::NudgeFallback(false)),
            (bind(Hk::NudgeFallbackUp), CmdAction::NudgeFallback(true)),
            (bind(Hk::NudgeLeashDown), CmdAction::NudgeLeash(false)),
            (bind(Hk::NudgeLeashUp), CmdAction::NudgeLeash(true)),
        ] {
            if keys.just_pressed(key) {
                actions.push(action);
            }
        }
    }
    // The region nudges are NOT gated on a selection, unlike the two above:
    // marking ground is about the ground, and requiring a footman to be
    // selected before you may resize a circle would be a rule with no reason
    // behind it. Same raw-key idiom otherwise — no tile, advertised on the cost
    // line of the tile they tune.
    if !ctrl && ui.page == CardPage::Doctrine {
        for (key, action) in [
            (bind(Hk::NudgeRegionDown), CmdAction::NudgeRegion(false)),
            (bind(Hk::NudgeRegionUp), CmdAction::NudgeRegion(true)),
        ] {
            if keys.just_pressed(key) {
                actions.push(action);
            }
        }
    }
    for (interaction, el) in &pressed_buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let El::CmdBtn(i) = *el {
            // `card_actions` is last frame's layout, so re-check the action
            // against this frame's entries before letting a click through.
            if let Some(action) = ui.card_actions.get(i).copied() {
                if !entries.iter().any(|e| e.action == action && e.locked) {
                    actions.push(action);
                }
            }
        }
    }

    // Centre of mass of the group being told to hold / fall back.
    let centroid = || {
        own_units.iter().fold(Vec3::ZERO, |acc, (_, p)| acc + *p) / own_units.len().max(1) as f32
    };
    // Where a fallback sends the wounded: the nearest own completed hall (any
    // rung of the ladder is a place to run to), else the start base.
    let nearest_hall = |from: Vec3| -> Vec3 {
        all_buildings
            .iter()
            .filter(|(b, t, _, under, _)| **t == Team::Human && is_hall(b.kind) && !under)
            .map(|(_, _, tf, _, _)| tf.translation)
            .min_by(|a, b| {
                dist_xz(*a, from)
                    .partial_cmp(&dist_xz(*b, from))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(HUMAN_BASE)
    };
    // Which squad a doctrine-page gesture is about, submitting the `squad`
    // verb first when the selection is not already one squad. A compound
    // gesture becomes two sentences rather than a special case — the same rule
    // a mixed right-click follows — so the log reads
    // "3 units join squad 2" / "squad 2 pushes to (…)", which is exactly what
    // a bridge commander would have had to send.
    let resolve_squad = |submissions: &mut EventWriter<SubmitIntent>| -> Option<u8> {
        if own_units.is_empty() {
            return None;
        }
        if let Some(squad) = doc.squad {
            if doc.in_squad < doc.units {
                say(
                    submissions,
                    Intent::Squad {
                        units: own_ids(),
                        id: Some(squad),
                    },
                );
            }
            return Some(squad);
        }
        // Nobody selected is in a squad: mint the lowest control-group id that
        // is free, so [I][W] works without a Ctrl+N first and still lines up
        // with the digit that recalls it.
        let taken: Vec<u8> = all_units
            .iter()
            .filter(|(_, _, t, _, _, _, _)| **t == Team::Human)
            .filter_map(|(_, _, _, _, _, _, s)| s.map(|s| s.0))
            .collect();
        let squad = (1..=MAX_UI_SQUAD)
            .find(|id| !taken.contains(id))
            .unwrap_or(1);
        say(
            submissions,
            Intent::Squad {
                units: own_ids(),
                id: Some(squad),
            },
        );
        Some(squad)
    };

    // --- execute -----------------------------------------------------------
    for action in actions {
        match action {
            CmdAction::AttackMove => {
                ui.attack_move_armed = true;
                ui.placement = None;
                ui.teleport_place = None;
                ui.region_place = false;
            }
            CmdAction::Stop => {
                if !own_units.is_empty() {
                    say(&mut submissions, Intent::Stop { units: own_ids() });
                }
                ui.attack_move_armed = false;
            }
            CmdAction::Place(kind) => {
                // Belt and braces: placement mode is unreachable for a kind
                // whose tech requirements are not met.
                if !requirements_met(building_requires(kind), completed.iter().copied()) {
                    continue;
                }
                ui.placement = Some(kind);
                ui.wall_chain.clear();
                ui.attack_move_armed = false;
                ui.teleport_place = None;
                ui.region_place = false;
            }
            // Abilities: combat.rs owns the unlock/mana/cooldown verdict,
            // exactly as it does for the AI and the bridge. The hotkey IS the
            // slot, so the UI is index-native; a commander may name the same
            // slot by id. Both spellings are the same intent.
            CmdAction::CastHero(index) => {
                // The geometry decides whether this key IS the cast or merely
                // arms it. Read from the first caster's table: the button only
                // exists for a slot they have, and a slot's geometry is a
                // property of the ability, not of who is holding it.
                let def = own_casters
                    .first()
                    .and_then(|(_, kind, _, _)| abilities_of_unit(*kind).get(index).copied());
                let Some(def) = def else { continue };
                if def.target.is_targeted() {
                    // Only the casters this aim is actually FOR. A mixed
                    // selection shares one hotkey column, so slot 1 can be a
                    // Sorcerer's Slow and a Champion's Warcry at once; sending
                    // the click's point to both would aim a spell that has
                    // nowhere to put it and earn a rejection the player never
                    // asked for. The armed gesture belongs to the units whose
                    // slot has the geometry the player is aiming.
                    let casters: Vec<Entity> = own_casters
                        .iter()
                        .filter(|(_, kind, _, _)| {
                            abilities_of_unit(*kind)
                                .get(index)
                                .is_some_and(|d| d.target == def.target)
                        })
                        .map(|(e, _, _, _)| *e)
                        .collect();
                    if casters.is_empty() {
                        continue;
                    }
                    ui.cast_place = Some(CastArm {
                        casters,
                        slot: index,
                        name: def.name,
                        wants_unit: def.target.wants_unit(),
                    });
                    ui.attack_move_armed = false;
                    ui.placement = None;
                    ui.posture_place = None;
                    ui.teleport_place = None;
                    ui.region_place = false;
                    continue;
                }
                for (hero, _, _, _) in &own_casters {
                    say(&mut submissions, cast_here(*hero, index, None));
                }
            }
            CmdAction::CastBuilding(index) => {
                if let Some((entity, kind, true, _)) = single {
                    let Some(def) = abilities_of_building(kind).get(index).copied() else {
                        continue;
                    };
                    if def.target.is_targeted() {
                        ui.cast_place = Some(CastArm {
                            casters: vec![entity],
                            slot: index,
                            name: def.name,
                            wants_unit: def.target.wants_unit(),
                        });
                        ui.attack_move_armed = false;
                        ui.placement = None;
                        ui.posture_place = None;
                        ui.teleport_place = None;
                        ui.region_place = false;
                        continue;
                    }
                    say(&mut submissions, cast_here(entity, index, None));
                }
            }
            CmdAction::Buy(item) => {
                // economy.rs re-validates ownership, slots and gold; the card
                // only greys the button so the player knows before clicking.
                // The buyer is NAMED rather than implied: with two heroes
                // possible, "the team's hero" is a coin flip, so the gesture
                // carries the customer the card was drawn for.
                if let (Some((shop, BuildingKind::Shop, true, _)), Some((customer, _))) =
                    (single, shop_customer)
                {
                    say(
                        &mut submissions,
                        Intent::Buy {
                            shop: intent_id(shop),
                            item: item_def(item).name.to_string(),
                            hero: Some(intent_id(customer)),
                        },
                    );
                }
            }
            CmdAction::Research(kind) => {
                // intent.rs owns the verdict (ownership, cap, busy forge,
                // affordability) and economy.rs owns the money, exactly as they
                // do for the bridge's `research` command. The card's job is
                // only to have meant it.
                if let Some((entity, bkind, true, false)) = single {
                    if building_researches(bkind).contains(&kind) {
                        say(
                            &mut submissions,
                            Intent::Research {
                                building: intent_id(entity),
                                upgrade: kind.id().to_string(),
                            },
                        );
                    }
                }
            }
            CmdAction::Upgrade(to) => {
                // economy.rs owns the verdict and the money, exactly as it does
                // for the bridge's `upgrade` command and the AI's tier-up.
                if let Some((entity, kind, true, false)) = single {
                    if building_upgrades_to(kind) == Some(to) {
                        say(
                            &mut submissions,
                            Intent::Upgrade {
                                building: intent_id(entity),
                            },
                        );
                    }
                }
            }
            CmdAction::UseSlot(slot) => {
                // The item buttons were drawn from THIS caster's bag
                // (`hero_cmds.items`), so the intent has to name it. Reading
                // "the team's hero" here would show the Priestess's potion and
                // drink the Champion's.
                let Some((entity, _, _, inventory)) = own_casters.first() else {
                    continue;
                };
                // A teleport item is the one consumable whose press is a
                // QUESTION, not an act: with a second hall standing, "use the
                // scroll" no longer names an outcome. So the key arms a
                // hall-pick, exactly as a targeted ability's key arms an aim —
                // and, exactly as a `Caster`-geometry ability fires on the
                // press, a team with one hall gets no ceremony at all,
                // because there is nothing to be asked.
                let teleport = inventory
                    .0
                    .get(slot)
                    .copied()
                    .flatten()
                    .filter(|id| item_chooses_destination(*id));
                if let Some(item) = teleport {
                    let halls = all_buildings
                        .iter()
                        .filter(|(b, team, _, under, _)| {
                            **team == Team::Human && !under && is_hall(b.kind)
                        })
                        .count();
                    if halls > 1 {
                        ui.teleport_place = Some(TeleportArm {
                            hero: *entity,
                            slot,
                            name: item_name(item),
                        });
                        ui.attack_move_armed = false;
                        ui.placement = None;
                        ui.posture_place = None;
                        ui.cast_place = None;
                        ui.region_place = false;
                        continue;
                    }
                }
                say(
                    &mut submissions,
                    Intent::UseItem {
                        slot,
                        hero: Some(intent_id(*entity)),
                        // One hall, or an item that does not teleport: the
                        // nearest-hall default is the only answer there is.
                        destination: None,
                    },
                );
            }
            // --- doctrine toggles ------------------------------------------
            // These are the clearest case of a gesture *compiling*: the card
            // offers one key, the language wants parameters, so the UI works
            // out the anchor, the threshold and the rally point from what is
            // selected and submits the same parameterised intent the bridge
            // spells out by hand. Same verb, same object, same log line.
            CmdAction::ToggleGuard => {
                if own_units.is_empty() {
                    continue;
                }
                if doc.leashed == 0 {
                    // Anchor on the centre of mass of the group being told to
                    // hold: "guard where you stand".
                    let anchor = clamp_to_map(centroid());
                    say(
                        &mut submissions,
                        Intent::Leash {
                            units: own_ids(),
                            x: Some(anchor.x),
                            z: Some(anchor.z),
                            region: None,
                            radius: Some(GUARD_RADIUS),
                        },
                    );
                } else {
                    // Mixed selection: any leash at all means "release all".
                    // Radius 0 is how the language spells "clear".
                    say(
                        &mut submissions,
                        Intent::Leash {
                            units: own_ids(),
                            x: None,
                            z: None,
                            region: None,
                            radius: Some(0.0),
                        },
                    );
                }
            }
            CmdAction::ToggleFallback => {
                if own_units.is_empty() {
                    continue;
                }
                if doc.fallback == 0 {
                    // Nearest own completed town hall, else the start base.
                    let rally = nearest_hall(centroid());
                    say(
                        &mut submissions,
                        Intent::Retreat {
                            units: own_ids(),
                            below: Some(FALLBACK_FRAC),
                            x: Some(rally.x),
                            z: Some(rally.z),
                            region: None,
                        },
                    );
                } else {
                    // `below: 0` is how the language spells "clear".
                    say(
                        &mut submissions,
                        Intent::Retreat {
                            units: own_ids(),
                            below: Some(0.0),
                            x: None,
                            z: None,
                            region: None,
                        },
                    );
                }
            }
            CmdAction::CyclePriority => {
                if own_units.is_empty() {
                    continue;
                }
                // The whole selection lands on the same preset, derived from
                // the first unit, so repeated presses stay in lock-step. An
                // empty class list is how the language spells "clear".
                let classes = priority_component(doc.prio.next())
                    .map(|p| p.0.iter().map(|c| c.name().to_string()).collect())
                    .unwrap_or_default();
                say(
                    &mut submissions,
                    Intent::Priority {
                        units: own_ids(),
                        classes,
                    },
                );
            }
            CmdAction::ToggleAutoCast => {
                // Heroes only — a footman has nothing to auto-cast.
                if own_casters.is_empty() {
                    continue;
                }
                // The card's one toggle governs the hero's FIRST ability;
                // per-ability rules are a bridge/doctrine affordance until a
                // hero ships with two spells. `ability: None` is exactly how
                // the language says "slot 0", so the card and a bare bridge
                // `autocast` are the same sentence.
                let units: Vec<IntentId> =
                    own_casters.iter().map(|(e, _, _, _)| intent_id(*e)).collect();
                say(
                    &mut submissions,
                    Intent::Autocast {
                        units,
                        min_enemies: Some(if doc.autocast == 0 {
                            AUTOCAST_MIN_ENEMIES
                        } else {
                            0 // "clear"
                        }),
                        ability: None,
                    },
                );
            }
            CmdAction::ToggleAutoCastSlot(slot) => {
                if own_casters.is_empty() {
                    continue;
                }
                let units: Vec<IntentId> =
                    own_casters.iter().map(|(e, _, _, _)| intent_id(*e)).collect();
                say(
                    &mut submissions,
                    Intent::Autocast {
                        units,
                        min_enemies: Some(if doc.autocast_slot_active(slot) {
                            0 // "clear this one rule"
                        } else {
                            AUTOCAST_MIN_ENEMIES
                        }),
                        // The one line that makes this per-ability: an explicit
                        // slot instead of the `None` that means "slot 0".
                        // intent.rs edits that rule and leaves the others
                        // standing, so two spells can carry two policies.
                        ability: Some(AbilitySelector::Index(slot)),
                    },
                );
            }
            // --- page two: the doctrine card -------------------------------
            CmdAction::TogglePage => {
                ui.page = match ui.page {
                    CardPage::Orders => CardPage::Doctrine,
                    CardPage::Doctrine => CardPage::Orders,
                };
                // A new vocabulary starts at its first page. Carrying the
                // overflow index across a mode flip would land the player on
                // page two of a card they have not seen page one of.
                ui.card_page = 0;
                // Flipping the card cancels whatever the other page armed.
                ui.attack_move_armed = false;
                ui.placement = None;
                ui.wall_chain.clear();
                ui.posture_place = None;
                ui.cast_place = None;
                ui.teleport_place = None;
                ui.region_place = false;
            }
            // The one trigger gesture the human has. A toggle, like [G] Guard:
            // armed, it clears; unarmed, it arms. Both halves submit an intent
            // a commander could have typed, and the replay log cannot tell
            // which of us pressed it.
            // --- territory ---------------------------------------------
            //
            // Arming, not acting: the mark needs a point, and the point is a
            // click. Same mutual exclusion every other armed mode observes —
            // two armed modes would make the next click ambiguous, and the
            // player would find out which one won by losing a building.
            CmdAction::MarkRegion => {
                if next_mark_name(&cast.regions).is_none() {
                    continue;
                }
                ui.region_place = !ui.region_place;
                if ui.region_place {
                    ui.attack_move_armed = false;
                    ui.placement = None;
                    ui.wall_chain.clear();
                    ui.posture_place = None;
                    ui.cast_place = None;
                    ui.teleport_place = None;
                }
            }
            CmdAction::ClearRegions => {
                if cast.regions.get(Team::Human).is_empty() {
                    continue;
                }
                ui.region_place = false;
                // The whole-slate form, which is the only one a mouse can
                // reach: picking WHICH mark to forget would need a click target
                // the minimap does not offer, and re-marking is one keypress.
                say(&mut submissions, Intent::RegionClear { name: None });
            }
            CmdAction::NudgeRegion(up) => {
                // Tunes the SIZE OF THE NEXT MARK, and does not touch the ones
                // already on the map. Re-marking over a name replaces it, so
                // resizing an existing circle is "tune, then mark again" — one
                // rule, no second verb.
                ui.region_radius = nudge_value(
                    ui.region_radius,
                    up,
                    REGION_NUDGE,
                    REGION_MARK_RADIUS,
                    REGION_RADIUS_MIN,
                    REGION_RADIUS_MAX,
                );
            }
            CmdAction::ToggleHomeGuard => {
                if has_trigger(&cast.triggers, HOME_GUARD) {
                    say(
                        &mut submissions,
                        Intent::TriggerClear {
                            name: Some(HOME_GUARD.to_string()),
                        },
                    );
                    continue;
                }
                if own_units.is_empty() {
                    continue;
                }
                // The squad is resolved AT PRESS TIME, exactly as an armed
                // posture resolves it: the rule names a squad, and which units
                // are in it must not change under the player between the press
                // and the day it fires.
                let Some(squad) = resolve_squad(&mut submissions) else {
                    continue;
                };
                let home = nearest_hall(centroid());
                say(
                    &mut submissions,
                    Intent::TriggerSet {
                        name: HOME_GUARD.to_string(),
                        when: TriggerWhen::BaseUnderAttack,
                        then: Box::new(Intent::Posture {
                            id: squad,
                            posture: Some(PostureIntent::Defend {
                                x: Some(home.x),
                                z: Some(home.z),
                                region: None,
                                radius: Some(HOME_GUARD_RADIUS),
                            }),
                        }),
                        // Repeating, and this is the only interesting choice in
                        // the preset. A base is raided more than once a match,
                        // and a home guard that spent itself on the first
                        // harassing Raider would be a rule that is armed
                        // exactly when it is not needed.
                        repeat: Some(HOME_GUARD_COOLDOWN),
                    },
                );
            }
            CmdAction::SetPosture(kind) => {
                let Some(squad) = resolve_squad(&mut submissions) else {
                    continue;
                };
                // Every posture is now "press, then click" — the same two-step
                // building placement already teaches. Three of them want a
                // point and Escort wants a unit; `left_mouse` reads
                // `PostureKind::needs_unit` to know which click it is holding.
                ui.posture_place = Some(PostureArm { squad, kind });
                ui.attack_move_armed = false;
                ui.placement = None;
                ui.wall_chain.clear();
                ui.cast_place = None;
                ui.teleport_place = None;
                ui.region_place = false;
            }
            CmdAction::ClearPosture => {
                // Clearing a posture leaves membership intact: the squad stops
                // being re-tasked but stays a squad, exactly as the bridge's
                // `{"type":"posture","id":N}` does.
                let Some(squad) = doc.squad else { continue };
                say(
                    &mut submissions,
                    Intent::Posture {
                        id: squad,
                        posture: None,
                    },
                );
            }
            CmdAction::CycleFallback => {
                if own_units.is_empty() {
                    continue;
                }
                match cycle_step(doc.fallback_value(), &FALLBACK_STEPS) {
                    Some(below) => {
                        let rally = nearest_hall(centroid());
                        say(
                            &mut submissions,
                            Intent::Retreat {
                                units: own_ids(),
                                below: Some(below),
                                x: Some(rally.x),
                                z: Some(rally.z),
                                region: None,
                            },
                        );
                    }
                    None => say(
                        &mut submissions,
                        Intent::Retreat {
                            units: own_ids(),
                            below: Some(0.0),
                            x: None,
                            z: None,
                            region: None,
                        },
                    ),
                }
            }
            // The nudges reuse the cycles' sentences exactly — same verb, same
            // rally/anchor derivation, same "0.0 means off" spelling. Only the
            // number is arrived at differently, which is the whole point: a
            // free-entry control that produced a DIFFERENT intent would be a
            // second dialect of the same idea.
            CmdAction::NudgeFallback(up) => {
                if own_units.is_empty() {
                    continue;
                }
                match nudge_value(
                    doc.fallback_value(),
                    up,
                    FALLBACK_NUDGE,
                    FALLBACK_STEPS[1],
                    FALLBACK_MIN,
                    FALLBACK_MAX,
                ) {
                    Some(below) => {
                        let rally = nearest_hall(centroid());
                        say(
                            &mut submissions,
                            Intent::Retreat {
                                units: own_ids(),
                                below: Some(below),
                                x: Some(rally.x),
                                z: Some(rally.z),
                                region: None,
                            },
                        );
                    }
                    None => say(
                        &mut submissions,
                        Intent::Retreat {
                            units: own_ids(),
                            below: Some(0.0),
                            x: None,
                            z: None,
                            region: None,
                        },
                    ),
                }
            }
            CmdAction::NudgeLeash(up) => {
                if own_units.is_empty() {
                    continue;
                }
                match nudge_value(
                    doc.leash_value(),
                    up,
                    LEASH_NUDGE,
                    LEASH_STEPS[1],
                    LEASH_MIN,
                    LEASH_MAX,
                ) {
                    Some(radius) => {
                        let anchor = clamp_to_map(centroid());
                        say(
                            &mut submissions,
                            Intent::Leash {
                                units: own_ids(),
                                x: Some(anchor.x),
                                z: Some(anchor.z),
                                region: None,
                                radius: Some(radius),
                            },
                        );
                    }
                    None => say(
                        &mut submissions,
                        Intent::Leash {
                            units: own_ids(),
                            x: None,
                            z: None,
                            region: None,
                            radius: Some(0.0),
                        },
                    ),
                }
            }
            CmdAction::CycleLeash => {
                if own_units.is_empty() {
                    continue;
                }
                match cycle_step(doc.leash_value(), &LEASH_STEPS) {
                    Some(radius) => {
                        let anchor = clamp_to_map(centroid());
                        say(
                            &mut submissions,
                            Intent::Leash {
                                units: own_ids(),
                                x: Some(anchor.x),
                                z: Some(anchor.z),
                                region: None,
                                radius: Some(radius),
                            },
                        );
                    }
                    None => say(
                        &mut submissions,
                        Intent::Leash {
                            units: own_ids(),
                            x: None,
                            z: None,
                            region: None,
                            radius: Some(0.0),
                        },
                    ),
                }
            }
            // The production template. Every piece is absolute, so each button
            // re-sends the WHOLE template with one field stepped — which is
            // also how a commander has to spell an edit.
            CmdAction::TemplateSquad
            | CmdAction::TemplateFallback
            | CmdAction::TemplatePriority
            | CmdAction::TemplateAutoCast
            | CmdAction::TemplateClear => {
                if !own_units.is_empty() || !single_template.capable {
                    continue;
                }
                let Some((entity, _, true, _)) = single else {
                    continue;
                };
                let mut next = single_template;
                match action {
                    CmdAction::TemplateSquad => {
                        next.squad = match next.squad {
                            None => Some(1),
                            Some(id) if id < MAX_UI_SQUAD => Some(id + 1),
                            Some(_) => None,
                        };
                    }
                    CmdAction::TemplateFallback => {
                        next.retreat = cycle_step(next.retreat, &FALLBACK_STEPS);
                    }
                    CmdAction::TemplatePriority => next.prio = next.prio.next(),
                    CmdAction::TemplateAutoCast => next.autocast = !next.autocast,
                    _ => next = TemplateView { capable: true, ..default() },
                }
                // A template rally cannot be the group's centroid (there is no
                // group yet — these units do not exist), so it is the hall
                // nearest the base: where a fresh recruit runs home to.
                let rally = nearest_hall(HUMAN_BASE);
                say(
                    &mut submissions,
                    Intent::Template {
                        building: intent_id(entity),
                        squad: next.squad,
                        retreat: next.retreat.map(|below| RetreatIntent {
                            below,
                            x: rally.x,
                            z: rally.z,
                        }),
                        priority: priority_component(next.prio)
                            .map(|p| p.0.iter().map(|c| c.name().to_string()).collect()),
                        autocast: next.autocast.then_some(AUTOCAST_MIN_ENEMIES),
                    },
                );
            }
            CmdAction::Train(kind) => {
                // Hero slots: the same UX gate the card draws. intent.rs
                // re-checks and economy.rs enforces at the pay-point; this
                // only keeps a greyed-out button from filling the error feed.
                if is_hero_kind(kind) && hero_train.offer(kind).is_none_or(|(.., ok)| !ok) {
                    continue;
                }
                let mut iter = sel_buildings.iter();
                let (first, second) = (iter.next(), iter.next());
                if second.is_some() {
                    continue;
                }
                let Some((entity, building, team, Some(queue), uc, upgrading, _)) = first else {
                    continue;
                };
                if *team != Team::Human || uc.is_some() || !trainable(building.kind).contains(&kind)
                {
                    continue;
                }
                // Queuing into a hall mid-upgrade is allowed on purpose — the
                // queue survives the conversion, it just doesn't advance. What
                // is NOT allowed is queuing into scaffolding (`uc`, above).
                let _ = upgrading;
                let (cost_gold, cost_lumber) = if is_hero_kind(kind) {
                    let (g, l, _) = hero_train_cost(&records, Team::Human, kind);
                    (g, l)
                } else {
                    let s = unit_stats(kind);
                    (s.cost_gold, s.cost_lumber)
                };
                let affordable = economies.get(Team::Human).can_afford(cost_gold, cost_lumber);
                // These stay here as *UX* gates: a click that can't work is
                // dropped silently rather than logged as a rejected intent.
                // intent.rs re-checks all of them regardless — this only keeps
                // a greyed-out button from filling the error channel.
                if affordable && queue.queue.len() < MAX_QUEUE {
                    say(
                        &mut submissions,
                        Intent::Train {
                            building: intent_id(entity),
                            unit: kind_name(kind).to_string(),
                        },
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Selection cards & training-queue tiles
// ---------------------------------------------------------------------------

fn panel_clicks(
    mut commands: Commands,
    ui: Res<UiState>,
    game_over: Res<GameOver>,
    mut submissions: EventWriter<SubmitIntent>,
    pressed_buttons: Query<(&Interaction, &El), Changed<Interaction>>,
    selected: Query<Entity, With<Selected>>,
    alive: Query<Entity, Or<(With<Unit>, With<Building>)>>,
    queues: Query<(Entity, &TrainingQueue), With<Selected>>,
) {
    if game_over.winner.is_some() {
        return;
    }
    for (interaction, el) in &pressed_buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *el {
            El::Card(i) => {
                // Stored entities can die at any time.
                if let Some(&e) = ui.card_entities.get(i) {
                    if alive.get(e).is_ok() {
                        apply_selection(&mut commands, &selected, &[e], false);
                    }
                }
            }
            El::QueueTile(i) => {
                if i >= ui.queue_len {
                    continue;
                }
                let mut iter = queues.iter();
                let (first, second) = (iter.next(), iter.next());
                if second.is_some() {
                    continue;
                }
                let Some((entity, queue)) = first else { continue };
                if i < queue.queue.len() {
                    say(
                        &mut submissions,
                        Intent::Cancel {
                            building: intent_id(entity),
                            index: i,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Control groups ARE squads (Ctrl+1..3 assign, Shift+1..3 add, 1..3 recall)
// ---------------------------------------------------------------------------

/// The control-group slots, which are also the squad ids they write. Three,
/// matching `UiState::groups` and the hint line; `DEFAULT_SQUAD` (0) is
/// deliberately not among them — that id is doctrine.rs's machine-only
/// anti-idle floor, not a group a player can claim.
const GROUP_DIGITS: [(KeyCode, u8); 3] = hotkeys::GROUP_DIGIT_KEYS;

/// Control groups, and the highest-leverage line in docs/TEMPO.md: `Ctrl+N`
/// does not just remember a selection, it submits the `squad` verb — the same
/// object a bridge commander creates with
/// `{"type":"squad","units":[...],"id":1}`. The human's existing muscle memory
/// becomes the shared strategic vocabulary, and `[I] Doctrine`'s postures then
/// have something to act on.
///
/// The three gestures:
///   * `Ctrl+N` — the selection *becomes* squad N. Anyone who was in squad N
///     and is not in the new selection leaves it, so the group the player sees
///     and the squad the engine executes never drift apart. That eviction is
///     its own sentence (`squad … id: null`), exactly as a commander would
///     have to spell it.
///   * `Shift+N` — add the selection to squad N, evicting nobody.
///   * `N` — recall. Pure selection: ui-local, no intent, nothing to say.
///
/// Buildings may sit in a control group (they always could) but never in a
/// squad — `SquadId` is a unit component, so a hall caught in the box is
/// remembered for recall and left out of the sentence.
fn control_groups(
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    keys: Res<ButtonInput<KeyCode>>,
    game_over: Res<GameOver>,
    mut submissions: EventWriter<SubmitIntent>,
    selected: Query<Entity, With<Selected>>,
    alive: Query<&Team>,
    // Every own unit and the squad it is in — the source for "who is leaving
    // squad N", which the UI must work out because the language has no
    // "replace the membership of squad N" verb (and should not: `squad` names
    // units, so a swap is two sentences).
    squad_members: Query<(Entity, &Team, Option<&SquadId>), With<Unit>>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    const DIGITS: [(KeyCode, u8); 3] = GROUP_DIGITS;

    for (key, slot) in DIGITS {
        if !keys.just_pressed(key) {
            continue;
        }
        if ctrl || shift {
            let members: Vec<Entity> = selected.iter().collect();
            // Only own units can join a squad; the rest of the selection is
            // still remembered for recall.
            let joining: Vec<Entity> = members
                .iter()
                .copied()
                .filter(|e| {
                    squad_members
                        .get(*e)
                        .is_ok_and(|(_, team, _)| *team == Team::Human)
                })
                .collect();

            if shift && !ctrl {
                // Add: union with what the group already held, minus the dead.
                let existing = ui.groups.get(&slot).cloned().unwrap_or_default();
                let mut merged = existing;
                merged.retain(|e| alive.get(*e).is_ok());
                for e in &members {
                    if !merged.contains(e) {
                        merged.push(*e);
                    }
                }
                ui.groups.insert(slot, merged);
                if !joining.is_empty() && game_over.winner.is_none() {
                    say(
                        &mut submissions,
                        Intent::Squad {
                            units: ids(&joining),
                            id: Some(slot),
                        },
                    );
                }
                continue;
            }

            // Replace. Anyone left behind in squad N is released first, so the
            // squad and the control group stay the same set of units.
            let leavers: Vec<Entity> = squad_members
                .iter()
                .filter(|(e, team, squad)| {
                    **team == Team::Human
                        && squad.is_some_and(|s| s.0 == slot)
                        && !joining.contains(e)
                })
                .map(|(e, _, _)| e)
                .collect();
            ui.groups.insert(slot, members);
            if game_over.winner.is_none() {
                if !leavers.is_empty() {
                    say(
                        &mut submissions,
                        Intent::Squad {
                            units: ids(&leavers),
                            id: None,
                        },
                    );
                }
                if !joining.is_empty() {
                    say(
                        &mut submissions,
                        Intent::Squad {
                            units: ids(&joining),
                            id: Some(slot),
                        },
                    );
                }
            }
        } else {
            let members: Vec<Entity> = ui
                .groups
                .get(&slot)
                .map(|v| v.iter().copied().filter(|e| alive.get(*e).is_ok()).collect())
                .unwrap_or_default();
            if let Some(stored) = ui.groups.get_mut(&slot) {
                *stored = members.clone();
            }
            if !members.is_empty() {
                apply_selection(&mut commands, &selected, &members, false);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Minimap interaction
// ---------------------------------------------------------------------------

fn minimap_input(
    hud: Res<HudLayout>,
    mut ui: ResMut<UiState>,
    buttons: Res<ButtonInput<MouseButton>>,
    game_over: Res<GameOver>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut focus: EventWriter<CameraFocus>,
    mut submissions: EventWriter<SubmitIntent>,
    sel_units: Query<(Entity, &Team), (With<Selected>, With<Unit>)>,
    // The hall pick's second home. Cheap because the plumbing was already
    // here: this system converts a minimap click to a world point for the
    // camera and for right-click orders, so naming a hall from it is that
    // point plus one nearest-own-hall search.
    halls: Query<(Entity, &Building, &Team, &Transform), Without<UnderConstruction>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    if game_over.winner.is_some() {
        ui.minimap_drag = false;
        return;
    }
    let rect = minimap_rect(window, &hud);
    let cursor = window.cursor_position();
    let inside = cursor.map_or(false, |c| rect.contains(c));

    // --- left click while a teleport is armed: name the hall ---------------
    //
    // The minimap is where the OTHER base is. A hall-pick that could only be
    // made in the world view would ask the player to scroll the camera to the
    // main they are trying to save, which is a second or two of exactly the
    // moment the scroll exists to buy back. Tolerance is generous
    // (`MINIMAP_HALL_PICK` world units, ~11 minimap pixels) because a hall is
    // only about seven pixels wide down here; halls stand far enough apart
    // that a forgiving radius still cannot mean two of them at once, and if
    // it did, the nearest wins. A miss leaves the gesture armed and falls
    // through to the ordinary camera drag, so a mis-click still just looks.
    if buttons.just_pressed(MouseButton::Left) && inside && ui.teleport_place.is_some() {
        if let (Some(arm), Some(c)) = (ui.teleport_place, cursor) {
            let uv = Vec2::new(c.x - rect.min.x, c.y - rect.min.y);
            let ground = clamp_to_map(minimap_to_world(uv, hud.minimap_px));
            let mut best: Option<(Entity, f32)> = None;
            for (e, b, team, tf) in &halls {
                if *team != Team::Human || !is_hall(b.kind) {
                    continue;
                }
                let d = dist_xz(tf.translation, ground);
                if d <= MINIMAP_HALL_PICK && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((e, d));
                }
            }
            if let Some((hall, _)) = best {
                say(
                    &mut submissions,
                    Intent::UseItem {
                        slot: arm.slot,
                        hero: Some(intent_id(arm.hero)),
                        destination: Some(intent_id(hall)),
                    },
                );
                ui.teleport_place = None;
                return;
            }
        }
    }

    // --- left click / drag: move the camera --------------------------------
    if buttons.just_pressed(MouseButton::Left) && inside {
        ui.minimap_drag = true;
    }
    if !buttons.pressed(MouseButton::Left) {
        ui.minimap_drag = false;
    }
    if ui.minimap_drag {
        if let Some(c) = cursor {
            let uv = Vec2::new(
                (c.x - rect.min.x).clamp(0.0, hud.minimap_px),
                (c.y - rect.min.y).clamp(0.0, hud.minimap_px),
            );
            focus.write(CameraFocus {
                pos: minimap_to_world(uv, hud.minimap_px),
            });
        }
    }

    // --- right click: context order at that world position -----------------
    if buttons.just_pressed(MouseButton::Right) && inside {
        let Some(c) = cursor else { return };
        let uv = Vec2::new(c.x - rect.min.x, c.y - rect.min.y);
        let ground = clamp_to_map(minimap_to_world(uv, hud.minimap_px));

        let mut group: Vec<Entity> = sel_units
            .iter()
            .filter(|(_, t)| **t == Team::Human)
            .map(|(e, _)| e)
            .collect();
        group.sort_by_key(|e| e.index());
        if group.is_empty() {
            return;
        }
        let attack_move = ui.attack_move_armed;
        ui.attack_move_armed = false;
        ground_intent(&mut submissions, &group, ground, attack_move);
    }
}

// ---------------------------------------------------------------------------
// Left mouse: selection, drag box, placement confirm, attack-move click
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn left_mouse(
    hud: Res<HudLayout>,
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    nav: Res<NavGrid>,
    economies: Res<Economies>,
    game_over: Res<GameOver>,
    // Read-only: which `mark N` is free, so an armed marker names the next one.
    regions: Res<Regions>,
    mut submissions: EventWriter<SubmitIntent>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    units: Query<(Entity, &Transform, &Unit, &Team, Has<Selected>)>,
    // `UnderConstruction` rides along for the hall pick: a hall still going up
    // is not a place a scroll can land, and offering it would compose a
    // gesture the compiler is about to refuse.
    buildings: Query<(
        Entity,
        &Transform,
        &Building,
        &Team,
        Has<Selected>,
        Has<UnderConstruction>,
    )>,
    selected: Query<Entity, With<Selected>>,
    mut drag_node: Query<&mut Node, With<DragRect>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_q.single() else {
        return;
    };

    if game_over.winner.is_some() {
        if let Ok(mut node) = drag_node.single_mut() {
            node.display = Display::None;
        }
        return;
    }

    let cursor = window.cursor_position();

    // ---- press ----------------------------------------------------------
    if buttons.just_pressed(MouseButton::Left) {
        if let Some(cursor) = cursor {
            if !cursor_over_hud(cursor, window, &ui, &hud) {
                let ground = cursor_to_ground(camera, cam_tf, cursor);

                // Placement confirm.
                if let (Some(kind), Some(ground)) = (ui.placement, ground) {
                    let size = building_stats(kind).size;
                    let pos = snap_footprint(ground, size);
                    if placement_valid(&nav, economies.get(Team::Human), kind, pos) {
                        // Selected workers, nearest the site first; economy pays.
                        let mut workers: Vec<(Entity, f32)> = units
                            .iter()
                            .filter(|(_, _, u, t, sel)| {
                                *sel && **t == Team::Human && is_worker_kind(u.kind)
                            })
                            .map(|(e, tf, _, _, _)| (e, dist_xz(tf.translation, pos)))
                            .collect();
                        workers.sort_by(|a, b| {
                            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                        });

                        // Walls chain: stay in placement mode and hand each
                        // segment to the nearest worker that has not taken one
                        // yet this chain (a new Build order would overwrite the
                        // last), starting a fresh lap once everyone has.
                        let chaining = kind == BuildingKind::Wall;
                        let worker = if chaining {
                            match workers
                                .iter()
                                .map(|(e, _)| *e)
                                .find(|e| !ui.wall_chain.contains(e))
                            {
                                Some(e) => Some(e),
                                None => {
                                    ui.wall_chain.clear();
                                    workers.first().map(|(e, _)| *e)
                                }
                            }
                        } else {
                            workers.first().map(|(e, _)| *e)
                        };

                        if let Some(worker) = worker {
                            say(
                                &mut submissions,
                                Intent::Build {
                                    worker: intent_id(worker),
                                    kind: building_name(kind).to_string(),
                                    x: Some(pos.x),
                                    z: Some(pos.z),
                                    region: None,
                                },
                            );
                            if chaining {
                                ui.wall_chain.push(worker);
                            } else {
                                ui.placement = None;
                            }
                        }
                    }
                    return;
                }

                // Posture point. The doctrine card armed a squad posture; this
                // click supplies the ground it is about, and the pair becomes
                // one `posture` sentence — the same object a commander sends
                // as {"type":"posture","id":1,"posture":{...}}.
                if let Some(arm) = ui.posture_place {
                    if arm.kind.needs_unit() {
                        // Escort: the click names one of OUR units. Same picker
                        // the plain-click selection uses — nearest own unit
                        // within `UNIT_PICK_RADIUS` of where the ray meets that
                        // unit's altitude, so a Gryphon is clickable where it
                        // is drawn rather than where its shadow falls.
                        let picked = ground.and_then(|g| {
                            let ray = cursor_ray(camera, cam_tf, cursor);
                            let mut best: Option<(Entity, f32)> = None;
                            for (e, tf, _, team, _) in &units {
                                if *team != Team::Human {
                                    continue;
                                }
                                let d = dist_xz(
                                    tf.translation,
                                    pick_point_for(ray, g, tf.translation.y),
                                );
                                if d <= UNIT_PICK_RADIUS && best.is_none_or(|(_, bd)| d < bd) {
                                    best = Some((e, d));
                                }
                            }
                            best.map(|(e, _)| e)
                        });
                        // A miss leaves the gesture ARMED. Clicking a point is
                        // hard to miss; clicking a 0.7-radius unit in a moving
                        // fight is not, and disarming on every near-miss would
                        // make the escortee you actually wanted the one target
                        // you keep failing to pick.
                        if let Some(intent) = picked.and_then(|e| posture_unit_intent(arm, e)) {
                            say(&mut submissions, intent);
                            ui.posture_place = None;
                        }
                        return;
                    }
                    if let Some(intent) = ground.and_then(|g| posture_intent(arm, g)) {
                        say(&mut submissions, intent);
                    }
                    ui.posture_place = None;
                    return;
                }

                // Region mark. `[M]` armed it; this click is the centre, and
                // the radius came off the card. One `region_set` sentence — the
                // same object a commander sends as
                // {"type":"region_set","name":"mark 1","x":..,"z":..,"radius":..},
                // which is what lets a co-commander sharing this team read the
                // human's marks and name them back.
                if ui.region_place {
                    if let (Some(g), Some(name)) =
                        (ground, next_mark_name(&regions))
                    {
                        let p = clamp_to_map(g);
                        say(
                            &mut submissions,
                            Intent::RegionSet {
                                name,
                                x: p.x,
                                z: p.z,
                                radius: ui.region_radius.unwrap_or(REGION_MARK_RADIUS),
                            },
                        );
                    }
                    ui.region_place = false;
                    return;
                }

                // Targeted cast. The command card armed a slot; this click
                // supplies the place, and the pair becomes exactly the
                // sentence a commander sends as
                // {"type":"cast","caster":7,"ability":"Slow","x":..,"z":..}.
                //
                // Placed AFTER placement and posture and before attack-move
                // for no deeper reason than that the armed modes are mutually
                // exclusive — each arming clears the others — so the order is
                // about reading, not precedence.
                if let Some(arm) = ui.cast_place.clone() {
                    if arm.wants_unit {
                        // Same picker Escort uses, but over BOTH teams: the
                        // shipping targeted abilities are debuffs, and a
                        // picker that only saw your own units could not aim
                        // one. Who it may legally affect is the effect's
                        // question, asked in combat.rs.
                        let picked = ground.and_then(|g| {
                            let ray = cursor_ray(camera, cam_tf, cursor);
                            let mut best: Option<(Entity, f32)> = None;
                            for (e, tf, _, _, _) in &units {
                                let d = dist_xz(
                                    tf.translation,
                                    pick_point_for(ray, g, tf.translation.y),
                                );
                                if d <= UNIT_PICK_RADIUS && best.is_none_or(|(_, bd)| d < bd) {
                                    best = Some((e, d));
                                }
                            }
                            best.map(|(e, _)| e)
                        });
                        // A miss leaves it armed, for the reason Escort does.
                        if let Some(victim) = picked {
                            for caster in &arm.casters {
                                say(
                                    &mut submissions,
                                    cast_here(*caster, arm.slot, Some(CastTarget::Unit(victim))),
                                );
                            }
                            ui.cast_place = None;
                        }
                        return;
                    }
                    if let Some(g) = ground {
                        for caster in &arm.casters {
                            say(
                                &mut submissions,
                                cast_here(*caster, arm.slot, Some(CastTarget::Point(g))),
                            );
                        }
                    }
                    ui.cast_place = None;
                    return;
                }

                // Hall pick. The item card armed a teleport; this click names
                // WHICH of your halls it arrives at, and the pair becomes the
                // same sentence a commander sends as
                // {"type":"use_item","slot":0,"hero":7,"destination":34}.
                //
                // The picker is the SELECTION picker's building rule verbatim
                // (own, within half a footprint of the ground point), so "the
                // hall you can click to select" and "the hall you can click to
                // port to" are the same hall. A miss leaves it armed, for the
                // reason Escort and a targeted cast do: this gesture is
                // usually made with an enemy army on top of the thing you are
                // aiming at, and disarming on a near-miss would make the hall
                // you most need the one you keep failing to name.
                if let Some(arm) = ui.teleport_place {
                    if let Some(ground) = ground {
                        let mut best: Option<(Entity, f32)> = None;
                        for (e, tf, b, team, _, under) in &buildings {
                            if *team != Team::Human || under || !is_hall(b.kind) {
                                continue;
                            }
                            let d = dist_xz(tf.translation, ground);
                            if d <= building_stats(b.kind).size * 0.5
                                && best.is_none_or(|(_, bd)| d < bd)
                            {
                                best = Some((e, d));
                            }
                        }
                        if let Some((hall, _)) = best {
                            say(
                                &mut submissions,
                                Intent::UseItem {
                                    slot: arm.slot,
                                    hero: Some(intent_id(arm.hero)),
                                    destination: Some(intent_id(hall)),
                                },
                            );
                            ui.teleport_place = None;
                        }
                    }
                    return;
                }

                // Attack-move click.
                if ui.attack_move_armed {
                    if let Some(ground) = ground {
                        let mut group: Vec<Entity> = units
                            .iter()
                            .filter(|(_, _, _, t, sel)| *sel && **t == Team::Human)
                            .map(|(e, _, _, _, _)| e)
                            .collect();
                        group.sort_by_key(|e| e.index());
                        ground_intent(&mut submissions, &group, ground, true);
                    }
                    ui.attack_move_armed = false;
                    return;
                }

                ui.drag_start = Some(cursor);
                ui.dragging = false;
            }
        }
    }

    // ---- drag ------------------------------------------------------------
    if buttons.pressed(MouseButton::Left) {
        if let (Some(start), Some(cursor)) = (ui.drag_start, cursor) {
            if start.distance(cursor) >= DRAG_THRESHOLD {
                ui.dragging = true;
            }
        }
        if ui.dragging {
            if let (Some(start), Some(cursor)) = (ui.drag_start, cursor) {
                let rect = Rect::from_corners(start, cursor);
                if let Ok(mut node) = drag_node.single_mut() {
                    node.display = Display::Flex;
                    node.left = Val::Px(rect.min.x);
                    node.top = Val::Px(rect.min.y);
                    node.width = Val::Px(rect.width());
                    node.height = Val::Px(rect.height());
                }
            }
        }
    }

    // ---- release ---------------------------------------------------------
    if buttons.just_released(MouseButton::Left) {
        if let Ok(mut node) = drag_node.single_mut() {
            node.display = Display::None;
        }
        let start = ui.drag_start.take();
        let was_dragging = ui.dragging;
        ui.dragging = false;

        let Some(start) = start else {
            return;
        };
        let additive = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let Some(cursor) = cursor else {
            return;
        };

        if was_dragging {
            // Rubber-band: project every own entity to the viewport.
            let rect = Rect::from_corners(start, cursor);
            let mut picked_units = Vec::new();
            for (e, tf, _, team, _) in &units {
                if *team != Team::Human {
                    continue;
                }
                if let Ok(sp) = camera.world_to_viewport(cam_tf, tf.translation) {
                    if rect.contains(sp) {
                        picked_units.push(e);
                    }
                }
            }
            if !picked_units.is_empty() {
                apply_selection(&mut commands, &selected, &picked_units, additive);
                return;
            }
            // Only buildings caught by the box.
            let mut picked_buildings = Vec::new();
            for (e, tf, _, team, _, _) in &buildings {
                if *team != Team::Human {
                    continue;
                }
                if let Ok(sp) = camera.world_to_viewport(cam_tf, tf.translation) {
                    if rect.contains(sp) {
                        picked_buildings.push(e);
                    }
                }
            }
            if !picked_buildings.is_empty() {
                apply_selection(&mut commands, &selected, &picked_buildings, additive);
            } else if !additive {
                apply_selection(&mut commands, &selected, &[], false);
            }
            return;
        }

        // Plain click: closest own unit, else own building, else clear.
        if cursor_over_hud(cursor, window, &ui, &hud) {
            return;
        }
        let Some(ground) = cursor_to_ground(camera, cam_tf, cursor) else {
            return;
        };
        let ray = cursor_ray(camera, cam_tf, cursor);

        let mut best: Option<(Entity, f32)> = None;
        for (e, tf, _, team, _) in &units {
            if *team != Team::Human {
                continue;
            }
            let d = dist_xz(
                tf.translation,
                pick_point_for(ray, ground, tf.translation.y),
            );
            if d <= UNIT_PICK_RADIUS && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((e, d));
            }
        }
        if best.is_none() {
            for (e, tf, b, team, _, _) in &buildings {
                if *team != Team::Human {
                    continue;
                }
                let d = dist_xz(tf.translation, ground);
                if d <= building_stats(b.kind).size * 0.5 && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((e, d));
                }
            }
        }

        match best {
            Some((e, _)) => apply_selection(&mut commands, &selected, &[e], additive),
            None => {
                if !additive {
                    apply_selection(&mut commands, &selected, &[], false);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Right mouse: context orders
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn right_mouse(
    hud: Res<HudLayout>,
    mut ui: ResMut<UiState>,
    buttons: Res<ButtonInput<MouseButton>>,
    game_over: Res<GameOver>,
    mut submissions: EventWriter<SubmitIntent>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    units: Query<(Entity, &Transform, &Unit, &Team, Has<Selected>, Has<Carrying>)>,
    buildings: Query<(
        Entity,
        &Transform,
        &Building,
        &Team,
        Has<Selected>,
        Has<UnderConstruction>,
    )>,
    nodes: Query<(Entity, &Transform, &ResourceNode)>,
    fog: Res<FogGrids>,
) {
    if !buttons.just_pressed(MouseButton::Right) || game_over.winner.is_some() {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_q.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    // Console clicks belong to the console (minimap_input handles them).
    if cursor_over_hud(cursor, window, &ui, &hud) {
        return;
    }

    // Right-click on the world always cancels transient modes first.
    if ui.placement.is_some()
        || ui.attack_move_armed
        || ui.posture_place.is_some()
        || ui.cast_place.is_some()
        || ui.teleport_place.is_some()
        || ui.region_place
    {
        ui.placement = None;
        ui.wall_chain.clear();
        ui.attack_move_armed = false;
        ui.posture_place = None;
        ui.cast_place = None;
        ui.teleport_place = None;
        ui.region_place = false;
        return;
    }

    let Some(ground) = cursor_to_ground(camera, cam_tf, cursor) else {
        return;
    };

    // Selected human units, ordered for a stable formation layout.
    let mut selected_units: Vec<(Entity, UnitKind, bool)> = units
        .iter()
        .filter(|(_, _, _, t, sel, _)| *sel && **t == Team::Human)
        .map(|(e, _, u, _, _, carrying)| (e, u.kind, carrying))
        .collect();
    selected_units.sort_by_key(|(e, _, _)| e.index());

    // --- pick whatever the cursor is over (once, shared by every branch) --
    let ray = cursor_ray(camera, cam_tf, cursor);
    let mut enemy: Option<(Entity, f32)> = None;
    let mut own_unit: Option<(Entity, f32)> = None;
    for (e, tf, _, team, _, _) in &units {
        let d = dist_xz(
            tf.translation,
            pick_point_for(ray, ground, tf.translation.y),
        );
        if d > UNIT_PICK_RADIUS {
            continue;
        }
        match team {
            Team::Claude => {
                // An enemy the fog is hiding is not clickable. The bridge
                // rejects `attack` orders against unseen ids for the same
                // reason: a target you cannot be shown must not be a target
                // you can name, or the filtering is decoration.
                if !fog_sees(&fog, tf.translation) {
                    continue;
                }
                if enemy.is_none_or(|(_, bd)| d < bd) {
                    enemy = Some((e, d));
                }
            }
            Team::Human => {
                if own_unit.is_none_or(|(_, bd)| d < bd) {
                    own_unit = Some((e, d));
                }
            }
        }
    }
    // Enemy buildings only count when no enemy unit was hit (units win), and
    // own completed town halls are remembered as resource drop-off points.
    let hit_enemy_unit = enemy.is_some();
    let mut own_depot: Option<f32> = None;
    for (e, tf, b, team, _, under) in &buildings {
        let d = dist_xz(tf.translation, ground);
        if d > building_stats(b.kind).size * 0.5 {
            continue;
        }
        match team {
            Team::Claude => {
                if !fog_sees(&fog, tf.translation) {
                    continue;
                }
                if !hit_enemy_unit && enemy.is_none_or(|(_, bd)| d < bd) {
                    enemy = Some((e, d));
                }
            }
            Team::Human => {
                if is_hall(b.kind) && !under && own_depot.is_none_or(|bd| d < bd) {
                    own_depot = Some(d);
                }
            }
        }
    }

    // --- a remembered enemy structure under the cursor? -------------------
    //
    // This closes docs/INTENT.md's "one residual asymmetry". The compiler's
    // rule is `knows_entity` — visible now OR a structure this team remembers
    // — but the loop above only offers what `fog_sees` allows, so a scouted
    // barracks the player is currently looking at a GHOST of was a legal
    // target for a bridge commander and un-clickable for the human. The AI
    // could say something the human could not, which is the one thing
    // THESIS.md's fairness claim does not survive.
    //
    // The picker reads `FogGrid::ghosts()` — the same iterator
    // `sync_building_ghosts` builds the boxes on screen from, so what is
    // clickable is exactly what is drawn, by construction rather than by
    // agreement. The record's `id` is the real entity's `to_bits()`: the same
    // number the bridge names in `{"type":"attack","target":N}`, the same key
    // `knows_entity` looks up. So this produces the *same* `Intent::Attack`
    // against the *same* id, not an attack-move to the remembered position —
    // an attack-move would be a different verb with different behaviour, and
    // "the human has a gesture that is nearly it" is what the gap already was.
    //
    // `ghosts()` never yields a record whose cell is currently visible, so
    // this set and the live-building set above are disjoint and nothing can
    // be picked twice. Enemy units still win, exactly as live buildings lose
    // to them.
    //
    // A ghost whose building has since been razed resolves to a dead entity
    // and the compiler answers `target N not found` — which is precisely what
    // the bridge already gets for the same id, and, now that rejections reach
    // the alert stack, is how the player learns their intel was stale.
    if !hit_enemy_unit {
        for ghost in fog.get(Team::Human).ghosts() {
            let d = dist_xz(ghost.pos, ground);
            if d > building_stats(ghost.kind).size * 0.5 {
                continue;
            }
            let Ok(entity) = Entity::try_from_bits(ghost.id) else {
                continue;
            };
            if enemy.is_none_or(|(_, bd)| d < bd) {
                enemy = Some((entity, d));
            }
        }
    }

    // --- resource node under the cursor? ---------------------------------
    let mut node: Option<(Entity, f32)> = None;
    for (e, tf, res) in &nodes {
        let radius = match res.kind {
            ResourceKind::Gold => MINE_PICK_RADIUS,
            ResourceKind::Lumber => TREE_PICK_RADIUS,
        };
        let d = dist_xz(tf.translation, ground);
        if d <= radius && node.is_none_or(|(_, bd)| d < bd) {
            node = Some((e, d));
        }
    }

    // --- no units selected: production buildings set their rally ----------
    if selected_units.is_empty() {
        let rally_buildings: Vec<Entity> = buildings
            .iter()
            .filter(|(_, _, b, t, sel, _)| {
                *sel && **t == Team::Human && !trainable(b.kind).is_empty()
            })
            .map(|(e, _, _, _, _, _)| e)
            .collect();
        if rally_buildings.is_empty() {
            return;
        }
        // The rally intent names one target the same three ways the bridge
        // does: a node id, an own-unit id, or bare ground coordinates.
        let (x, z, target) = if let Some((n, _)) = node {
            (None, None, Some(intent_id(n)))
        } else if let Some((u, _)) = own_unit {
            (None, None, Some(intent_id(u)))
        } else {
            (Some(ground.x), Some(ground.z), None)
        };
        for e in rally_buildings {
            say(
                &mut submissions,
                Intent::Rally {
                    building: intent_id(e),
                    x,
                    z,
                    region: None,
                    target,
                },
            );
        }
        return;
    }

    // --- enemy under the cursor? -----------------------------------------
    if let Some((target, _)) = enemy {
        say(
            &mut submissions,
            Intent::Attack {
                units: ids(&entities_of(&selected_units)),
                target: intent_id(target),
            },
        );
        return;
    }

    // --- resource node: workers harvest, everyone else walks over ---------
    // A compound gesture is two sentences, not a special case: the workers
    // get a harvest intent and the escorts get a move intent, each with the
    // formation spread intent.rs applies to any group.
    if let Some((target, _)) = node {
        let (workers, others): (Vec<_>, Vec<_>) = selected_units
            .iter()
            .copied()
            .partition(|(_, k, _)| is_worker_kind(*k));
        if !workers.is_empty() {
            say(
                &mut submissions,
                Intent::Harvest {
                    units: ids(&entities_of(&workers)),
                    target: intent_id(target),
                },
            );
        }
        ground_intent(&mut submissions, &entities_of(&others), ground, false);
        return;
    }

    // --- own town hall + loaded workers: drop the cargo off ---------------
    let carriers = selected_units.iter().filter(|(_, _, c)| *c).count();
    if own_depot.is_some() && carriers > 0 {
        let (loaded, others): (Vec<_>, Vec<_>) =
            selected_units.iter().copied().partition(|(_, _, c)| *c);
        say(
            &mut submissions,
            Intent::Return {
                units: ids(&entities_of(&loaded)),
            },
        );
        ground_intent(&mut submissions, &entities_of(&others), ground, false);
        return;
    }

    // --- own unit under the cursor: the rest of the selection escorts it ---
    if let Some((leader, _)) = own_unit {
        let followers: Vec<Entity> = selected_units
            .iter()
            .map(|(e, _, _)| *e)
            .filter(|e| *e != leader) // the leader keeps whatever it was doing
            .collect();
        if !followers.is_empty() {
            say(
                &mut submissions,
                Intent::Follow {
                    units: ids(&followers),
                    target: intent_id(leader),
                },
            );
            return;
        }
        // Only the clicked unit was selected — fall through to a plain move.
    }

    // --- plain ground move with formation spread -------------------------
    ground_intent(&mut submissions, &entities_of(&selected_units), ground, false);
}

// ---------------------------------------------------------------------------
// Placement ghost
// ---------------------------------------------------------------------------

fn update_ghost(
    ui: Res<UiState>,
    assets: Res<UiAssets>,
    nav: Res<NavGrid>,
    economies: Res<Economies>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut ghost: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<Ghost>,
    >,
) {
    let Ok((mut tf, mut vis, mut mat)) = ghost.single_mut() else {
        return;
    };

    let Some(kind) = ui.placement else {
        *vis = Visibility::Hidden;
        return;
    };
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), camera_q.single()) else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(ground) = cursor_to_ground(camera, cam_tf, cursor) else {
        *vis = Visibility::Hidden;
        return;
    };

    let size = building_stats(kind).size;
    let pos = snap_footprint(ground, size);
    let ok = placement_valid(&nav, economies.get(Team::Human), kind, pos);

    tf.translation = Vec3::new(pos.x, 0.2, pos.z);
    tf.scale = Vec3::new(size, 0.35, size);
    *vis = Visibility::Visible;
    let wanted = if ok {
        assets.ghost_ok.clone()
    } else {
        assets.ghost_bad.clone()
    };
    if mat.0 != wanted {
        *mat = MeshMaterial3d(wanted);
    }
}

// ---------------------------------------------------------------------------
// Pending-posture marker
// ---------------------------------------------------------------------------

/// How wide the pending-posture disc is drawn, per posture kind.
///
/// Defend has a REAL radius (`DEFEND_RADIUS`) and the disc is that radius, so
/// the circle the player is aiming is the circle doctrine.rs will hold. Push
/// and Forage name a single point, so they get a small puck instead — big
/// enough to see under the cursor, not big enough to imply an area they do not
/// have. Escort names a unit and never reaches this function.
fn posture_marker_radius(kind: PostureKind) -> Option<f32> {
    match kind {
        PostureKind::Defend => Some(DEFEND_RADIUS),
        PostureKind::Push | PostureKind::Forage => Some(3.0),
        PostureKind::Escort => None,
    }
}

/// The doctrine card's twin of `update_ghost`: while a ground-pointed posture
/// is armed, park a translucent disc on the ground point the cursor is over.
///
/// It uses `clamp_to_map` — the same clamp `posture_intent` applies — so the
/// disc never shows a spot the order would not actually be given at.
fn update_posture_marker(
    ui: Res<UiState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut marker: Query<(&mut Transform, &mut Visibility), With<PostureMarker>>,
) {
    let Ok((mut tf, mut vis)) = marker.single_mut() else {
        return;
    };

    let radius = ui
        .posture_place
        .and_then(|arm| posture_marker_radius(arm.kind));
    let (Some(radius), Ok(window), Ok((camera, cam_tf))) =
        (radius, windows.single(), camera_q.single())
    else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(ground) = window
        .cursor_position()
        .and_then(|cursor| cursor_to_ground(camera, cam_tf, cursor))
    else {
        *vis = Visibility::Hidden;
        return;
    };

    let p = clamp_to_map(ground);
    // Just off the ground plane, under the units rather than through them.
    tf.translation = Vec3::new(p.x, 0.06, p.z);
    tf.scale = Vec3::splat(radius);
    *vis = Visibility::Visible;
}

// ---------------------------------------------------------------------------
// Rally banner
// ---------------------------------------------------------------------------

/// Keeps rally points honest (a rally onto a dead node/unit is dropped) and
/// parks the single pooled banner on the rally of the one selected production
/// building. Hidden when the selection is anything else.
///
/// Query disjointness: the banner owns `&mut Transform` behind `With<RallyFlag>`
/// while every world lookup is `Without<RallyFlag>` — provably non-aliasing.
fn update_rally_flag(
    mut commands: Commands,
    rallied: Query<(Entity, &RallyPoint), With<Building>>,
    selected_buildings: Query<(&Team, &Building, Option<&RallyPoint>), (With<Selected>, With<Building>)>,
    selected_units: Query<(), (With<Unit>, With<Selected>)>,
    positions: Query<&Transform, Without<RallyFlag>>,
    mut flag: Query<(&mut Transform, &mut Visibility), With<RallyFlag>>,
) {
    let Ok((mut tf, mut vis)) = flag.single_mut() else {
        return;
    };

    // The rallied-at entity can die at any moment — forget it when it does.
    for (entity, rally) in &rallied {
        let gone = match rally.target {
            RallyTarget::Ground(_) => false,
            RallyTarget::Node(e) | RallyTarget::Unit(e) => positions.get(e).is_err(),
        };
        if gone {
            commands.entity(entity).try_remove::<RallyPoint>();
        }
    }

    // Exactly one building selected, no units, and it has a live rally.
    let mut show: Option<Vec3> = None;
    if selected_units.iter().next().is_none() {
        let mut iter = selected_buildings.iter();
        if let (Some((team, _, rally)), None) = (iter.next(), iter.next()) {
            if *team == Team::Human {
                show = match rally.map(|r| r.target) {
                    Some(RallyTarget::Ground(p)) => Some(p),
                    // Follows the target entity around while it lives.
                    Some(RallyTarget::Node(e)) | Some(RallyTarget::Unit(e)) => {
                        positions.get(e).ok().map(|t| t.translation)
                    }
                    None => None,
                };
            }
        }
    }

    match show {
        Some(p) => {
            tf.translation = Vec3::new(p.x, 0.0, p.z);
            *vis = Visibility::Visible;
        }
        None => *vis = Visibility::Hidden,
    }
}

// ---------------------------------------------------------------------------
// Chain of Command feedback (docs/TEMPO.md §4, follow-up 7)
//
// Three readouts, one rule: with `WC3_COMMAND_LATENCY` off the HUD is
// pixel-identical to v1. Two of the three get that for free rather than by a
// check — no `PendingOrder` can exist with the feature off, and the node cache
// is never built — and the third asks `latency.on` once.
// ---------------------------------------------------------------------------

/// Height of the node-coverage rings. Above `FOG_PLANE_Y` so a ring is not
/// dimmed by the fog quad: these circles describe your own halls and your own
/// hero, and there is nothing about them you have to scout.
const LINK_RING_Y: f32 = 0.2;
/// The in-transit marker's ring at the moment the order is spoken...
const TRANSIT_RING_MAX: f32 = 5.0;
/// ...and at the moment it lands. It never reaches zero: the last frame before
/// arrival should still be a visible mark on the ground.
const TRANSIT_RING_MIN: f32 = 1.2;

/// Where an order is sending the unit, if it names a place at all.
///
/// `Idle` and `ReturnResources` name none — a returning worker picks its
/// drop-off on arrival, so there is no one point to draw — and neither can be
/// the subject of a delayed direct order anyway (`stop` compiles to a Move to
/// the unit's own feet, precisely so that it *is* one).
fn order_destination(order: &Order, at: impl Fn(Entity) -> Option<Vec3>) -> Option<Vec3> {
    match order {
        Order::Move(p) | Order::AttackMove(p) => Some(*p),
        Order::Build { pos, .. } => Some(*pos),
        Order::Attack(e) | Order::Harvest(e) | Order::Follow(e) => at(*e),
        Order::Idle | Order::ReturnResources => None,
    }
}

/// Draw the free radius of each of the player's own command nodes.
///
/// Own team only, exactly as the snapshot reports it to a commander
/// (docs/TEMPO.md §4: "symmetric with what the HUD shows the human"). The
/// enemy's chain of command is something you learn by razing it.
fn update_link_rings(
    mut commands: Commands,
    assets: Res<UiAssets>,
    link: CommandLink,
    mut rings: Query<(&mut Transform, &mut Visibility), With<LinkRing>>,
) {
    // With the feature off the cache is never refreshed, but ask the flag
    // rather than lean on that: this is the one of the three readouts that
    // could otherwise draw a stale circle.
    let wanted: Vec<(Vec3, f32)> = if link.latency.on {
        link.nodes.own(Team::Human).collect()
    } else {
        Vec::new()
    };

    // Pool: reuse, hide the surplus, spawn the shortfall — the same shape as
    // `update_minimap_bounties`, because halls are built and razed and a hero
    // dies and respawns.
    let mut used = 0usize;
    for (mut tf, mut vis) in &mut rings {
        match wanted.get(used) {
            Some((pos, radius)) => {
                tf.translation = Vec3::new(pos.x, LINK_RING_Y, pos.z);
                tf.scale = Vec3::new(*radius, 0.12, *radius);
                *vis = Visibility::Visible;
            }
            None => *vis = Visibility::Hidden,
        }
        used += 1;
    }
    for (pos, radius) in wanted.iter().skip(used) {
        commands.spawn((
            Mesh3d(assets.hairline_mesh.clone()),
            MeshMaterial3d(assets.node_ring_mat.clone()),
            Transform::from_xyz(pos.x, LINK_RING_Y, pos.z)
                .with_scale(Vec3::new(*radius, 0.12, *radius)),
            LinkRing,
        ));
    }
}

/// Draw every named place on the ground: the map's built-ins, always, and this
/// team's own regions on top of them.
///
/// **Both kinds, always on.** The built-ins were the harder call — they are
/// permanent, they are on every map, and seven faint circles could easily be
/// seven pieces of clutter. They are drawn anyway, and at 16% alpha, because
/// the vocabulary is only shared if the human can SEE what the words mean: a
/// commander that says "5 enemies in the center ford" and a human watching a
/// circle labelled center ford are then demonstrably talking about the same
/// ground. Screenshotted and checked at the default camera height, which is
/// what settled the alpha.
///
/// Own team only for the second list, matching the snapshot exactly: a region
/// is doctrine, and the enemy learns nothing about which ground you decided to
/// care about.
fn update_region_rings(
    mut commands: Commands,
    assets: Res<UiAssets>,
    regions: Res<Regions>,
    mut rings: Query<
        (&mut Transform, &mut Visibility, &mut MeshMaterial3d<StandardMaterial>),
        With<RegionRing>,
    >,
) {
    // Built-ins first so an own region drawn over the same ground (a mark on a
    // ford) is the one on top — the pool preserves order, and the player's own
    // circle is the one they are looking for.
    let mut wanted: Vec<(Vec3, f32, bool)> = builtin_places(Team::Human)
        .into_iter()
        .map(|r| (r.center, r.radius, false))
        .collect();
    wanted.extend(
        regions
            .get(Team::Human)
            .iter()
            .map(|r| (r.center, r.radius, true)),
    );

    // The same pool shape `update_link_rings` uses: reuse, hide the surplus,
    // spawn the shortfall. Regions are set and cleared mid-match, so the count
    // moves.
    let mut used = 0usize;
    for (mut tf, mut vis, mut mat) in &mut rings {
        match wanted.get(used) {
            Some((pos, radius, mine)) => {
                tf.translation = Vec3::new(pos.x, REGION_RING_Y, pos.z);
                tf.scale = Vec3::new(*radius, 0.12, *radius);
                let want = if *mine {
                    assets.region_mine_mat.clone()
                } else {
                    assets.region_map_mat.clone()
                };
                // Assigned every frame rather than compared: a pooled slot can
                // change kind when a region is cleared, and a stale material
                // would draw somebody's mark in the map's colour.
                mat.0 = want;
                *vis = Visibility::Visible;
            }
            None => *vis = Visibility::Hidden,
        }
        used += 1;
    }
    for (pos, radius, mine) in wanted.iter().skip(used) {
        commands.spawn((
            Mesh3d(assets.hairline_mesh.clone()),
            MeshMaterial3d(if *mine {
                assets.region_mine_mat.clone()
            } else {
                assets.region_map_mat.clone()
            }),
            Transform::from_xyz(pos.x, REGION_RING_Y, pos.z)
                .with_scale(Vec3::new(*radius, 0.12, *radius)),
            RegionRing,
        ));
    }
}

/// Region rings sit at the command-ring layer and BELOW `FOG_PLANE_Y` (0.16):
/// a place you have named is not a place you can see into, and a circle that
/// stayed bright through black fog would be the one overlay in this HUD that
/// lied about knowability.
const REGION_RING_Y: f32 = 0.1;

/// **The countdown.** A ring at the destination of every selected unit's
/// in-transit order, closing as the order arrives.
///
/// This is the piece that decides whether the mechanic reads as a game rule or
/// as a broken mouse. A player who clicks and sees nothing happen concludes the
/// game dropped the click; a player who clicks and sees a marker appear where
/// they clicked, and tighten, concludes the order is on its way — which is the
/// truth, and is also the information they need to decide whether to wait.
///
/// No flag check and none needed: `PendingOrder` cannot exist with the feature
/// off, so the query is empty, no marker is ever spawned, and the flag-off HUD
/// is untouched by construction rather than by promise.
#[allow(clippy::type_complexity)]
fn update_transit_markers(
    mut commands: Commands,
    time: Res<Time>,
    assets: Res<UiAssets>,
    travelling: Query<(&Team, &PendingOrder), (With<Selected>, With<Unit>)>,
    targets: Query<&Transform, Without<TransitRing>>,
    mut markers: Query<(&mut Transform, &mut Visibility), With<TransitRing>>,
) {
    let now = time.elapsed_secs();
    let at = |e: Entity| targets.get(e).ok().map(|tf| tf.translation);

    let wanted: Vec<(Vec3, f32)> = travelling
        .iter()
        .filter(|(team, _)| **team == Team::Human)
        .filter_map(|(_, pending)| {
            let dest = order_destination(&pending.order, at)?;
            // How much of the journey is left, 1 at the moment it was spoken
            // and 0 as it lands. `link()` is never zero here — a zero-delay
            // order is applied instantly and never becomes a `PendingOrder`.
            let remaining = ((pending.ready_at - now) / pending.link()).clamp(0.0, 1.0);
            Some((dest, remaining))
        })
        .collect();

    let mut used = 0usize;
    for (mut tf, mut vis) in &mut markers {
        match wanted.get(used) {
            Some((dest, remaining)) => {
                let r = TRANSIT_RING_MIN + (TRANSIT_RING_MAX - TRANSIT_RING_MIN) * remaining;
                tf.translation = Vec3::new(dest.x, LINK_RING_Y, dest.z);
                tf.scale = Vec3::new(r, 0.12, r);
                *vis = Visibility::Visible;
            }
            None => *vis = Visibility::Hidden,
        }
        used += 1;
    }
    for (dest, _) in wanted.iter().skip(used) {
        commands.spawn((
            Mesh3d(assets.ring_mesh.clone()),
            MeshMaterial3d(assets.transit_mat.clone()),
            Transform::from_xyz(dest.x, LINK_RING_Y, dest.z)
                .with_scale(Vec3::new(TRANSIT_RING_MAX, 0.12, TRANSIT_RING_MAX)),
            TransitRing,
        ));
    }
}

// ---------------------------------------------------------------------------
// Selection rings
// ---------------------------------------------------------------------------

fn sync_selection_rings(
    mut commands: Commands,
    assets: Res<UiAssets>,
    newly_selected: Query<
        (Entity, &Transform, Option<&Building>, Option<&Unit>),
        (With<Selected>, Without<HasRing>),
    >,
    deselected: Query<(Entity, &HasRing), Without<Selected>>,
) {
    for (entity, tf, building, unit) in &newly_selected {
        let radius = match building {
            Some(b) => building_stats(b.kind).size * 0.62,
            None => 1.1,
        };
        // A flyer's ring rides with it. Cancelling its height like a ground
        // unit's would leave the ring on the dirt six units below the model,
        // where it reads as a selection of something else entirely.
        let flying = unit.is_some_and(|u| is_flying_kind(u.kind));
        let ring_world_y = if flying { tf.translation.y - 1.2 } else { 0.08 };
        let ring = commands
            .spawn((
                Mesh3d(assets.ring_mesh.clone()),
                MeshMaterial3d(assets.ring_mat.clone()),
                // Local offset cancels the parent's height so the ring lies flat
                // on the ground; flattened to a thin disc via Y scale.
                // Child coords: divide by the parent's scale (units are
                // UNIT_SCALE'd, constructing buildings are Y-squashed) so the
                // ring lands at world y ~0.08 regardless.
                Transform::from_xyz(
                    0.0,
                    (ring_world_y - tf.translation.y) / tf.scale.y.max(0.001),
                    0.0,
                )
                    .with_scale(Vec3::new(radius, 0.12, radius)),
                SelectionRing,
                ChildOf(entity),
            ))
            .id();
        commands.entity(entity).try_insert(HasRing(ring));
    }

    for (entity, ring) in &deselected {
        commands.entity(ring.0).try_despawn();
        commands.entity(entity).try_remove::<HasRing>();
    }
}

// ---------------------------------------------------------------------------
// Minimap rendering
// ---------------------------------------------------------------------------

/// Trees, gold mines and impassable terrain never move — one dot each, spawned
/// on the first frame after terrain.rs has created them (Startup ordering isn't
/// guaranteed).
///
/// The terrain dots matter for fairness as much as for convenience: a bridge
/// commander is told the map's chokepoints in every snapshot, so the human must
/// be able to see where the ground is closed without panning the camera along
/// it.
fn minimap_static_markers(
    hud: Res<HudLayout>,
    mut commands: Commands,
    mut done: Local<bool>,
    root: Query<Entity, With<MinimapRoot>>,
    nodes: Query<(&Transform, &ResourceNode)>,
) {
    if *done || nodes.is_empty() {
        return;
    }
    let Ok(root) = root.single() else {
        return;
    };
    // Gold mines draw ABOVE the fog layer, trees below it. Mine positions are
    // map geography — they ship unfiltered in every bridge snapshot, so hiding
    // them from the player would be the asymmetry running backwards. Tree
    // clusters are scenery and can sit in the dark like the rest of it.
    for (tf, node) in &nodes {
        let (size, color, z) = match node.kind {
            ResourceKind::Gold => (5.0, Color::srgb(1.0, 0.82, 0.25), 2),
            ResourceKind::Lumber => (2.0, Color::srgb(0.16, 0.42, 0.18), 0),
        };
        let p = world_to_minimap(tf.translation, hud.minimap_px);
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(p.x - size * 0.5),
                top: Val::Px(p.y - size * 0.5),
                width: Val::Px(size),
                height: Val::Px(size),
                ..default()
            },
            BackgroundColor(color),
            ZIndex(z),
            MinimapStatic,
            ChildOf(root),
        ));
    }

    // Impassable terrain (none on the open map): dots slightly larger than a
    // nav cell so the barrier reads as one continuous wall.
    let rock = Color::srgb(0.26, 0.26, 0.30);
    for cell in crate::terrain::barrier_cells() {
        let p = world_to_minimap(cell, hud.minimap_px);
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(p.x - 1.25),
                top: Val::Px(p.y - 1.25),
                width: Val::Px(2.5),
                height: Val::Px(2.5),
                ..default()
            },
            BackgroundColor(rock),
            // Above the fog: the map's layout is public information, and the
            // snapshot's `map.chokes` says so to the other player.
            ZIndex(2),
            MinimapStatic,
            ChildOf(root),
        ));
    }
    *done = true;
}

/// Bounty caches: a bright-gold dot that pulses so it stands out from the
/// static gold-mine dots it shares a colour family with. Same pooled pattern as
/// `update_minimap` (mutate in place, never despawn) on its own small pool.
// ---------------------------------------------------------------------------
// Fog of war — the player's renderer of `shared::FogGrids`
// ---------------------------------------------------------------------------
//
// Nothing here decides anything. shared.rs computes one grid per team at ~4 Hz
// and bridge.rs filters a commander's snapshot through it; these systems draw
// the identical grid for the player at the keyboard. That is the whole point:
// if this file made its own judgement about what the human may see, the game
// would have two definitions of knowability again, and the one the machine got
// would quietly be the better one.
//
// Three renderings of the same array:
//
//   * a translucent black quad over the whole ground plane, textured with the
//     grid itself — one 100x100 image, one entity, linearly filtered so the
//     boundary is soft rather than a staircase of nav cells;
//   * the SAME image on the minimap as an `ImageNode` (flipped, because the
//     minimap puts +Z up while the texture puts +Z at the bottom), so the two
//     views can never disagree;
//   * translucent boxes standing in for enemy structures the player has
//     scouted and can no longer see.
//
// And one system that is not drawing at all but hiding: `apply_fog_visibility`
// takes enemy units and buildings out of the 3D scene entirely. Health bars are
// children of their owner, so they inherit the hidden state for free.

/// Above the ground plane and its cosmetic patches (0.02-0.046) and above the
/// selection/hover rings (0.08/0.1), below the placement ghost's box. Low
/// enough that it reads as lying ON the ground rather than hanging over it.
const FOG_PLANE_Y: f32 = 0.16;

/// The single ground-plane fog quad.
#[derive(Component)]
struct FogPlane;

/// The minimap's fog image node.
#[derive(Component)]
struct MinimapFog;

/// A pooled stand-in for an enemy structure the player remembers but cannot
/// currently see.
#[derive(Component)]
struct BuildingGhost;

/// A pooled marker on the ground where the player last SAW an enemy unit.
///
/// Deliberately a different shape from `BuildingGhost` — a flat tile lying on
/// the earth rather than a standing box — because the two memories mean
/// different things. A ghost says *there is a barracks there*; a tile says
/// *something stood here once*, and confusing the two would be worse than
/// drawing neither.
#[derive(Component)]
struct IntelMarker;

/// How many discrete age-fade materials a last-seen marker picks from.
///
/// Four handles, pre-built, swapped by the frame — never one material
/// repainted. That is the `FogTinted` discipline and it is here for the
/// identical reason: a `StandardMaterial`'s bind group is rebuilt only when
/// the material asset changes, so anything repainted in place silently goes on
/// rendering last time's colour. See `update_fog_overlay`.
const INTEL_FADE_STEPS: usize = 4;

/// Which fade a sighting of this age wears. Oldest step at the horizon, so a
/// marker is at its faintest just before the ledger drops it and nothing ever
/// blinks out at full strength.
fn intel_fade_step(age: f32) -> usize {
    let t = (age / SIGHTING_TTL_S).clamp(0.0, 1.0);
    ((t * INTEL_FADE_STEPS as f32) as usize).min(INTEL_FADE_STEPS - 1)
}

#[derive(Resource)]
struct FogAssets {
    /// Shared by the world quad's material and the minimap node — the literal
    /// "computed once, rendered twice".
    image: Handle<Image>,
    /// The ground quad's material. Held so `update_fog_overlay` can republish
    /// it and force its bind group to be rebuilt against the freshly uploaded
    /// texture; see the note there.
    fog_mat: Handle<StandardMaterial>,
    ghost_mesh: Handle<Mesh>,
    ghost_mat: Handle<StandardMaterial>,
    /// A flat tile for the last-seen unit markers.
    intel_mesh: Handle<Mesh>,
    /// One material per age band, freshest first. `INTEL_FADE_STEPS` long.
    intel_mats: Vec<Handle<StandardMaterial>>,
}

/// Build the fog texture, the quad that wears it, and the minimap node that
/// wears the same one. Runs after `setup_ui` because it needs `MinimapRoot`.
fn setup_fog(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    minimap: Query<Entity, With<MinimapRoot>>,
) {
    // One texel per nav cell: fog reuses the nav grid's geometry all the way
    // out to the screen.
    let image = images.add(Image::new_fill(
        Extent3d {
            width: GRID_DIM as u32,
            height: GRID_DIM as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        // Start fully dark: the first `update_fog_overlay` lights the opening
        // position, and an unpainted first frame should hide the map rather
        // than reveal it.
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));

    // A hand-built quad rather than `Plane3d`, so the UV mapping is pinned to
    // the grid's own convention (u along +X, v along +Z) instead of whatever
    // the mesh builder happens to emit.
    let mut quad = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    quad.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-MAP_HALF, 0.0, -MAP_HALF],
            [MAP_HALF, 0.0, -MAP_HALF],
            [MAP_HALF, 0.0, MAP_HALF],
            [-MAP_HALF, 0.0, MAP_HALF],
        ],
    );
    quad.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    );
    quad.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    quad.insert_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]));

    let fog_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(image.clone()),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        // The quad is viewed from above, but leaving culling off costs nothing
        // and removes winding as a failure mode.
        cull_mode: None,
        ..default()
    });

    commands.spawn((
        Mesh3d(meshes.add(quad)),
        MeshMaterial3d(fog_mat.clone()),
        Transform::from_xyz(0.0, FOG_PLANE_Y, 0.0),
        FogPlane,
    ));

    let ghost_mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    // Washed-out and translucent: a memory should never be mistaken for a
    // sighting.
    let ghost_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.62, 0.38, 0.36, 0.40),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    // Last-seen unit markers: a flat amber tile, four pre-built fades deep.
    // Amber rather than the ghost's dusty red so the two memories are told
    // apart at a glance, and low enough to read as a mark ON the ground
    // instead of a thing standing on it.
    let intel_mesh = meshes.add(Cuboid::new(1.0, 0.12, 1.0));
    let intel_mats: Vec<Handle<StandardMaterial>> = (0..INTEL_FADE_STEPS)
        .map(|i| {
            let t = i as f32 / (INTEL_FADE_STEPS - 1) as f32;
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.90, 0.58, 0.28, 0.52 - 0.38 * t),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })
        })
        .collect();

    if let Ok(root) = minimap.single() {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(HudLayout::default().minimap_px),
                height: Val::Px(HudLayout::default().minimap_px),
                ..default()
            },
            ImageNode {
                image: image.clone(),
                // The minimap draws +Z upward; the texture stores +Z downward.
                flip_y: true,
                ..default()
            },
            // Above the pooled dots (which are spawned later and would
            // otherwise win), below the camera viewport outline.
            ZIndex(1),
            MinimapFog,
            ChildOf(root),
        ));
    }

    commands.insert_resource(FogAssets {
        image,
        fog_mat,
        ghost_mesh,
        ghost_mat,
        intel_mesh,
        intel_mats,
    });
}

/// How much black the overlay lays over a cell in each of the three states.
///
/// The whole legibility of the fog is these three numbers: a spectator should
/// be able to read a team's vision off the ground without hunting for the
/// boundary, so they are spread deliberately wide rather than being a gentle
/// ramp — 0.0 / 0.44 / 0.88.
///
/// It is now *derived* rather than declared, and that is the point.
/// `shared::fog_shade` says how much of a thing's colour survives at each
/// state; the quad's job is to remove the rest, and the scenery tint's job is
/// to keep exactly that much. Two renderers, one rule, no way for them to
/// disagree about what "remembered" looks like.
fn fog_alpha(cell: CellVis) -> f32 {
    1.0 - fog_shade(cell)
}

/// Dress every fog-tinted doodad in its cell's shade.
///
/// The flat-quad limitation, closed. The overlay lies on the ground at
/// `FOG_PLANE_Y` (0.16) and can therefore only darken things shorter than
/// 0.16 — which is nothing. A rock is a sphere half a unit across; a pine's
/// canopy is four units up. Both stood in full sun over black earth, and
/// docs/FOG.md named it as the known limitation a shader would fix.
///
/// This is that fix, done with materials rather than WGSL: every doodad owns
/// three pre-built copies of its own look (`FogTinted`), and all this system
/// does is decide which one it wears. No shader, no extended material, no
/// per-frame texture upload — and, crucially, no repaint, so the bind-group
/// staleness trap that `update_fog_overlay` has to defend against with a
/// republish simply does not exist here. Swapping the handle *is* the update.
///
/// Trees are still hidden outright while `Unexplored` (`apply_fog_visibility`),
/// and the reason has changed: it used to be a rendering workaround, and it is
/// now an information rule. Where a forest stands is worth knowing — it is
/// lumber, and it is cover — so a team that has never been there does not get
/// to see its silhouette, however dark.
fn apply_fog_tint(
    fog: Res<FogGrids>,
    mut tinted: Query<(
        &FogTinted,
        &GlobalTransform,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    // `WC3_FOG=0` hands back a fully-lit grid, so this resolves to the
    // `Visible` shade everywhere — the same "no branch at the call site"
    // discipline the rest of the fog code keeps.
    let grid = fog.get(Team::Human);
    for (shades, gt, mut mat) in &mut tinted {
        // `GlobalTransform`, so a leaf cluster is shaded by the ground its
        // trunk stands on rather than by wherever its local offset points.
        let want = shades.at(grid.at(gt.translation()));
        if mat.0.id() != want.id() {
            mat.0 = want.clone();
        }
    }
}

/// Repaint the fog texture from the human's grid. Cheap enough to do every
/// frame (40 KB) and doing so keeps the overlay in lockstep with the 4 Hz
/// recompute without a second clock to get out of sync.
fn update_fog_overlay(
    fog: Res<FogGrids>,
    assets: Res<FogAssets>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut plane: Query<&mut Visibility, With<FogPlane>>,
    mut minimap_fog: Query<&mut Node, With<MinimapFog>>,
) {
    // `WC3_FOG=0`: take the overlay off the screen entirely rather than
    // painting a fully transparent one every frame.
    if !fog.enabled() {
        for mut vis in &mut plane {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
        for mut node in &mut minimap_fog {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    }

    let Some(image) = images.get_mut(&assets.image) else {
        return;
    };
    let Some(data) = image.data.as_mut() else {
        return;
    };
    for (i, cell) in fog.get(Team::Human).cells().iter().enumerate() {
        // Texel layout is `NavGrid::idx` order, which is exactly the grid's
        // own iteration order — no transposition anywhere in the pipeline.
        data[i * 4 + 3] = (fog_alpha(*cell) * 255.0) as u8;
    }
    // Repainting `data` marks the *image* asset modified, which re-uploads it
    // and is all the minimap needs: the UI pipeline resolves the image handle
    // to its current `GpuImage` every frame, so an `ImageNode` always samples
    // the newest texture.
    //
    // A mesh material does NOT. A `StandardMaterial`'s bind group is built once
    // and rebuilt only when the *material* asset changes, so it goes on
    // pointing at the `GpuImage` that existed when it was prepared — the
    // opening frame, where the start base is lit and nothing is explored yet.
    // That is the whole bug: the minimap tracked the match while the ground
    // wore a snapshot of the first quarter-second forever, which reads exactly
    // like "the terrain has no fog on it at all".
    //
    // Touching the material republishes it so its bind group is rebuilt
    // against the current texture. One material, once a frame.
    let _ = materials.get_mut(&assets.fog_mat);
}

/// Take enemy units and buildings the player cannot see out of the 3D scene.
///
/// The same rule bridge.rs applies to a seat's snapshot, applied to the
/// player's eyes: an enemy is drawn only while visible, and a scouted
/// structure is replaced by a ghost (below) rather than left standing — which
/// matters, because leaving the real building rendered would keep reporting
/// its health and its destruction to somebody with nothing watching it.
fn apply_fog_visibility(
    fog: Res<FogGrids>,
    mut units: Query<(&Team, &Transform, &mut Visibility), (With<Unit>, Without<Building>)>,
    mut buildings: Query<(&Team, &Transform, &mut Visibility), (With<Building>, Without<Unit>)>,
    mut trees: Query<
        (&ResourceNode, &Transform, &mut Visibility),
        (Without<Unit>, Without<Building>),
    >,
) {
    if !fog.enabled() {
        return;
    }
    let grid = fog.get(Team::Human);

    // Tree clusters are hidden until their ground has been EXPLORED (not
    // seen — terrain is remembered), for a reason that is as much correctness
    // as polish: the fog overlay is a flat quad lying on the ground plane, so
    // anything tall enough pokes through it and a forest in never-visited
    // terrain would stand there fully lit above a black floor. Gold mines are
    // exempt: mine positions are public geography, they ship unfiltered in
    // every bridge snapshot, and the minimap draws them above the fog for the
    // same reason.
    for (node, tf, mut vis) in &mut trees {
        if node.kind == ResourceKind::Gold {
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
            continue;
        }
        let want = if grid.known(tf.translation) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
    let apply = |team: &Team, tf: &Transform, vis: &mut Visibility| {
        let want = if *team == Team::Human || grid.sees(tf.translation) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    };
    for (team, tf, mut vis) in &mut units {
        apply(team, tf, &mut vis);
    }
    for (team, tf, mut vis) in &mut buildings {
        apply(team, tf, &mut vis);
    }
}

/// Pooled translucent boxes where the player remembers enemy structures.
/// Position, footprint and existence come from the shared grid's memory, so
/// what the player sees standing in the fog is precisely what a bridge

/// One outlined circle per named place on the minimap.
///
/// The minimap is where a region earns its keep: the 3D ring is only visible
/// where the camera is pointing, and the whole reason to name `north-pass` is
/// to reason about ground you are NOT looking at. Same two-tone scheme as the
/// world rings, so the two readouts are one picture.
///
/// `MinimapStatic` would have been wrong here even though built-ins never move:
/// own regions are set and cleared mid-match, and one pooled system for both
/// keeps the layering rule in one place.
fn update_minimap_regions(
    mut commands: Commands,
    hud: Res<HudLayout>,
    regions: Res<Regions>,
    root: Query<Entity, With<MinimapRoot>>,
    mut rings: Query<(&mut Node, &mut BorderColor, &mut Visibility), With<MinimapRegion>>,
) {
    let Ok(root) = root.single() else {
        return;
    };
    let mut wanted: Vec<(Vec3, f32, bool)> = builtin_places(Team::Human)
        .into_iter()
        .map(|r| (r.center, r.radius, false))
        .collect();
    wanted.extend(
        regions
            .get(Team::Human)
            .iter()
            .map(|r| (r.center, r.radius, true)),
    );

    let px = hud.minimap_px;
    // World units -> minimap pixels. `world_to_minimap` maps the whole
    // 2*MAP_HALF span onto `px`, so a radius scales by the same ratio.
    let scale = px / (2.0 * MAP_HALF);
    let mut used = 0usize;
    for (mut node, mut border, mut vis) in &mut rings {
        match wanted.get(used) {
            Some((pos, radius, mine)) => {
                let c = world_to_minimap(*pos, px);
                let r = (radius * scale).max(1.5);
                node.left = Val::Px(c.x - r);
                node.top = Val::Px(c.y - r);
                node.width = Val::Px(r * 2.0);
                node.height = Val::Px(r * 2.0);
                *border = BorderColor(region_minimap_color(*mine));
                *vis = Visibility::Visible;
            }
            None => *vis = Visibility::Hidden,
        }
        used += 1;
    }
    for (pos, radius, mine) in wanted.iter().skip(used) {
        let c = world_to_minimap(*pos, px);
        let r = (radius * scale).max(1.5);
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(c.x - r),
                top: Val::Px(c.y - r),
                width: Val::Px(r * 2.0),
                height: Val::Px(r * 2.0),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderRadius::MAX,
            BorderColor(region_minimap_color(*mine)),
            BackgroundColor(Color::NONE),
            // Above the fog layer (1), with the mines and pings. A named place
            // is something you know rather than something you see, so unlike a
            // unit dot it is not hidden by fog — the same rule the 3D ring
            // follows by sitting under the fog plane and this one has to state.
            ZIndex(2),
            MinimapRegion,
            ChildOf(root),
        ));
    }
}

/// Minimap ink for the two kinds of named place. Brighter than the world rings
/// at both ends: a 100px map has no room for a 16%-alpha hairline.
fn region_minimap_color(mine: bool) -> Color {
    if mine {
        Color::srgba(1.0, 0.78, 0.35, 0.85)
    } else {
        Color::srgba(0.72, 0.78, 0.86, 0.34)
    }
}

/// commander receives as a `last_seen` building record.
fn sync_building_ghosts(
    mut commands: Commands,
    fog: Res<FogGrids>,
    assets: Res<FogAssets>,
    mut ghosts: Query<(&mut Transform, &mut Visibility), With<BuildingGhost>>,
) {
    let wanted: Vec<(Vec3, f32)> = if fog.enabled() {
        fog.get(Team::Human)
            .ghosts()
            .map(|g| (g.pos, building_stats(g.kind).size))
            .collect()
    } else {
        Vec::new()
    };

    let mut used = 0usize;
    for (mut tf, mut vis) in &mut ghosts {
        match wanted.get(used) {
            Some((pos, size)) => {
                let height = size * 0.6;
                tf.translation = Vec3::new(pos.x, height * 0.5, pos.z);
                tf.scale = Vec3::new(*size, height, *size);
                if *vis != Visibility::Inherited {
                    *vis = Visibility::Inherited;
                }
            }
            None => {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
        }
        used += 1;
    }
    // Grow the pool; never shrink it (same discipline as the minimap dots).
    for _ in used..wanted.len() {
        commands.spawn((
            Mesh3d(assets.ghost_mesh.clone()),
            MeshMaterial3d(assets.ghost_mat.clone()),
            Transform::from_xyz(0.0, -50.0, 0.0),
            Visibility::Hidden,
            BuildingGhost,
        ));
    }
}

/// Pooled ground tiles where the player last saw an enemy unit — the human's
/// rendering of the snapshot's `intel.sightings`.
///
/// Position, age and existence come from the shared ledger, so what the player
/// sees fading in the fog is precisely what a bridge commander receives as a
/// sighting record. Same knowability, both renderers — which is the promise
/// this whole system is arranged to keep, now extended from structures to the
/// things that move.
///
/// Markers under **currently visible** ground are suppressed, on exactly the
/// rule `FogGrid::ghosts()` applies: sight beats memory, and a "something was
/// here" tile lying on grass the player is looking at right now is memory
/// arguing with eyes. Walk the scout on and the tile appears behind it.
fn sync_intel_markers(
    mut commands: Commands,
    time: Res<Time>,
    fog: Res<FogGrids>,
    assets: Res<FogAssets>,
    mut markers: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<IntelMarker>,
    >,
) {
    let now = time.elapsed_secs();
    let grid = fog.get(Team::Human);
    let wanted: Vec<(Vec3, usize)> = if fog.enabled() {
        grid.sightings()
            .filter(|s| !grid.sees(s.pos))
            .map(|s| (s.pos, intel_fade_step(s.age(now))))
            .collect()
    } else {
        Vec::new()
    };

    let mut used = 0usize;
    for (mut tf, mut vis, mut mat) in &mut markers {
        match wanted.get(used) {
            Some((pos, step)) => {
                // Just above the fog quad at 0.16, so a remembered contact is
                // legible on remembered ground rather than buried under it.
                tf.translation = Vec3::new(pos.x, 0.20, pos.z);
                tf.scale = Vec3::new(1.8, 1.0, 1.8);
                let want = &assets.intel_mats[*step];
                if mat.0 != *want {
                    mat.0 = want.clone();
                }
                if *vis != Visibility::Inherited {
                    *vis = Visibility::Inherited;
                }
            }
            None => {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
        }
        used += 1;
    }
    // Grow the pool; never shrink it (same discipline as the ghost boxes).
    for (pos, step) in wanted.iter().skip(used) {
        commands.spawn((
            Mesh3d(assets.intel_mesh.clone()),
            MeshMaterial3d(assets.intel_mats[*step].clone()),
            Transform::from_xyz(pos.x, 0.20, pos.z).with_scale(Vec3::new(1.8, 1.0, 1.8)),
            Visibility::Inherited,
            IntelMarker,
        ));
    }
}

fn update_minimap_bounties(
    hud: Res<HudLayout>,
    mut commands: Commands,
    time: Res<Time>,
    root: Query<Entity, With<MinimapRoot>>,
    bounties: Query<&Transform, With<Bounty>>,
    fog: Res<FogGrids>,
    mut markers: Query<&mut Node, With<MinimapBounty>>,
) {
    let Ok(root) = root.single() else {
        return;
    };
    let grid = fog.get(Team::Human);
    // 5px to 6px and back, ~2.5 rad/s — a slow, unmistakable throb.
    let size = 5.5 + 0.5 * (time.elapsed_secs() * 2.5).sin();
    // Only caches the player can see, matching the `bounties` array a bridge
    // commander gets. Treasure is neutral, but noticing it is not free.
    let wanted: Vec<Vec2> = bounties
        .iter()
        .filter(|tf| grid.sees(tf.translation))
        .map(|tf| world_to_minimap(tf.translation, hud.minimap_px))
        .collect();

    let mut used = 0usize;
    for mut node in &mut markers {
        match wanted.get(used) {
            Some(p) => {
                node.display = Display::Flex;
                node.left = Val::Px(p.x - size * 0.5);
                node.top = Val::Px(p.y - size * 0.5);
                node.width = Val::Px(size);
                node.height = Val::Px(size);
            }
            None => node.display = Display::None,
        }
        used += 1;
    }
    for _ in used..wanted.len() {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                width: Val::Px(size),
                height: Val::Px(size),
                ..default()
            },
            BackgroundColor(BOUNTY_DOT),
            MinimapBounty,
            ChildOf(root),
        ));
    }
}

fn update_minimap(
    hud: Res<HudLayout>,
    mut commands: Commands,
    root: Query<Entity, With<MinimapRoot>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    units: Query<(&Transform, &Team, Has<Hero>), (With<Unit>, Without<Building>)>,
    buildings: Query<(&Transform, &Team), (With<Building>, Without<Unit>)>,
    fog: Res<FogGrids>,
    mut markers: Query<
        (&mut Node, &mut BackgroundColor),
        (With<MinimapMarker>, Without<MinimapViewport>),
    >,
    mut viewport: Query<&mut Node, (With<MinimapViewport>, Without<MinimapMarker>)>,
) {
    let Ok(root) = root.single() else {
        return;
    };
    let grid = fog.get(Team::Human);
    // The minimap is the most tempting place in the game to cheat, because a
    // dot costs nothing to draw and reveals everything. Same rule as the 3D
    // scene and the same rule as a bridge snapshot: ours always, theirs only
    // while seen.
    let known = |team: &Team, p: Vec3| *team == Team::Human || grid.sees(p);

    // Desired dots: units first, then buildings (drawn later == on top).
    let mut wanted: Vec<(Vec2, f32, Color)> = Vec::new();
    for (tf, team, is_hero) in &units {
        if !known(team, tf.translation) {
            continue;
        }
        // Heroes read as bigger, brighter dots.
        let (size, color) = if is_hero {
            (5.0, lighten(team.color(), 0.35))
        } else {
            (3.0, team.color())
        };
        wanted.push((world_to_minimap(tf.translation, hud.minimap_px), size, color));
    }
    for (tf, team) in &buildings {
        if !known(team, tf.translation) {
            continue;
        }
        wanted.push((
            world_to_minimap(tf.translation, hud.minimap_px),
            6.0,
            lighten(team.color(), 0.12),
        ));
    }
    // Scouted enemy structures, dimmed. They also sit under the `Explored`
    // shading of the fog layer, so a remembered base reads as distinctly
    // fainter than one being looked at.
    for ghost in grid.ghosts() {
        wanted.push((
            world_to_minimap(ghost.pos, hud.minimap_px),
            6.0,
            lighten(ghost.team.color(), -0.35),
        ));
    }
    // Where enemy UNITS were last seen. Two pixels: smaller than a live
    // contact (3.0) and far smaller than a structure (6.0), and darkened on
    // top of that, so a memory can never be misread as a unit standing there.
    // Suppressed over visible ground for the same reason the world markers
    // are — sight beats memory.
    for s in grid.sightings() {
        if grid.sees(s.pos) {
            continue;
        }
        wanted.push((
            world_to_minimap(s.pos, hud.minimap_px),
            2.0,
            lighten(s.team.color(), -0.25),
        ));
    }

    // Mutate the pool in place; never despawn.
    let mut used = 0usize;
    for (mut node, mut bg) in &mut markers {
        match wanted.get(used) {
            Some((p, size, color)) => {
                node.display = Display::Flex;
                node.left = Val::Px(p.x - size * 0.5);
                node.top = Val::Px(p.y - size * 0.5);
                node.width = Val::Px(*size);
                node.height = Val::Px(*size);
                bg.0 = *color;
            }
            None => node.display = Display::None,
        }
        used += 1;
    }
    for _ in used..wanted.len() {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                width: Val::Px(3.0),
                height: Val::Px(3.0),
                ..default()
            },
            BackgroundColor(Color::WHITE),
            MinimapMarker,
            ChildOf(root),
        ));
    }

    // --- camera viewport outline ------------------------------------------
    let Ok(mut vp) = viewport.single_mut() else {
        return;
    };
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), camera_q.single()) else {
        return;
    };
    let (w, h) = (window.width(), window.height());
    let mut min = Vec2::splat(f32::MAX);
    let mut max = Vec2::splat(f32::MIN);
    let mut hits = 0;
    for corner in [
        Vec2::new(0.0, 0.0),
        Vec2::new(w, 0.0),
        Vec2::new(0.0, h),
        Vec2::new(w, h),
    ] {
        if let Some(p) = cursor_to_ground(camera, cam_tf, corner) {
            let limit = MAP_HALF * 3.0;
            let v = Vec2::new(p.x.clamp(-limit, limit), p.z.clamp(-limit, limit));
            min = min.min(v);
            max = max.max(v);
            hits += 1;
        }
    }
    if hits < 2 {
        vp.display = Display::None;
        return;
    }
    // +Z maps to smaller Y on the minimap, so the corners swap.
    let a = world_to_minimap(Vec3::new(min.x, 0.0, max.y), hud.minimap_px);
    let b = world_to_minimap(Vec3::new(max.x, 0.0, min.y), hud.minimap_px);
    vp.display = Display::Flex;
    vp.left = Val::Px(a.x);
    vp.top = Val::Px(a.y);
    vp.width = Val::Px((b.x - a.x).max(2.0));
    vp.height = Val::Px((b.y - a.y).max(2.0));
}

// ---------------------------------------------------------------------------
// HUD refresh
// ---------------------------------------------------------------------------

struct CardView {
    entity: Entity,
    letter: String,
    hp: f32,
    color: Color,
    /// Squad membership, for the corner badge. `None` for a unit in no squad
    /// and for every building — a building is never a squad member, it stamps
    /// one via its template.
    squad: Option<u8>,
}

/// Everything the panel needs to answer **"why is this selection doing that,
/// and what would it cost me to change its mind?"** — one `SystemParam` because
/// they are one question, and because `update_hud` sits on Bevy's
/// 16-parameter ceiling and Chain of Command would otherwise have needed three
/// of the slots on its own.
#[allow(clippy::type_complexity)]
#[derive(SystemParam)]
struct SelectionReasons<'w, 's> {
    /// The selection's `Provenance`, verbatim what the snapshot reports.
    why: Query<'w, 's, (&'static Team, Option<&'static Provenance>), (With<Selected>, With<Unit>)>,
    /// The curve and the node cache — `link.delay(team, pos)` is the estimate
    /// the panel prints and the snapshot's `units[].link` reports.
    link: CommandLink<'w>,
    /// The selection's positions and anything already travelling to it.
    selected: Query<
        'w,
        's,
        (&'static Team, &'static Transform, Option<&'static PendingOrder>),
        (With<Selected>, With<Unit>),
    >,
    /// Every unit on the map — the coverage indicator is about the whole army,
    /// not the part of it that happens to be selected.
    all: Query<'w, 's, (&'static Team, &'static Transform), With<Unit>>,
    /// The intel ledger, for the enemy-hero line. It rides in this bundle
    /// rather than as a parameter of its own because `update_hud` is already
    /// on Bevy's 16-parameter ceiling — see the note on that function.
    fog: Res<'w, FogGrids>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_hud(
    mut ui: ResMut<UiState>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    game_over: Res<GameOver>,
    ai_controlled: Res<AiControlled>,
    // The same reads `command_input` builds its entries from — tech tier, the
    // squad postures doctrine.rs is currently executing, ability cooldowns and
    // team research — so the card the player sees and the card the keyboard
    // dispatches against are computed from one set of facts.
    cast: CastLookup,
    // Latched the frame the match ends: was this an AI-vs-AI spectate?
    mut spectated: Local<Option<bool>>,
    mut texts: Query<(&Slot, &mut Text, &mut TextColor)>,
    mut nodes: Query<(&El, &mut Node)>,
    mut colors: Query<(&El, &mut BackgroundColor, Option<&Interaction>)>,
    // Read-only view of every hero on the map (slot tally, card labels, and
    // the Shop's customer + its inventory). `Unit` for the class.
    heroes: Query<(&Team, &Unit, Option<&Inventory>), With<Hero>>,
    // Read-only: which buildings the player has FINISHED (the tech gate), and
    // what is in their queues (heroes in flight spend a slot).
    // `Transform` and `Health` ride along for ONE reader: the hall-pick hint,
    // which names the hall that is bleeding. See the hint block below for why
    // the answer is a sentence rather than a highlight.
    all_buildings: Query<(
        &Building,
        &Team,
        &Transform,
        &Health,
        Has<UnderConstruction>,
        Option<&TrainingQueue>,
    )>,
    sel_units: Query<
        (
            Entity,
            &Unit,
            &Health,
            &Team,
            Option<&Carrying>,
            Option<&Hero>,
            Option<&LeashPolicy>,
            Option<&RetreatPolicy>,
            Option<&TargetPriority>,
            Option<&AutoCastPolicy>,
            Option<&Inventory>,
            Has<Militia>,
            Option<&SquadId>,
        ),
        With<Selected>,
    >,
    // Kept out of `sel_units` rather than added as a 14th column: provenance
    // is orthogonal to everything that panel shows, and widening that tuple
    // means editing five positional destructures.
    reasons: SelectionReasons,
    sel_buildings: Query<
        (
            Entity,
            &Building,
            &Health,
            &Team,
            Option<&TrainingQueue>,
            Option<&UnderConstruction>,
            Option<&Upgrading>,
            Option<&DoctrineTemplate>,
        ),
        With<Selected>,
    >,
) {
    let econ = *economies.get(Team::Human);
    let supply_blocked = econ.supply_cap > 0 && econ.supply_used >= econ.supply_cap;

    let unit_count = sel_units.iter().count();
    let building_count = sel_buildings.iter().count();
    let total = unit_count + building_count;

    // --- single-entity pane ------------------------------------------------
    let mut show_single = false;
    let mut show_multi = false;
    let mut portrait_letter = String::new();
    let mut portrait_color = SLOT_BG;
    let mut name = String::new();
    let mut hp_text = String::new();
    let mut hp_frac = 0.0f32;
    let mut stats_text = String::new();
    let mut extra_text = String::new();
    let mut show_prog = false;
    let mut prog = 0.0f32;
    let mut queue_letters: Vec<String> = Vec::new();
    let mut overflow_text = String::new();
    let mut cards: Vec<CardView> = Vec::new();
    // Hero-only bars: (xp fraction, mana fraction); None hides them.
    let mut hero_bars: Option<(f32, f32)> = None;
    let mut items_text = String::new();

    if total == 1 && unit_count == 1 {
        if let Some((_, unit, health, team, carrying, hero, _, _, _, _, inventory, militia, _)) =
            sel_units.iter().next()
        {
            show_single = true;
            let stats = unit_stats(unit.kind);
            name = unit_name(unit.kind).to_string();
            portrait_letter = initial(&name);
            // Call to Arms turns a worker into a soldier — say so in the title.
            if militia {
                name = format!("{} (Militia)", name);
            }
            portrait_color = team.color();
            hp_frac = (health.current / health.max.max(0.001)).clamp(0.0, 1.0);
            hp_text = format!(
                "HP {}/{}",
                health.current.max(0.0).round() as i32,
                health.max.round() as i32
            );
            let damage = stats.damage * hero.map_or(1.0, |h| Hero::damage_mult(h.level));
            // Siege engines hit buildings far harder than the flat damage
            // number suggests — say so, or the Catapult reads as a bad Archer.
            let vs_buildings = if stats.vs_building_mult > 1.0 {
                format!(" (x{:.0} vs buildings)", stats.vs_building_mult)
            } else {
                String::new()
            };
            stats_text = format!(
                "Damage {:.0}{}    Range {:.1}    Speed {:.0}",
                damage, vs_buildings, stats.range, stats.speed
            );
            if let Some(hero) = hero {
                name = format!("{}  Lv {}", name, hero.level);
                let max_mana = Hero::max_mana(hero.level).max(0.001);
                let to_next = Hero::xp_to_next(hero.level).max(0.001);
                hero_bars = Some((
                    (hero.xp / to_next).clamp(0.0, 1.0),
                    (hero.mana / max_mana).clamp(0.0, 1.0),
                ));
                extra_text = format!(
                    "XP {}/{}    Mana {}/{}",
                    hero.xp.max(0.0).round() as i32,
                    to_next.round() as i32,
                    hero.mana.max(0.0).round() as i32,
                    max_mana.round() as i32
                );
                // Both slots always shown, so an empty one reads as capacity.
                let slots = inventory.copied().unwrap_or_default().0;
                items_text = slots
                    .iter()
                    .enumerate()
                    .map(|(i, slot)| {
                        format!(
                            "[{}] {}",
                            i + 1,
                            slot.map(item_name).unwrap_or("-")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
            } else if let Some(c) = carrying {
                extra_text = format!("Carrying: {} {}", c.amount, resource_name(c.kind));
            }
        }
    } else if total == 1 && building_count == 1 {
        if let Some((sel_entity, building, health, team, queue, under, upgrading, _)) =
            sel_buildings.iter().next()
        {
            // Looked up by entity rather than added to `sel_buildings`, so the
            // seven-column query (and its four destructures) keeps its shape.
            let researching = cast.researching.get(sel_entity).ok();
            show_single = true;
            name = building_name(building.kind).to_string();
            portrait_letter = initial(&name);
            portrait_color = team.color();
            hp_frac = (health.current / health.max.max(0.001)).clamp(0.0, 1.0);
            hp_text = format!(
                "HP {}/{}",
                health.current.max(0.0).round() as i32,
                health.max.round() as i32
            );
            let stats = building_stats(building.kind);
            if let Some(uc) = under {
                let total_time = stats.build_time.max(0.001);
                prog = ((total_time - uc.remaining) / total_time).clamp(0.0, 1.0);
                show_prog = true;
                stats_text = format!("Under construction: {:.0}%", prog * 100.0);
            } else {
                // Towers read like a unit (damage/range); farms/halls show the
                // supply they add; a Wall has nothing but its HP bar.
                if let Some(attack) = stats.attack {
                    stats_text = format!(
                        "Damage {:.0}    Range {:.1}    Attack {:.1}s",
                        attack.damage, attack.range, attack.cooldown
                    );
                } else if stats.supply_provided > 0 {
                    stats_text = format!("Supply +{}", stats.supply_provided);
                }
                // A forge reads out the TEAM's levels, not its own — the whole
                // point of research is that it belongs to the faction and not
                // to the building that bought it, and a second Blacksmith
                // showing the same numbers is the clearest way to say so.
                let ladders = building_researches(building.kind);
                if !ladders.is_empty() {
                    let levels = cast.research.get(Team::Human);
                    stats_text = ladders
                        .iter()
                        .map(|&k| {
                            format!(
                                "{} {}/{}",
                                research_name(k),
                                levels.level(k),
                                RESEARCH_MAX_LEVEL
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("    ");
                }
                // A building on an upgrade ladder always says which rung it is
                // on — the tier is what tech requirements are written against.
                if building_tier(building.kind) > 1 || building_upgrades_to(building.kind).is_some()
                {
                    let tier = format!("Tier {}", building_tier(building.kind));
                    stats_text = if stats_text.is_empty() {
                        tier
                    } else {
                        format!("{stats_text}    {tier}")
                    };
                }
                if let Some(up) = upgrading {
                    // The conversion owns the progress bar and the status line
                    // while it runs: training is frozen, so reporting it would
                    // show a percentage that never moves.
                    let total = up.total.max(0.001);
                    prog = ((total - up.remaining) / total).clamp(0.0, 1.0);
                    show_prog = true;
                    extra_text = format!(
                        "Upgrading to {}: {:.0}%   (training paused, {} queued)",
                        building_name(up.to),
                        prog * 100.0,
                        queue.map(|q| q.queue.len()).unwrap_or(0)
                    );
                    for kind in queue.iter().flat_map(|q| q.queue.iter()) {
                        queue_letters.push(initial(unit_name(*kind)));
                    }
                } else if let Some(job) = researching {
                    // A forge working owns the bar for the same reason a
                    // conversion does: it is the one thing about this building
                    // that is changing, and it has nothing else to report.
                    let total = job.total.max(0.001);
                    prog = ((total - job.remaining) / total).clamp(0.0, 1.0);
                    show_prog = true;
                    extra_text = format!(
                        "Researching {} {}: {:.0}%   ({:.0}s left, +{:.0} to every unit)",
                        research_name(job.kind),
                        job.to_level,
                        prog * 100.0,
                        job.remaining.max(0.0),
                        research_bonus(job.kind, job.to_level),
                    );
                } else if let Some(queue) = queue {
                    for kind in queue.queue.iter() {
                        queue_letters.push(initial(unit_name(*kind)));
                    }
                    if let Some(front) = queue.queue.front() {
                        let train = if is_hero_kind(*front) {
                            hero_train_cost(&records, Team::Human, *front).2
                        } else {
                            unit_stats(*front).train_time
                        }
                        .max(0.001);
                        prog = (queue.progress / train).clamp(0.0, 1.0);
                        show_prog = true;
                        extra_text = format!(
                            "Training {}: {:.0}%   ({} queued)",
                            unit_name(*front),
                            prog * 100.0,
                            queue.queue.len()
                        );
                    } else {
                        extra_text = "Ready to train".to_string();
                    }
                }
            }
        }
    } else if total > 1 {
        show_multi = true;
        for (e, unit, health, team, _, hero, _, _, _, _, _, _, squad) in &sel_units {
            cards.push(CardView {
                entity: e,
                // Heroes show "H<level>" instead of a plain initial.
                letter: match hero {
                    Some(hero) => format!("H{}", hero.level),
                    None => initial(unit_name(unit.kind)),
                },
                hp: (health.current / health.max.max(0.001)).clamp(0.0, 1.0),
                color: team.color(),
                // Own units only: an enemy's squad is not ours to know, and
                // the snapshot does not report it either.
                squad: (*team == Team::Human).then(|| squad.map(|s| s.0)).flatten(),
            });
        }
        for (e, building, health, team, _, _, _, _) in &sel_buildings {
            cards.push(CardView {
                entity: e,
                letter: initial(building_name(building.kind)),
                hp: (health.current / health.max.max(0.001)).clamp(0.0, 1.0),
                color: lighten(team.color(), 0.12),
                squad: None,
            });
        }
        cards.sort_by_key(|c| c.entity.index());
        if cards.len() > MAX_CARDS {
            overflow_text = format!("+{}", cards.len() - MAX_CARDS);
            cards.truncate(MAX_CARDS);
        }
    }

    // --- command card ------------------------------------------------------
    let own_units = sel_units
        .iter()
        .filter(|(_, _, _, t, _, _, _, _, _, _, _, _, _)| **t == Team::Human)
        .count();
    let has_worker = sel_units.iter().any(|(_, u, _, t, _, _, _, _, _, _, _, _, _)| {
        *t == Team::Human && is_worker_kind(u.kind)
    });
    // Same aggregate the input system builds, so caption/highlight and the
    // toggle that a click executes can never disagree.
    let doc = DoctrineState::of(&sorted_doctrine(
        sel_units
            .iter()
            .filter(|(_, _, _, t, ..)| **t == Team::Human)
            .map(|(e, u, _, _, _, _, leash, retreat, prio, autocast, _, _, squad)| {
                (
                    e.index(),
                    UnitDoctrine::read(
                        leash,
                        retreat,
                        prio,
                        autocast,
                        u.kind,
                        squad,
                    ),
                )
            })
            .collect(),
    ));
    // What doctrine.rs is executing for that squad right now. Since the
    // executor was ungated this is a live readout of the engine acting on the
    // player's behalf, not a stored preference — so it belongs on screen.
    let live_posture = doc
        .squad
        .and_then(|s| cast.squads.0.get(&(Team::Human, s)));
    // The one selected own building: kind, finished, ability cooldown.
    let single = if building_count == 1 && unit_count == 0 {
        sel_buildings
            .iter()
            .next()
            .filter(|(_, _, _, t, _, _, _, _)| **t == Team::Human)
            .map(|(e, b, _, _, _, uc, up, _)| (e, b.kind, uc.is_none(), up.is_some()))
    } else {
        None
    };
    let single_building = single.map(|(_, kind, done, _)| (kind, done));
    // Same template view the input system builds, from the same conditions.
    let mut t_iter = sel_buildings.iter();
    let single_template = match (t_iter.next(), t_iter.next()) {
        (Some((_, b, _, t, queue, uc, _, tmpl)), None) => TemplateView::read(
            *t == Team::Human && uc.is_none() && queue.is_some() && !trainable(b.kind).is_empty(),
            tmpl,
        ),
        _ => TemplateView::default(),
    };
    // Units line, else the building's template line — the panel always answers
    // "what standing orders is this selection under?".
    let doctrine_line = if own_units > 0 {
        doc.line(live_posture.map(posture_tag).as_deref())
    } else {
        single_template.line()
    };
    // Own units only — reading an opponent's chain of command would be reading
    // their plan, which is exactly what the snapshot refuses the other seat.
    let why_text = why_line(
        reasons
            .why
            .iter()
            .filter(|(team, _)| **team == Team::Human)
            .map(|(_, why)| why.map_or_else(|| NO_PROVENANCE.to_string(), Provenance::why))
            .collect(),
    );

    // What reaching this selection costs, and what is already on its way.
    // `link.delay` with the feature off is a constant zero by construction
    // (`CommandLatency::delay_for_slack` returns early), so this is a
    // meaningless-but-harmless "Link: 0.0s" — which is why the panel line is
    // suppressed outright below rather than allowed to print it.
    let latency_on = reasons.link.latency.on;
    let (link_text, coverage_text) = if latency_on {
        let mut links = Vec::new();
        let mut in_transit = Vec::new();
        for (team, tf, pending) in &reasons.selected {
            if *team != Team::Human {
                continue;
            }
            links.push(reasons.link.delay(Team::Human, tf.translation));
            if let Some(p) = pending {
                in_transit.push(p.link());
            }
        }
        let (mut covered, mut total) = (0usize, 0usize);
        for (team, tf) in &reasons.all {
            if *team != Team::Human {
                continue;
            }
            total += 1;
            if reasons.link.delay(Team::Human, tf.translation) <= 0.0 {
                covered += 1;
            }
        }
        let nodes = reasons.link.nodes.own(Team::Human).count();
        (
            link_line(links, in_transit),
            coverage_line(true, nodes, covered, total),
        )
    } else {
        (String::new(), String::new())
    };
    // The team's armed rules — nothing to do with the selection or the link,
    // and drawn whether or not either exists.
    let triggers_text = trigger_line(&cast.triggers, cast.clock.elapsed_secs());
    let regions_text = region_line(&cast.regions);
    let plans_text = plan_line(&cast.plans);
    // The same grid the snapshot's `intel.heroes` is built from, read for the
    // same seat this whole file renders for.
    let enemy_heroes_text =
        enemy_hero_line(reasons.fog.get(Team::Human), cast.clock.elapsed_secs());

    // Hero commands: the ability of a selected caster, one train/revive button
    // per hero class the team's slots have room for, the building's own
    // ability, and the Shop's wares.
    let team_hero = heroes.iter().find(|(t, _, _)| **t == Team::Human);
    let mut held_heroes: Vec<UnitKind> = heroes
        .iter()
        .filter(|(t, _, _)| **t == Team::Human)
        .map(|(_, u, _)| u.kind)
        .collect();
    for (_, team, _, _, _, queue) in all_buildings.iter() {
        if *team != Team::Human {
            continue;
        }
        held_heroes.extend(
            queue
                .into_iter()
                .flat_map(|q| q.queue.iter().copied())
                .filter(|k| is_hero_kind(*k)),
        );
    }
    let selected_caster = sel_units
        .iter()
        .find(|(_, u, _, t, ..)| **t == Team::Human && !abilities_of_unit(u.kind).is_empty());
    let hero_cmds = HeroCmds {
        train: Some(hero_train_state(
            &records,
            cast.tiers.get(Team::Human),
            held_heroes,
        )),
        abilities: selected_caster
            .map(|(e, u, _, _, _, h, _, _, _, _, _, _, _)| {
                ability_slots(
                    abilities_of_unit(u.kind),
                    UnlockCtx::new(h.map_or(0, |hero| hero.level), cast.tiers.get(Team::Human)),
                    h,
                    cast.cooldowns.get(e).ok(),
                )
            })
            .unwrap_or_default(),
        building_abilities: single
            .filter(|(_, _, done, _)| *done)
            .map(|(entity, kind, _, _)| {
                ability_slots(
                    abilities_of_building(kind),
                    UnlockCtx::building(cast.tiers.get(Team::Human)),
                    None,
                    cast.cooldowns.get(entity).ok(),
                )
            })
            .unwrap_or_default(),
        upgrade: single.and_then(|(_, kind, done, upgrading)| {
            (done && !upgrading)
                .then(|| upgrade_cost(kind).zip(building_upgrades_to(kind)))
                .flatten()
                .map(|((gold, lumber, _), to)| (to, gold, lumber))
        }),
        shop: single.and_then(|(_, kind, done, _)| {
            (done && kind == BuildingKind::Shop).then(|| ShopState {
                hero: team_hero.is_some(),
                room: team_hero
                    .and_then(|(_, _, inv)| inv)
                    .is_some_and(|inv| inv.0.iter().any(|s| s.is_none())),
                tier: cast.tiers.get(Team::Human),
            })
        }),
        items: selected_caster
            .and_then(|(_, _, _, _, _, _, _, _, _, _, inv, _, _)| inv.copied())
            .unwrap_or_default()
            .0,
        research: single
            .map(|(entity, kind, done, _)| {
                research_cmds(
                    kind,
                    done,
                    cast.research.get(Team::Human),
                    cast.researching.get(entity).ok(),
                )
            })
            .unwrap_or_default(),
    };
    let completed: Vec<BuildingKind> = all_buildings
        .iter()
        .filter(|(_, t, _, _, under, _)| **t == Team::Human && !under)
        .map(|(b, ..)| b.kind)
        .collect();
    let race = cast.races.get(Team::Human);
    let all_entries = command_entries(
        ui.page,
        race,
        own_units,
        has_worker,
        single_building,
        hero_cmds,
        DoctrineCard {
            doc,
            posture: live_posture.map(posture_kind),
            tmpl: single_template,
            region_mark: mark_number(&cast.regions),
            region_count: cast.regions.get(Team::Human).len(),
            region_radius: ui.region_radius.unwrap_or(REGION_MARK_RADIUS),
            region_armed: ui.region_place,
            home_guard: has_trigger(&cast.triggers, HOME_GUARD),
        },
        &completed,
    );
    // What the twelve tiles actually show. `paginate` clamps the stored page,
    // so a selection that shrank under the player snaps back to a page that
    // exists rather than drawing an empty card.
    let view = paginate(&all_entries, ui.card_page);
    let page_label = card_page_label(ui.page, view.page, view.pages);
    ui.card_page = view.page;
    let entries = view.tiles;

    // Right-click sets the rally only when the selection is purely own
    // production buildings — the hint has to say the same thing.
    let rally_capable = unit_count == 0
        && building_count > 0
        && sel_buildings
            .iter()
            .all(|(_, b, _, t, _, _, _, _)| *t == Team::Human && !trainable(b.kind).is_empty());

    // Publish the click targets for the input systems (they run earlier next
    // frame and read exactly what the player is looking at now).
    ui.card_entities = cards.iter().map(|c| c.entity).collect();
    ui.card_actions = entries.iter().map(|e| e.action).collect();
    ui.queue_len = queue_letters.len();

    // --- hints -------------------------------------------------------------
    let hints = if let Some(arm) = ui.teleport_place {
        // SHOULD A HALL UNDER ATTACK BE HIGHLIGHTED? Yes — as a sentence, not
        // as a highlight. The decision, and why:
        //
        // The reason to arm this gesture at all is almost always that one of
        // your halls is being hit, and a pick UI that made the player read the
        // alert stack to find out WHICH would be asking them to do the lookup
        // twice, with a modal gesture held open. So the hint names it.
        //
        // It is a sentence because a highlight is not cheap: the world view
        // has no per-building marker to borrow (rings are parented on
        // selection), and the minimap draws buildings as flat dots with no
        // per-entity state, so either would mean new render plumbing for one
        // transient mode. The hint line is already recomputed every frame from
        // this exact query.
        //
        // The predicate is the building's own health fraction against
        // `BUILDING_HURT_FRAC` — the SAME threshold `shared::…` uses to decide
        // a building is "under attack" for the alert stack and the bridge
        // event feed. Not a re-derivation: the hint and the alert agree by
        // construction, so the line the player reads here cannot contradict
        // the line they just read there. (`GameEvents`' structured threat
        // state was the other candidate and is the wrong shape — it is one
        // hostile COUNT for the whole base, not a per-hall verdict, so it
        // could not name a hall even though it is what raises the alarm.)
        let hurt: Vec<String> = all_buildings
            .iter()
            .filter(|(b, team, _, hp, under, _)| {
                **team == Team::Human
                    && !under
                    && is_hall(b.kind)
                    && hp.max > 0.0
                    && hp.current / hp.max < BUILDING_HURT_FRAC
            })
            .map(|(b, _, tf, _, _, _)| {
                format!(
                    "{} at ({:.0},{:.0})",
                    building_name(b.kind),
                    tf.translation.x,
                    tf.translation.z
                )
            })
            .collect();
        let tail = if hurt.is_empty() {
            "the far hall is the one that saves it".to_string()
        } else {
            format!("UNDER ATTACK: {}", hurt.join(", "))
        };
        format!(
            "{} armed: left-click the HALL to arrive at, on the map or the minimap \
             (Right-click / Esc cancels) - {tail}",
            arm.name
        )
    } else if let Some(kind) = ui.placement {
        let s = building_stats(kind);
        let tail = if kind == BuildingKind::Wall {
            "Left-click each segment (placement stays armed), Right-click / Esc to stop"
        } else {
            "Left-click to place, Right-click / Esc to cancel"
        };
        format!(
            "Placing {} ({}g {}l) - {}",
            building_name(kind),
            s.cost_gold,
            s.cost_lumber,
            tail
        )
    } else if let Some(arm) = ui.cast_place.as_ref() {
        format!(
            "{} armed: left-click {} (Right-click / Esc cancels) - out of range is refused, \
             the caster will not walk in",
            arm.name,
            if arm.wants_unit {
                "the unit to cast it on - misses keep trying"
            } else {
                "where it lands"
            }
        )
    } else if let Some(arm) = ui.posture_place {
        format!(
            "Squad {} - {} posture armed: left-click {} (Right-click / Esc cancels)",
            arm.squad,
            arm.kind.label(),
            if arm.kind.needs_unit() {
                // Says "keeps trying" because it does: a missed unit click
                // leaves the gesture armed, and a hint that implied otherwise
                // would have the player re-press R after every near-miss.
                "one of your units to screen - misses keep trying"
            } else {
                "the ground it is about"
            }
        )
    } else if ui.attack_move_armed {
        "Attack-move armed - left-click a destination (Esc cancels)".to_string()
    } else if ui.page == CardPage::Doctrine {
        match doc.squad {
            Some(squad) => format!(
                "Doctrine card - squad {squad}. Postures run at machine speed for whoever set them.   [I] / Esc: back to orders."
            ),
            None if own_units > 0 => {
                "Doctrine card - this selection has no squad yet; a posture will enrol it in one.   [I] / Esc: back to orders."
                    .to_string()
            }
            None => "Doctrine card - standing orders stamped on everything this building trains.   [I] / Esc: back to orders."
                .to_string(),
        }
    } else if total == 0 {
        "Left-click / drag to select.   Ctrl+1-3 set squad, Shift+1-3 add, 1-3 recall.   [I] doctrine   [Tab] more commands   '.' idle worker   F9: AI plays Blue   F12 x2: surrender"
            .to_string()
    } else if rally_capable {
        "Right-click: set rally (ground, resource node or own unit).   Shift-click adds to selection."
            .to_string()
    } else {
        "Right-click to order.   Shift-click adds to selection.".to_string()
    };

    // --- banner ------------------------------------------------------------
    // Whether we were spectating is latched at the moment the match ends, so
    // toggling F9 on the result screen can't rewrite history.
    match game_over.winner {
        Some(_) if spectated.is_none() => *spectated = Some(ai_controlled.human),
        None => *spectated = None,
        _ => {}
    }
    let (banner, banner_sub, banner_color) = match (game_over.winner, spectated.unwrap_or(false)) {
        // AI vs AI: team-neutral result, no "you".
        (Some(Team::Human), true) => ("BLUE WINS", "AI vs AI", Color::srgb(0.45, 0.65, 1.0)),
        (Some(Team::Claude), true) => ("RED WINS", "AI vs AI", Color::srgb(1.0, 0.45, 0.35)),
        (Some(Team::Human), false) => ("VICTORY!", "You win", Color::srgb(0.45, 1.0, 0.5)),
        (Some(Team::Claude), false) => ("DEFEAT", "Claude wins", Color::srgb(1.0, 0.35, 0.3)),
        (None, _) => ("", "", Color::WHITE),
    };
    // Which win it was, in the sub-line — the human's copy of the snapshot's
    // `game_over_reason`. Both seats get told the same fact in the same words;
    // only the frame around it differs, which is the usual rule.
    let banner_sub = match game_over.reason {
        Some(GameOverReason::Razed) => format!("{banner_sub} — production razed"),
        Some(GameOverReason::Surrender) => format!("{banner_sub} — by surrender"),
        None => banner_sub.to_string(),
    };

    // --- texts -------------------------------------------------------------
    for (slot, mut text, mut color) in &mut texts {
        match *slot {
            Slot::Resources => {
                text.0 = format!("Gold: {}   Lumber: {}", econ.gold, econ.lumber);
            }
            Slot::Supply => {
                text.0 = format!(
                    "Supply: {}/{} · {}",
                    econ.supply_used,
                    econ.supply_cap,
                    upkeep_label(econ.supply_used)
                );
                color.0 = if supply_blocked {
                    Color::srgb(1.0, 0.35, 0.3)
                } else {
                    Color::WHITE
                };
            }
            Slot::Hints => text.0 = hints.clone(),
            Slot::Banner => {
                text.0 = banner.to_string();
                color.0 = banner_color;
            }
            Slot::BannerSub => text.0 = banner_sub.clone(),
            Slot::PortraitLetter => text.0 = portrait_letter.clone(),
            Slot::Name => text.0 = name.clone(),
            Slot::Hp => text.0 = hp_text.clone(),
            Slot::Stats => text.0 = stats_text.clone(),
            Slot::Extra => text.0 = extra_text.clone(),
            Slot::Items => text.0 = items_text.clone(),
            Slot::Doctrine => text.0 = doctrine_line.clone(),
            Slot::Why => text.0 = why_text.clone(),
            Slot::Link => text.0 = link_text.clone(),
            Slot::Triggers => text.0 = triggers_text.clone(),
            Slot::Regions => text.0 = regions_text.clone(),
            Slot::Plans => text.0 = plans_text.clone(),
            Slot::EnemyHeroes => text.0 = enemy_heroes_text.clone(),
            Slot::Coverage => text.0 = coverage_text.clone(),
            Slot::Overflow => text.0 = overflow_text.clone(),
            Slot::CardLetter(i) => {
                text.0 = cards.get(i).map(|c| c.letter.clone()).unwrap_or_default();
            }
            Slot::CardSquad(i) => {
                text.0 = squad_badge(cards.get(i).and_then(|c| c.squad));
            }
            Slot::QueueLetter(i) => {
                text.0 = queue_letters.get(i).cloned().unwrap_or_default();
            }
            // The tile's letter, derived from the key that fires it. There is no
            // stored caption to drift: rebinding an action in `hotkeys::REGISTRY`
            // moves the letter on the tile in the same edit.
            Slot::CmdKey(i) => {
                text.0 = entries
                    .get(i)
                    .map(|e| hotkeys::key_caption(e.key).to_string())
                    .unwrap_or_default();
            }
            Slot::CmdPage => text.0 = page_label.clone(),
            Slot::CmdLabel(i) => {
                text.0 = entries.get(i).map(|e| e.label.clone()).unwrap_or_default();
            }
            Slot::CmdCost(i) => {
                text.0 = entries.get(i).map(|e| e.cost.clone()).unwrap_or_default();
            }
        }
    }

    // --- layout ------------------------------------------------------------
    for (el, mut node) in &mut nodes {
        match *el {
            El::SinglePane => {
                node.display = if show_single {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::MultiPane => {
                node.display = if show_multi {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::HpFill => node.width = Val::Percent(hp_frac * 100.0),
            El::HeroBars => {
                node.display = if hero_bars.is_some() {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::XpFill => node.width = Val::Percent(hero_bars.map_or(0.0, |(xp, _)| xp) * 100.0),
            El::ManaFill => {
                node.width = Val::Percent(hero_bars.map_or(0.0, |(_, mana)| mana) * 100.0)
            }
            El::ProgWrap => {
                node.display = if show_prog {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::ProgFill => node.width = Val::Percent(prog * 100.0),
            El::Card(i) => {
                node.display = if i < cards.len() {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::CardHp(i) => {
                node.width = Val::Percent(cards.get(i).map_or(0.0, |c| c.hp) * 100.0)
            }
            El::QueueTile(i) => {
                node.display = if i < queue_letters.len() {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::CmdBtn(i) => {
                node.display = if i < entries.len() {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            El::Portrait => {}
        }
    }

    // --- colours -----------------------------------------------------------
    for (el, mut bg, interaction) in &mut colors {
        let hovered = matches!(interaction, Some(Interaction::Hovered));
        let pressed = matches!(interaction, Some(Interaction::Pressed));
        match *el {
            El::Portrait => bg.0 = portrait_color,
            El::HpFill => bg.0 = hp_color(hp_frac),
            El::Card(i) => {
                if let Some(c) = cards.get(i) {
                    bg.0 = if pressed {
                        lighten(c.color, 0.28)
                    } else if hovered {
                        lighten(c.color, 0.16)
                    } else {
                        c.color
                    };
                }
            }
            El::CardHp(i) => {
                if let Some(c) = cards.get(i) {
                    bg.0 = hp_color(c.hp);
                }
            }
            El::QueueTile(_) => {
                bg.0 = if hovered || pressed {
                    Color::srgb(0.42, 0.20, 0.20)
                } else {
                    SLOT_BG
                };
            }
            El::CmdBtn(i) => {
                let Some(entry) = entries.get(i) else {
                    bg.0 = SLOT_BG;
                    continue;
                };
                let affordable = match entry.afford {
                    Some((g, l)) => econ.can_afford(g, l),
                    None => true,
                };
                // Doctrine toggles reuse the armed highlight via `active`.
                let armed = entry.active
                    || match entry.action {
                        CmdAction::AttackMove => ui.attack_move_armed,
                        CmdAction::Place(k) => ui.placement == Some(k),
                        // A posture waiting for its ground click is armed in
                        // exactly the sense the other two are.
                        CmdAction::SetPosture(k) => {
                            ui.posture_place.is_some_and(|arm| arm.kind == k)
                        }
                        _ => false,
                    };
                let base = if !entry.enabled {
                    // Ability on cooldown / out of mana.
                    Color::srgb(0.09, 0.09, 0.12)
                } else if armed {
                    Color::srgb(0.30, 0.55, 0.30)
                } else if !affordable {
                    Color::srgb(0.24, 0.11, 0.11)
                } else {
                    SLOT_BG
                };
                bg.0 = if pressed {
                    lighten(base, 0.22)
                } else if hovered {
                    lighten(base, 0.12)
                } else {
                    base
                };
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Hover feedback: cursor icon + ring under whatever a click would pick
// ---------------------------------------------------------------------------

fn setup_hover(
    mut commands: Commands,
    assets: Res<UiAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut mk = |color: Color| {
        materials.add(StandardMaterial {
            base_color: color,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })
    };
    let friendly = mk(Color::srgba(1.0, 1.0, 1.0, 0.75));
    let hostile = mk(Color::srgba(1.0, 0.25, 0.2, 0.85));
    let resource = mk(Color::srgba(1.0, 0.85, 0.2, 0.85));
    commands.spawn((
        HoverRing,
        Mesh3d(assets.ring_mesh.clone()),
        MeshMaterial3d(friendly.clone()),
        Transform::from_xyz(0.0, 0.1, 0.0),
        Visibility::Hidden,
    ));
    commands.insert_resource(HoverAssets {
        friendly,
        hostile,
        resource,
    });
}

#[allow(clippy::too_many_arguments)]
fn hover_feedback(
    hud: Res<HudLayout>,
    mut commands: Commands,
    state: Res<UiState>,
    game_over: Res<GameOver>,
    assets: Res<HoverAssets>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    units: Query<(&Transform, &Team), With<Unit>>,
    buildings: Query<(&Transform, &Team, &Building), Without<HoverRing>>,
    nodes: Query<(&Transform, &ResourceNode)>,
    selected: Query<&Unit, With<Selected>>,
    fog: Res<FogGrids>,
    mut ring: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (
            With<HoverRing>,
            Without<Unit>,
            Without<Building>,
            Without<ResourceNode>,
        ),
    >,
    mut last_icon: Local<Option<SystemCursorIcon>>,
) {
    let Ok((window_entity, window)) = windows.single() else {
        return;
    };
    let Ok((mut ring_tf, mut ring_vis, mut ring_mat)) = ring.single_mut() else {
        return;
    };

    // (ground pos, ring radius, material) of the pick target, if any.
    let mut hit: Option<(Vec3, f32, Handle<StandardMaterial>)> = None;
    let mut icon = SystemCursorIcon::Default;
    let workers_selected = selected.iter().any(|u| is_worker_kind(u.kind));

    let pickable = game_over.winner.is_none() && state.placement.is_none() && !state.dragging;
    if pickable {
        if let (Some(cursor), Ok((cam, cam_tf))) = (window.cursor_position(), camera.single()) {
            if !cursor_over_hud(cursor, window, &state, &hud) {
                if let Some(ground) = cursor_to_ground(cam, cam_tf, cursor) {
                    // Closest unit first (units win ties against buildings),
                    // then buildings, then resource nodes.
                    let ray = cursor_ray(cam, cam_tf, cursor);
                    let mut best_unit: Option<(f32, Vec3, Team)> = None;
                    for (tf, team) in &units {
                        // A hover ring over an invisible enemy would be a
                        // perfect enemy detector — sweep the cursor across the
                        // fog and watch the crosshair light up. Same gate as
                        // the click that follows it.
                        if *team != Team::Human && !fog_sees(&fog, tf.translation) {
                            continue;
                        }
                        let d = dist_xz(
                            tf.translation,
                            pick_point_for(ray, ground, tf.translation.y),
                        );
                        if d <= UNIT_PICK_RADIUS && best_unit.is_none_or(|(bd, _, _)| d < bd) {
                            best_unit = Some((d, tf.translation, *team));
                        }
                    }
                    if let Some((_, pos, team)) = best_unit {
                        let ring = UNIT_RADIUS * 1.55;
                        hit = Some(match team {
                            Team::Human => (pos, ring, assets.friendly.clone()),
                            Team::Claude => (pos, ring, assets.hostile.clone()),
                        });
                        icon = match team {
                            Team::Human => SystemCursorIcon::Pointer,
                            Team::Claude => SystemCursorIcon::Crosshair,
                        };
                    } else {
                        let mut best_bld: Option<(f32, Vec3, Team, f32)> = None;
                        for (tf, team, building) in &buildings {
                            if *team != Team::Human && !fog_sees(&fog, tf.translation) {
                                continue;
                            }
                            let r = building_stats(building.kind).size * 0.5;
                            let d = dist_xz(tf.translation, ground);
                            if d <= r && best_bld.is_none_or(|(bd, _, _, _)| d < bd) {
                                best_bld = Some((d, tf.translation, *team, r));
                            }
                        }
                        // A remembered structure the player can right-click is
                        // a structure the crosshair has to acknowledge, or the
                        // gesture is undiscoverable. Driving this off the
                        // ghost RECORD rather than the live entity is what
                        // keeps it honest: the ring appears for a razed
                        // building's ghost exactly as it does for a standing
                        // one, so hovering can never answer "is it still
                        // there?" — the question only walking back over the
                        // rubble is allowed to answer.
                        if best_bld.is_none() {
                            for ghost in fog.get(Team::Human).ghosts() {
                                let r = building_stats(ghost.kind).size * 0.5;
                                let d = dist_xz(ghost.pos, ground);
                                if d <= r && best_bld.is_none_or(|(bd, _, _, _)| d < bd) {
                                    best_bld = Some((d, ghost.pos, Team::Claude, r));
                                }
                            }
                        }
                        if let Some((_, pos, team, r)) = best_bld {
                            let (mat, ic) = match team {
                                Team::Human => (assets.friendly.clone(), SystemCursorIcon::Pointer),
                                Team::Claude => (assets.hostile.clone(), SystemCursorIcon::Crosshair),
                            };
                            hit = Some((pos, r * 1.24, mat));
                            icon = ic;
                        } else if workers_selected {
                            for (tf, node) in &nodes {
                                let r = match node.kind {
                                    ResourceKind::Gold => MINE_PICK_RADIUS,
                                    ResourceKind::Lumber => TREE_PICK_RADIUS,
                                };
                                if dist_xz(tf.translation, ground) <= r {
                                    hit = Some((tf.translation, r * 0.8, assets.resource.clone()));
                                    icon = SystemCursorIcon::Grab;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Armed attack-move always reads as "next click is an attack".
    if state.attack_move_armed && game_over.winner.is_none() {
        icon = SystemCursorIcon::Crosshair;
    }

    match hit {
        Some((pos, radius, mat)) => {
            // Hover ring sits just under whatever is hovered — on the ground
            // for ground things, at altitude for a flyer, so the highlight is
            // always attached to the thing the cursor actually picked.
            let ring_y = if pos.y > FLYER_ALTITUDE * 0.5 { pos.y - 1.2 } else { 0.1 };
            ring_tf.translation = Vec3::new(pos.x, ring_y, pos.z);
            ring_tf.scale = Vec3::new(radius, 0.12, radius);
            if ring_mat.0 != mat {
                ring_mat.0 = mat;
            }
            *ring_vis = Visibility::Visible;
        }
        None => *ring_vis = Visibility::Hidden,
    }

    if *last_icon != Some(icon) {
        *last_icon = Some(icon);
        commands
            .entity(window_entity)
            .insert(CursorIcon::System(icon));
    }
}

// ---------------------------------------------------------------------------
// Surrender: F12 twice within 3 seconds concedes the match
// ---------------------------------------------------------------------------

fn surrender_hotkey(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    game_over: Res<GameOver>,
    mut armed_at: Local<Option<f32>>,
    mut submissions: EventWriter<SubmitIntent>,
) {
    if game_over.winner.is_some() || !keys.just_pressed(hotkeys::SURRENDER) {
        return;
    }
    let now = time.elapsed_secs();
    match *armed_at {
        Some(t) if now - t < 3.0 => {
            say(&mut submissions, Intent::Surrender);
            *armed_at = None;
        }
        _ => {
            info!("Press F12 again within 3 seconds to surrender");
            *armed_at = Some(now);
        }
    }
}

// ---------------------------------------------------------------------------
// F10: the game takes its own picture
// ---------------------------------------------------------------------------
//
// Three agents in a row tried to photograph this game with an external capture
// tool and filed a stale pixmap as evidence: under XWayland the X11 window
// contents are not what is on the screen, so the screenshot showed a frame
// from minutes earlier — or nothing at all — and nobody could tell, because a
// stale frame of an RTS looks exactly like a fresh one.
//
// The fix is not a better capture tool. The only process that reliably knows
// what this frame looks like is the one that drew it, so the engine takes its
// own pictures: F10 asks the renderer for the primary window's contents at the
// end of this frame and writes a PNG to `shots/` (or `$WC3_SHOT_DIR`).
//
// Registered by `UiPlugin`, which main.rs adds only when there is a window —
// so a headless run has no key to press and no renderer to ask, and simply
// never does this. That is the graceful no-op: not a branch, an absence.

/// Take a picture of the game.
const SCREENSHOT_KEY: KeyCode = KeyCode::F10;
/// Overrides the output directory — the arena runner points it at the round's
/// own evidence directory so shots file themselves with the match.
const SHOT_DIR_ENV: &str = "WC3_SHOT_DIR";
const DEFAULT_SHOT_DIR: &str = "shots";

/// Where screenshots go, given the raw environment value. Split from
/// `shot_dir` so the policy — including "an empty variable is not an answer" —
/// is testable without mutating the process environment.
fn shot_dir_from(raw: Option<&str>) -> PathBuf {
    match raw.map(str::trim) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(DEFAULT_SHOT_DIR),
    }
}

/// Where this session's screenshots go.
pub fn shot_dir() -> PathBuf {
    shot_dir_from(std::env::var(SHOT_DIR_ENV).ok().as_deref())
}

/// The file name of the `nth` shot of a run, taken at game time `game_secs`.
///
/// Both clocks are in the name deliberately. The wall-clock `stamp` keeps two
/// runs that share one directory from overwriting each other; the game time is
/// the only number an after-action report can use, because "the push at t=324"
/// is a thing you can look up and `screenshot_3.png` is not.
fn shot_name(stamp: u64, game_secs: f32, nth: u32) -> String {
    format!("wc3-{stamp}-t{:04}-{nth:02}.png", game_secs.max(0.0) as u32)
}

/// Game-time seconds at which the engine photographs itself unattended, comma
/// separated: `WC3_SHOT_AT=20,90,240`.
///
/// The same reasoning that produced F10, taken one step further. F10 solved
/// "the capture tool photographs a stale frame"; it did not solve "an agent
/// with no hands cannot press F10", and the workaround for *that* was reaching
/// back for the external tools this whole section exists to avoid. So the
/// engine keeps a list of moments and takes its own picture at each of them,
/// through the identical code path — a scheduled shot and a pressed one are
/// the same function, so evidence gathered without a human at the keyboard is
/// the same evidence.
const SHOT_AT_ENV: &str = "WC3_SHOT_AT";

/// Parse the schedule, sorted and with the unparseable dropped. Split out so
/// the policy is testable without touching the process environment.
fn shot_schedule_from(raw: Option<&str>) -> Vec<f32> {
    let mut out: Vec<f32> = raw
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .filter(|s| *s >= 0.0)
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The one place a picture is actually requested. `taken` numbers the shots of
/// this run so two in the same game-second do not collide.
fn take_shot(commands: &mut Commands, game_secs: f32, taken: &mut u32) {
    let dir = shot_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        warn!("screenshot: cannot create {} — {err}", dir.display());
        return;
    }
    *taken += 1;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(shot_name(stamp, game_secs, *taken));
    // Logged on request rather than on success: `save_to_disk` reports its own
    // failures, and a line that only appears when the write worked cannot tell
    // you whether the request happened at all.
    info!("screenshot: {}", path.display());
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

fn screenshot_hotkey(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut taken: Local<u32>,
    mut commands: Commands,
) {
    if !keys.just_pressed(SCREENSHOT_KEY) {
        return;
    }
    take_shot(&mut commands, time.elapsed_secs(), &mut taken);
}

/// Fire the `WC3_SHOT_AT` schedule. Shares `taken` with nothing — the two
/// counters are independent, and the wall-clock stamp in the file name is what
/// keeps a scheduled shot and a pressed one from landing on the same path.
fn scheduled_screenshots(
    time: Res<Time>,
    mut due: Local<Option<std::collections::VecDeque<f32>>>,
    mut taken: Local<u32>,
    mut commands: Commands,
) {
    let queue = due.get_or_insert_with(|| {
        shot_schedule_from(std::env::var(SHOT_AT_ENV).ok().as_deref())
            .into_iter()
            .collect()
    });
    let now = time.elapsed_secs();
    // A `while`, not an `if`: at `WC3_SPEED=8` one frame can step past several
    // scheduled moments, and silently dropping the ones it skipped would make
    // the evidence depend on the frame rate.
    while queue.front().is_some_and(|t| *t <= now) {
        queue.pop_front();
        take_shot(&mut commands, now, &mut taken);
    }
}

// ---------------------------------------------------------------------------
// Alert stack: the human's half of the shared event feed
// ---------------------------------------------------------------------------
//
// An external commander reads `shared::GameEvents` out of `bridge/<seat>/
// state.json` and cannot miss a line of it. Before this existed, the human
// looked at the map and missed whatever wasn't on screen — the same match, two
// very different amounts of knowledge, and the difference had nothing to do
// with skill.
//
// So this renders the identical buffer, filtered to `Team::Human` exactly the
// way the bridge filters to its seat's team. Not a similar feed built from
// similar queries: the same `GameEvent`s, in the same order, with the same
// text. If the diff ever learns a new event, both sides learn it in the same
// commit. Nothing here produces — production lives in shared.rs.
//
// The renderers differ where the *readers* differ, and only there. A file
// reader gets forty lines of history and all the time in the world; a human
// gets six lines, colour-coded, that fade after nine seconds, and one key to
// send the camera where the news came from.

#[derive(Resource, Default)]
struct Notifications {
    /// Highest `GameEvent::seq` already pulled off the shared feed. Monotonic,
    /// so a frame that misses nothing and a frame that misses forty events are
    /// handled by the same line of code.
    seen: u64,
    /// Newest first, so index 0 is the top row and the first thing Space finds.
    live: VecDeque<Notice>,
    /// Where the next Space press starts looking. Reset to the top whenever
    /// fresh news arrives — the newest alert is almost always the one you meant.
    focus_cursor: usize,
}

/// One alert on screen: a `GameEvent` plus the wall-clock moment it appeared.
struct Notice {
    message: String,
    severity: EventSeverity,
    pos: Option<Vec3>,
    /// `Time<Real>` seconds. Real time on purpose: at `WC3_SPEED=8` a
    /// game-time lifetime would blink out before it could be read.
    born: f32,
}

// ---------------------------------------------------------------------------
// Alert pings: the alert stack, on the minimap and in the ear
// ---------------------------------------------------------------------------
//
// `GameEvent` has carried a `pos` since the feed was built, and until now the
// only thing that used it was the Space key. That is a lot of information to
// keep and never show: the stack tells you *what* happened, and it is a list
// of sentences in the corner you look at least, so "hostiles near your base"
// arrives as text while your eyes are on your build queue.
//
// Two cheap renderings of the same field close it:
//
//   * an expanding ring on the minimap, at the place, in the severity's own
//     colour — the eye catches motion at the periphery far better than text;
//   * a short tone, one per severity, which reaches a player who is not
//     looking at the screen's corner at all.
//
// Neither carries information the alert row does not, which is the rule: this
// is a second and third *rendering* of one event, not a new source of it. In
// particular the ring is drawn only where an alert already told you something
// happened, so it can never reveal a position the feed itself would not — the
// feed's own fog audit (docs/FOG.md, "the event feed was audited category by
// category") is what makes that safe, and it is doing the work here too.

/// How long a ring lives, in real seconds. Long enough to catch an eye that
/// was elsewhere, short enough that six alerts do not leave the minimap under
/// a permanent light show.
const PING_LIFETIME: f32 = 3.0;
/// Ring diameter in minimap pixels, at birth and at death.
const PING_MIN_PX: f32 = 5.0;
const PING_MAX_PX: f32 = 34.0;
/// Ring stroke. Two pixels reads as a ring rather than a dot at every size in
/// the range above.
const PING_STROKE: f32 = 2.0;
/// Silences the alert cues. A game whose *sound* cannot be turned off without
/// turning the game off is a game people mute at the OS, and then never hear
/// again — including the parts they wanted.
const MUTE_ENV: &str = "WC3_MUTE";

/// Which severity wins when several arrive in one frame.
fn severity_rank(severity: EventSeverity) -> u8 {
    match severity {
        EventSeverity::Info => 0,
        EventSeverity::Warning => 1,
        EventSeverity::Critical => 2,
    }
}

/// One live ring.
struct AlertPing {
    pos: Vec3,
    severity: EventSeverity,
    /// `Time<Real>` seconds, like `Notice::born` and for the same reason: at
    /// `WC3_SPEED=8` a game-time ring would be gone before it was seen.
    born: f32,
}

#[derive(Resource, Default)]
struct AlertPings {
    live: Vec<AlertPing>,
}

/// Marker on a pooled ring node.
#[derive(Component)]
struct MinimapPing;

/// Ring radius and opacity as a fraction of its life. Expanding and fading, so
/// the eye reads "something happened HERE, just now" and then stops being
/// bothered by it.
fn ping_shape(age: f32) -> (f32, f32) {
    let t = (age / PING_LIFETIME).clamp(0.0, 1.0);
    // Ease out: fast at the start, so the motion that catches the eye happens
    // in the first half-second rather than being spread evenly over three.
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    (
        PING_MIN_PX + (PING_MAX_PX - PING_MIN_PX) * eased,
        1.0 - t,
    )
}

/// Paint the rings. Pooled and mutated in place, exactly like the minimap's
/// unit dots and bounty markers.
fn update_minimap_pings(
    hud: Res<HudLayout>,
    mut commands: Commands,
    real: Res<Time<Real>>,
    pings: Res<AlertPings>,
    root: Query<Entity, With<MinimapRoot>>,
    mut nodes: Query<(&mut Node, &mut BorderColor), With<MinimapPing>>,
) {
    let Ok(root) = root.single() else {
        return;
    };
    let now = real.elapsed_secs();
    let wanted: Vec<(Vec2, f32, Color)> = pings
        .live
        .iter()
        .map(|p| {
            let (size, alpha) = ping_shape(now - p.born);
            (
                world_to_minimap(p.pos, hud.minimap_px),
                size,
                severity_color(p.severity).with_alpha(alpha),
            )
        })
        .collect();

    let mut used = 0usize;
    for (mut node, mut border) in &mut nodes {
        match wanted.get(used) {
            Some((at, size, color)) => {
                node.display = Display::Flex;
                node.left = Val::Px(at.x - size * 0.5);
                node.top = Val::Px(at.y - size * 0.5);
                node.width = Val::Px(*size);
                node.height = Val::Px(*size);
                border.0 = *color;
            }
            None => node.display = Display::None,
        }
        used += 1;
    }
    for _ in used..wanted.len() {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                border: UiRect::all(Val::Px(PING_STROKE)),
                ..default()
            },
            // A ring, not a disc: the fog, the dots and the mines underneath it
            // all stay readable through the hole.
            BorderRadius::MAX,
            BorderColor(Color::NONE),
            BackgroundColor(Color::NONE),
            // Above the fog layer (1) — a ping is news about a place, and the
            // whole point is that it shows up over unexplored black.
            ZIndex(2),
            MinimapPing,
            ChildOf(root),
        ));
    }
}

// --- The cues -------------------------------------------------------------
//
// Three sounds, generated at startup into an in-memory WAV each. No files: a
// game that ships stat tables as its only assets should not grow an `audio/`
// directory for six hundred milliseconds of beep, and a synthesized cue can be
// re-tuned by editing a number in this file rather than by opening a DAW.

/// Sample rate of the synthesized cues. 44.1 kHz is taken by every backend
/// without resampling, and a cue is far too short to be worth economising on.
const CUE_RATE: u32 = 44_100;
/// Attack ramp. Without it a tone starts at full amplitude mid-cycle and the
/// speaker cone's step response is an audible click — which is the one thing
/// worse than no sound at all.
const CUE_ATTACK: f32 = 0.006;

/// One note of a cue.
struct Tone {
    hz: f32,
    secs: f32,
    /// Peak amplitude, 0..1.
    gain: f32,
}

/// Render a sequence of tones as a 16-bit mono WAV, header and all.
///
/// Hand-rolled rather than pulled from a crate because the format's header is
/// 44 bytes of well-documented constants and the alternative is a dependency
/// that exists to write those 44 bytes.
fn synth_wav(tones: &[Tone]) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::new();
    for tone in tones {
        let n = (tone.secs * CUE_RATE as f32) as usize;
        for i in 0..n {
            let t = i as f32 / CUE_RATE as f32;
            // Attack ramp in, then a linear decay to silence over the rest of
            // the note — so notes butt against each other without clicking and
            // the cue reads as a struck sound rather than a held one.
            let attack = (t / CUE_ATTACK).clamp(0.0, 1.0);
            let decay = 1.0 - (i as f32 / n.max(1) as f32);
            let phase = std::f32::consts::TAU * tone.hz * t;
            // A little second harmonic: a pure sine sounds like a hearing test,
            // and this is meant to sound like an instrument being tapped.
            let wave = phase.sin() + 0.25 * (2.0 * phase).sin();
            let v = wave * tone.gain * attack * decay * 0.8;
            samples.push((v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
    }

    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    let byte_rate = CUE_RATE * 2; // mono, 2 bytes per sample
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&CUE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// The notes of each cue.
///
/// The shape carries the meaning, not the pitch: **Info** is one light tick,
/// **Warning** *rises* (look up), **Critical** *falls* and is lower and louder
/// (something is wrong). A player learns those three in a match without being
/// told, which is the only kind of audio vocabulary worth having.
fn cue_tones(severity: EventSeverity) -> Vec<Tone> {
    match severity {
        EventSeverity::Info => vec![Tone { hz: 784.0, secs: 0.10, gain: 0.22 }],
        EventSeverity::Warning => vec![
            Tone { hz: 587.0, secs: 0.085, gain: 0.34 },
            Tone { hz: 880.0, secs: 0.13, gain: 0.34 },
        ],
        EventSeverity::Critical => vec![
            Tone { hz: 392.0, secs: 0.10, gain: 0.46 },
            Tone { hz: 294.0, secs: 0.18, gain: 0.46 },
        ],
    }
}

#[derive(Resource)]
struct AlertCues {
    info: Handle<AudioSource>,
    warning: Handle<AudioSource>,
    critical: Handle<AudioSource>,
    /// `WC3_MUTE`. Read once at setup so no system can observe a different
    /// answer than another, the same discipline `WC3_FOG` is read with.
    muted: bool,
}

impl AlertCues {
    fn handle(&self, severity: EventSeverity) -> &Handle<AudioSource> {
        match severity {
            EventSeverity::Info => &self.info,
            EventSeverity::Warning => &self.warning,
            EventSeverity::Critical => &self.critical,
        }
    }

    /// Spawn a one-shot player that removes itself when the note ends.
    fn play(&self, commands: &mut Commands, severity: EventSeverity) {
        if self.muted {
            return;
        }
        commands.spawn((
            AudioPlayer(self.handle(severity).clone()),
            PlaybackSettings::DESPAWN,
        ));
    }
}

/// Synthesize the three cues once.
///
/// Registered by `UiPlugin`, which main.rs adds only when there is a window —
/// so a headless run never synthesizes a tone, never asks for an audio device
/// and never touches `Assets<AudioSource>`, which under `MinimalPlugins` does
/// not exist. The same graceful absence F10 gets, for the same reason. Every
/// system that plays a cue takes `Option<Res<AlertCues>>` besides, so even a
/// hand-built test app that registers a renderer without this setup runs
/// silently instead of panicking.
fn setup_alert_cues(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let muted = std::env::var(MUTE_ENV)
        .map(|v| !v.is_empty() && v.trim() != "0")
        .unwrap_or(false);
    if muted {
        info!("alert cues: muted ({MUTE_ENV})");
    }
    let mut cue = |severity| {
        sources.add(AudioSource {
            bytes: synth_wav(&cue_tones(severity)).into(),
        })
    };
    commands.insert_resource(AlertCues {
        info: cue(EventSeverity::Info),
        warning: cue(EventSeverity::Warning),
        critical: cue(EventSeverity::Critical),
        muted,
    });
}

fn severity_color(severity: EventSeverity) -> Color {
    match severity {
        // The same three colours the rest of the HUD already means things by:
        // doctrine blue, resource gold, damage red.
        EventSeverity::Info => Color::srgb(0.62, 0.80, 1.0),
        EventSeverity::Warning => Color::srgb(1.0, 0.86, 0.35),
        EventSeverity::Critical => Color::srgb(1.0, 0.42, 0.36),
    }
}

/// Full opacity until the last `NOTIF_FADE` seconds, then a linear fade out.
fn notif_alpha(age: f32) -> f32 {
    ((NOTIF_LIFETIME - age) / NOTIF_FADE).clamp(0.0, 1.0)
}

/// Space (or a click on a row) sends the camera to where an alert happened,
/// reusing the same `CameraFocus` event the minimap and the idle-worker key
/// already speak. Runs before `minimap_input` in the chain so an in-progress
/// minimap drag — a live, deliberate act — always wins the frame.
fn notification_input(
    keys: Res<ButtonInput<KeyCode>>,
    game_over: Res<GameOver>,
    mut notes: ResMut<Notifications>,
    mut focus: EventWriter<CameraFocus>,
    pressed_rows: Query<(&Interaction, &NotifRow), Changed<Interaction>>,
) {
    if game_over.winner.is_some() {
        return;
    }

    // A click names its alert exactly; Space then continues down from there.
    for (interaction, row) in &pressed_rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(pos) = notes.live.get(row.0).and_then(|n| n.pos) {
            focus.write(CameraFocus { pos });
            notes.focus_cursor = row.0 + 1;
            return;
        }
    }

    if !keys.just_pressed(NOTIF_FOCUS_KEY) {
        return;
    }
    // Newest placeable alert first, then older ones, wrapping — the same
    // round-robin the idle-worker key uses. Alerts without a location (there
    // are none today, but the contract allows them) are skipped, not counted.
    let n = notes.live.len();
    for step in 0..n {
        let i = (notes.focus_cursor + step) % n;
        if let Some(pos) = notes.live[i].pos {
            focus.write(CameraFocus { pos });
            notes.focus_cursor = (i + 1) % n;
            return;
        }
    }
}

/// Drain the shared feed into the stack, expire what has had its time, and
/// paint the pool. Pooled and mutated in place — nothing is spawned or
/// despawned, matching every other refreshed part of this HUD.
#[allow(clippy::type_complexity)]
fn update_notifications(
    real: Res<Time<Real>>,
    feed: Res<GameEvents>,
    mut notes: ResMut<Notifications>,
    mut pings: ResMut<AlertPings>,
    mut ui: ResMut<UiState>,
    mut commands: Commands,
    cues: Option<Res<AlertCues>>,
    mut rows: Query<(
        &NotifRow,
        &mut Node,
        &mut BackgroundColor,
        &mut BorderColor,
        Option<&Interaction>,
    )>,
    mut texts: Query<(&NotifText, &mut Text, &mut TextColor), Without<NotifHint>>,
    mut hint: Query<&mut Text, With<NotifHint>>,
) {
    let now = real.elapsed_secs();

    // --- pull whatever is new --------------------------------------------
    // `Team::Human` is the whole filter, and it is the same one the bridge
    // applies to its seat: a renderer sees one team's feed and has no way to
    // ask for the other's.
    //
    // The same drain now feeds three senses' worth of the one event: the row
    // you read, the ring on the minimap that says *where*, and the tone that
    // says *how bad* without needing you to be looking at the HUD at all.
    // One producer, three renderers — the rule this codebase keeps.
    let mut loudest: Option<EventSeverity> = None;
    for event in feed.feed(Team::Human) {
        if event.seq <= notes.seen {
            continue;
        }
        notes.seen = event.seq;
        if let Some(pos) = event.pos {
            pings.live.push(AlertPing {
                pos,
                severity: event.severity,
                born: now,
            });
        }
        loudest = Some(match loudest {
            Some(prev) if severity_rank(prev) >= severity_rank(event.severity) => prev,
            _ => event.severity,
        });
        notes.live.push_front(Notice {
            message: event.message.clone(),
            severity: event.severity,
            pos: event.pos,
            born: now,
        });
        notes.focus_cursor = 0;
    }
    notes.live.truncate(NOTIF_SLOTS);
    // At most ONE cue per frame, and it is the worst thing that happened.
    // A base under attack while two buildings finish should sound like a base
    // under attack, not like three sounds at once — and a frame that drains
    // forty backed-up events must not fire forty tones.
    if let (Some(severity), Some(cues)) = (loudest, cues.as_deref()) {
        cues.play(&mut commands, severity);
    }
    // Rings outlive nothing: a ping is a two-and-a-bit-second flourish, and
    // the alert row it belongs to stays for nine.
    pings.live.retain(|p| now - p.born < PING_LIFETIME);

    // Ordered newest-first and stamped in arrival order, so everything stale
    // is at the back and one pop per expiry suffices.
    while notes
        .live
        .back()
        .is_some_and(|n| now - n.born >= NOTIF_LIFETIME)
    {
        notes.live.pop_back();
    }

    // Hand the row count to `cursor_over_hud` so a click on an alert is not
    // also a click on the battlefield behind it.
    ui.notif_rows = notes.live.len();

    // --- paint -------------------------------------------------------------
    for (row, mut node, mut bg, mut border, interaction) in &mut rows {
        let Some(notice) = notes.live.get(row.0) else {
            node.display = Display::None;
            continue;
        };
        node.display = Display::Flex;
        let alpha = notif_alpha(now - notice.born);
        let base = match interaction {
            Some(Interaction::Pressed) => lighten(PANEL_BG, 0.24),
            Some(Interaction::Hovered) => lighten(PANEL_BG, 0.14),
            _ => PANEL_BG,
        };
        // `lighten` returns an opaque colour; the panel is translucent and the
        // fade needs the alpha channel, so set it explicitly either way.
        bg.0 = base.with_alpha(PANEL_BG.alpha() * alpha);
        border.0 = severity_color(notice.severity).with_alpha(alpha);
    }

    for (slot, mut text, mut color) in &mut texts {
        match notes.live.get(slot.0) {
            Some(notice) => {
                if text.0 != notice.message {
                    text.0.clone_from(&notice.message);
                }
                let tint = lighten(severity_color(notice.severity), 0.15);
                color.0 = tint.with_alpha(notif_alpha(now - notice.born));
            }
            None => {
                if !text.0.is_empty() {
                    text.0.clear();
                }
            }
        }
    }

    if let Ok(mut text) = hint.single_mut() {
        let wanted = if notes.live.is_empty() {
            ""
        } else {
            "[Space] focus alert"
        };
        if text.0 != wanted {
            text.0 = wanted.to_string();
        }
    }
}

// ---------------------------------------------------------------------------
// Co-command: answering the partner
// ---------------------------------------------------------------------------
//
// The human's whole half of the negotiation is two keys and two buttons. That
// is on purpose: an approval that costs a menu is an approval that gets given
// on reflex, and the point of the loop is that the player actually reads the
// note before their gold is spent.
//
// `[Enter]` and `[Backspace]` act on the OLDEST pending proposal — the top
// card, the one whose clock is shortest. A player mid-fight can answer without
// aiming a mouse; a player with a moment can click the card they mean.

/// Which of the three answers a veto is, read off the modifiers HELD when the
/// veto is given.
///
/// A held modifier rather than a follow-up key, and that is the whole input
/// decision. Surrender's "F12 twice within 3 seconds" is the right shape for
/// an irreversible act — it buys a moment of doubt. A veto is the *safe*
/// answer, the one given under pressure, and charging two keystrokes for it
/// would push the player toward the cheaper button, which is approval. So the
/// reason rides along with the same press:
///
/// * `[Bksp]` — **not now**. The bare key stays one key, and the softest of
///   the three is the right thing to mean when you had no time to modify.
/// * `[Shift]+[Bksp]` — **wrong target**. Shift already means "same gesture,
///   different scope" in this HUD (shift-click adds to a selection): keep the
///   thing, change what it covers.
/// * `[Ctrl]+[Bksp]` — **never**. Ctrl is how this HUD makes something
///   standing (ctrl-digit binds a control group), and `never` is the standing
///   answer: do not raise it again this match.
///
/// Ctrl wins a Ctrl+Shift press, because the stronger refusal is the one you
/// meant if you managed to hold both.
fn veto_reason(keys: &ButtonInput<KeyCode>) -> VetoReason {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        VetoReason::Never
    } else if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        VetoReason::WrongTarget
    } else {
        VetoReason::NotNow
    }
}

/// Turn the two keys and the per-card buttons into `ProposalVerdict`s.
/// copilot.rs is the only thing that acts on them — this system knows nothing
/// about what a proposal contains, which is what keeps the rendering side out
/// of the trust policy.
fn proposal_input(
    keys: Res<ButtonInput<KeyCode>>,
    game_over: Res<GameOver>,
    copilot: Res<Copilot>,
    mut verdicts: EventWriter<ProposalVerdict>,
    pressed: Query<(&Interaction, &PropBtn), Changed<Interaction>>,
) {
    if game_over.winner.is_some() || copilot.seat.is_none() {
        return;
    }
    // A click names its proposal exactly — and reads the same modifiers, so
    // the mouse and the keyboard can say all three things.
    for (interaction, btn) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(proposal) = copilot.pending.get(btn.card) {
            verdicts.write(if btn.approve {
                ProposalVerdict::approve(proposal.id)
            } else {
                ProposalVerdict::veto(proposal.id, veto_reason(&keys))
            });
        }
    }
    // The keys always mean the top card — index 0, which copilot.rs keeps as
    // the most-urgent-oldest rather than the plain oldest.
    let Some(top) = copilot.pending.first() else {
        return;
    };
    if keys.just_pressed(PROP_APPROVE_KEY) {
        verdicts.write(ProposalVerdict::approve(top.id));
    } else if keys.just_pressed(PROP_VETO_KEY) {
        verdicts.write(ProposalVerdict::veto(top.id, veto_reason(&keys)));
    }
}

/// Paint the pending queue. Runs `.after(CopilotSet)`, so a proposal answered
/// this frame is off the screen this frame rather than blinking once more.
fn update_proposals(
    time: Res<Time>,
    copilot: Res<Copilot>,
    mut ui: ResMut<UiState>,
    mut cards: Query<(&PropCard, &mut Node, &mut BackgroundColor, &mut BorderColor)>,
    mut texts: Query<(&PropText, &mut Text, &mut TextColor)>,
    mut buttons: Query<(&PropBtn, &mut Node), Without<PropCard>>,
) {
    let now = time.elapsed_secs();
    let pending = &copilot.pending;
    // Hand the count to `cursor_over_hud` so a click on a card is not also a
    // click on the battlefield behind it.
    ui.prop_cards = pending.len();

    for (card, mut node, mut bg, mut border) in &mut cards {
        let Some(proposal) = pending.get(card.0) else {
            node.display = Display::None;
            continue;
        };
        node.display = Display::Flex;
        // The top card — the one the keys answer — reads slightly brighter, so
        // "which one will Enter take?" is answerable without counting.
        bg.0 = if card.0 == 0 {
            lighten(PANEL_BG, 0.10).with_alpha(PANEL_BG.alpha())
        } else {
            PANEL_BG
        };
        // The spine is the card's severity, exactly as it is in the alert
        // stack. Since urgent proposals sort to the front, the amber spines
        // are always the top of the panel — the block of colour IS the
        // "answer these first" instruction, with nothing to read.
        border.0 = accent_of(proposal);
    }

    for (btn, mut node) in &mut buttons {
        node.display = if btn.card < pending.len() {
            Display::Flex
        } else {
            Display::None
        };
    }

    for (slot, mut text, mut color) in &mut texts {
        let wanted = match pending.get(slot.card) {
            None => String::new(),
            Some(proposal) => match slot.part {
                PropPart::Head => {
                    let left = proposal.expires_in(now);
                    // The age is the honest way to show a clock in a game with
                    // `WC3_SPEED`: game seconds, the same unit the co-commander
                    // read off its snapshot when it wrote this.
                    //
                    // The key legend is on its own line and only on the top
                    // card, because it is three answers now and one line of
                    // header cannot hold both a clock and a menu. Only the
                    // card the keys act on needs it, which is also the only
                    // card that would have room.
                    let keys = if slot.card == 0 {
                        "\n[Enter] approve   [Bksp] not now   \
                         +Shift wrong target   +Ctrl never"
                    } else {
                        ""
                    };
                    let urgent = if proposal.is_urgent() { "  URGENT" } else { "" };
                    // ASCII only, here and in `proposal_body`. Bevy's default
                    // font has no glyph for `·`, and a HUD that renders its
                    // own bullet points as tofu boxes is a HUD nobody reads
                    // twice — caught by looking at the screenshot, which is
                    // the only way this class of bug is ever caught.
                    format!(
                        "#{}  copilot{urgent}   {left:.0}s left   ({}/{}){keys}",
                        proposal.id,
                        (now - proposal.proposed_at).min(PROPOSAL_TTL).round(),
                        PROPOSAL_TTL.round(),
                    )
                }
                PropPart::Note => proposal.note.clone(),
                PropPart::Body => proposal_body(proposal),
            },
        };
        if text.0 != wanted {
            text.0 = wanted;
        }
        // Only the headline changes colour with severity. The note is the
        // partner's own words and the body is compiled English; tinting those
        // would say the SENTENCES are urgent, which is not the claim.
        if slot.part == PropPart::Head {
            let wanted = pending.get(slot.card).map_or(PROP_ACCENT, accent_of);
            if color.0 != wanted {
                color.0 = wanted;
            }
        }
    }
}

/// A card's colour: the co-commander's violet, or the HUD's Warning amber when
/// the proposal claims a closing window.
fn accent_of(proposal: &crate::copilot::Proposal) -> Color {
    if proposal.is_urgent() {
        PROP_URGENT
    } else {
        PROP_ACCENT
    }
}

/// The card's lower half: what would happen, then what it would disturb.
///
/// The sentences are `Intent::sentence()` — the identical English the replay
/// log writes and a bridge commander's own compile prints back. Nothing here
/// renders a proposal into words; the intent layer already did, which is why
/// the human is reading exactly what the co-commander asked for rather than a
/// summary of it.
fn proposal_body(proposal: &crate::copilot::Proposal) -> String {
    let mut lines: Vec<String> = proposal
        .sentences
        .iter()
        .take(PROP_MAX_SENTENCES)
        .map(|s| format!("  - {s}"))
        .collect();
    if let Some(rest) = proposal.sentences.len().checked_sub(PROP_MAX_SENTENCES) {
        if rest > 0 {
            lines.push(format!("  - (+{rest} more)"));
        }
    }
    for conflict in &proposal.conflicts {
        lines.push(format!("  ! {conflict}"));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Tests — the human half of docs/TEMPO.md's doctrine parity
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // Only the co-command tests need the negotiation's own types; the panel
    // itself is written against `Copilot` and knows no more than it must.
    use crate::copilot::{ProposalSeverity, Verdict};

    /// Every intent the interface submitted, in order. Standing in for
    /// `bridge/intent_log.jsonl`, which is fed from exactly this event.
    #[derive(Resource, Default)]
    struct Said(Vec<Intent>);

    fn record(mut said: ResMut<Said>, mut events: EventReader<SubmitIntent>) {
        for e in events.read() {
            // The source is recorded, never consulted: the compiler cannot
            // tell a gesture from a command, and neither can this test.
            assert_eq!(e.source, IntentSource::Ui);
            assert_eq!(e.team, Team::Human);
            said.0.push(e.intent.clone());
        }
    }

    /// A world with the real input systems and nothing else — no window, no
    /// camera, no renderer. Every gesture below is the production code path.
    fn ui_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<UiState>()
            .init_resource::<Said>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Economies>()
            .init_resource::<HeroRecords>()
            .init_resource::<GameOver>()
            .init_resource::<TechTiers>()
            .init_resource::<SquadOrders>()
            // `CastLookup` reads them, so the card cannot be built without
            // them: research for the forge buttons, triggers and the clock for
            // the home-guard toggle's lit state, regions for the mark tile's.
            .init_resource::<TeamResearch>()
            .init_resource::<Triggers>()
            .init_resource::<Regions>()
            .init_resource::<Plans>()
            .init_resource::<Time>()
            .add_event::<CameraFocus>()
            .add_event::<SubmitIntent>()
            .add_systems(Update, (control_groups, command_input, record).chain());
        app
    }

    /// One frame with `keys` held. `ButtonInput::just_pressed` is cleared by
    /// bevy's input plugin, which is not running here, so the test clears it.
    fn press(app: &mut App, keys: &[KeyCode]) {
        {
            let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            input.clear();
            input.release_all();
            for key in keys {
                input.press(*key);
            }
        }
        app.update();
    }

    fn spawn_selected_footman(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(at),
                Health::new(100.0),
                Order::Idle,
                Selected,
            ))
            .id()
    }

    fn said(app: &App) -> &[Intent] {
        &app.world().resource::<Said>().0
    }

    /// The renderer half of the equitable-error-visibility change.
    ///
    /// `intent.rs` raises a refused `ui` gesture on the team's `GameEvents`
    /// feed; this asserts the alert stack actually picks such a notice up,
    /// colours it as a warning, and counts it for `cursor_over_hud` — the
    /// three things a screenshot of the corner of the screen would show. The
    /// compiler-side half (that the notice is raised at all, with the bridge's
    /// exact string, once per distinct problem) lives in `intent::tests`.
    #[test]
    fn a_refused_gesture_shows_up_in_the_alert_stack() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<UiState>()
            .init_resource::<Time<Real>>()
            .init_resource::<GameEvents>()
            .init_resource::<Notifications>()
            // The drain feeds the ping list too. No `AlertCues` here, though —
            // that one is deliberately `Option<Res<_>>` so a renderer test
            // never has to stand up an audio device to check a text row.
            .init_resource::<AlertPings>()
            .add_systems(Update, update_notifications);

        // Exactly what `UiNotices::raise` pushes.
        let message = "order refused: target 41 not found".to_string();
        app.world_mut().resource_mut::<GameEvents>().push(
            Team::Human,
            12.5,
            message.clone(),
            EventSeverity::Warning,
            None,
        );
        app.update();

        let notes = app.world().resource::<Notifications>();
        assert_eq!(notes.live.len(), 1, "the refusal reached the stack");
        assert_eq!(notes.live[0].message, message);
        assert_eq!(notes.live[0].severity, EventSeverity::Warning);
        // Amber, the HUD's existing "something of yours went wrong" colour —
        // not the red reserved for a hero down or a building lost.
        assert_eq!(
            severity_color(notes.live[0].severity),
            Color::srgb(1.0, 0.86, 0.35)
        );
        // A placeless alert must not be a camera-jump target: `[Space]` skips
        // it rather than sending the view somewhere nothing happened.
        assert_eq!(notes.live[0].pos, None);
        // And the stack now occupies rows, so a click on it is not also a
        // click on the battlefield behind it.
        assert_eq!(app.world().resource::<UiState>().notif_rows, 1);

        // Re-running with nothing new must not duplicate it — the feed is
        // drained by `seq`, and a rejection is news exactly once.
        app.update();
        assert_eq!(app.world().resource::<Notifications>().live.len(), 1);
    }

    // -----------------------------------------------------------------
    // Co-command: the human's half of the negotiation
    // -----------------------------------------------------------------

    fn a_proposal(id: u32, note: &str, sentences: &[&str], conflicts: &[&str]) -> crate::copilot::Proposal {
        crate::copilot::Proposal {
            id,
            note: note.to_string(),
            intents: vec![Intent::Stop { units: vec![1] }],
            sentences: sentences.iter().map(|s| s.to_string()).collect(),
            conflicts: conflicts.iter().map(|s| s.to_string()).collect(),
            severity: ProposalSeverity::Routine,
            proposed_at: 0.0,
            expires_at: PROPOSAL_TTL,
            pos: None,
        }
    }

    fn urgent(mut proposal: crate::copilot::Proposal) -> crate::copilot::Proposal {
        proposal.severity = ProposalSeverity::Urgent;
        proposal
    }

    /// The panel and its two keys, with no window and no renderer — the same
    /// headless-App discipline the doctrine-card gesture tests use. Every
    /// keystroke below goes through the production system.
    fn proposal_app(pending: Vec<crate::copilot::Proposal>) -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<UiState>()
            .init_resource::<Time>()
            .init_resource::<GameOver>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(Copilot::seated_with(Team::Human, pending))
            .add_event::<ProposalVerdict>()
            .add_systems(Startup, |mut commands: Commands| {
                spawn_proposals(&mut commands)
            })
            .add_systems(Update, (proposal_input, update_proposals).chain());
        app
    }

    fn verdicts(app: &mut App) -> Vec<ProposalVerdict> {
        let events = app.world().resource::<Events<ProposalVerdict>>();
        events.get_cursor().read(events).copied().collect()
    }

    /// The approve path, through the real hotkey.
    ///
    /// This is the human side of `copilot::a_proposal_waits_then_lands_stamped_by_the_copilot`:
    /// that test proves an approved batch reaches the compiler stamped by the
    /// partner, this one proves a keystroke is what approves it. Between them
    /// the loop has no hand-waved step.
    #[test]
    fn enter_approves_the_top_proposal_and_backspace_vetoes_it() {
        let mut app = proposal_app(vec![
            a_proposal(7, "take the ford", &["3 units attack-move to (0.0, 0.0)"], &[]),
            a_proposal(8, "and build a tower", &["worker 41 builds Tower at (1.0, 2.0)"], &[]),
        ]);

        press(&mut app, &[PROP_APPROVE_KEY]);
        let said = verdicts(&mut app);
        assert_eq!(said.len(), 1, "one key, one verdict");
        // The TOP card — index 0 of the queue copilot.rs keeps in answer
        // order. Not the newest, which is what a stack would have given.
        assert_eq!(said[0].id, 7);
        assert_eq!(said[0].verdict, Verdict::Approve);

        let mut app = proposal_app(vec![a_proposal(7, "spend it all", &["x"], &[])]);
        press(&mut app, &[PROP_VETO_KEY]);
        let said = verdicts(&mut app);
        assert_eq!(said.len(), 1);
        // A bare Backspace is still one key, and it means the softest of the
        // three answers — the fast path must not get slower for the reasons.
        assert_eq!(
            (said[0].id, said[0].verdict),
            (7, Verdict::Veto(VetoReason::NotNow))
        );
    }

    /// **The two-sided veto, at the keyboard.** Which of the three answers a
    /// veto is comes from the modifiers HELD during the same press, so the
    /// reason costs nothing on top of the refusal. A follow-up key would have
    /// charged two keystrokes for the safe answer and one for approval, which
    /// is exactly the wrong incentive to build into a consent loop.
    #[test]
    fn the_held_modifier_picks_which_no_the_veto_is() {
        let cases = [
            (vec![PROP_VETO_KEY], VetoReason::NotNow),
            (vec![KeyCode::ShiftLeft, PROP_VETO_KEY], VetoReason::WrongTarget),
            (vec![KeyCode::ControlLeft, PROP_VETO_KEY], VetoReason::Never),
            // Both held: the stronger refusal is the one you managed to mean.
            (
                vec![KeyCode::ControlLeft, KeyCode::ShiftLeft, PROP_VETO_KEY],
                VetoReason::Never,
            ),
            (vec![KeyCode::ShiftRight, PROP_VETO_KEY], VetoReason::WrongTarget),
            (vec![KeyCode::ControlRight, PROP_VETO_KEY], VetoReason::Never),
        ];
        for (keys, want) in cases {
            let mut app = proposal_app(vec![a_proposal(7, "hit their siege", &["x"], &[])]);
            press(&mut app, &keys);
            let said = verdicts(&mut app);
            assert_eq!(said.len(), 1, "{keys:?}");
            assert_eq!(said[0].verdict, Verdict::Veto(want), "{keys:?}");
        }

        // Approval is unmodified by any of it: a held Ctrl must never turn a
        // yes into a no.
        let mut app = proposal_app(vec![a_proposal(7, "x", &["x"], &[])]);
        press(&mut app, &[KeyCode::ControlLeft, PROP_APPROVE_KEY]);
        assert_eq!(verdicts(&mut app)[0].verdict, Verdict::Approve);
    }

    /// The top card has to TEACH the three answers, or two of them may as well
    /// not exist. `[Bksp]` is labelled by what it means, not by "veto".
    #[test]
    fn the_top_card_names_all_three_answers() {
        let mut app = proposal_app(vec![
            a_proposal(3, "counter now", &["x"], &[]),
            a_proposal(4, "and expand", &["y"], &[]),
        ]);
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&PropText, &Text)>();
        let heads: Vec<(usize, String)> = q
            .iter(world)
            .filter(|(slot, _)| slot.part == PropPart::Head)
            .map(|(slot, text)| (slot.card, text.0.clone()))
            .collect();
        let top = &heads.iter().find(|(card, _)| *card == 0).unwrap().1;
        for word in ["[Enter] approve", "[Bksp] not now", "+Shift wrong target", "+Ctrl never"] {
            assert!(top.contains(word), "top card must say {word:?}: {top:?}");
        }
        // Only the card the keys act on carries the legend — the others have
        // no room and the keys do not reach them anyway.
        let second = &heads.iter().find(|(card, _)| *card == 1).unwrap().1;
        assert!(!second.contains("[Enter]"), "got {second:?}");
        assert!(second.starts_with("#4  copilot"), "got {second:?}");
    }

    /// Urgency is legible before it is read: the spine and the headline take
    /// the HUD's own Warning amber, and the word `URGENT` is in the header.
    #[test]
    fn an_urgent_card_wears_the_warning_tint() {
        assert_eq!(
            PROP_URGENT,
            severity_color(EventSeverity::Warning),
            "the urgent tint IS the HUD's warning colour, not a lookalike"
        );
        let mut app = proposal_app(vec![
            urgent(a_proposal(9, "they are flanking", &["x"], &[])),
            a_proposal(10, "expand north", &["y"], &[]),
        ]);
        app.update();

        let world = app.world_mut();
        let mut cards = world.query::<(&PropCard, &BorderColor)>();
        let spines: Vec<(usize, Color)> = cards
            .iter(world)
            .map(|(card, border)| (card.0, border.0))
            .collect();
        assert_eq!(
            spines.iter().find(|(i, _)| *i == 0).unwrap().1,
            PROP_URGENT
        );
        assert_eq!(
            spines.iter().find(|(i, _)| *i == 1).unwrap().1,
            PROP_ACCENT,
            "a routine card keeps the partner's own violet"
        );

        let mut texts = world.query::<(&PropText, &Text, &TextColor)>();
        for (slot, text, color) in texts.iter(world) {
            if slot.part != PropPart::Head {
                continue;
            }
            match slot.card {
                0 => {
                    assert!(text.0.contains("URGENT"), "got {:?}", text.0);
                    assert_eq!(color.0, PROP_URGENT);
                }
                1 => {
                    assert!(!text.0.contains("URGENT"), "got {:?}", text.0);
                    assert_eq!(color.0, PROP_ACCENT);
                }
                _ => {}
            }
        }
    }

    /// **The whole negotiation over one App, with nothing stubbed between the
    /// keystroke and the wire.**
    ///
    /// The tests above each hold one end of the loop: copilot.rs proves a
    /// reason reaches the feed and the tail, this module proves a modifier
    /// picks the reason. Neither on its own rules out the two halves having
    /// been wired to different things. This one runs the real `CopilotPlugin`
    /// beside the real panel systems, delivers a proposal down the real wire,
    /// and answers it with a real `[Ctrl]+[Bksp]` — so the sentence the
    /// co-commander will read is produced by the key the human actually
    /// pressed.
    #[test]
    fn ctrl_backspace_on_a_wired_proposal_tells_the_partner_never() {
        use crate::copilot::{CopilotPlugin, CopilotWire};
        use crate::intent::{IntentLog, IntentPlugin};

        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<UiState>()
            .init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Economies>()
            .init_resource::<HeroRecords>()
            .init_resource::<TechTiers>()
            .init_resource::<NavGrid>()
            .init_resource::<TeamResearch>()
            .init_resource::<SquadOrders>()
            .init_resource::<AiControlled>()
            .init_resource::<GameEvents>()
            .init_resource::<GameOver>()
            .init_resource::<FogGrids>()
            .init_resource::<crate::command::CommandNodes>()
            .init_resource::<crate::command::CommandLatency>()
            .add_event::<CastAbility>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .add_plugins((IntentPlugin, CopilotPlugin))
            .insert_resource(IntentLog::disabled())
            .add_systems(Startup, |mut commands: Commands| {
                spawn_proposals(&mut commands)
            })
            .add_systems(
                Update,
                (
                    proposal_input.before(CopilotSet),
                    update_proposals.after(CopilotSet),
                ),
            );
        app.world_mut().resource_mut::<Copilot>().seat(Team::Human);
        let unit = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::default(),
                Health::new(100.0),
                Order::Idle,
            ))
            .id();

        // A routine proposal, then an urgent one down the same wire.
        for (note, severity) in [
            ("expand north later", "routine"),
            ("their siege is unescorted RIGHT NOW", "urgent"),
        ] {
            let raw: serde_json::Value = serde_json::from_str(&format!(
                r#"{{"type":"propose","note":"{note}","severity":"{severity}",
                     "commands":[{{"type":"move","units":[{}],"x":9.0,"z":9.0}}]}}"#,
                intent_id(unit)
            ))
            .expect("test json");
            app.world_mut().send_event(CopilotWire {
                team: Team::Human,
                tag: "cmd 0".to_string(),
                raw,
            });
            app.update();
        }

        // The urgent one jumped: it is the top card, so it is what the keys
        // answer even though the routine one was asked first.
        {
            let copilot = app.world().resource::<Copilot>();
            assert_eq!(copilot.pending.len(), 2);
            assert_eq!(copilot.pending[0].id, 2, "urgent is answered first");
            assert!(copilot.pending[0].is_urgent());
        }

        // The human says: never. One press, Ctrl held.
        press(&mut app, &[KeyCode::ControlLeft, PROP_VETO_KEY]);

        let copilot = app.world().resource::<Copilot>();
        assert_eq!(
            copilot.pending.len(),
            1,
            "only the card the keys act on was answered"
        );
        assert_eq!(copilot.pending[0].id, 1, "the routine one still waits");
        let resolution = copilot.resolved.back().expect("it left a resolution");
        assert_eq!(resolution.id, 2);
        assert_eq!(
            resolution.outcome,
            crate::copilot::Outcome::Vetoed(VetoReason::Never)
        );
        assert_eq!(resolution.severity, ProposalSeverity::Urgent);
        // And the sentence the co-commander will actually read.
        let line = app
            .world()
            .resource::<GameEvents>()
            .feed(Team::Human)
            .iter()
            .map(|e| e.message.clone())
            .find(|m| m.contains("vetoed"))
            .expect("announced");
        assert!(
            line.contains("never") && line.contains("do not re-propose this match"),
            "got {line}"
        );
        // Nothing was submitted: a veto is not a delay, whatever its reason.
        assert!(
            app.world()
                .entity(unit)
                .get::<Provenance>()
                .is_none(),
            "the unit was never touched"
        );
    }

    /// The veto BUTTON reads the same modifiers as the key, so the mouse can
    /// say all three things too — a player who is clicking cards rather than
    /// hammering keys is exactly the one with time to be specific.
    #[test]
    fn the_veto_button_reads_the_modifiers_too() {
        let mut app = proposal_app(vec![a_proposal(7, "x", &["x"], &[])]);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        let world = app.world_mut();
        let mut q = world.query::<(Entity, &PropBtn)>();
        let veto = q
            .iter(world)
            .find(|(_, btn)| !btn.approve && btn.card == 0)
            .map(|(e, _)| e)
            .expect("card 0 has a veto button");
        world.entity_mut(veto).insert(Interaction::Pressed);
        app.update();

        let said = verdicts(&mut app);
        assert_eq!(said.len(), 1);
        assert_eq!(
            said[0].verdict,
            Verdict::Veto(VetoReason::WrongTarget),
            "a shift-click on veto is the same sentence as shift-Backspace"
        );
    }

    /// Nothing pending, nothing to answer — a stray Enter must not become a
    /// verdict for a proposal that lapsed a second ago.
    #[test]
    fn the_keys_are_inert_with_an_empty_queue() {
        let mut app = proposal_app(Vec::new());
        press(&mut app, &[PROP_APPROVE_KEY]);
        assert!(verdicts(&mut app).is_empty());
        assert_eq!(app.world().resource::<UiState>().prop_cards, 0);
    }

    /// What the card actually says. The note is the co-commander's argument,
    /// the sentences are `Intent::sentence()` — the same English the replay
    /// log writes — and the conflict tags are the part that makes approval an
    /// informed act rather than a reflex.
    #[test]
    fn a_card_shows_the_note_the_sentences_and_the_conflicts() {
        let mut app = proposal_app(vec![a_proposal(
            3,
            "their push is committed — counter now",
            &["squad 1 pushes to (40.0, 40.0)", "4 units focus Siege"],
            &["re-tasks squad 1 (defend)"],
        )]);
        app.update();

        let world = app.world_mut();
        let mut q = world.query::<(&PropText, &Text)>();
        let mut head = String::new();
        let mut note = String::new();
        let mut body = String::new();
        for (slot, text) in q.iter(world) {
            if slot.card != 0 {
                // Unused cards are blanked, not left showing the last match's
                // directive.
                assert!(text.0.is_empty(), "card {} should be blank", slot.card);
                continue;
            }
            match slot.part {
                PropPart::Head => head = text.0.clone(),
                PropPart::Note => note = text.0.clone(),
                PropPart::Body => body = text.0.clone(),
            }
        }

        assert!(head.starts_with("#3  copilot"), "got {head:?}");
        assert!(head.contains("[Enter] approve"), "the top card names its keys");
        assert_eq!(note, "their push is committed — counter now");
        assert!(body.contains("- squad 1 pushes to (40.0, 40.0)"));
        assert!(body.contains("- 4 units focus Siege"));
        // The conflict is marked differently from the sentences: one says what
        // would happen, the other says what it would cost you.
        assert!(body.contains("! re-tasks squad 1 (defend)"), "got {body:?}");
        // Everything the panel draws must be renderable by the default font —
        // Bevy's has no `·`, and the first screenshot of this panel was full
        // of tofu boxes where the bullets should have been.
        assert!(
            head.is_ascii() && body.is_ascii(),
            "panel chrome must be ASCII: {head:?} / {body:?}"
        );
        // And the panel now occupies screen, so a click on "Approve" is not
        // also a move order on the ground behind it.
        assert_eq!(app.world().resource::<UiState>().prop_cards, 1);
    }

    fn json(intent: &Intent) -> serde_json::Value {
        serde_json::to_value(intent).unwrap()
    }

    /// docs/TEMPO.md's highest-leverage item: `Ctrl+1` is not a UI bookmark
    /// any more, it is the `squad` verb. The muscle memory every RTS player
    /// already has becomes the shared strategic vocabulary.
    #[test]
    fn ctrl_digit_is_the_squad_verb() {
        let mut app = ui_app();
        let a = spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));
        let b = spawn_selected_footman(&mut app, Vec3::new(-8.0, 0.0, -10.0));

        press(&mut app, &[KeyCode::ControlLeft, KeyCode::Digit1]);

        assert_eq!(said(&app).len(), 1, "expected exactly one sentence");
        let gesture = &said(&app)[0];
        assert_eq!(gesture.sentence(), "2 units join squad 1");
        // Indistinguishable from what a commander sends.
        let typed: Intent = serde_json::from_str(&format!(
            r#"{{"type":"squad","units":[{},{}],"id":1}}"#,
            intent_id(a),
            intent_id(b)
        ))
        .unwrap();
        assert_eq!(json(gesture), json(&typed));
    }

    /// Re-assigning a control group must not leave a ghost membership behind,
    /// or the group the player sees and the squad doctrine.rs executes drift
    /// apart. The eviction is its own sentence, as the language requires.
    #[test]
    fn reassigning_a_group_releases_whoever_left_it() {
        let mut app = ui_app();
        let a = spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));
        let b = spawn_selected_footman(&mut app, Vec3::new(-8.0, 0.0, -10.0));
        // `a` is already in squad 1; only `b` is selected now.
        app.world_mut().entity_mut(a).insert(SquadId(1));
        app.world_mut().entity_mut(a).remove::<Selected>();

        press(&mut app, &[KeyCode::ControlLeft, KeyCode::Digit1]);

        let sentences: Vec<String> = said(&app).iter().map(|i| i.sentence()).collect();
        assert_eq!(
            sentences,
            vec![
                format!("unit {} leave their squad", intent_id(a)),
                format!("unit {} join squad 1", intent_id(b)),
            ]
        );
    }

    /// The doctrine page, end to end: [I] opens it, [W] arms Push, the ground
    /// click supplies the point. An unsquadded selection is enrolled first, so
    /// the gesture becomes the same two sentences a commander would have to
    /// send — and the log cannot tell which of them happened.
    #[test]
    fn the_doctrine_page_composes_squad_then_posture() {
        let mut app = ui_app();
        spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));
        spawn_selected_footman(&mut app, Vec3::new(-8.0, 0.0, -10.0));

        press(&mut app, &[KeyCode::KeyI]);
        assert_eq!(app.world().resource::<UiState>().page, CardPage::Doctrine);
        assert!(said(&app).is_empty(), "opening a page says nothing");

        press(&mut app, &[KeyCode::KeyW]);
        let arm = app
            .world()
            .resource::<UiState>()
            .posture_place
            .expect("Push should arm a ground click");
        assert_eq!(arm.kind, PostureKind::Push);

        // The click. Same function `left_mouse` calls.
        let click = posture_intent(arm, Vec3::new(40.0, 0.0, 40.0)).unwrap();

        let mut sentences: Vec<String> = said(&app).iter().map(|i| i.sentence()).collect();
        sentences.push(click.sentence());
        assert_eq!(
            sentences,
            vec![
                "2 units join squad 1".to_string(),
                "squad 1 pushes to (40.0, 40.0)".to_string(),
            ]
        );
        let typed: Intent = serde_json::from_str(
            r#"{"type":"posture","id":1,"posture":{"type":"push","x":40.0,"z":40.0}}"#,
        )
        .unwrap();
        assert_eq!(json(&click), json(&typed));
    }

    /// **The human's trigger gesture is a sentence a commander could type.**
    ///
    /// `[I][H]` on a selection compiles to `squad` + `trigger_set`, and the
    /// second of those is byte-identical to the JSON in COMMANDER_BRIEF.md's
    /// home-guard recipe. This is the fairness invariant applied to the newest
    /// verb in the language: the human's surface is *narrower* (one preset
    /// against thirteen predicates and 29 verbs), but nothing it produces is
    /// outside what the wire can say, and nothing the wire says is outside what
    /// the engine will do for the human.
    #[test]
    fn the_home_guard_preset_is_a_trigger_a_commander_could_have_typed() {
        let mut app = ui_app();
        spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));
        spawn_selected_footman(&mut app, Vec3::new(-8.0, 0.0, -10.0));

        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyH]);

        let out = said(&app);
        let sentences: Vec<String> = out.iter().map(|i| i.sentence()).collect();
        assert_eq!(
            sentences,
            vec![
                "2 units join squad 1".to_string(),
                "when the base is attacked: squad 1 defends (-70.0, -70.0) within 26 \
                 (trigger: home-guard, repeating every 30s)"
                    .to_string(),
            ]
        );
        let typed: Intent = serde_json::from_str(
            r#"{"type":"trigger_set","name":"home-guard",
                "when":{"type":"base_under_attack"},
                "then":{"type":"posture","id":1,
                        "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}},
                "repeat":30.0}"#,
        )
        .unwrap();
        assert_eq!(json(&out[1]), json(&typed));
    }

    // -----------------------------------------------------------------------
    // Territory: the human's half
    // -----------------------------------------------------------------------

    /// `[M]` ARMS; it does not name. A click is the second half of the
    /// gesture, exactly like building placement and postures — and until it
    /// comes, nothing has been said.
    #[test]
    fn marking_a_region_arms_a_click_and_says_nothing_yet() {
        let mut app = ui_app();
        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyM]);
        assert!(
            app.world().resource::<UiState>().region_place,
            "[M] arms the marker"
        );
        assert!(
            said(&app).is_empty(),
            "an armed gesture is not a sentence — the ground has not been picked"
        );
        // Pressed again, it disarms: the one key the human has must not be a
        // one-way door, the same rule the home-guard tile follows.
        press(&mut app, &[KeyCode::KeyM]);
        assert!(!app.world().resource::<UiState>().region_place);
    }

    /// **Armed modes are mutually exclusive**, and the region marker joins the
    /// set rather than sitting beside it. Two armed gestures would make the
    /// next click ambiguous, and the player would find out which one won by
    /// losing a building.
    #[test]
    fn arming_anything_else_disarms_the_region_marker() {
        let mut app = ui_app();
        spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));

        // Marker armed, then a posture armed on top of it.
        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyM]);
        assert!(app.world().resource::<UiState>().region_place);
        press(&mut app, &[KeyCode::KeyQ]);
        assert!(
            !app.world().resource::<UiState>().region_place,
            "arming a posture must disarm the marker"
        );
        assert!(app.world().resource::<UiState>().posture_place.is_some());

        // ...and the other way: the marker disarms the posture.
        press(&mut app, &[KeyCode::KeyM]);
        assert!(app.world().resource::<UiState>().region_place);
        assert!(
            app.world().resource::<UiState>().posture_place.is_none(),
            "arming the marker must disarm the posture"
        );

        // Flipping the card cancels whatever the other page armed, the marker
        // included — it lives on the page being left.
        press(&mut app, &[KeyCode::KeyI]);
        assert!(!app.world().resource::<UiState>().region_place);
    }

    /// The mark's name comes from the engine, and it is the lowest free slot —
    /// so a player who marks, forgets and marks again gets `mark 1` back rather
    /// than climbing forever.
    #[test]
    fn marks_are_named_from_the_lowest_free_slot() {
        let mut regions = Regions::default();
        assert_eq!(next_mark_name(&regions).as_deref(), Some("mark 1"));
        regions
            .set(Team::Human, Region::new("mark 1", Vec3::ZERO, 20.0))
            .unwrap();
        assert_eq!(next_mark_name(&regions).as_deref(), Some("mark 2"));
        // A hole in the middle is reused rather than skipped.
        regions
            .set(Team::Human, Region::new("mark 3", Vec3::ZERO, 20.0))
            .unwrap();
        assert_eq!(next_mark_name(&regions).as_deref(), Some("mark 2"));
        // Full: the tile has nothing left to offer and says so by going dark.
        for n in 2..=MAX_REGIONS_PER_TEAM {
            let _ = regions.set(Team::Human, Region::new(format!("mark {n}"), Vec3::ZERO, 20.0));
        }
        assert_eq!(next_mark_name(&regions), None);
    }

    /// The radius is free entry, on the same helper the other two numeric
    /// parameters use, clamped by the language's own bounds rather than by a
    /// second opinion in the HUD.
    #[test]
    fn the_region_radius_is_free_entry_between_the_languages_own_bounds() {
        // From "never touched" the first nudge lands on the default rather
        // than on zero.
        assert_eq!(
            nudge_value(None, true, REGION_NUDGE, REGION_MARK_RADIUS, REGION_RADIUS_MIN, REGION_RADIUS_MAX),
            Some(REGION_MARK_RADIUS)
        );
        // It climbs and stops at the ceiling the compiler would refuse past.
        let mut r = Some(REGION_RADIUS_MAX - 1.0);
        r = nudge_value(r, true, REGION_NUDGE, REGION_MARK_RADIUS, REGION_RADIUS_MIN, REGION_RADIUS_MAX);
        assert_eq!(r, Some(REGION_RADIUS_MAX), "clamped, not refused");
        // And it cannot be driven under the floor into a circle the compiler
        // would reject — the HUD never composes an illegal sentence.
        let mut r = Some(REGION_RADIUS_MIN);
        r = nudge_value(r, false, REGION_NUDGE, REGION_MARK_RADIUS, REGION_RADIUS_MIN, REGION_RADIUS_MAX);
        assert_eq!(r, None, "below the floor is 'off', not an illegal radius");
    }

    /// The panel names what the player marked, with its size, and says nothing
    /// at all when there is nothing to say.
    #[test]
    fn the_region_readout_names_every_mark_and_its_size() {
        let mut regions = Regions::default();
        assert_eq!(region_line(&regions), "", "no marks, no line");
        regions
            .set(Team::Human, Region::new("mark 1", Vec3::new(-60.0, 0.0, 60.0), 20.0))
            .unwrap();
        regions
            .set(Team::Human, Region::new("mark 2", Vec3::new(0.0, 0.0, 0.0), 26.0))
            .unwrap();
        assert_eq!(region_line(&regions), "Regions: mark 1 r20  mark 2 r26");
        // The enemy's marks are not this seat's business, and the line is the
        // snapshot's rule rendered.
        regions
            .set(Team::Claude, Region::new("their plan", Vec3::ZERO, 30.0))
            .unwrap();
        assert!(!region_line(&regions).contains("their plan"));
    }

    /// The two region tiles are on the doctrine page whatever is selected,
    /// because naming ground is not about the selection — and they carry the
    /// nudge keys on their cost line, the way every free-entry control here
    /// advertises itself.
    #[test]
    fn the_mark_tiles_are_offered_without_a_selection() {
        let card = DoctrineCard {
            region_mark: Some(1),
            region_radius: 22.0,
            ..default()
        };
        for units in [0usize, 3usize] {
            let entries = doctrine_entries(units, card);
            let mark = entries
                .iter()
                .find(|e| e.action == CmdAction::MarkRegion)
                .expect("the mark tile is always offered");
            assert!(mark.enabled);
            assert_eq!(mark.label, "Mark 1 r22");
            assert_eq!(mark.cost, "; / ' size");
            assert!(entries.iter().any(|e| e.action == CmdAction::ClearRegions));
        }
        // With all eight named the tile goes dark rather than silently
        // stealing a name already in use.
        let full = DoctrineCard {
            region_mark: None,
            region_count: MAX_REGIONS_PER_TEAM,
            ..default()
        };
        let entries = doctrine_entries(0, full);
        let mark = entries
            .iter()
            .find(|e| e.action == CmdAction::MarkRegion)
            .unwrap();
        assert!(!mark.enabled);
        assert!(mark.cost.contains(&MAX_REGIONS_PER_TEAM.to_string()));
    }

    /// Forgetting is the whole-slate form — the only one a mouse can reach,
    /// and the same sentence a commander sends as
    /// `{"type":"region_clear"}`.
    #[test]
    fn forget_marks_is_the_sentence_a_commander_could_have_typed() {
        let mut app = ui_app();
        app.world_mut()
            .resource_mut::<Regions>()
            .set(Team::Human, Region::new("mark 1", Vec3::ZERO, 20.0))
            .unwrap();
        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyN]);
        let out = said(&app);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].sentence(), "forget every region");
        let typed: Intent = serde_json::from_str(r#"{"type":"region_clear"}"#).unwrap();
        assert_eq!(json(&out[0]), json(&typed));
    }

    /// The tile is a TOGGLE, like `[G] Guard`: pressed while armed it clears
    /// the rule rather than arming a second copy. Without this the one key the
    /// human has would be a one-way door.
    #[test]
    fn pressing_home_guard_again_clears_it() {
        let mut app = ui_app();
        spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));

        // Arm it for real, through the resource the card reads.
        app.world_mut()
            .resource_mut::<Triggers>()
            .set(
                Team::Human,
                TriggerRule {
                    name: TriggerName::new(HOME_GUARD).unwrap(),
                    when: TriggerWhen::BaseUnderAttack,
                    then: Intent::Stop { units: vec![] },
                    repeat: Some(HOME_GUARD_COOLDOWN),
                    source: IntentSource::Ui,
                    armed: true,
                    last_fired: None,
                },
            )
            .unwrap();

        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyH]);
        let sentences: Vec<String> = said(&app).iter().map(|i| i.sentence()).collect();
        assert_eq!(sentences, vec!["clear trigger home-guard".to_string()]);
    }

    /// The human's whole "list" view. One line, because eight short names fit
    /// on one and a scrolling panel would be the authoring UI this deliberately
    /// defers.
    #[test]
    fn the_trigger_readout_names_every_rule_and_its_state() {
        let mut triggers = Triggers::default();
        let rule = |name: &str, repeat, armed, last| TriggerRule {
            name: TriggerName::new(name).unwrap(),
            when: TriggerWhen::BaseUnderAttack,
            then: Intent::Stop { units: vec![] },
            repeat,
            source: IntentSource::Bridge,
            armed,
            last_fired: last,
        };
        assert_eq!(trigger_line(&triggers, 0.0), "", "silent until there is one");

        triggers.set(Team::Human, rule("home-guard", Some(30.0), true, None)).unwrap();
        triggers.set(Team::Human, rule("hero-save", None, false, Some(12.0))).unwrap();
        triggers.set(Team::Human, rule("alarm", Some(60.0), true, Some(90.0))).unwrap();
        assert_eq!(
            trigger_line(&triggers, 100.0),
            "Triggers: home-guard  hero-save (spent)  alarm (cooling)"
        );

        // The opponent's rules are their plans, and the panel never sees them.
        triggers.set(Team::Claude, rule("theirs", None, true, None)).unwrap();
        assert!(!trigger_line(&triggers, 100.0).contains("theirs"));
    }

    /// The human's plan readout. Status only — see `plan_line` for why
    /// authoring stays in NL/preset territory, which is the same asymmetry the
    /// trigger readout above documents.
    ///
    /// The load-bearing part is that a stopped plan says WHY on the line. A
    /// status of "blocked" that made its owner go read `errors` would put the
    /// human back in the polling loop this whole vocabulary exists to delete.
    #[test]
    fn the_plan_readout_names_every_plan_its_step_and_why_it_stopped() {
        let mut plans = Plans::default();
        let make = |name: &str, steps: usize, at: usize, state: PlanState| PlanRun {
            name: PlanName::new(name).unwrap(),
            steps: (0..steps)
                .map(|_| PlanStep {
                    intent: Intent::Stop { units: vec![] },
                    advance: PlanAdvance::OnApplied,
                })
                .collect(),
            source: IntentSource::Bridge,
            state,
            at,
            submitted: true,
            applied: true,
            applied_at: 0.0,
            last_try: 0.0,
            blocked_since: None,
            told_blocked: false,
        };
        assert_eq!(plan_line(&plans), "", "silent until there is one");

        plans
            .set(Team::Human, make("opening", 5, 1, PlanState::Running))
            .unwrap();
        assert_eq!(plan_line(&plans), "Plans: opening 2/5");

        plans
            .set(
                Team::Human,
                make(
                    "boom",
                    3,
                    2,
                    PlanState::Blocked("not enough gold".to_string()),
                ),
            )
            .unwrap();
        assert_eq!(
            plan_line(&plans),
            "Plans: opening 2/5  boom 3/3 (blocked: not enough gold)"
        );

        // A long refusal is truncated rather than allowed to push the other
        // plan off the end of the line.
        plans.get_mut(Team::Human)[1].state = PlanState::Halted(
            "site (56.0, -56.0) is blocked for TownHall; try (49.0, -56.0)".to_string(),
        );
        let line = plan_line(&plans);
        assert!(line.contains("boom 3/3 (halted: site (56.0, -56.0) is blocked for …)"), "{line}");
        assert!(line.starts_with("Plans: opening 2/5"), "{line}");

        // A finished plan stays visible and says so.
        plans.get_mut(Team::Human)[0].state = PlanState::Done;
        assert!(plan_line(&plans).contains("opening 2/5 (done)"));

        // The opponent's plans are theirs, and the panel never sees them.
        plans
            .set(Team::Claude, make("theirs", 2, 0, PlanState::Running))
            .unwrap();
        assert!(!plan_line(&plans).contains("theirs"));
    }

    /// **The hero intel line says only what the human could have seen.**
    ///
    /// The renderer half of the one rule: this line and the snapshot's
    /// `intel.heroes` are built from the same `FogGrid::hero_intel()`, so they
    /// cannot disagree about what is known. What it must never print is a
    /// LEVEL — a human cannot select an enemy hero, so no number about one has
    /// ever been on their screen, and printing it here would hand the keyboard
    /// an information right the wire does not have.
    #[test]
    fn the_enemy_hero_line_reports_belief_and_never_a_level() {
        let mut grids = FogGrids::test_dark();
        assert_eq!(
            enemy_hero_line(grids.get(Team::Human), 100.0),
            "",
            "silent until one has been laid eyes on — an unmet roster is not intel"
        );

        grids.test_hero_intel(
            Team::Human,
            UnitKind::Hero,
            HeroStatus::Alive,
            Vec3::new(4.0, 0.0, 8.0),
        );
        let line = enemy_hero_line(grids.get(Team::Human), 40.0);
        assert_eq!(line, "Their heroes: Hero alive 40s ago");
        // The class never met stays off the line entirely.
        assert!(!line.contains("Priestess"));

        grids.test_hero_intel(
            Team::Human,
            UnitKind::Priestess,
            HeroStatus::SeenDying,
            Vec3::new(4.0, 0.0, 8.0),
        );
        let line = enemy_hero_line(grids.get(Team::Human), 40.0);
        assert_eq!(line, "Their heroes: Hero alive 40s ago   Priestess down");
        // Nothing a human has no gesture to obtain.
        for forbidden in ["Lv", "level", "mana", "XP"] {
            assert!(!line.contains(forbidden), "leaked {forbidden}: {line}");
        }
    }

    /// The fade bands a last-seen marker wears, pinned at both ends: a fresh
    /// sighting is at full strength and one at the horizon is at the faintest
    /// band, so nothing ever blinks out while still bright.
    #[test]
    fn intel_markers_fade_across_the_whole_staleness_horizon() {
        assert_eq!(intel_fade_step(0.0), 0);
        assert_eq!(intel_fade_step(SIGHTING_TTL_S - 0.1), INTEL_FADE_STEPS - 1);
        // Clamped rather than panicking on a record the ledger has not yet
        // swept: the renderer must never be the thing that crashes.
        assert_eq!(intel_fade_step(SIGHTING_TTL_S * 4.0), INTEL_FADE_STEPS - 1);
        // Monotonic — a marker may never get BRIGHTER with age.
        let mut prev = 0;
        for i in 0..40 {
            let step = intel_fade_step(i as f32 * (SIGHTING_TTL_S / 40.0));
            assert!(step >= prev, "fade went backwards at {i}");
            prev = step;
        }
    }

    /// The parameterised half of the gap: the coarse [V] writes one fixed
    /// threshold, [F] on the doctrine page walks the ladder — an actual number,
    /// chosen by the human, exactly as the bridge's `below` field is.
    #[test]
    fn the_doctrine_page_parameterises_retreat_and_leash() {
        let mut app = ui_app();
        let unit = spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));

        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyF]);
        press(&mut app, &[KeyCode::KeyG]);

        let sentences: Vec<String> = said(&app).iter().map(|i| i.sentence()).collect();
        assert_eq!(
            sentences,
            vec![
                // No town hall in this world, so the rally is the start base.
                format!(
                    "unit {} fall back to (-70.0, -70.0) below 25% health",
                    intent_id(unit)
                ),
                format!("unit {} hold within 10 of (-10.0, -10.0)", intent_id(unit)),
            ]
        );
        // The steps are a ladder, not a toggle: the same key again moves up.
        assert_eq!(cycle_step(Some(0.25), &FALLBACK_STEPS), Some(0.35));
        assert_eq!(cycle_step(Some(0.50), &FALLBACK_STEPS), None);
        assert_eq!(cycle_step(None, &LEASH_STEPS), Some(10.0));
        assert_eq!(cycle_step(Some(30.0), &LEASH_STEPS), None);
    }

    /// Three rungs is not a number line. The wire carries any float and a
    /// commander types any float; `[-]/[=]` and `[[]/[]]` are how the human
    /// says one — and they must produce the SAME sentence the preset key does,
    /// or the seats have two dialects for one idea.
    #[test]
    fn the_nudge_keys_give_the_human_the_whole_number_line() {
        let mut app = ui_app();
        let unit = spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));

        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::Equal]);
        press(&mut app, &[KeyCode::BracketRight]);

        // No intent compiler is running here, so the components never appear
        // and each press starts from "off" — which is exactly the case worth
        // pinning: the first nudge lands on the middle preset rather than at
        // the bottom of the range.
        let sentences: Vec<String> = said(&app).iter().map(|i| i.sentence()).collect();
        assert_eq!(
            sentences,
            vec![
                format!(
                    "unit {} fall back to (-70.0, -70.0) below 35% health",
                    intent_id(unit)
                ),
                format!("unit {} hold within 18 of (-10.0, -10.0)", intent_id(unit)),
            ]
        );

        // The ladder itself: any value, not just the three rungs.
        assert_eq!(
            nudge_value(Some(0.35), true, FALLBACK_NUDGE, 0.35, FALLBACK_MIN, FALLBACK_MAX),
            Some(0.40)
        );
        // A bridge-written 0.375 stays off-grid rather than snapping.
        let odd = nudge_value(Some(0.375), true, FALLBACK_NUDGE, 0.35, FALLBACK_MIN, FALLBACK_MAX);
        assert!((odd.unwrap() - 0.425).abs() < 1e-5, "got {odd:?}");
        // Down past the floor is "off" — the same exit the [F] cycle wraps to.
        assert_eq!(
            nudge_value(Some(FALLBACK_MIN), false, FALLBACK_NUDGE, 0.35, FALLBACK_MIN, FALLBACK_MAX),
            None
        );
        // ...and the ceiling clamps instead of running away.
        assert_eq!(
            nudge_value(Some(FALLBACK_MAX), true, FALLBACK_NUDGE, 0.35, FALLBACK_MIN, FALLBACK_MAX),
            Some(FALLBACK_MAX)
        );
        assert_eq!(
            nudge_value(Some(LEASH_MAX), true, LEASH_NUDGE, 18.0, LEASH_MIN, LEASH_MAX),
            Some(LEASH_MAX)
        );
        // Captions must not round a number the unit is really using.
        assert_eq!(trim_num(37.5), "37.5");
        assert_eq!(trim_num(35.0), "35");
    }

    /// `PostureIntent::Escort` always took any own unit; the card only ever
    /// offered the hero. Now it arms a unit click like the other three arm a
    /// ground click, and a Catapult can be given a screen.
    #[test]
    fn escort_can_screen_any_own_unit_not_just_the_hero() {
        let mut app = ui_app();
        spawn_selected_footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));
        // The escortee: ours, and deliberately NOT a hero and NOT selected.
        let catapult = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Catapult },
                Team::Human,
                Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
                Health::new(220.0),
                Order::Idle,
            ))
            .id();

        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyR]);
        let arm = app
            .world()
            .resource::<UiState>()
            .posture_place
            .expect("Escort should now arm a click instead of firing at the hero");
        assert_eq!(arm.kind, PostureKind::Escort);
        assert!(arm.kind.needs_unit());
        // A ground click means nothing for this posture, and says nothing.
        assert!(posture_intent(arm, Vec3::new(5.0, 0.0, 5.0)).is_none());

        let click = posture_unit_intent(arm, catapult).unwrap();
        assert_eq!(
            click.sentence(),
            format!("squad 1 escorts {}", intent_id(catapult))
        );
        let typed: Intent = serde_json::from_str(&format!(
            r#"{{"type":"posture","id":1,"posture":{{"type":"escort","unit":{}}}}}"#,
            intent_id(catapult)
        ))
        .unwrap();
        assert_eq!(json(&click), json(&typed));
    }

    /// `AutoCastPolicy` has been per-slot since abilities v2 and the card had
    /// one switch, wired to slot 0. A Champion who has learned Warcry could not
    /// be told to auto-cast it from the human seat at all.
    #[test]
    fn autocast_is_per_ability_on_the_doctrine_card() {
        let mut app = ui_app();
        let hero = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Transform::from_translation(Vec3::new(-10.0, 0.0, -10.0)),
                Health::new(600.0),
                Order::Idle,
                Hero { level: 6, xp: 0.0, mana: 200.0 },
                Selected,
            ))
            .id();

        press(&mut app, &[KeyCode::KeyI]);
        // [X] is slot 1 — the Champion's SECOND ability.
        press(&mut app, &[KeyCode::KeyX]);

        let gesture = said(&app).last().expect("a rule should have been set");
        let typed: Intent = serde_json::from_str(&format!(
            r#"{{"type":"autocast","units":[{}],"min_enemies":{AUTOCAST_MIN_ENEMIES},"ability":1}}"#,
            intent_id(hero)
        ))
        .unwrap();
        assert_eq!(json(gesture), json(&typed));

        // And the card really does offer one per ability, named after it.
        let doc = DoctrineState::of(&[UnitDoctrine::read(
            None,
            None,
            None,
            None,
            UnitKind::Hero,
            None,
        )]);
        let entries = doctrine_entries(1, DoctrineCard { doc, ..default() });
        let slots: Vec<CmdAction> = entries
            .iter()
            .map(|e| e.action)
            .filter(|a| matches!(a, CmdAction::ToggleAutoCastSlot(_)))
            .collect();
        assert_eq!(
            slots.len(),
            abilities_of_unit(UnitKind::Hero).len(),
            "one toggle per ability, no more and no fewer"
        );
        for (i, def) in abilities_of_unit(UnitKind::Hero).iter().enumerate() {
            let entry = entries
                .iter()
                .find(|e| e.action == CmdAction::ToggleAutoCastSlot(i))
                .unwrap();
            assert!(entry.label.contains(def.name), "{} not named", def.name);
        }
        // A selection with nothing to cast gets none of them.
        let footmen = DoctrineState::of(&[UnitDoctrine::read(
            None,
            None,
            None,
            None,
            UnitKind::Footman,
            None,
        )]);
        assert!(!doctrine_entries(1, DoctrineCard { doc: footmen, ..default() })
            .iter()
            .any(|e| matches!(e.action, CmdAction::ToggleAutoCastSlot(_))));
    }

    /// The doctrine page was never covered by the hotkey-uniqueness tests, and
    /// it just gained five keys. A duplicate would mean one press firing two
    /// orders — silently, since `command_input` walks every matching entry.
    #[test]
    fn the_doctrine_page_keeps_its_hotkeys_unique() {
        let caster = DoctrineState::of(&[UnitDoctrine::read(
            None,
            None,
            None,
            None,
            UnitKind::Hero,
            None,
        )]);
        for (units, card, want_pages) in [
            (2usize, DoctrineCard { doc: caster, ..default() }, 2usize),
            (
                0usize,
                DoctrineCard {
                    tmpl: TemplateView { capable: true, ..default() },
                    ..default()
                },
                1usize,
            ),
        ] {
            let entries = doctrine_entries(units, card);
            // BUDGET NOTE, and the decision it demanded. A page holds
            // `CMD_SLOTS - 1` = 11 content tiles (the mode toggle is pinned).
            // A two-ability caster's doctrine card was EXACTLY 11 and this
            // assertion was the tripwire on the twelfth: not a ban, a demand
            // that spilling the card be chosen rather than discovered.
            //
            // Territory spent it, deliberately. `Mark region` and `Forget
            // marks` are tiles 12 and 13, so a caster's doctrine card is now
            // two [Tab] pages. What made it the right trade:
            //
            //   * The two new tiles are the ONLY authoring surface the human
            //     has for regions. Everything else on this card has a bridge
            //     equivalent a co-commander can send; a mark has to be clicked,
            //     because it is a point on the ground.
            //   * Paging is already load-bearing here and already tested —
            //     `a_hotkey_on_page_two_still_fires_from_page_one` proves [M]
            //     and [N] work from page one regardless of which page shows
            //     them, so the spill costs discoverability, not reach.
            //   * The overflow lands on the two tiles that are about GROUND
            //     rather than about the selection, which is the honest seam:
            //     page one stays "what these units are for", page two is "what
            //     the map is called".
            //
            // The tripwire stays armed at the new number. Tile 14 is somebody
            // else's deliberate decision.
            assert_eq!(
                paginate(&entries, 0).pages,
                want_pages,
                "the doctrine card's page budget changed at {} entries",
                entries.len()
            );
            let mut keys: Vec<KeyCode> = entries.iter().map(|e| e.key).collect();
            keys.sort_by_key(|k| format!("{k:?}"));
            let before = keys.len();
            keys.dedup();
            assert_eq!(before, keys.len(), "duplicate hotkey on the doctrine card");
            assert!(
                entries.last().map(|e| e.action) == Some(CmdAction::TogglePage),
                "the way back must survive a full card"
            );
        }
        // The four nudge keys are raw, not entries — they must not collide
        // with any doctrine-card hotkey either.
        let entries = doctrine_entries(2, DoctrineCard { doc: caster, ..default() });
        for nudge in [
            KeyCode::Minus,
            KeyCode::Equal,
            KeyCode::BracketLeft,
            KeyCode::BracketRight,
        ] {
            assert!(
                !entries.iter().any(|e| e.key == nudge),
                "{nudge:?} is both a nudge and a card button"
            );
        }
    }

    /// The badge exists for one situation: a drag box that scooped up units
    /// from two squads (or none). The doctrine card names ONE squad — the first
    /// unit's — so without a per-tile mark the player aims a posture at a group
    /// and only part of it moves.
    #[test]
    fn squad_badges_make_a_mixed_selection_visible() {
        let of = |squad: Option<u8>| UnitDoctrine {
            squad,
            ..UnitDoctrine::read(None, None, None, None, UnitKind::Footman, None)
        };
        let mixed = DoctrineState::of(&[of(Some(1)), of(Some(2)), of(None)]);
        assert_eq!(mixed.squad, Some(1), "the card speaks for the first unit");
        assert!(
            mixed.in_squad < mixed.units,
            "and two of the three tiles are not in it — which is the whole \
             hazard the badge is there to show"
        );

        // Badge text: the id, and nothing at all for a unit in no squad.
        assert_eq!(squad_badge(Some(1)), "1");
        assert_eq!(squad_badge(Some(3)), "3");
        assert_eq!(squad_badge(None), "");
    }

    /// A production building carries standing doctrine for everything it will
    /// ever train. This was bridge-only; it is now a button.
    #[test]
    fn a_building_can_be_stamped_with_a_doctrine_template() {
        let mut app = ui_app();
        let barracks = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Human,
                Transform::from_translation(Vec3::new(-60.0, 0.0, -60.0)),
                Health::new(700.0),
                TrainingQueue::default(),
                Selected,
            ))
            .id();

        press(&mut app, &[KeyCode::KeyI]);
        press(&mut app, &[KeyCode::KeyQ]); // squad piece: none -> 1
        assert_eq!(said(&app).len(), 1);
        let gesture = &said(&app)[0];
        assert_eq!(
            gesture.sentence(),
            "building 4294967296 stamps every unit it trains with squad 1"
                .replace("4294967296", &intent_id(barracks).to_string())
        );
        let typed: Intent = serde_json::from_str(&format!(
            r#"{{"type":"template","building":{},"squad":1}}"#,
            intent_id(barracks)
        ))
        .unwrap();
        assert_eq!(json(gesture), json(&typed));
    }

    /// The card is a projection of state, not a second opinion about it: the
    /// page the input system reads actions from is the page the HUD draws.
    #[test]
    fn the_doctrine_page_offers_a_way_back() {
        let card = DoctrineCard::default();
        let orders = command_entries(CardPage::Orders, Race::Kingdom, 2, false, None, HeroCmds::default(), card, &[]);
        assert!(
            orders.iter().any(|e| e.action == CmdAction::TogglePage),
            "a unit selection must be able to reach the doctrine page"
        );
        let doctrine =
            command_entries(CardPage::Doctrine, Race::Kingdom, 2, false, None, HeroCmds::default(), card, &[]);
        assert!(paginate(&doctrine, 0).tiles.len() <= CMD_SLOTS);
        assert_eq!(
            doctrine.last().map(|e| e.action),
            Some(CmdAction::TogglePage),
            "the doctrine page must always end with the way out"
        );
        assert!(doctrine
            .iter()
            .any(|e| e.action == CmdAction::SetPosture(PostureKind::Defend)));
    }

    /// The most crowded card in the game — worker + hero, three spells, two
    /// carried items — and doctrine is still one keystroke away.
    ///
    /// This test has outlived two mechanisms. At 3x3 the premise was that the
    /// [I] BUTTON got truncated away and only the raw [I] KEY saved the player;
    /// growing the card to 4x3 moved the overflow case up to worker+hero but
    /// kept the same shape. Paging retires the premise entirely: the mode toggle
    /// is PINNED to every page, so the button cannot yield any more, and the
    /// content it used to displace is on an overflow page instead of gone. Both
    /// routes are asserted here, because "the key always works" is the promise
    /// that survives whatever the layout does next.
    #[test]
    fn the_doctrine_page_is_reachable_however_full_the_card() {
        // The Champion's real first spell, so the fixture cannot drift from
        // whatever the ability tables actually say.
        let def = abilities_of_unit(UnitKind::Hero)[0];
        let slot = |index: usize| AbilitySlot {
            index,
            def,
            ready: true,
            cooldown: 0.0,
        };
        let crowded = HeroCmds {
            abilities: vec![slot(0), slot(1), slot(2)],
            items: [Some(ItemId::HealingPotion), Some(ItemId::TownPortal)],
            ..HeroCmds::default()
        };
        let entries = command_entries(
            CardPage::Orders,
            Race::Kingdom,
            3,
            true,
            None,
            crowded,
            DoctrineCard::default(),
            &[],
        );
        assert!(
            entries.len() > CMD_SLOTS,
            "a worker+hero card is the overflow case — that is the premise"
        );
        let view = paginate(&entries, 0);
        assert!(view.pages > 1, "and it must therefore page");
        assert_eq!(view.tiles.len(), CMD_SLOTS, "page one is full");
        assert_eq!(
            view.tiles.last().map(|e| e.action),
            Some(CmdAction::TogglePage),
            "the mode toggle is pinned to the last slot of page one"
        );
        // ...and of every other page: the way to doctrine is never a page away.
        for page in 0..view.pages {
            assert_eq!(
                paginate(&entries, page).tiles.last().map(|e| e.action),
                Some(CmdAction::TogglePage),
                "page {page} lost the mode toggle",
            );
        }

        let mut app = ui_app();
        app.world_mut().spawn((
            Unit { kind: UnitKind::Worker },
            Team::Human,
            Transform::from_translation(Vec3::new(-60.0, 0.0, -60.0)),
            Health::new(100.0),
            Order::Idle,
            Selected,
        ));
        press(&mut app, &[KeyCode::KeyI]);
        assert_eq!(app.world().resource::<UiState>().page, CardPage::Doctrine);
    }

    /// A plain worker selection keeps every build button — including the eighth
    /// (the Blacksmith) and the ninth (the Sanctum) — **on page one**, and still
    /// has the page toggle. At 3x3 the eighth build card was silently eaten by a
    /// `truncate`; the fix then was a fourth column, which bought exactly one
    /// building's worth of room and was spent immediately.
    ///
    /// Paging is the version of that fix which does not run out: the assertion
    /// below is about the visible page, and the systematic test further down
    /// proves the same for a tenth building, and an eleventh.
    #[test]
    fn a_worker_card_holds_every_build_button_and_the_page_toggle() {
        let entries = command_entries(
            CardPage::Orders,
            Race::Kingdom,
            3,
            true,
            None,
            HeroCmds::default(),
            DoctrineCard::default(),
            &[],
        );
        let view = paginate(&entries, 0);
        assert_eq!(view.tiles.len(), CMD_SLOTS, "page one fills the card");
        for (kind, _) in build_cards(Race::Kingdom) {
            assert!(
                view.tiles.iter().any(|e| e.action == CmdAction::Place(kind)),
                "{kind:?} must have a button on page one — a building the player \
                 cannot see on the card has no other route in"
            );
        }
        assert!(
            view.tiles
                .iter()
                .any(|e| e.action == CmdAction::Place(BuildingKind::Blacksmith)),
            "the eighth build card is the one 3x3 used to drop"
        );
        assert!(
            view.tiles.iter().any(|e| e.action == CmdAction::TogglePage),
            "and there is still room for the way to page two"
        );
        // The quick toggles are what pages now — they used to be DELETED by a
        // hand-written order of sacrifice, and this is the difference paging
        // makes: [G Guard] is one [Tab] away instead of unreachable.
        assert!(
            entries.iter().any(|e| e.action == CmdAction::ToggleGuard),
            "the quick toggles survive on an overflow page"
        );
        assert!(view.pages > 1, "...which means the card pages");
    }

    /// The same invariant, carried onto the cards the test above never
    /// **The HUD's answer to "is my order lost?"** (docs/TEMPO.md follow-up 7).
    ///
    /// Without a readout the mechanic is indistinguishable from input lag —
    /// which is the stated reason `WC3_COMMAND_LATENCY` still defaults off — so
    /// what these two lines say is part of the feature, not decoration on it.
    ///
    /// Worst-first is the load-bearing choice: a player deciding whether to
    /// reach for a strung-out selection is asking about its slowest unit, not
    /// its typical one, and a line that led with "0.0s" because most of the
    /// group is at home would answer the wrong question.
    #[test]
    fn the_link_readout_leads_with_the_slowest_unit_in_the_selection() {
        // One line when the selection is coherent.
        assert_eq!(link_line(vec![1.2, 1.2, 1.2], vec![]), "Link: 1.2s x3");
        // Worst first when it is not, whatever order the query yielded.
        assert_eq!(
            link_line(vec![0.0, 2.4, 0.0, 0.0], vec![]),
            "Link: 2.4s   0.0s x3"
        );
        // More than two distinct costs: the two that matter, then a count.
        let spread = link_line(vec![0.0, 1.0, 2.0, 3.0], vec![]);
        assert!(
            spread.starts_with("Link: 3.0s   2.0s") && spread.ends_with("(+2 more)"),
            "a strung-out selection should still lead with its worst: {spread}"
        );
        // Orders already travelling are counted, and reported by the link they
        // are paying — the time REMAINING is the closing ring's job.
        assert_eq!(
            link_line(vec![1.8, 1.8], vec![1.8, 0.9]),
            "Link: 1.8s x2   ·   2 in transit (1.8s)"
        );
        // Nothing selected, nothing to say.
        assert_eq!(link_line(vec![], vec![]), "");
    }

    /// **Flag off, HUD unchanged.** The promise the whole feature ships on: a
    /// match played without `WC3_COMMAND_LATENCY` must look exactly like v1.
    /// Both readouts collapse to the empty string, and an empty `Text` in a
    /// left-packed bar and a column of panel lines occupies nothing.
    ///
    /// The two world-space markers get the same guarantee structurally rather
    /// than by a check — `update_link_rings` asks `latency.on` before it
    /// collects anything, and `update_transit_markers` queries `PendingOrder`,
    /// which cannot exist with the feature off — so this test covers the half
    /// that is a decision rather than a consequence.
    #[test]
    fn the_hud_says_nothing_at_all_when_the_chain_of_command_is_off() {
        assert_eq!(coverage_line(false, 3, 8, 12), "");
        // ...and says something useful when it is on.
        assert_eq!(coverage_line(true, 3, 8, 12), "Chain: 3 nodes · 8/12 in reach");
        assert_eq!(coverage_line(true, 1, 0, 4), "Chain: 1 node · 0/4 in reach");
    }

    /// An in-transit order has to point somewhere for the countdown ring to be
    /// drawn there. Every verb that can be delayed names a place — either
    /// directly or through the entity it targets — and the two that do not are
    /// two that can never be delayed: `stop` compiles to a Move onto the unit's
    /// own feet rather than to `Idle`, precisely so that it has a destination.
    #[test]
    fn every_delayable_order_names_a_place_to_draw_its_marker() {
        let target = Vec3::new(12.0, 0.0, -4.0);
        let somewhere = Vec3::new(30.0, 0.0, 30.0);
        let at = |_| Some(target);

        assert_eq!(order_destination(&Order::Move(somewhere), at), Some(somewhere));
        assert_eq!(
            order_destination(&Order::AttackMove(somewhere), at),
            Some(somewhere)
        );
        // Entity-targeted orders resolve through the target's transform, so the
        // ring sits on the thing being attacked rather than on stale ground.
        let victim = Entity::from_raw(7);
        assert_eq!(order_destination(&Order::Attack(victim), at), Some(target));
        assert_eq!(order_destination(&Order::Harvest(victim), at), Some(target));
        assert_eq!(order_destination(&Order::Follow(victim), at), Some(target));
        // ...and gracefully give up when the target died while the order flew.
        assert_eq!(order_destination(&Order::Attack(victim), |_| None), None);

        assert_eq!(order_destination(&Order::Idle, at), None);
        assert_eq!(order_destination(&Order::ReturnResources, at), None);
    }

    /// reaches: a PRODUCTION building's. A worker card is builds and toggles;
    /// a building card is train slots plus that building's abilities, its
    /// tier-up and the page toggle, and those letters are chosen from a
    /// different pool. The Barracks is the pressing case — it now offers five
    /// units, so `Hk::TrainSlot` had to grow a fifth rung, and [T] is only safe
    /// because the other [T] in the game (Auto-Slam) lives on a unit
    /// selection and [T Stand Down] on the doctrine page, both disjoint from
    /// this card.
    #[test]
    fn every_production_card_keeps_its_hotkeys_unique() {
        // Tier 3 in hand, so nothing is hidden behind a tech gate and every
        // train slot a building can ever show is on the card at once.
        let completed = [
            BuildingKind::TownHall,
            BuildingKind::Castle,
            BuildingKind::Barracks,
            BuildingKind::Workshop,
            BuildingKind::Blacksmith,
            BuildingKind::Shop,
        ];
        for kind in ALL_BUILDING_KINDS {
            let entries = command_entries(
                CardPage::Orders,
                Race::Kingdom,
                0,
                false,
                Some((kind, true)),
                HeroCmds::default(),
                DoctrineCard::default(),
                &completed,
            );
            assert_eq!(
                paginate(&entries, 0).pages,
                1,
                "{kind:?}'s card should still fit on one page: {} entries",
                entries.len(),
            );
            let mut keys: Vec<KeyCode> = entries.iter().map(|e| e.key).collect();
            keys.sort_by_key(|k| format!("{k:?}"));
            let before = keys.len();
            keys.dedup();
            assert_eq!(
                before,
                keys.len(),
                "two buttons share a hotkey on the {kind:?} card",
            );
        }

        // ...and the two tier-3 slots really are on those cards, which is what
        // makes the check above worth running: a unit with no button has no
        // route in for a player at the keyboard.
        let barracks = command_entries(
            CardPage::Orders,
            Race::Kingdom,
            0,
            false,
            Some((BuildingKind::Barracks, true)),
            HeroCmds::default(),
            DoctrineCard::default(),
            &completed,
        );
        let knight = barracks
            .iter()
            .find(|e| e.action == CmdAction::Train(UnitKind::Knight))
            .expect("the Barracks must offer the Knight at T3");
        assert_eq!(knight.key, KeyCode::KeyT, "the Knight sits on [T]");
        assert!(knight.enabled, "and a Castle standing must enable it");

        let workshop = command_entries(
            CardPage::Orders,
            Race::Kingdom,
            0,
            false,
            Some((BuildingKind::Workshop, true)),
            HeroCmds::default(),
            DoctrineCard::default(),
            &completed,
        );
        let gryphon = workshop
            .iter()
            .find(|e| e.action == CmdAction::Train(UnitKind::GryphonRider))
            .expect("the Workshop must offer the Gryphon Rider at T3");
        assert_eq!(gryphon.key, KeyCode::KeyW, "the Gryphon sits on [W]");
        assert!(gryphon.enabled);
    }

    // -----------------------------------------------------------------------
    // The systematic hotkey invariant
    // -----------------------------------------------------------------------

    /// Every card the game can draw, with everything on it at once.
    ///
    /// The two hand-written invariant tests above cover the two cards their
    /// authors were worried about at the time — a worker's and a production
    /// building's, each with a default `HeroCmds`. This walks the whole cross
    /// product of selection type and mode, with a MAXIMAL `HeroCmds` (three
    /// spells, a full inventory, a stocked shelf, a tier-up, both research
    /// ladders) so that buttons which only appear in rare combinations are on
    /// the card when it is checked. The Shop's shelf in particular was never
    /// exercised by any test before this one — and it is the card whose fifth
    /// rung had quietly landed on the mode toggle's letter.
    ///
    /// `hotkeys::validate` proves the same thing one level up, over the table
    /// rather than over rendered cards. Both are worth having: the registry
    /// check catches a bad binding before any card is built, this one catches a
    /// card that draws buttons the registry did not expect it to.
    fn every_card_fixture() -> Vec<(String, Vec<CmdEntry>)> {
        let completed = [
            BuildingKind::TownHall,
            BuildingKind::Castle,
            BuildingKind::Barracks,
            BuildingKind::Workshop,
            BuildingKind::Blacksmith,
            BuildingKind::Shop,
            BuildingKind::Sanctum,
        ];
        let def = abilities_of_unit(UnitKind::Hero)[0];
        let slot = |index: usize| AbilitySlot {
            index,
            def,
            ready: true,
            cooldown: 0.0,
        };
        let caster = DoctrineState::of(&[UnitDoctrine::read(
            None,
            None,
            None,
            None,
            UnitKind::Hero,
            None,
        )]);
        let mut out: Vec<(String, Vec<CmdEntry>)> = Vec::new();

        // --- unit selections, orders page ---------------------------------
        for worker in [false, true] {
            for hero in [false, true] {
                let cmds = if hero {
                    HeroCmds {
                        abilities: vec![slot(0), slot(1), slot(2)],
                        items: [Some(ItemId::HealingPotion), Some(ItemId::TownPortal)],
                        ..HeroCmds::default()
                    }
                } else {
                    HeroCmds::default()
                };
                let doc = if hero {
                    DoctrineCard { doc: caster, ..default() }
                } else {
                    DoctrineCard::default()
                };
                out.push((
                    format!("units (worker: {worker}, hero: {hero}), orders"),
                    command_entries(CardPage::Orders, Race::Kingdom, 3, worker, None, cmds, doc, &completed),
                ));
            }
        }

        // --- unit selection, doctrine page --------------------------------
        out.push((
            "units, doctrine".to_string(),
            command_entries(
                CardPage::Doctrine,
                Race::Kingdom,
                3,
                false,
                None,
                HeroCmds::default(),
                DoctrineCard { doc: caster, ..default() },
                &completed,
            ),
        ));

        // --- one building of every kind, orders page ----------------------
        for kind in ALL_BUILDING_KINDS {
            let cmds = HeroCmds {
                train: None,
                abilities: Vec::new(),
                building_abilities: abilities_of_building(kind)
                    .iter()
                    .enumerate()
                    .map(|(i, _)| slot(i))
                    .collect(),
                // Every kind is offered the shelf, not just the Shop: if a
                // second building ever sells, its card is already covered.
                shop: Some(ShopState {
                    hero: true,
                    room: true,
                    tier: TechTier::T3,
                })
                .filter(|_| hotkeys::sells_items(kind)),
                upgrade: building_upgrades_to(kind).map(|to| (to, 100, 0)),
                items: [None, None],
                research: building_researches(kind)
                    .iter()
                    .map(|k| ResearchCmd {
                        kind: *k,
                        level: 0,
                        next: None,
                        in_progress: None,
                        blocked: false,
                    })
                    .collect(),
            };
            out.push((
                format!("{kind:?}, orders"),
                command_entries(
                    CardPage::Orders,
                    Race::Kingdom,
                    0,
                    false,
                    Some((kind, true)),
                    cmds,
                    DoctrineCard::default(),
                    &completed,
                ),
            ));
        }

        // --- one production building, doctrine page (the template card) ---
        out.push((
            "production building, doctrine".to_string(),
            command_entries(
                CardPage::Doctrine,
                Race::Kingdom,
                0,
                false,
                Some((BuildingKind::Barracks, true)),
                HeroCmds::default(),
                DoctrineCard {
                    tmpl: TemplateView { capable: true, ..default() },
                    ..default()
                },
                &completed,
            ),
        ));
        out
    }

    #[test]
    fn no_card_the_game_can_draw_has_two_buttons_on_one_key() {
        for (name, entries) in every_card_fixture() {
            for (i, entry) in entries.iter().enumerate() {
                if let Some(clash) = entries[..i].iter().find(|p| p.key == entry.key) {
                    panic!(
                        "{name}: [{}] is both {:?} and {:?}",
                        hotkeys::key_caption(entry.key),
                        clash.action,
                        entry.action,
                    );
                }
            }
        }
    }

    /// The raw keys — the four nudges and the [Tab] pager — have no tile, so
    /// they are invisible to the check above and would silently double-fire
    /// alongside a button that shared their key.
    #[test]
    fn no_raw_key_collides_with_a_button_on_any_card() {
        let raw = [
            hotkeys::NEXT_CARD_PAGE,
            bind(Hk::NudgeFallbackDown),
            bind(Hk::NudgeFallbackUp),
            bind(Hk::NudgeLeashDown),
            bind(Hk::NudgeLeashUp),
        ];
        for (name, entries) in every_card_fixture() {
            for key in raw {
                assert!(
                    !entries.iter().any(|e| e.key == key),
                    "{name}: [{}] is both a raw key and a card button",
                    hotkeys::key_caption(key),
                );
            }
        }
    }

    /// Nothing is ever silently dropped again. Every card, every mode: the
    /// entries the input system dispatches against are exactly the entries the
    /// player can reach by tiles, once the pages are walked.
    #[test]
    fn every_entry_is_reachable_on_some_page() {
        for (name, entries) in every_card_fixture() {
            let pages = paginate(&entries, 0).pages;
            let mut seen: Vec<CmdAction> = Vec::new();
            for page in 0..pages {
                let view = paginate(&entries, page);
                assert!(
                    view.tiles.len() <= CMD_SLOTS,
                    "{name}: page {page} draws {} tiles into {CMD_SLOTS} slots",
                    view.tiles.len(),
                );
                seen.extend(view.tiles.iter().map(|e| e.action));
            }
            for entry in &entries {
                assert!(
                    seen.contains(&entry.action),
                    "{name}: {:?} is on no page — the failure mode the old \
                     `truncate` had",
                    entry.action,
                );
            }
        }
    }

    /// A key means one thing across every overflow page of a card. That is the
    /// paging semantics this bead chose (modes may repeat keys, overflow pages
    /// may not), and it is what lets `command_input` dispatch hotkeys against
    /// the whole list rather than the visible slice.
    #[test]
    fn a_key_means_the_same_thing_on_every_overflow_page_of_a_card() {
        for (name, entries) in every_card_fixture() {
            let pages = paginate(&entries, 0).pages;
            let mut bound: Vec<(KeyCode, CmdAction)> = Vec::new();
            for page in 0..pages {
                for tile in paginate(&entries, page).tiles {
                    if let Some((_, other)) = bound.iter().find(|(k, _)| *k == tile.key) {
                        assert_eq!(
                            *other, tile.action,
                            "{name}: [{}] means two things across pages",
                            hotkeys::key_caption(tile.key),
                        );
                    } else {
                        bound.push((tile.key, tile.action));
                    }
                }
            }
        }
    }

    /// The pager itself: pages are full, in order, lossless, and the mode
    /// toggle is pinned to the last slot of each one. Walking off the end
    /// clamps rather than blanking the card — a selection can shrink under a
    /// player who is looking at page two.
    #[test]
    fn the_pager_slices_a_card_without_losing_or_repeating_a_tile() {
        let entry = |n: u8| CmdEntry::plain(CmdAction::Train(UnitKind::Footman), KeyCode::KeyQ, &n.to_string());
        // No pinned toggle: the full twelve slots are content.
        let flat: Vec<CmdEntry> = (0..12).map(entry).collect();
        assert_eq!(paginate(&flat, 0).pages, 1);
        assert_eq!(paginate(&flat, 0).tiles.len(), 12);

        let mut pinned: Vec<CmdEntry> = (0..11).map(entry).collect();
        pinned.push(CmdEntry::plain(CmdAction::TogglePage, KeyCode::KeyI, "Doctrine"));
        let view = paginate(&pinned, 0);
        assert_eq!(view.pages, 1, "eleven content + the toggle is exactly a card");
        assert_eq!(view.tiles.len(), CMD_SLOTS);

        // One more content entry, and it pages rather than falling off.
        let mut over: Vec<CmdEntry> = (0..12).map(entry).collect();
        over.push(CmdEntry::plain(CmdAction::TogglePage, KeyCode::KeyI, "Doctrine"));
        let first = paginate(&over, 0);
        assert_eq!(first.pages, 2);
        assert_eq!(first.tiles.len(), CMD_SLOTS);
        assert_eq!(first.tiles[0].label, "0");
        assert_eq!(first.tiles[10].label, "10");
        let second = paginate(&over, 1);
        assert_eq!(second.tiles.len(), 2, "one leftover plus the pinned toggle");
        assert_eq!(second.tiles[0].label, "11");
        assert_eq!(second.tiles[1].action, CmdAction::TogglePage);
        // Off the end clamps back onto the last real page.
        assert_eq!(paginate(&over, 9).page, 1);
        assert_eq!(paginate(&over, 9).tiles[0].label, "11");
        // An empty card is one page, not zero.
        assert_eq!(paginate(&[], 0).pages, 1);
    }

    /// The indicator says nothing while everything fits, and names both the
    /// mode and the key when it does not. A page number with no way to turn the
    /// page is a worse HUD than no page number at all.
    #[test]
    fn the_page_indicator_appears_only_when_there_is_a_page_to_turn() {
        assert_eq!(card_page_label(CardPage::Orders, 0, 1), "");
        assert_eq!(
            card_page_label(CardPage::Orders, 0, 2),
            "Orders 1/2   [Tab] more"
        );
        assert_eq!(
            card_page_label(CardPage::Doctrine, 1, 2),
            "Doctrine 2/2   [Tab] more"
        );
    }

    /// [Tab] walks the overflow pages of the card the player is looking at, and
    /// wraps. Driven through the real system, so this covers the input path and
    /// not just the pager.
    #[test]
    fn tab_walks_the_overflow_pages_and_wraps() {
        let mut app = ui_app();
        // A worker: nine build buttons plus [A][S] fill page one exactly, and
        // the quick toggles spill onto page two.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Worker },
            Team::Human,
            Transform::from_translation(Vec3::new(-60.0, 0.0, -60.0)),
            Health::new(100.0),
            Order::Idle,
            Selected,
        ));
        app.update();
        assert_eq!(app.world().resource::<UiState>().card_page, 0);
        press(&mut app, &[hotkeys::NEXT_CARD_PAGE]);
        assert_eq!(
            app.world().resource::<UiState>().card_page,
            1,
            "[Tab] turns the page"
        );
        press(&mut app, &[hotkeys::NEXT_CARD_PAGE]);
        assert_eq!(
            app.world().resource::<UiState>().card_page,
            0,
            "...and wraps back to the first"
        );
    }

    /// A hotkey works from any overflow page of its own card. This is the
    /// player-facing half of the paging decision: only the tiles move.
    #[test]
    fn a_hotkey_on_page_two_still_fires_from_page_one() {
        let entries = command_entries(
            CardPage::Orders,
            Race::Kingdom,
            3,
            true,
            None,
            HeroCmds::default(),
            DoctrineCard::default(),
            &[],
        );
        let guard = entries
            .iter()
            .find(|e| e.action == CmdAction::ToggleGuard)
            .expect("a unit selection always offers [G Guard]");
        assert!(
            !paginate(&entries, 0)
                .tiles
                .iter()
                .any(|e| e.action == CmdAction::ToggleGuard),
            "the premise: [G] has been pushed onto an overflow page"
        );

        let mut app = ui_app();
        app.world_mut().spawn((
            Unit { kind: UnitKind::Worker },
            Team::Human,
            Transform::from_translation(Vec3::new(-60.0, 0.0, -60.0)),
            Health::new(100.0),
            Order::Idle,
            Selected,
        ));
        app.update();
        assert_eq!(
            app.world().resource::<UiState>().card_page,
            0,
            "still looking at page one"
        );
        press(&mut app, &[guard.key]);
        assert!(
            said(&app)
                .iter()
                .any(|i| matches!(i, Intent::Leash { .. })),
            "[G] must fire from a page its tile is not on; said: {:?}",
            said(&app),
        );
    }

    // -----------------------------------------------------------------------
    // Alert pings and cues
    // -----------------------------------------------------------------------

    /// A ring is born small and opaque and dies large and invisible. Both ends
    /// matter: a ring that started big is a flash rather than a ping, and one
    /// that did not fade would leave the minimap permanently decorated.
    #[test]
    fn a_ping_expands_and_fades_over_its_life() {
        let (born_px, born_a) = ping_shape(0.0);
        let (mid_px, mid_a) = ping_shape(PING_LIFETIME * 0.5);
        let (dead_px, dead_a) = ping_shape(PING_LIFETIME);

        assert_eq!(born_px, PING_MIN_PX);
        assert_eq!(born_a, 1.0);
        assert_eq!(dead_px, PING_MAX_PX);
        assert_eq!(dead_a, 0.0);
        assert!(born_px < mid_px && mid_px < dead_px, "the ring must expand");
        assert!(born_a > mid_a && mid_a > dead_a, "the ring must fade");
        // Eased out, so most of the motion is in the first half — that is the
        // half an eye at the edge of vision actually catches.
        assert!(
            mid_px > (PING_MIN_PX + PING_MAX_PX) * 0.5,
            "the expansion is not front-loaded: {mid_px}"
        );
        // Past the end it clamps rather than running away.
        assert_eq!(ping_shape(PING_LIFETIME * 10.0), (PING_MAX_PX, 0.0));
    }

    /// Several events in one frame make ONE sound, and it is the worst of them.
    /// A frame that drains a backlog must not fire a chord.
    #[test]
    fn the_loudest_severity_wins_a_shared_frame() {
        assert!(severity_rank(EventSeverity::Critical) > severity_rank(EventSeverity::Warning));
        assert!(severity_rank(EventSeverity::Warning) > severity_rank(EventSeverity::Info));
    }

    /// Only events that know WHERE they happened get a ring — the ping's whole
    /// content is the position, so a placeless alert has nothing to draw.
    #[test]
    fn only_a_located_alert_earns_a_ping() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<UiState>()
            .init_resource::<Time<Real>>()
            .init_resource::<GameEvents>()
            .init_resource::<Notifications>()
            .init_resource::<AlertPings>()
            .add_systems(Update, update_notifications);

        let at = Vec3::new(20.0, 0.0, -40.0);
        {
            let mut feed = app.world_mut().resource_mut::<GameEvents>();
            feed.push(
                Team::Human,
                1.0,
                "somewhere".into(),
                EventSeverity::Critical,
                Some(at),
            );
            feed.push(Team::Human, 1.0, "nowhere".into(), EventSeverity::Info, None);
        }
        app.update();

        let pings = app.world().resource::<AlertPings>();
        assert_eq!(pings.live.len(), 1, "one located alert, one ring");
        assert_eq!(pings.live[0].pos, at);
        assert_eq!(pings.live[0].severity, EventSeverity::Critical);

        // Both alerts still reached the stack: the ping is a second rendering
        // of the feed, not a filter on it.
        assert_eq!(app.world().resource::<Notifications>().live.len(), 2);
    }

    /// The cues are synthesized, so the "asset" that has to be right is a byte
    /// buffer this file writes. A malformed header would be a decoder panic at
    /// the first alert of a match — the worst possible place to find out.
    #[test]
    fn a_synthesized_cue_is_a_wav_with_sound_in_it() {
        for severity in [
            EventSeverity::Info,
            EventSeverity::Warning,
            EventSeverity::Critical,
        ] {
            let wav = synth_wav(&cue_tones(severity));
            assert_eq!(&wav[0..4], b"RIFF", "{severity:?}: not a RIFF file");
            assert_eq!(&wav[8..12], b"WAVE", "{severity:?}: not a WAVE file");
            assert_eq!(&wav[36..40], b"data", "{severity:?}: no data chunk");

            // The header's two lengths must describe the buffer that follows,
            // or a decoder reads off the end or stops early.
            let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
            assert_eq!(data_len, wav.len() - 44, "{severity:?}: data length lies");
            let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
            assert_eq!(riff_len, wav.len() - 8, "{severity:?}: RIFF length lies");

            // A cue you cannot hear is the same bug as no cue at all.
            let peak = wav[44..]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs())
                .max()
                .unwrap_or(0);
            assert!(peak > 2000, "{severity:?}: peak amplitude {peak} is silence");

            // Short. A cue that outlasts the moment it reports is noise.
            let secs = (data_len / 2) as f32 / CUE_RATE as f32;
            assert!(
                (0.05..0.5).contains(&secs),
                "{severity:?}: {secs}s is not a cue"
            );
            // No click: the first sample must start from rest.
            let first = i16::from_le_bytes([wav[44], wav[45]]).unsigned_abs();
            assert!(first < 200, "{severity:?}: opens on a step of {first}");
        }
    }

    // -----------------------------------------------------------------------
    // Responsive console
    // -----------------------------------------------------------------------

    /// Every window size this HUD might plausibly be handed, including the
    /// tiling-WM sizes it was never tried at.
    const SIZES: [(f32, f32); 8] = [
        (2560.0, 1440.0),
        (1920.0, 1080.0),
        (1280.0, 800.0),
        (1024.0, 768.0),
        (900.0, 700.0),
        (800.0, 600.0),
        (700.0, 520.0),
        (640.0, 480.0),
    ];

    /// **The bug this whole responsive pass exists for.** 184 + 8 + 8 + 1 + 1
    /// is 202, and the console was 200, so the minimap's bottom border and the
    /// map's southern edge were clipped away by `overflow: clip()` at every
    /// size the game has ever run at. It looked like a design choice.
    #[test]
    fn the_minimap_always_fits_inside_the_console_it_lives_in() {
        for (w, h) in SIZES {
            let hud = hud_layout(w, h);
            assert!(
                hud.minimap_px + MINIMAP_CHROME <= hud.console_h + 1e-3,
                "{w}x{h}: a {}px minimap plus {MINIMAP_CHROME}px of chrome does \
                 not fit a {}px console",
                hud.minimap_px,
                hud.console_h
            );
        }
    }

    /// The command card is a fixed grid of fixed tiles. A console shorter than
    /// the grid does not look cramped, it hides buttons — so the card's height
    /// is a floor the responsive rule may never go under, however small the
    /// window gets.
    #[test]
    fn the_command_card_grid_is_never_clipped_by_a_short_window() {
        let card_h = 3.0 * CMD_PX + 2.0 * CMD_GAP + CMD_PAGE_LINE_H + 2.0 * PAD;
        for (w, h) in SIZES {
            let hud = hud_layout(w, h);
            assert!(
                hud.console_h >= card_h - 1e-3,
                "{w}x{h}: console {} is shorter than the {card_h} the build \
                 menu needs — tiles would be cut off",
                hud.console_h
            );
        }
        // …and it really does bind on a short window, or the test above is
        // only asserting that 202 > 198.
        assert!(
            hud_layout(800.0, 400.0).console_h >= card_h - 1e-3,
            "the floor must survive a window far shorter than the console"
        );
    }

    /// The console must leave a battlefield. A third of the window is already
    /// generous; more than that and a narrow tile is all HUD.
    #[test]
    fn the_console_never_eats_more_than_it_has_to() {
        for (w, h) in SIZES {
            let hud = hud_layout(w, h);
            assert!(
                hud.console_h <= MINIMAP_PX + MINIMAP_CHROME + 1e-3,
                "{w}x{h}: console {} grew past full size",
                hud.console_h
            );
            assert!(
                hud.minimap_px >= MINIMAP_MIN_PX - 1e-3
                    && hud.minimap_px <= MINIMAP_PX + 1e-3,
                "{w}x{h}: minimap {} left its legibility range",
                hud.minimap_px
            );
        }
        // At the sizes this HUD was designed for, nothing is scaled at all.
        assert_eq!(hud_layout(1920.0, 1080.0), hud_layout(1280.0, 800.0));
    }

    /// The minimap's own coordinate transform has to follow the size, both
    /// ways, or a click on the map lands somewhere else on the map.
    #[test]
    fn minimap_coordinates_round_trip_at_every_size() {
        for (w, h) in SIZES {
            let px = hud_layout(w, h).minimap_px;
            for p in [
                Vec3::new(-MAP_HALF, 0.0, -MAP_HALF),
                Vec3::new(MAP_HALF, 0.0, MAP_HALF),
                Vec3::new(37.0, 0.0, -61.0),
                Vec3::ZERO,
            ] {
                let back = minimap_to_world(world_to_minimap(p, px), px);
                assert!(
                    (back.x - p.x).abs() < 0.01 && (back.z - p.z).abs() < 0.01,
                    "{w}x{h} ({px}px): {p:?} came back as {back:?}"
                );
            }
        }
    }

    /// The two floating panels sit in opposite top corners and are hit-tested
    /// by two independent rects. If their widths can ever sum past the window
    /// they overlap — printed on top of each other, and both claiming the same
    /// clicks.
    #[test]
    fn the_alert_stack_and_the_proposal_panel_cannot_collide() {
        assert!(
            NOTIF_MAX_FRAC + PROP_MAX_FRAC <= 1.0,
            "the two top panels may together claim {}% of the window",
            (NOTIF_MAX_FRAC + PROP_MAX_FRAC) * 100.0
        );
        for (w, _) in SIZES {
            let notif = NOTIF_W.min(w * NOTIF_MAX_FRAC);
            let prop = PROP_W.min(w * PROP_MAX_FRAC);
            assert!(
                notif + prop + 2.0 * PAD <= w,
                "{w} wide: {notif} of alerts and {prop} of proposals do not fit"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Fog overlay
    // -----------------------------------------------------------------------

    /// The three states have to be three *visibly different* amounts of black,
    /// in the right order. A ramp that collapsed any two of them together
    /// would leave the ground unreadable even with every other part of the
    /// pipeline working.
    #[test]
    fn the_three_fog_states_are_three_clearly_separated_shades() {
        let vis = fog_alpha(CellVis::Visible);
        let expl = fog_alpha(CellVis::Explored);
        let unexp = fog_alpha(CellVis::Unexplored);

        assert_eq!(vis, 0.0, "what a team can see now is not dimmed at all");
        assert!(
            vis < expl && expl < unexp,
            "darkness must increase as knowledge decreases: {vis} / {expl} / {unexp}"
        );
        // A quarter of the range between neighbours is the floor for "told
        // apart at a glance" — the states are 0.0 / 0.44 / 0.88 today.
        assert!(
            expl - vis > 0.25 && unexp - expl > 0.25,
            "neighbouring states too close to distinguish: {vis} / {expl} / {unexp}"
        );
    }

    /// **The regression this file exists to prevent.**
    ///
    /// Repainting the fog image is enough for the minimap and *not* enough for
    /// the ground: the UI pipeline resolves an `ImageNode`'s handle to its
    /// current `GpuImage` every frame, but a mesh material's bind group is
    /// built once and rebuilt only when the *material* asset changes. Leave
    /// the material alone and the quad samples the texture as it stood in the
    /// opening frames forever — a lit disc around the start base, nothing ever
    /// explored — while the minimap tracks the match perfectly. The two
    /// renderings of one grid silently disagree, which is the one thing
    /// docs/FOG.md promises cannot happen.
    ///
    /// So: painting the overlay must republish the material every time.
    #[test]
    fn repainting_the_fog_overlay_republishes_the_material_it_is_worn_by() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .insert_resource(FogGrids::test_dark())
            .add_systems(Startup, setup_fog)
            .add_systems(Update, update_fog_overlay);
        app.update();

        let fog_mat = app.world().resource::<FogAssets>().fog_mat.id();

        // Drain whatever setup emitted, then run one more frame: that frame is
        // the claim under test.
        app.world_mut()
            .resource_mut::<Events<AssetEvent<StandardMaterial>>>()
            .clear();
        app.update();

        let events = app.world().resource::<Events<AssetEvent<StandardMaterial>>>();
        let mut reader = events.get_cursor();
        let republished = reader
            .read(events)
            .any(|e| matches!(e, AssetEvent::Modified { id } if *id == fog_mat));
        assert!(
            republished,
            "the fog quad's material must be republished when the fog texture is \
             repainted, or the ground goes on wearing the opening frame's fog"
        );
    }

    /// The painted texture is the grid, texel for texel — including the
    /// remembered middle state, which is the one that vanished when the
    /// material went stale.
    #[test]
    fn the_fog_texture_carries_all_three_states_from_the_grid() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .insert_resource(FogGrids::test_dark())
            .add_systems(Startup, setup_fog)
            .add_systems(Update, update_fog_overlay);
        app.update();

        // Plant one cell of each state, exactly as a recompute would leave them.
        app.world_mut()
            .resource_mut::<FogGrids>()
            .test_set_cell(Team::Human, 10, 10, CellVis::Visible);
        app.world_mut()
            .resource_mut::<FogGrids>()
            .test_set_cell(Team::Human, 20, 20, CellVis::Explored);
        app.update();

        let image_handle = app.world().resource::<FogAssets>().image.clone();
        let images = app.world().resource::<Assets<Image>>();
        let data = images.get(&image_handle).unwrap().data.as_ref().unwrap();
        let alpha_at = |cx: usize, cz: usize| data[NavGrid::idx(cx, cz) * 4 + 3];

        assert_eq!(alpha_at(10, 10), 0, "a visible cell is not dimmed");
        assert_eq!(
            alpha_at(20, 20),
            (fog_alpha(CellVis::Explored) * 255.0) as u8,
            "a remembered cell wears the middle shade"
        );
        assert_eq!(
            alpha_at(50, 50),
            (fog_alpha(CellVis::Unexplored) * 255.0) as u8,
            "unvisited ground stays dark"
        );
    }

    /// The scenery tint is the *same* darkening as the ground overlay, reached
    /// from the other side. If these two ever stop summing to 1, a tree and the
    /// earth under it are being told two different stories about how well the
    /// player knows that spot — the exact class of bug docs/FOG.md exists to
    /// forbid, just moved from the grid to the shading.
    #[test]
    fn the_scenery_tint_and_the_ground_overlay_are_one_darkness() {
        for cell in [CellVis::Unexplored, CellVis::Explored, CellVis::Visible] {
            assert!(
                (fog_alpha(cell) + fog_shade(cell) - 1.0).abs() < 1e-6,
                "{cell:?}: overlay {} + tint {} is not one darkness",
                fog_alpha(cell),
                fog_shade(cell)
            );
        }
        // And the same legibility bar the overlay is held to, measured on the
        // tint: 100% / 56% / 12%, a quarter of the range apart at minimum.
        let (vis, expl, unexp) = (
            fog_shade(CellVis::Visible),
            fog_shade(CellVis::Explored),
            fog_shade(CellVis::Unexplored),
        );
        assert_eq!(vis, 1.0, "what a team can see now keeps its own colour");
        assert!(
            vis - expl > 0.25 && expl - unexp > 0.25,
            "neighbouring shades too close to distinguish: {vis} / {expl} / {unexp}"
        );
    }

    /// The flat-quad limitation, closed and asserted: a doodad standing in a
    /// cell wears that cell's shade, and changes shade when the cell does.
    ///
    /// The `GlobalTransform` half is the part that would silently rot — a
    /// canopy is a *child* four units above its trunk, so a tinter reading the
    /// local `Transform` would shade every leaf cluster in the game by the cell
    /// at the map's origin and look almost right until someone walked there.
    #[test]
    fn a_doodad_wears_the_shade_of_the_cell_it_stands_in() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.add_plugins(AssetPlugin::default())
            .init_asset::<StandardMaterial>()
            .insert_resource(FogGrids::test_dark())
            .add_systems(Update, apply_fog_tint);
        // Bevy needs the transform propagation to give a child a global
        // position; without it the assertion below would pass for the wrong
        // reason.
        app.add_plugins(bevy::transform::TransformPlugin);

        let shades = {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            let mut add = |c: f32| {
                materials.add(StandardMaterial {
                    base_color: Color::srgb(c, c, c),
                    ..default()
                })
            };
            FogTinted { shades: [add(0.1), add(0.5), add(1.0)] }
        };

        // A trunk at a known cell, with its canopy parented four units up.
        let ground = Vec3::new(30.0, 0.0, -30.0);
        let (cx, cz) = NavGrid::world_to_cell(ground).expect("on the map");
        let canopy = app
            .world_mut()
            .spawn((
                shades.clone(),
                MeshMaterial3d(shades.at(CellVis::Unexplored).clone()),
                Transform::from_xyz(0.0, 4.0, 0.0),
            ))
            .id();
        app.world_mut()
            .spawn((
                shades.clone(),
                MeshMaterial3d(shades.at(CellVis::Unexplored).clone()),
                Transform::from_translation(ground),
            ))
            .add_child(canopy);

        let worn = |app: &App, e: Entity| {
            app.world()
                .entity(e)
                .get::<MeshMaterial3d<StandardMaterial>>()
                .unwrap()
                .0
                .id()
        };

        app.update();
        assert_eq!(
            worn(&app, canopy),
            shades.at(CellVis::Unexplored).id(),
            "a canopy over never-visited ground is not lit"
        );

        app.world_mut()
            .resource_mut::<FogGrids>()
            .test_set_cell(Team::Human, cx, cz, CellVis::Explored);
        app.update();
        assert_eq!(
            worn(&app, canopy),
            shades.at(CellVis::Explored).id(),
            "a canopy over remembered ground must be dimmed with it — this is \
             the lit-forest-over-dark-earth bug"
        );

        app.world_mut()
            .resource_mut::<FogGrids>()
            .test_set_cell(Team::Human, cx, cz, CellVis::Visible);
        app.update();
        assert_eq!(
            worn(&app, canopy),
            shades.at(CellVis::Visible).id(),
            "ground in sight gets its colour back"
        );
    }

    // -- F10 screenshots ---------------------------------------------------
    //
    // The capture itself needs a GPU and a window, so what is testable here is
    // the part that bit people: where the file goes and what it is called.

    #[test]
    fn a_shot_is_named_for_the_moment_it_shows() {
        // Zero-padded game seconds so a directory listing sorts into match
        // order, and the counter breaks ties inside one second.
        assert_eq!(shot_name(1754870400, 324.6, 1), "wc3-1754870400-t0324-01.png");
        assert_eq!(shot_name(1754870400, 324.9, 2), "wc3-1754870400-t0324-02.png");
        // Two runs sharing one directory cannot collide: the wall clock differs.
        assert_ne!(
            shot_name(1754870400, 12.0, 1),
            shot_name(1754870999, 12.0, 1),
            "the wall-clock stamp is what keeps two runs apart"
        );
        // A match can outlive four digits; the name must not wrap or truncate.
        assert_eq!(shot_name(7, 12345.0, 3), "wc3-7-t12345-03.png");
    }

    #[test]
    fn an_unset_or_blank_shot_dir_falls_back() {
        assert_eq!(shot_dir_from(Some("arena/r11/shots")), PathBuf::from("arena/r11/shots"));
        assert_eq!(shot_dir_from(None), PathBuf::from(DEFAULT_SHOT_DIR));
        // `WC3_SHOT_DIR= cargo run` sets the variable to nothing at all. Taking
        // that literally would write PNGs into the process's own directory.
        assert_eq!(shot_dir_from(Some("")), PathBuf::from(DEFAULT_SHOT_DIR));
        assert_eq!(shot_dir_from(Some("   ")), PathBuf::from(DEFAULT_SHOT_DIR));
    }

    // -----------------------------------------------------------------------
    // The hall pick: when the item key is a question and when it is an act
    // -----------------------------------------------------------------------

    /// A selected hero carrying `item` in slot 0.
    fn spawn_selected_hero_with(app: &mut App, item: ItemId) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Transform::from_translation(Vec3::new(20.0, 0.0, 20.0)),
                Health::new(600.0),
                Order::Idle,
                Hero { level: 3, xp: 0.0, mana: 200.0 },
                Inventory([Some(item), None]),
                Selected,
            ))
            .id()
    }

    fn spawn_hall(app: &mut App, kind: BuildingKind, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Building { kind },
                Team::Human,
                Transform::from_translation(at),
                Health::new(building_stats(kind).hp),
            ))
            .id()
    }

    /// **Two halls make the key a question.** Pressing it must not spend the
    /// scroll — it arms the pick and says nothing, because with a choice
    /// available "use the scroll" no longer names an outcome.
    #[test]
    fn a_second_hall_turns_the_teleport_key_into_a_hall_pick() {
        let mut app = ui_app();
        spawn_selected_hero_with(&mut app, ItemId::ScrollOfMassTeleport);
        spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(60.0, 0.0, 60.0));
        spawn_hall(&mut app, BuildingKind::Keep, Vec3::new(-70.0, 0.0, -70.0));

        press(&mut app, &[hotkeys::key(Hk::ItemSlot(0)).unwrap()]);

        assert!(
            said(&app).is_empty(),
            "arming is not using: {:?}",
            said(&app)
        );
        let armed = app.world().resource::<UiState>().teleport_place;
        assert!(armed.is_some(), "the key armed a hall pick");
        assert_eq!(armed.unwrap().slot, 0, "and it remembers which slot it is spending");
        // The card's own short label, so the hint names the button the player
        // just pressed rather than a second name for the same thing.
        assert_eq!(armed.unwrap().name, item_name(ItemId::ScrollOfMassTeleport));
    }

    /// **One hall is no choice at all, so there is no ceremony.** The key
    /// fires the scroll outright, with no destination — which is exactly the
    /// pre-existing behaviour, and the nearest-hall default is the only answer
    /// there is anyway.
    #[test]
    fn with_one_hall_the_teleport_key_still_just_fires() {
        let mut app = ui_app();
        let hero = spawn_selected_hero_with(&mut app, ItemId::ScrollOfMassTeleport);
        spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(60.0, 0.0, 60.0));

        press(&mut app, &[hotkeys::key(Hk::ItemSlot(0)).unwrap()]);

        assert!(
            app.world().resource::<UiState>().teleport_place.is_none(),
            "nothing to ask, so nothing is armed"
        );
        assert!(
            matches!(
                said(&app),
                [Intent::UseItem { slot: 0, hero: Some(h), destination: None }] if *h == hero.to_bits()
            ),
            "the item fires on the press, unaimed: {:?}",
            said(&app)
        );
    }

    /// A hall still going up is not a place a scroll can land, so it does not
    /// count toward "is there a choice". Two buildings, one finished hall, no
    /// question.
    #[test]
    fn a_hall_under_construction_is_not_a_choice() {
        let mut app = ui_app();
        spawn_selected_hero_with(&mut app, ItemId::TownPortal);
        spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(60.0, 0.0, 60.0));
        let going_up = spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(-70.0, 0.0, -70.0));
        app.world_mut()
            .entity_mut(going_up)
            .insert(UnderConstruction { remaining: 20.0 });

        press(&mut app, &[hotkeys::key(Hk::ItemSlot(0)).unwrap()]);

        assert!(
            app.world().resource::<UiState>().teleport_place.is_none(),
            "an unfinished hall is not a destination, so there is still only one"
        );
        assert_eq!(said(&app).len(), 1, "and the portal fires: {:?}", said(&app));
    }

    /// Only the teleport items ask. A potion with two halls standing is still
    /// just a potion — the arming rule is a property of the ITEM, read from
    /// the one function that answers it.
    #[test]
    fn a_potion_never_asks_which_hall() {
        let mut app = ui_app();
        spawn_selected_hero_with(&mut app, ItemId::HealingPotion);
        spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(60.0, 0.0, 60.0));
        spawn_hall(&mut app, BuildingKind::Keep, Vec3::new(-70.0, 0.0, -70.0));

        press(&mut app, &[hotkeys::key(Hk::ItemSlot(0)).unwrap()]);

        assert!(
            app.world().resource::<UiState>().teleport_place.is_none(),
            "a potion has nowhere to go"
        );
        assert_eq!(said(&app).len(), 1, "it is drunk on the press: {:?}", said(&app));
    }

    /// Escape means "one step out", and the hall pick is the innermost step.
    /// Backing out of it must spend nothing and must not also leave the
    /// command card.
    #[test]
    fn escape_cancels_the_hall_pick_and_spends_nothing() {
        let mut app = ui_app();
        spawn_selected_hero_with(&mut app, ItemId::ScrollOfMassTeleport);
        spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(60.0, 0.0, 60.0));
        spawn_hall(&mut app, BuildingKind::Keep, Vec3::new(-70.0, 0.0, -70.0));

        press(&mut app, &[hotkeys::key(Hk::ItemSlot(0)).unwrap()]);
        assert!(app.world().resource::<UiState>().teleport_place.is_some());

        press(&mut app, &[hotkeys::CANCEL]);

        let ui = app.world().resource::<UiState>();
        assert!(ui.teleport_place.is_none(), "one step out cancels the pick");
        assert_eq!(ui.page, CardPage::Orders, "and it does not also flip the card");
        assert!(said(&app).is_empty(), "a cancelled gesture spends nothing: {:?}", said(&app));
    }

    /// Arming something else disarms the hall pick. The armed modes are
    /// mutually exclusive, and this one is checked FIRST by the click handler,
    /// so a stale one would swallow the click meant for the new mode.
    #[test]
    fn arming_another_mode_drops_a_pending_hall_pick() {
        let mut app = ui_app();
        spawn_selected_hero_with(&mut app, ItemId::ScrollOfMassTeleport);
        spawn_hall(&mut app, BuildingKind::TownHall, Vec3::new(60.0, 0.0, 60.0));
        spawn_hall(&mut app, BuildingKind::Keep, Vec3::new(-70.0, 0.0, -70.0));

        press(&mut app, &[hotkeys::key(Hk::ItemSlot(0)).unwrap()]);
        assert!(app.world().resource::<UiState>().teleport_place.is_some());

        press(&mut app, &[hotkeys::key(Hk::AttackMove).unwrap()]);

        let ui = app.world().resource::<UiState>();
        assert!(ui.attack_move_armed, "the new mode is armed");
        assert!(
            ui.teleport_place.is_none(),
            "and the old one is gone — two armed clicks cannot both want the next press"
        );
    }
}
