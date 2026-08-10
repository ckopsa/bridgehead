//! ui.rs — player controls & HUD.
//!
//! Owns: `Selected` marker, selection rings, right-click context orders,
//! building placement ghost, command hotkeys/buttons, control groups, and the
//! bevy_ui HUD: a top resource bar plus a classic WC3-style bottom console
//! (minimap | selection panel | command card), the drag rectangle, the
//! game-over banner, and the top-right alert stack.
//!
//! The alert stack renders `shared::GameEvents` — the very buffer bridge.rs
//! serializes for an external commander, filtered to `Team::Human` the same way
//! the bridge filters to its seat. One producer, two renderers: whatever the
//! machine is told about the match, the player is told too. Space (or a click
//! on a row) sends the camera to where the news came from.
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
use bevy::window::{PrimaryWindow, SystemCursorIcon};
use bevy::winit::cursor::CursorIcon;
use std::collections::{HashMap, VecDeque};

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
/// Spacing of the move-order formation grid.
const FORMATION_SPACING: f32 = 2.6;
/// Maximum queued units per production building.
const MAX_QUEUE: usize = 7;

/// Leash radius written by the [G Guard] command-card toggle.
const GUARD_RADIUS: f32 = 18.0;
/// HP fraction at which the [V Fallback] toggle breaks a unit off.
const FALLBACK_FRAC: f32 = 0.35;
/// Enemies inside the slam radius before [T Auto-Slam] fires.
const AUTOCAST_MIN_ENEMIES: u32 = 3;

const TOP_BAR_H: f32 = 34.0;
/// Height of the bottom console; also the "not a world click" strip.
const CONSOLE_H: f32 = 200.0;
/// Uniform gap between console zones and the console edge.
const PAD: f32 = 8.0;
/// Minimap is a square of this many logical pixels.
const MINIMAP_PX: f32 = 184.0;

/// Selection cards: 2 rows of 6.
const MAX_CARDS: usize = 12;
const CARD_PX: f32 = 44.0;
const CARD_GAP: f32 = 5.0;

/// Command card: 3x3 grid.
const CMD_SLOTS: usize = 9;
const CMD_PX: f32 = 52.0;
const CMD_GAP: f32 = 6.0;
const CMD_COLS: f32 = 3.0;

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
const NOTIF_MAX_FRAC: f32 = 0.9;
/// Height budgeted per row when hit-testing (see `notif_rect`). A row is one
/// line of text most of the time, but a long message in a narrow window wraps,
/// and no analytic guess can know which. Two lines' worth means the stack
/// occasionally swallows a click just under it — much better than leaking one
/// through as a stray move order on the battlefield behind.
const NOTIF_ROW_HIT_H: f32 = NOTIF_ROW_H + NOTIF_FONT + NOTIF_GAP;
/// Jump the camera to the newest alert, then the one before it, and so on.
/// Space is free: letters are command hotkeys, arrows pan, `.` cycles workers.
const NOTIF_FOCUS_KEY: KeyCode = KeyCode::Space;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiState>()
            .init_resource::<Notifications>()
            .add_systems(Startup, (setup_ui, setup_hover).chain())
            .add_systems(
                Update,
                (
                    minimap_static_markers,
                    surrender_hotkey,
                    command_input,
                    panel_clicks,
                    // Before `minimap_input`: both write `CameraFocus` and
                    // terrain.rs honours the last one, so a live minimap drag
                    // outranks a Space press from earlier in the frame.
                    notification_input,
                    control_groups,
                    minimap_input,
                    left_mouse,
                    right_mouse,
                    update_ghost,
                    update_rally_flag,
                    hover_feedback,
                    sync_selection_rings,
                    update_minimap,
                    update_minimap_bounties,
                    update_notifications,
                    update_hud,
                )
                    .chain(),
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
    /// Control groups 1..3.
    groups: HashMap<u8, Vec<Entity>>,
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
}

#[derive(Resource)]
struct UiAssets {
    ring_mesh: Handle<Mesh>,
    ring_mat: Handle<StandardMaterial>,
    ghost_ok: Handle<StandardMaterial>,
    ghost_bad: Handle<StandardMaterial>,
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
#[derive(Component)]
struct HoverRing;

/// The single pooled rally-point banner, moved to the rally location of the
/// one selected production building (hidden otherwise).
#[derive(Component)]
struct RallyFlag;

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
    Overflow,
    CardLetter(usize),
    QueueLetter(usize),
    CmdKey(usize),
    CmdLabel(usize),
    CmdCost(usize),
}

// ---------------------------------------------------------------------------
// Commands (shared by hotkeys and command-card buttons)
// ---------------------------------------------------------------------------

/// Every event the command card can emit, in one system param.
///
/// `command_input` already reads most of the world to decide what the current
/// selection can do, and Bevy caps a system at 16 parameters — bundling the
/// writers keeps room for the next command that needs one instead of spending
/// the last slot on it.
#[derive(SystemParam)]
struct CardActions<'w> {
    casts: EventWriter<'w, CastAbility>,
    buys: EventWriter<'w, BuyItem>,
    item_uses: EventWriter<'w, UseItem>,
    upgrades: EventWriter<'w, UpgradeBuilding>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CmdAction {
    AttackMove,
    Stop,
    Place(BuildingKind),
    Train(UnitKind),
    /// The selected hero's ability, whatever `ability_of_unit` says it is
    /// (every selected own hero casts its own).
    CastHero,
    /// The single selected own building's ability (`ability_of_building`).
    CastBuilding,
    /// Buy a consumable at the single selected own Shop, for the team's hero.
    Buy(ItemId),
    /// Convert the single selected own building into its next tier in place.
    /// Carries the RESULT, so the button can name what you get.
    Upgrade(BuildingKind),
    /// Consume the selected own hero's inventory slot.
    UseSlot(usize),
    /// Doctrine: toggle `LeashPolicy` on the whole own-unit selection.
    ToggleGuard,
    /// Doctrine: toggle `RetreatPolicy` on the whole own-unit selection.
    ToggleFallback,
    /// Doctrine: advance the `TargetPriority` preset by one step.
    CyclePriority,
    /// Doctrine: toggle `AutoCastPolicy` on every selected own hero.
    ToggleAutoCast,
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
    hero: bool,
}

impl UnitDoctrine {
    fn read(
        leash: Option<&LeashPolicy>,
        retreat: Option<&RetreatPolicy>,
        prio: Option<&TargetPriority>,
        autocast: Option<&AutoCastPolicy>,
        hero: bool,
    ) -> Self {
        UnitDoctrine {
            leash: leash.map(|l| l.radius),
            retreat: retreat.map(|r| r.below_frac),
            prio: PrioPreset::of(prio.and_then(|p| p.0.first().copied())),
            autocast: autocast.is_some(),
            hero,
        }
    }
}

/// Aggregate doctrine of the current own-unit selection. `command_input` and
/// `update_hud` each build one from the same entity-index-sorted list, so the
/// captions, the highlight and the executed toggle always agree.
#[derive(Clone, Copy, Default)]
struct DoctrineState {
    units: usize,
    heroes: usize,
    leashed: usize,
    leash_radius: f32,
    fallback: usize,
    fallback_frac: f32,
    autocast: usize,
    /// Preset of the FIRST selected unit (lowest entity index).
    prio: PrioPreset,
}

impl DoctrineState {
    /// `sorted` must be ordered by entity index — that fixes "the first unit".
    fn of(sorted: &[UnitDoctrine]) -> Self {
        let mut s = DoctrineState {
            units: sorted.len(),
            prio: sorted.first().map(|u| u.prio).unwrap_or_default(),
            ..default()
        };
        for u in sorted {
            if u.hero {
                s.heroes += 1;
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
    /// Compact panel line; empty when the selection carries no policy at all.
    /// A trailing `xN` marks a policy only part of the selection has.
    fn line(&self) -> String {
        let tally = |count: usize, total: usize| {
            if count < total {
                format!(" x{}", count)
            } else {
                String::new()
            }
        };
        let mut parts: Vec<String> = Vec::new();
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
            parts.push(format!("autoslam{}", tally(self.autocast, self.heroes)));
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

struct CmdEntry {
    action: CmdAction,
    key: KeyCode,
    hotkey: &'static str,
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
    fn plain(action: CmdAction, key: KeyCode, hotkey: &'static str, label: &str) -> Self {
        CmdEntry {
            action,
            key,
            hotkey,
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
        self.cost = if lumber > 0 {
            format!("{}g {}l", gold, lumber)
        } else {
            format!("{}g", gold)
        };
        self.afford = Some((gold, lumber));
        self
    }
}

/// What the current selection can do about heroes, abilities and items.
#[derive(Clone, Copy, Default)]
struct HeroCmds {
    /// Hero training offered by the selected town hall.
    train: Option<HeroTrain>,
    /// A selected own hero: (its ability, ready, seconds of cooldown left).
    ability: Option<(AbilityDef, bool, f32)>,
    /// The single selected own completed building's ability, when it has one:
    /// (ability, seconds of cooldown left).
    building_ability: Option<(AbilityDef, f32)>,
    /// The single selected own completed Shop's buy state.
    shop: Option<ShopState>,
    /// The single selected own completed building's available tier-up, when it
    /// has one and is not already converting: `(result kind, gold, lumber)`.
    upgrade: Option<(BuildingKind, u32, u32)>,
    /// Inventory of the selected own hero (all None when none is selected).
    items: [Option<ItemId>; 2],
}

/// The town hall's hero button(s). A team plays exactly one hero class: the
/// first one trained locks it in (`HeroRecord.kind`), and from then on the card
/// offers only that class, as a cheaper "Revive".
#[derive(Clone, Copy)]
struct HeroTrain {
    gold: u32,
    lumber: u32,
    /// `Some(kind)` once a record exists — only that class may be revived.
    recorded: Option<UnitKind>,
}

/// Why a Shop's buy buttons are (not) usable. The Shop sells to the team's
/// hero wherever it stands — WC3 sells to whoever walks up, we sell to the one
/// hero the team is allowed to have.
#[derive(Clone, Copy, Default)]
struct ShopState {
    /// The team has a living hero to buy for.
    hero: bool,
    /// That hero has a free inventory slot.
    room: bool,
}

/// Where a building sits on the worker's build card: `(slot, key, caption)`,
/// or `None` for a kind the player may not place directly. This exhaustive
/// match is the ONLY thing a new `BuildingKind` has to declare here — cost,
/// name and tech gating all come from the shared tables via `build_cards`.
///
/// Free letters only: every other command key in this file is
/// A S R C B F H O L K N Q W E G V P T Z X, plus Esc / '.' / Ctrl+1-3;
/// shared.rs owns F1-F4, ai.rs F9, the surrender hotkey F12, and terrain.rs
/// the arrow keys. K (workshop) and N (shop) were picked against that whole
/// list — J and U are the remaining unclaimed candidates.
fn build_card_slot(kind: BuildingKind) -> Option<(u8, KeyCode, &'static str)> {
    match kind {
        BuildingKind::Barracks => Some((0, KeyCode::KeyB, "B")),
        BuildingKind::Farm => Some((1, KeyCode::KeyF, "F")),
        BuildingKind::TownHall => Some((2, KeyCode::KeyH, "H")),
        BuildingKind::Tower => Some((3, KeyCode::KeyO, "O")),
        BuildingKind::Wall => Some((4, KeyCode::KeyL, "L")),
        BuildingKind::Workshop => Some((5, KeyCode::KeyK, "K")),
        BuildingKind::Shop => Some((6, KeyCode::KeyN, "N")),
        // Reached by upgrading a hall, never by placing one — no build card,
        // and `build_cards` filters on `building_placeable` besides.
        BuildingKind::Keep | BuildingKind::Castle => None,
    }
}

/// Every placeable building, in card order — walked from the shared kind table
/// so new content appears on the card as soon as it has a slot.
fn build_cards() -> Vec<(u8, BuildingKind, KeyCode, &'static str)> {
    let mut cards: Vec<(u8, BuildingKind, KeyCode, &'static str)> = ALL_BUILDING_KINDS
        .into_iter()
        .filter(|kind| building_placeable(*kind))
        .filter_map(|kind| {
            build_card_slot(kind).map(|(slot, key, hotkey)| (slot, kind, key, hotkey))
        })
        .collect();
    cards.sort_by_key(|(slot, ..)| *slot);
    cards
}

/// Production hotkeys, by index into `trainable()`: Q, W, E. A Shop trains
/// nothing, so its buy buttons reuse the same letters without colliding.
const TRAIN_KEYS: [(KeyCode, &str); 3] = [
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyE, "E"),
];

/// Inventory-slot hotkeys, by slot index.
const ITEM_KEYS: [(KeyCode, &str); 2] = [(KeyCode::KeyZ, "Z"), (KeyCode::KeyX, "X")];

/// Ability button caption: the ability's own name, plus the countdown while it
/// is cooling down. Works for hero and building casters alike.
fn ability_label(def: &AbilityDef, cooldown: f32) -> String {
    if cooldown > 0.0 {
        format!("{} {:.0}s", def.name, cooldown.ceil())
    } else {
        def.name.to_string()
    }
}

/// Can this hero cast *its* ability right now? `Hero::ability_ready` prices
/// every class at the Champion's 40 mana, which would light the button up for a
/// Priestess sitting on 42 of the 45 Heal costs; combat.rs would then refuse.
fn hero_ability_ready(hero: &Hero, def: &AbilityDef) -> bool {
    hero.ability_cooldown <= 0.0 && hero.mana >= def.mana_cost
}

/// The contextual command set for the current selection. Both the keyboard and
/// the command card drive off this list, so a click and a key press run the
/// exact same code path.
///
/// Slot budget (the card is 3x3 = `CMD_SLOTS`). Layout per selection type:
///   worker(s)            A S | B F H O L K N               (9, all toggles dropped)
///   worker(s) + hero     A S | B F H O L K N               (9, [R Z X] dropped too)
///   fighters             A S | G V P                       (5)
///   hero                 A S R | Z X (carried items) | G V P T   (<=9)
///   town hall            Q(Worker) W/E(hero class) C(CallToArms)
///   barracks             Q(Footman) W(Archer) E(Raider)    (3)
///   workshop             Q(Catapult)                       (1)
///   shop                 Q(Potion) W(Portal)               (2)
///
/// Build commands never yield — a greyed [K Workshop] is how the player learns
/// what unlocks it, and it is the only route to a building at all. The doctrine
/// toggles give way first, in the order [P Priority] (a preference),
/// [V Fallback], [G Guard]; [T Auto-Slam] is kept longest because it is the
/// only hero-specific toggle with no other route in. With seven buildable kinds
/// a worker card spends its whole budget on the classic layout, so a worker
/// selection loses the toggles outright — and a worker+hero selection loses the
/// hero's [R] and item buttons as well. Both are one deselect away; a building
/// the player cannot even see on the card is not.
///
/// Abilities and items are generic: the hero button reads `ability_of_unit`, so
/// a Champion shows [R Slam 40mp] and a Priestess [R Heal 45mp] with no code
/// here naming either; the building button reads `ability_of_building`, which
/// only the TownHall answers today ([C CallToArms]).
fn command_entries(
    own_units: usize,
    has_worker: bool,
    // (kind, completed) of the only selected building, when it is the whole selection.
    single_building: Option<(BuildingKind, bool)>,
    hero: HeroCmds,
    doc: DoctrineState,
    // Completed buildings the player owns — the tech gate for build entries.
    completed: &[BuildingKind],
) -> Vec<CmdEntry> {
    let mut out: Vec<CmdEntry> = Vec::new();

    if own_units > 0 {
        out.push(CmdEntry::plain(
            CmdAction::AttackMove,
            KeyCode::KeyA,
            "A",
            "Attack",
        ));
        out.push(CmdEntry::plain(CmdAction::Stop, KeyCode::KeyS, "S", "Stop"));
    }

    // Builds first: they never yield (see above), so with a full seven kinds a
    // worker selection always shows the classic layout even when a hero got
    // caught in the drag box.
    if has_worker {
        for (_, kind, key, hotkey) in build_cards() {
            out.push(
                CmdEntry::plain(CmdAction::Place(kind), key, hotkey, building_name(kind))
                    // ...after `priced`: an unmet requirement takes the cost line.
                    .priced_as_building(kind)
                    .requires(building_requires(kind), completed),
            );
        }
    }

    // The hero's ability, whichever class it is.
    if let Some((def, ready, cooldown)) = hero.ability {
        let mut entry = CmdEntry::plain(
            CmdAction::CastHero,
            KeyCode::KeyR,
            "R",
            &ability_label(&def, cooldown),
        );
        entry.cost = format!("{:.0}mp", def.mana_cost);
        entry.enabled = ready;
        out.push(entry);
    }

    // Carried consumables: one button per filled slot, so an empty inventory
    // costs the card nothing.
    for (slot, (key, hotkey)) in ITEM_KEYS.iter().copied().enumerate() {
        let Some(Some(item)) = hero.items.get(slot).copied() else {
            continue;
        };
        out.push(CmdEntry::plain(
            CmdAction::UseSlot(slot),
            key,
            hotkey,
            item_name(item),
        ));
    }

    if own_units == 0 {
        if let Some((kind, true)) = single_building {
            for (i, unit) in trainable(kind).iter().enumerate() {
                let Some((key, hotkey)) = TRAIN_KEYS.get(i).copied() else {
                    continue;
                };
                if is_hero_kind(*unit) {
                    // Hidden entirely while the team already has (or is
                    // training) its one hero...
                    let Some(train) = hero.train else {
                        continue;
                    };
                    // ...and once a record exists the team is locked to that
                    // class: only the recorded one is offered, as a Revive.
                    let label = match train.recorded {
                        Some(recorded) if recorded != *unit => continue,
                        Some(_) => "Revive",
                        None => unit_name(*unit),
                    };
                    out.push(
                        CmdEntry::plain(CmdAction::Train(*unit), key, hotkey, label)
                            .priced(train.gold, train.lumber),
                    );
                } else {
                    let s = unit_stats(*unit);
                    out.push(
                        CmdEntry::plain(
                            CmdAction::Train(*unit),
                            key,
                            hotkey,
                            unit_name(*unit),
                        )
                        .priced(s.cost_gold, s.cost_lumber)
                        // No unit has a tech requirement today; wiring it here
                        // means the first one that does is gated for free.
                        .requires(unit_requires(*unit), completed),
                    );
                }
            }

            // The building's own active ability (TownHall: Call to Arms).
            if let Some((def, cooldown)) = hero.building_ability {
                let mut entry = CmdEntry::plain(
                    CmdAction::CastBuilding,
                    KeyCode::KeyC,
                    "C",
                    &ability_label(&def, cooldown),
                );
                entry.enabled = cooldown <= 0.0;
                out.push(entry);
            }

            // Tier up in place. [U] because it is the last free letter that
            // says what it does; the card has room here because a hall spends
            // at most four slots on training and Call to Arms.
            if let Some((to, gold, lumber)) = hero.upgrade {
                out.push(
                    CmdEntry::plain(
                        CmdAction::Upgrade(to),
                        KeyCode::KeyU,
                        "U",
                        &format!("Upgrade: {}", building_name(to)),
                    )
                    .priced(gold, lumber),
                );
            }

            // A Shop sells to the team's one hero: dark without a hero, with a
            // full inventory, or with an empty purse.
            if let Some(shop) = hero.shop {
                for (i, item) in ALL_ITEMS.iter().enumerate() {
                    let Some((key, hotkey)) = TRAIN_KEYS.get(i).copied() else {
                        continue;
                    };
                    let def = item_def(*item);
                    let mut entry =
                        CmdEntry::plain(CmdAction::Buy(*item), key, hotkey, item_name(*item))
                            .priced(def.cost_gold, 0);
                    entry.enabled = shop.hero && shop.room;
                    out.push(entry);
                }
            }
        }
    }

    // --- doctrine toggles (appended: the classic layout keeps its slots) ---
    if own_units > 0 {
        let mut doctrine = vec![
            CmdEntry::plain(CmdAction::ToggleGuard, KeyCode::KeyG, "G", "Guard")
                .active(doc.guard_active()),
            CmdEntry::plain(CmdAction::ToggleFallback, KeyCode::KeyV, "V", "Fallback")
                .active(doc.fallback_active()),
            CmdEntry::plain(
                CmdAction::CyclePriority,
                KeyCode::KeyP,
                "P",
                doc.prio.label(),
            )
            .active(doc.prio != PrioPreset::None),
        ];
        if doc.heroes > 0 {
            doctrine.push(
                CmdEntry::plain(CmdAction::ToggleAutoCast, KeyCode::KeyT, "T", "Auto-Slam")
                    .active(doc.autocast_active()),
            );
        }
        // Worker selections push past the 3x3 card (A S + B F H O L = 7, or 8
        // with a hero's [R Slam]), so the toggles yield in the documented
        // order until what is left fits. See the layout table above.
        const YIELD_ORDER: [CmdAction; 3] = [
            CmdAction::CyclePriority,
            CmdAction::ToggleFallback,
            CmdAction::ToggleGuard,
        ];
        let room = CMD_SLOTS.saturating_sub(out.len());
        for dropped in YIELD_ORDER {
            if doctrine.len() <= room {
                break;
            }
            if let Some(i) = doctrine.iter().position(|e| e.action == dropped) {
                doctrine.remove(i);
            }
        }
        doctrine.truncate(room);
        out.extend(doctrine);
    }

    out.truncate(CMD_SLOTS);
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
    }
}

/// Short, card-sized item name ("Potion" reads better on a 52px button than
/// the catalog id "HealingPotion").
fn item_name(id: ItemId) -> &'static str {
    match id {
        ItemId::HealingPotion => "Potion",
        ItemId::TownPortal => "Portal",
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
        BuildingKind::Keep => "Keep",
        BuildingKind::Castle => "Castle",
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
fn cursor_over_hud(cursor: Vec2, window: &Window, ui: &UiState) -> bool {
    if cursor.y < TOP_BAR_H || cursor.y > window.height() - CONSOLE_H {
        return true;
    }
    notif_rect(window, ui.notif_rows).is_some_and(|r| r.contains(cursor))
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

/// Screen-space rectangle of the minimap. The console is a fixed-height strip
/// pinned to the bottom of the window and the minimap sits at its top-left with
/// a `PAD` margin, so the rect is exact without touching layout internals.
fn minimap_rect(window: &Window) -> Rect {
    let top = window.height() - CONSOLE_H + PAD;
    Rect::new(PAD, top, PAD + MINIMAP_PX, top + MINIMAP_PX)
}

/// World XZ -> minimap pixel offset. +X is right, +Z is up (matching the
/// default camera view: the Human base ends up bottom-left).
fn world_to_minimap(p: Vec3) -> Vec2 {
    Vec2::new(
        (p.x + MAP_HALF) / (2.0 * MAP_HALF) * MINIMAP_PX,
        (MAP_HALF - p.z) / (2.0 * MAP_HALF) * MINIMAP_PX,
    )
}

/// Minimap pixel offset -> world XZ.
fn minimap_to_world(uv: Vec2) -> Vec3 {
    Vec3::new(
        uv.x / MINIMAP_PX * 2.0 * MAP_HALF - MAP_HALF,
        0.0,
        MAP_HALF - uv.y / MINIMAP_PX * 2.0 * MAP_HALF,
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

fn lighten(c: Color, amount: f32) -> Color {
    let s = c.to_srgba();
    Color::srgb(
        (s.red + amount).min(1.0),
        (s.green + amount).min(1.0),
        (s.blue + amount).min(1.0),
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

/// Grid offsets around a click point so a group doesn't stack on one spot.
fn formation_offset(index: usize, count: usize) -> Vec3 {
    if count <= 1 {
        return Vec3::ZERO;
    }
    let cols = (count as f32).sqrt().ceil().max(1.0) as usize;
    let rows = count.div_ceil(cols);
    let col = index % cols;
    let row = index / cols;
    Vec3::new(
        (col as f32 - (cols as f32 - 1.0) * 0.5) * FORMATION_SPACING,
        0.0,
        (row as f32 - (rows as f32 - 1.0) * 0.5) * FORMATION_SPACING,
    )
}

/// Issue a Move / AttackMove to a group with the usual formation spread.
fn issue_ground_order(commands: &mut Commands, group: &[Entity], ground: Vec3, attack_move: bool) {
    let count = group.len();
    for (i, e) in group.iter().enumerate() {
        let p = clamp_to_map(ground + formation_offset(i, count));
        let order = if attack_move {
            Order::AttackMove(p)
        } else {
            Order::Move(p)
        };
        commands.entity(*e).try_insert(order);
    }
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
        ring_mat,
        ghost_ok,
        ghost_bad,
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
        });

    // --- Bottom console ----------------------------------------------------
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Px(CONSOLE_H),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                border: UiRect::top(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(CONSOLE_BG),
            BorderColor(EDGE),
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
                width: Val::Px(MINIMAP_PX),
                height: Val::Px(MINIMAP_PX),
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
            c.spawn(text_bundle(
                "Left-click / drag to select.",
                13.0,
                Color::srgb(0.62, 0.92, 0.68),
                Slot::Hints,
            ));
        });
}

fn spawn_command_card(console: &mut ChildSpawnerCommands) {
    console
        .spawn(Node {
            width: Val::Px(CMD_COLS * CMD_PX + (CMD_COLS - 1.0) * CMD_GAP),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_content: AlignContent::FlexStart,
            column_gap: Val::Px(CMD_GAP),
            row_gap: Val::Px(CMD_GAP),
            margin: UiRect::all(Val::Px(PAD)),
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
    mut focus: EventWriter<CameraFocus>,
    mut acts: CardActions,
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
        ),
        With<Selected>,
    >,
    mut sel_buildings: Query<
        (
            Entity,
            &Building,
            &Team,
            Option<&mut TrainingQueue>,
            Option<&UnderConstruction>,
            Option<&AbilityCooldown>,
            Option<&Upgrading>,
        ),
        With<Selected>,
    >,
    // Read-only: the team's hero (anywhere on the map) is the Shop's customer.
    all_units: Query<(Entity, &Unit, &Team, &Order, &Transform, Option<&Inventory>)>,
    // Read-only: the fallback rally looks for the nearest own town hall.
    all_buildings: Query<(&Building, &Team, &Transform, Has<UnderConstruction>)>,
) {
    if game_over.0.is_some() {
        return;
    }

    // Escape cancels every transient mode.
    if keys.just_pressed(KeyCode::Escape) {
        if ui.placement.is_some() {
            ui.placement = None;
            ui.wall_chain.clear();
        } else if ui.attack_move_armed {
            ui.attack_move_armed = false;
        }
        ui.dragging = false;
        ui.drag_start = None;
        return;
    }

    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    // --- idle worker cycling (not a command-card entry) -------------------
    if !ctrl && keys.just_pressed(KeyCode::Period) {
        let idle: Vec<(Entity, Vec3)> = all_units
            .iter()
            .filter(|(_, u, t, o, _, _)| {
                **t == Team::Human && u.kind == UnitKind::Worker && matches!(o, Order::Idle)
            })
            .map(|(e, _, _, _, tf, _)| (e, tf.translation))
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
        .filter(|(_, _, t, _, _, _, _, _, _, _)| **t == Team::Human)
        .map(|(e, _, _, tf, _, _, _, _, _, _)| (e, tf.translation))
        .collect();
    let has_worker = sel_units
        .iter()
        .any(|(_, u, t, _, _, _, _, _, _, _)| *t == Team::Human && u.kind == UnitKind::Worker);
    // Every selected own hero (there is only ever one, but iterate anyway).
    // Keyed off the `Hero` component, so both classes qualify.
    let own_heroes: Vec<(Entity, UnitKind, Hero, Inventory)> = sel_units
        .iter()
        .filter(|(_, _, t, _, h, _, _, _, _, _)| **t == Team::Human && h.is_some())
        .map(|(e, u, _, _, h, _, _, _, _, inv)| {
            (e, u.kind, *h.unwrap(), inv.copied().unwrap_or_default())
        })
        .collect();
    // Doctrine of the own-unit selection, in a stable order.
    let doc = DoctrineState::of(&sorted_doctrine(
        sel_units
            .iter()
            .filter(|(_, _, t, _, _, _, _, _, _, _)| **t == Team::Human)
            .map(|(e, _, _, _, hero, leash, retreat, prio, autocast, _)| {
                (
                    e.index(),
                    UnitDoctrine::read(leash, retreat, prio, autocast, hero.is_some()),
                )
            })
            .collect(),
    ));

    let mut b_iter = sel_buildings.iter();
    // The one selected own building: its kind, whether it is finished, its
    // entity (buy/cast target) and its ability cooldown, if it has one.
    let single = match (b_iter.next(), b_iter.next()) {
        (Some((e, b, t, _, uc, cd, up)), None) if *t == Team::Human => {
            Some((e, b.kind, uc.is_none(), cd.map(|c| c.0), up.is_some()))
        }
        _ => None,
    };
    let single_building = single.map(|(_, kind, done, _, _)| (kind, done));

    // Hero training is offered only while the team has neither a living
    // hero (of either class) nor one already queued in this building.
    let team_has_hero = all_units
        .iter()
        .any(|(_, u, t, _, _, _)| *t == Team::Human && is_hero_kind(u.kind));
    let hero_in_queue = sel_buildings.iter().any(|(_, _, t, q, _, _, _)| {
        *t == Team::Human
            && q.map(|q| q.queue.iter().any(|k| is_hero_kind(*k)))
                .unwrap_or(false)
    });
    // The team's hero wherever it stands — the Shop's only customer.
    let team_hero: Option<(Entity, Inventory)> = all_units
        .iter()
        .find(|(_, u, t, _, _, _)| **t == Team::Human && is_hero_kind(u.kind))
        .map(|(e, _, _, _, _, inv)| (e, inv.copied().unwrap_or_default()));
    let hero_cmds = HeroCmds {
        train: (!team_has_hero && !hero_in_queue).then(|| {
            let (gold, lumber, _) = hero_train_cost(&records, Team::Human);
            HeroTrain {
                gold,
                lumber,
                recorded: records.get(Team::Human).map(|r| r.kind),
            }
        }),
        ability: own_heroes.first().and_then(|(_, kind, h, _)| {
            ability_of_unit(*kind)
                .map(|def| (def, hero_ability_ready(h, &def), h.ability_cooldown))
        }),
        building_ability: single.and_then(|(_, kind, done, cd, _)| {
            (done)
                .then(|| ability_of_building(kind))
                .flatten()
                .map(|def| (def, cd.unwrap_or(0.0)))
        }),
        shop: single.and_then(|(_, kind, done, _, _)| {
            (done && kind == BuildingKind::Shop).then(|| ShopState {
                hero: team_hero.is_some(),
                room: team_hero.is_some_and(|(_, inv)| inv.0.iter().any(|s| s.is_none())),
            })
        }),
        upgrade: single.and_then(|(_, kind, done, _, upgrading)| {
            (done && !upgrading)
                .then(|| upgrade_cost(kind).zip(building_upgrades_to(kind)))
                .flatten()
                .map(|((gold, lumber, _), to)| (to, gold, lumber))
        }),
        items: own_heroes.first().map(|(_, _, _, inv)| inv.0).unwrap_or_default(),
    };

    // Completed own buildings = the tech state every build entry is gated on.
    let completed: Vec<BuildingKind> = all_buildings
        .iter()
        .filter(|(_, t, _, under)| **t == Team::Human && !under)
        .map(|(b, _, _, _)| b.kind)
        .collect();

    let entries = command_entries(
        own_units.len(),
        has_worker,
        single_building,
        hero_cmds,
        doc,
        &completed,
    );

    // --- collect this frame's commands ------------------------------------
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

    // --- execute -----------------------------------------------------------
    for action in actions {
        match action {
            CmdAction::AttackMove => {
                ui.attack_move_armed = true;
                ui.placement = None;
            }
            CmdAction::Stop => {
                // Re-issuing a move to the unit's own position halts it and
                // clears any attack target; combat.rs re-acquires afterwards.
                for (e, pos) in &own_units {
                    commands.entity(*e).try_insert(Order::Move(*pos));
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
            }
            // Abilities: combat.rs owns the mana/cooldown verdict, exactly as
            // it does for the AI and the bridge.
            CmdAction::CastHero => {
                for (hero, _, _, _) in &own_heroes {
                    acts.casts.write(CastAbility { caster: *hero });
                }
            }
            CmdAction::CastBuilding => {
                if let Some((entity, kind, true, _, _)) = single {
                    if ability_of_building(kind).is_some() {
                        acts.casts.write(CastAbility { caster: entity });
                    }
                }
            }
            CmdAction::Buy(item) => {
                // economy.rs re-validates ownership, slots and gold; the card
                // only greys the button so the player knows before clicking.
                if let (Some((shop, BuildingKind::Shop, true, _, _)), Some((hero, _))) =
                    (single, team_hero)
                {
                    acts.buys.write(BuyItem { shop, hero, item });
                }
            }
            CmdAction::Upgrade(to) => {
                // economy.rs owns the verdict and the money, exactly as it does
                // for the bridge's `upgrade` command and the AI's tier-up.
                if let Some((entity, kind, true, _, false)) = single {
                    if building_upgrades_to(kind) == Some(to) {
                        acts.upgrades.write(UpgradeBuilding { building: entity });
                    }
                }
            }
            CmdAction::UseSlot(slot) => {
                if let Some((hero, _, _, _)) = own_heroes.first() {
                    acts.item_uses.write(UseItem { hero: *hero, slot });
                }
            }
            // --- doctrine toggles: every mutation goes through Commands, so
            // no new &mut query can ever alias the reads above (B0001).
            CmdAction::ToggleGuard => {
                if doc.leashed == 0 {
                    // Anchor on the centre of mass of the group being told to
                    // hold: "guard where you stand".
                    let centroid = own_units
                        .iter()
                        .fold(Vec3::ZERO, |acc, (_, p)| acc + *p)
                        / own_units.len().max(1) as f32;
                    let anchor = clamp_to_map(centroid);
                    for (e, _) in &own_units {
                        commands.entity(*e).try_insert(LeashPolicy {
                            anchor,
                            radius: GUARD_RADIUS,
                        });
                    }
                } else {
                    // Mixed selection: any leash at all means "release all".
                    for (e, _) in &own_units {
                        commands.entity(*e).try_remove::<LeashPolicy>();
                    }
                }
            }
            CmdAction::ToggleFallback => {
                if doc.fallback == 0 {
                    let centroid = own_units
                        .iter()
                        .fold(Vec3::ZERO, |acc, (_, p)| acc + *p)
                        / own_units.len().max(1) as f32;
                    // Nearest own completed town hall, else the start base.
                    let rally = all_buildings
                        .iter()
                        .filter(|(b, t, _, under)| {
                            // Any rung of the hall ladder is a place to fall
                            // back to.
                            **t == Team::Human && is_hall(b.kind) && !under
                        })
                        .map(|(_, _, tf, _)| tf.translation)
                        .min_by(|a, b| {
                            dist_xz(*a, centroid)
                                .partial_cmp(&dist_xz(*b, centroid))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .unwrap_or(HUMAN_BASE);
                    let rally = Vec3::new(rally.x, 0.0, rally.z);
                    for (e, _) in &own_units {
                        commands.entity(*e).try_insert(RetreatPolicy {
                            below_frac: FALLBACK_FRAC,
                            rally,
                        });
                    }
                } else {
                    for (e, _) in &own_units {
                        commands.entity(*e).try_remove::<RetreatPolicy>();
                    }
                }
            }
            CmdAction::CyclePriority => {
                // The whole selection lands on the same preset, derived from
                // the first unit, so repeated presses stay in lock-step.
                match priority_component(doc.prio.next()) {
                    Some(priority) => {
                        for (e, _) in &own_units {
                            commands.entity(*e).try_insert(priority.clone());
                        }
                    }
                    None => {
                        for (e, _) in &own_units {
                            commands.entity(*e).try_remove::<TargetPriority>();
                        }
                    }
                }
            }
            CmdAction::ToggleAutoCast => {
                // Heroes only — a footman has nothing to auto-cast.
                if doc.autocast == 0 {
                    for (hero, _, _, _) in &own_heroes {
                        commands.entity(*hero).try_insert(AutoCastPolicy {
                            min_enemies: AUTOCAST_MIN_ENEMIES,
                        });
                    }
                } else {
                    for (hero, _, _, _) in &own_heroes {
                        commands.entity(*hero).try_remove::<AutoCastPolicy>();
                    }
                }
            }
            CmdAction::Train(kind) => {
                // One hero per team — never let a second one be queued, and
                // never a class the team's record doesn't name.
                if is_hero_kind(kind) {
                    if team_has_hero || hero_in_queue {
                        continue;
                    }
                    if records.get(Team::Human).is_some_and(|r| r.kind != kind) {
                        continue;
                    }
                }
                let mut iter = sel_buildings.iter_mut();
                let (first, second) = (iter.next(), iter.next());
                if second.is_some() {
                    continue;
                }
                let Some((_, building, team, Some(mut queue), uc, _, upgrading)) = first else {
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
                    let (g, l, _) = hero_train_cost(&records, Team::Human);
                    (g, l)
                } else {
                    let s = unit_stats(kind);
                    (s.cost_gold, s.cost_lumber)
                };
                let affordable = economies.get(Team::Human).can_afford(cost_gold, cost_lumber);
                // Economy pays when training starts; we only gate for UX.
                if affordable && queue.queue.len() < MAX_QUEUE {
                    queue.queue.push_back(kind);
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
    pressed_buttons: Query<(&Interaction, &El), Changed<Interaction>>,
    selected: Query<Entity, With<Selected>>,
    alive: Query<Entity, Or<(With<Unit>, With<Building>)>>,
    mut queues: Query<&mut TrainingQueue, With<Selected>>,
) {
    if game_over.0.is_some() {
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
                let mut iter = queues.iter_mut();
                let (first, second) = (iter.next(), iter.next());
                if second.is_some() {
                    continue;
                }
                let Some(mut queue) = first else { continue };
                if i < queue.queue.len() {
                    queue.queue.remove(i);
                    if i == 0 {
                        queue.progress = 0.0;
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Control groups (Ctrl+1..3 assign, 1..3 recall)
// ---------------------------------------------------------------------------

fn control_groups(
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    keys: Res<ButtonInput<KeyCode>>,
    selected: Query<Entity, With<Selected>>,
    alive: Query<&Team>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    const DIGITS: [(KeyCode, u8); 3] = [
        (KeyCode::Digit1, 1),
        (KeyCode::Digit2, 2),
        (KeyCode::Digit3, 3),
    ];

    for (key, slot) in DIGITS {
        if !keys.just_pressed(key) {
            continue;
        }
        if ctrl {
            let members: Vec<Entity> = selected.iter().collect();
            ui.groups.insert(slot, members);
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
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    buttons: Res<ButtonInput<MouseButton>>,
    game_over: Res<GameOver>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut focus: EventWriter<CameraFocus>,
    sel_units: Query<(Entity, &Team), (With<Selected>, With<Unit>)>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    if game_over.0.is_some() {
        ui.minimap_drag = false;
        return;
    }
    let rect = minimap_rect(window);
    let cursor = window.cursor_position();
    let inside = cursor.map_or(false, |c| rect.contains(c));

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
                (c.x - rect.min.x).clamp(0.0, MINIMAP_PX),
                (c.y - rect.min.y).clamp(0.0, MINIMAP_PX),
            );
            focus.write(CameraFocus {
                pos: minimap_to_world(uv),
            });
        }
    }

    // --- right click: context order at that world position -----------------
    if buttons.just_pressed(MouseButton::Right) && inside {
        let Some(c) = cursor else { return };
        let uv = Vec2::new(c.x - rect.min.x, c.y - rect.min.y);
        let ground = clamp_to_map(minimap_to_world(uv));

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
        issue_ground_order(&mut commands, &group, ground, attack_move);
    }
}

// ---------------------------------------------------------------------------
// Left mouse: selection, drag box, placement confirm, attack-move click
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn left_mouse(
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    nav: Res<NavGrid>,
    economies: Res<Economies>,
    game_over: Res<GameOver>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    units: Query<(Entity, &Transform, &Unit, &Team, Has<Selected>)>,
    buildings: Query<(Entity, &Transform, &Building, &Team, Has<Selected>)>,
    selected: Query<Entity, With<Selected>>,
    mut drag_node: Query<&mut Node, With<DragRect>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_q.single() else {
        return;
    };

    if game_over.0.is_some() {
        if let Ok(mut node) = drag_node.single_mut() {
            node.display = Display::None;
        }
        return;
    }

    let cursor = window.cursor_position();

    // ---- press ----------------------------------------------------------
    if buttons.just_pressed(MouseButton::Left) {
        if let Some(cursor) = cursor {
            if !cursor_over_hud(cursor, window, &ui) {
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
                                *sel && **t == Team::Human && u.kind == UnitKind::Worker
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
                            commands
                                .entity(worker)
                                .try_insert(Order::Build { kind, pos });
                            if chaining {
                                ui.wall_chain.push(worker);
                            } else {
                                ui.placement = None;
                            }
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
                        issue_ground_order(&mut commands, &group, ground, true);
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
            for (e, tf, _, team, _) in &buildings {
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
        if cursor_over_hud(cursor, window, &ui) {
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
            for (e, tf, b, team, _) in &buildings {
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
    mut commands: Commands,
    mut ui: ResMut<UiState>,
    buttons: Res<ButtonInput<MouseButton>>,
    game_over: Res<GameOver>,
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
) {
    if !buttons.just_pressed(MouseButton::Right) || game_over.0.is_some() {
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
    if cursor_over_hud(cursor, window, &ui) {
        return;
    }

    // Right-click on the world always cancels transient modes first.
    if ui.placement.is_some() || ui.attack_move_armed {
        ui.placement = None;
        ui.wall_chain.clear();
        ui.attack_move_armed = false;
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
        let target = if let Some((n, _)) = node {
            RallyTarget::Node(n)
        } else if let Some((u, _)) = own_unit {
            RallyTarget::Unit(u)
        } else {
            RallyTarget::Ground(clamp_to_map(ground))
        };
        for e in rally_buildings {
            commands.entity(e).try_insert(RallyPoint { target });
        }
        return;
    }

    // --- enemy under the cursor? -----------------------------------------
    if let Some((target, _)) = enemy {
        for (e, _, _) in &selected_units {
            commands.entity(*e).try_insert(Order::Attack(target));
        }
        return;
    }

    // --- resource node: workers harvest, everyone else walks over ---------
    if let Some((target, _)) = node {
        let mut movers = 0usize;
        let non_workers = selected_units
            .iter()
            .filter(|(_, k, _)| *k != UnitKind::Worker)
            .count();
        for (e, kind, _) in &selected_units {
            if *kind == UnitKind::Worker {
                commands.entity(*e).try_insert(Order::Harvest(target));
            } else {
                let p = clamp_to_map(ground + formation_offset(movers, non_workers));
                movers += 1;
                commands.entity(*e).try_insert(Order::Move(p));
            }
        }
        return;
    }

    // --- own town hall + loaded workers: drop the cargo off ---------------
    let carriers = selected_units.iter().filter(|(_, _, c)| *c).count();
    if own_depot.is_some() && carriers > 0 {
        let mut movers = 0usize;
        let others = selected_units.len() - carriers;
        for (e, _, carrying) in &selected_units {
            if *carrying {
                commands.entity(*e).try_insert(Order::ReturnResources);
            } else {
                let p = clamp_to_map(ground + formation_offset(movers, others));
                movers += 1;
                commands.entity(*e).try_insert(Order::Move(p));
            }
        }
        return;
    }

    // --- own unit under the cursor: the rest of the selection escorts it ---
    if let Some((leader, _)) = own_unit {
        let mut issued = false;
        for (e, _, _) in &selected_units {
            if *e == leader {
                continue; // the leader keeps whatever it was doing
            }
            commands.entity(*e).try_insert(Order::Follow(leader));
            issued = true;
        }
        if issued {
            return;
        }
        // Only the clicked unit was selected — fall through to a plain move.
    }

    // --- plain ground move with formation spread -------------------------
    let group: Vec<Entity> = selected_units.iter().map(|(e, _, _)| *e).collect();
    issue_ground_order(&mut commands, &group, ground, false);
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
    for (tf, node) in &nodes {
        let (size, color) = match node.kind {
            ResourceKind::Gold => (5.0, Color::srgb(1.0, 0.82, 0.25)),
            ResourceKind::Lumber => (2.0, Color::srgb(0.16, 0.42, 0.18)),
        };
        let p = world_to_minimap(tf.translation);
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
            MinimapStatic,
            ChildOf(root),
        ));
    }

    // Impassable terrain (none on the open map): dots slightly larger than a
    // nav cell so the barrier reads as one continuous wall.
    let rock = Color::srgb(0.26, 0.26, 0.30);
    for cell in crate::terrain::barrier_cells() {
        let p = world_to_minimap(cell);
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
            MinimapStatic,
            ChildOf(root),
        ));
    }
    *done = true;
}

/// Bounty caches: a bright-gold dot that pulses so it stands out from the
/// static gold-mine dots it shares a colour family with. Same pooled pattern as
/// `update_minimap` (mutate in place, never despawn) on its own small pool.
fn update_minimap_bounties(
    mut commands: Commands,
    time: Res<Time>,
    root: Query<Entity, With<MinimapRoot>>,
    bounties: Query<&Transform, With<Bounty>>,
    mut markers: Query<&mut Node, With<MinimapBounty>>,
) {
    let Ok(root) = root.single() else {
        return;
    };
    // 5px to 6px and back, ~2.5 rad/s — a slow, unmistakable throb.
    let size = 5.5 + 0.5 * (time.elapsed_secs() * 2.5).sin();
    let wanted: Vec<Vec2> = bounties
        .iter()
        .map(|tf| world_to_minimap(tf.translation))
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
    mut commands: Commands,
    root: Query<Entity, With<MinimapRoot>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    units: Query<(&Transform, &Team, Has<Hero>), (With<Unit>, Without<Building>)>,
    buildings: Query<(&Transform, &Team), (With<Building>, Without<Unit>)>,
    mut markers: Query<
        (&mut Node, &mut BackgroundColor),
        (With<MinimapMarker>, Without<MinimapViewport>),
    >,
    mut viewport: Query<&mut Node, (With<MinimapViewport>, Without<MinimapMarker>)>,
) {
    let Ok(root) = root.single() else {
        return;
    };

    // Desired dots: units first, then buildings (drawn later == on top).
    let mut wanted: Vec<(Vec2, f32, Color)> = Vec::new();
    for (tf, team, is_hero) in &units {
        // Heroes read as bigger, brighter dots.
        let (size, color) = if is_hero {
            (5.0, lighten(team.color(), 0.35))
        } else {
            (3.0, team.color())
        };
        wanted.push((world_to_minimap(tf.translation), size, color));
    }
    for (tf, team) in &buildings {
        wanted.push((
            world_to_minimap(tf.translation),
            6.0,
            lighten(team.color(), 0.12),
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
    let a = world_to_minimap(Vec3::new(min.x, 0.0, max.y));
    let b = world_to_minimap(Vec3::new(max.x, 0.0, min.y));
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
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_hud(
    mut ui: ResMut<UiState>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    game_over: Res<GameOver>,
    ai_controlled: Res<AiControlled>,
    // Latched the frame the match ends: was this an AI-vs-AI spectate?
    mut spectated: Local<Option<bool>>,
    mut texts: Query<(&Slot, &mut Text, &mut TextColor)>,
    mut nodes: Query<(&El, &mut Node)>,
    mut colors: Query<(&El, &mut BackgroundColor, Option<&Interaction>)>,
    // Read-only view of every hero on the map (one-hero rule, card labels, and
    // the Shop's customer + its inventory).
    heroes: Query<(&Team, Option<&Inventory>), With<Hero>>,
    // Read-only: which buildings the player has FINISHED (the tech gate).
    all_buildings: Query<(&Building, &Team, Has<UnderConstruction>)>,
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
        ),
        With<Selected>,
    >,
    sel_buildings: Query<
        (
            Entity,
            &Building,
            &Health,
            &Team,
            Option<&TrainingQueue>,
            Option<&UnderConstruction>,
            Option<&AbilityCooldown>,
            Option<&Upgrading>,
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
        if let Some((_, unit, health, team, carrying, hero, _, _, _, _, inventory, militia)) =
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
        if let Some((_, building, health, team, queue, under, _, upgrading)) =
            sel_buildings.iter().next()
        {
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
                } else if let Some(queue) = queue {
                    for kind in queue.queue.iter() {
                        queue_letters.push(initial(unit_name(*kind)));
                    }
                    if let Some(front) = queue.queue.front() {
                        let train = if is_hero_kind(*front) {
                            hero_train_cost(&records, Team::Human).2
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
        for (e, unit, health, team, _, hero, _, _, _, _, _, _) in &sel_units {
            cards.push(CardView {
                entity: e,
                // Heroes show "H<level>" instead of a plain initial.
                letter: match hero {
                    Some(hero) => format!("H{}", hero.level),
                    None => initial(unit_name(unit.kind)),
                },
                hp: (health.current / health.max.max(0.001)).clamp(0.0, 1.0),
                color: team.color(),
            });
        }
        for (e, building, health, team, _, _, _, _) in &sel_buildings {
            cards.push(CardView {
                entity: e,
                letter: initial(building_name(building.kind)),
                hp: (health.current / health.max.max(0.001)).clamp(0.0, 1.0),
                color: lighten(team.color(), 0.12),
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
        .filter(|(_, _, _, t, _, _, _, _, _, _, _, _)| **t == Team::Human)
        .count();
    let has_worker = sel_units
        .iter()
        .any(|(_, u, _, t, _, _, _, _, _, _, _, _)| *t == Team::Human && u.kind == UnitKind::Worker);
    // Same aggregate the input system builds, so caption/highlight and the
    // toggle that a click executes can never disagree.
    let doc = DoctrineState::of(&sorted_doctrine(
        sel_units
            .iter()
            .filter(|(_, _, _, t, _, _, _, _, _, _, _, _)| **t == Team::Human)
            .map(|(e, _, _, _, _, hero, leash, retreat, prio, autocast, _, _)| {
                (
                    e.index(),
                    UnitDoctrine::read(leash, retreat, prio, autocast, hero.is_some()),
                )
            })
            .collect(),
    ));
    let doctrine_line = doc.line();
    // The one selected own building: kind, finished, ability cooldown.
    let single = if building_count == 1 && unit_count == 0 {
        sel_buildings
            .iter()
            .next()
            .filter(|(_, _, _, t, _, _, _, _)| **t == Team::Human)
            .map(|(_, b, _, _, _, uc, cd, up)| {
                (b.kind, uc.is_none(), cd.map(|c| c.0), up.is_some())
            })
    } else {
        None
    };
    let single_building = single.map(|(kind, done, _, _)| (kind, done));

    // Hero commands: the ability of a selected hero (whichever class), the
    // train/revive button on a town hall while the team is hero-less, the
    // building's own ability, and the Shop's wares.
    let team_hero = heroes.iter().find(|(t, _)| **t == Team::Human);
    let team_has_hero = team_hero.is_some();
    let hero_in_queue = sel_buildings.iter().any(|(_, _, _, t, q, _, _, _)| {
        *t == Team::Human
            && q.map(|q| q.queue.iter().any(|k| is_hero_kind(*k)))
                .unwrap_or(false)
    });
    let selected_hero = sel_units
        .iter()
        .find(|(_, _, _, t, _, h, _, _, _, _, _, _)| **t == Team::Human && h.is_some());
    let hero_cmds = HeroCmds {
        train: (!team_has_hero && !hero_in_queue).then(|| {
            let (gold, lumber, _) = hero_train_cost(&records, Team::Human);
            HeroTrain {
                gold,
                lumber,
                recorded: records.get(Team::Human).map(|r| r.kind),
            }
        }),
        ability: selected_hero.and_then(|(_, u, _, _, _, h, _, _, _, _, _, _)| {
            let h = h?;
            ability_of_unit(u.kind)
                .map(|def| (def, hero_ability_ready(h, &def), h.ability_cooldown))
        }),
        building_ability: single.and_then(|(kind, done, cd, _)| {
            done.then(|| ability_of_building(kind))
                .flatten()
                .map(|def| (def, cd.unwrap_or(0.0)))
        }),
        upgrade: single.and_then(|(kind, done, _, upgrading)| {
            (done && !upgrading)
                .then(|| upgrade_cost(kind).zip(building_upgrades_to(kind)))
                .flatten()
                .map(|((gold, lumber, _), to)| (to, gold, lumber))
        }),
        shop: single.and_then(|(kind, done, _, _)| {
            (done && kind == BuildingKind::Shop).then(|| ShopState {
                hero: team_has_hero,
                room: team_hero
                    .and_then(|(_, inv)| inv)
                    .is_some_and(|inv| inv.0.iter().any(|s| s.is_none())),
            })
        }),
        items: selected_hero
            .and_then(|(_, _, _, _, _, _, _, _, _, _, inv, _)| inv.copied())
            .unwrap_or_default()
            .0,
    };
    let completed: Vec<BuildingKind> = all_buildings
        .iter()
        .filter(|(_, t, under)| **t == Team::Human && !under)
        .map(|(b, _, _)| b.kind)
        .collect();
    let entries = command_entries(
        own_units,
        has_worker,
        single_building,
        hero_cmds,
        doc,
        &completed,
    );

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
    let hints = if let Some(kind) = ui.placement {
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
    } else if ui.attack_move_armed {
        "Attack-move armed - left-click a destination (Esc cancels)".to_string()
    } else if total == 0 {
        "Left-click / drag to select.   Ctrl+1-3 set group, 1-3 recall.   '.' idle worker   F9: AI plays Blue   F12 x2: surrender   Minimap: left-click to look, right-click to order"
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
    match game_over.0 {
        Some(_) if spectated.is_none() => *spectated = Some(ai_controlled.human),
        None => *spectated = None,
        _ => {}
    }
    let (banner, banner_sub, banner_color) = match (game_over.0, spectated.unwrap_or(false)) {
        // AI vs AI: team-neutral result, no "you".
        (Some(Team::Human), true) => ("BLUE WINS", "AI vs AI", Color::srgb(0.45, 0.65, 1.0)),
        (Some(Team::Claude), true) => ("RED WINS", "AI vs AI", Color::srgb(1.0, 0.45, 0.35)),
        (Some(Team::Human), false) => ("VICTORY!", "You win", Color::srgb(0.45, 1.0, 0.5)),
        (Some(Team::Claude), false) => ("DEFEAT", "Claude wins", Color::srgb(1.0, 0.35, 0.3)),
        (None, _) => ("", "", Color::WHITE),
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
            Slot::BannerSub => text.0 = banner_sub.to_string(),
            Slot::PortraitLetter => text.0 = portrait_letter.clone(),
            Slot::Name => text.0 = name.clone(),
            Slot::Hp => text.0 = hp_text.clone(),
            Slot::Stats => text.0 = stats_text.clone(),
            Slot::Extra => text.0 = extra_text.clone(),
            Slot::Items => text.0 = items_text.clone(),
            Slot::Doctrine => text.0 = doctrine_line.clone(),
            Slot::Overflow => text.0 = overflow_text.clone(),
            Slot::CardLetter(i) => {
                text.0 = cards.get(i).map(|c| c.letter.clone()).unwrap_or_default();
            }
            Slot::QueueLetter(i) => {
                text.0 = queue_letters.get(i).cloned().unwrap_or_default();
            }
            Slot::CmdKey(i) => {
                text.0 = entries.get(i).map(|e| e.hotkey.to_string()).unwrap_or_default();
            }
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
    let workers_selected = selected.iter().any(|u| u.kind == UnitKind::Worker);

    let pickable = game_over.0.is_none() && state.placement.is_none() && !state.dragging;
    if pickable {
        if let (Some(cursor), Ok((cam, cam_tf))) = (window.cursor_position(), camera.single()) {
            if !cursor_over_hud(cursor, window, &state) {
                if let Some(ground) = cursor_to_ground(cam, cam_tf, cursor) {
                    // Closest unit first (units win ties against buildings),
                    // then buildings, then resource nodes.
                    let ray = cursor_ray(cam, cam_tf, cursor);
                    let mut best_unit: Option<(f32, Vec3, Team)> = None;
                    for (tf, team) in &units {
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
                            let r = building_stats(building.kind).size * 0.5;
                            let d = dist_xz(tf.translation, ground);
                            if d <= r && best_bld.is_none_or(|(bd, _, _, _)| d < bd) {
                                best_bld = Some((d, tf.translation, *team, r));
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
    if state.attack_move_armed && game_over.0.is_none() {
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
    mut surrenders: EventWriter<Surrender>,
) {
    if game_over.0.is_some() || !keys.just_pressed(KeyCode::F12) {
        return;
    }
    let now = time.elapsed_secs();
    match *armed_at {
        Some(t) if now - t < 3.0 => {
            surrenders.write(Surrender { team: Team::Human });
            *armed_at = None;
        }
        _ => {
            info!("Press F12 again within 3 seconds to surrender");
            *armed_at = Some(now);
        }
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
    if game_over.0.is_some() {
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
    mut ui: ResMut<UiState>,
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
    for event in feed.feed(Team::Human) {
        if event.seq <= notes.seen {
            continue;
        }
        notes.seen = event.seq;
        notes.live.push_front(Notice {
            message: event.message.clone(),
            severity: event.severity,
            pos: event.pos,
            born: now,
        });
        notes.focus_cursor = 0;
    }
    notes.live.truncate(NOTIF_SLOTS);

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
