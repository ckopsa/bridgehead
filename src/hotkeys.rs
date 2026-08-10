//! The hotkey registry: every key this game binds, in ONE table.
//!
//! # Why this module exists
//!
//! Until v3 the bindings lived as literals scattered through ui.rs —
//! `TRAIN_KEYS`, `SHOP_KEYS`, `HERO_ABILITY_KEYS`, a `build_card_slot` match, a
//! dozen inline `KeyCode::KeyG`s — and safety rested on a paragraph of prose per
//! site arguing that *this* letter is free on *that* card because a worker
//! selection and a single-building selection cannot both be on screen. Those
//! arguments were correct, but they were arguments: every A-Z letter is claimed
//! somewhere, so each new button meant re-deriving the whole disjointness proof
//! by hand, and two hand-written tests covered a sample of the cards rather than
//! all of them.
//!
//! This table replaces the prose with structure. A binding declares WHICH CARD
//! CONTEXT it can appear in ([`Ctx`]); [`validate`] walks every constructible
//! context and proves no key appears twice in any one of them. Reusing [Q] for
//! both a train slot and a shop shelf is still allowed — it is still the right
//! call — but it is now allowed *because the two tags never co-occur*, checked,
//! rather than because a comment says so.
//!
//! # How to add an action
//!
//! 1. Add a variant to [`Action`]. Name the SEMANTIC thing ("tier this building
//!    up"), never the key ("the U button") — the whole point is that the key is
//!    a property of the action, held in one place.
//! 2. Add a row to [`REGISTRY`] under the right `// ---- tag ----` heading:
//!    `b(Action::YourThing, KeyCode::KeyN, Ctx::WhereItAppears)`. Put it in the
//!    section for the context it belongs to; within a section, order is card
//!    order (the build rows ARE the build card's left-to-right order).
//! 3. If the new action can appear on a card no existing [`CardContext`]
//!    describes, add that context and its tag set too — a tag no context
//!    includes is never collision-checked, which
//!    [`every_tag_is_reachable_from_some_context`] refuses.
//! 4. Run the tests. [`the_registry_has_no_collision_in_any_card_context`] walks
//!    every context and will name the clash if the letter is taken. If it is,
//!    pick another letter — do NOT widen the context to make the check pass.
//! 5. Draw it: in ui.rs, `hotkeys::key(Action::YourThing)` gives the `KeyCode`
//!    (or `bind(..)` when the binding must exist), and the tile caption is
//!    derived from that `KeyCode` at draw time by [`key_caption`], so a caption
//!    cannot drift from its key. Nothing in ui.rs writes a card key literal.
//! 6. If the card it lands on now exceeds twelve tiles, that is fine and needs
//!    no action: `ui::paginate` gives it an overflow page reached with [Tab],
//!    and the hotkey stays live on every page of the card.
//!
//! There is deliberately no `data/hotkeys.ron`. Card layout is computed by pure
//! functions that the tests call without a `World` (`command_entries` and
//! friends), so a runtime-loaded table would either have to be threaded through
//! every one of them as an argument or read from a global — and the registry's
//! whole value is that it is checkable at compile/test time with no app running.
//! Stat tables are a different case and belong in RON; interface bindings are
//! not content.

use bevy::prelude::KeyCode;

use crate::shared::{
    abilities_of_building, building_placeable, building_researches, building_upgrades_to,
    trainable, BuildingKind, ALL_BUILDING_KINDS,
};

// ---------------------------------------------------------------------------
// Keys that other modules need in const context
// ---------------------------------------------------------------------------
//
// These are the registry's declaration for those bindings — the `REGISTRY` rows
// below reference the same consts, so there is still exactly one place each key
// is written down. They exist as named consts only because their users
// (`const DIGITS: [(KeyCode, u8); 3]`, a `just_pressed` in a system with no room
// for a lookup) want them before `key()` can be called.

/// One step out of whatever mode is innermost; finally, back to the orders page.
pub const CANCEL: KeyCode = KeyCode::Escape;
/// Cycle to the next idle worker and focus the camera on it.
pub const IDLE_WORKER: KeyCode = KeyCode::Period;
/// Jump the camera to the newest alert, then the one before it.
pub const FOCUS_ALERT: KeyCode = KeyCode::Space;
/// Approve / veto the top pending co-commander proposal.
pub const APPROVE_PROPOSAL: KeyCode = KeyCode::Enter;
pub const VETO_PROPOSAL: KeyCode = KeyCode::Backspace;
/// Surrender (pressed twice).
pub const SURRENDER: KeyCode = KeyCode::F12;
/// Next OVERFLOW page of the command card. See the paging note on [`Ctx`].
pub const NEXT_CARD_PAGE: KeyCode = KeyCode::Tab;
/// Control groups / squads 1-3, bare to recall, Ctrl to set, Shift to add.
pub const GROUP_DIGIT_KEYS: [(KeyCode, u8); 3] = [
    (KeyCode::Digit1, 1),
    (KeyCode::Digit2, 2),
    (KeyCode::Digit3, 3),
];

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// A semantic thing the player can do, independent of the key that does it.
///
/// Indexed variants (`TrainSlot(2)`, `HeroAbility(1)`) are *positions on a
/// card*, not specific content: the Barracks' third train slot and the
/// Workshop's third train slot are the same action, which is exactly why
/// production hotkeys are positional and stay that way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    // ---- global ----
    Cancel,
    IdleWorker,
    FocusAlert,
    ApproveProposal,
    VetoProposal,
    Surrender,
    /// Hand Blue to the AI and spectate (owned by ai.rs).
    AiTakeover,
    /// Game speed multiplier rungs, by index (owned by shared.rs).
    GameSpeed(usize),
    /// Camera pan (owned by terrain.rs).
    PanNorth,
    PanSouth,
    PanEast,
    PanWest,
    /// Control group / squad N.
    ControlGroup(u8),
    /// Next overflow page of the command card.
    NextCardPage,

    // ---- pinned to every card ----
    /// Flip between the orders card and the doctrine card.
    ModeToggle,

    // ---- orders ----
    AttackMove,
    Stop,

    // ---- worker-build ----
    /// Place a building. The order of these rows IS the build card's order.
    Build(BuildingKind),

    // ---- hero-cast ----
    HeroAbility(usize),
    ItemSlot(usize),

    // ---- production-train ----
    /// Train slot N of a production building — and, on a forge, research rung N.
    /// One action because they are one row of buttons in one position; a
    /// building that could do both would be a genuine collision and the
    /// validator would say so.
    TrainSlot(usize),

    // ---- building-ability ----
    BuildingAbility(usize),
    /// Convert this building into its next tier in place.
    TierUp,

    // ---- shop-shelf ----
    ShopSlot(usize),

    // ---- doctrine quick toggles (orders page, unit selection) ----
    QuickGuard,
    QuickFallback,
    QuickPriority,
    QuickAutoCast,

    // ---- doctrine page, unit selection ----
    PostureDefend,
    PosturePush,
    PostureForage,
    PostureEscort,
    StandDown,
    CycleFallback,
    CycleLeash,
    CyclePriority,
    AutoCastSlot(usize),
    /// Arm (or disarm) the `home-guard` trigger: when the base is attacked,
    /// this squad falls back and defends it. A preset, not an authoring UI —
    /// see docs/INTENT.md on the asymmetry this leaves open.
    HomeGuard,

    // ---- doctrine page, production building ----
    TemplateSquad,
    TemplateFallback,
    TemplatePriority,
    TemplateAutoCast,
    TemplateClear,

    // ---- free-entry nudges (doctrine page, raw keys, no tile) ----
    NudgeFallbackDown,
    NudgeFallbackUp,
    NudgeLeashDown,
    NudgeLeashUp,
}

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

/// The card-context tag a binding carries: the KIND of card it can appear on.
///
/// A [`CardContext`] is a set of these, and a key must be unique inside one
/// context. Two tags that never co-occur in any context may share letters
/// freely — that is how [Q] can be both a train slot and a shop shelf rung.
///
/// # Paging and context
///
/// The command card pages in two different senses, and they get different
/// rules — see [`CardContext`]:
///
/// * **Modes** (orders vs doctrine, the `[I]` toggle) are separate vocabularies
///   for the same selection. The mode IS part of the context, so keys may and do
///   repeat across it ([G] is Guard on the orders card and the leash radius on
///   the doctrine card). The hint line names the mode you are on.
/// * **Overflow pages** (the `[Tab]` pager) are one vocabulary that ran out of
///   tiles. They are the SAME context, so every key on them is unique — and
///   because they are, a hotkey stays live on every overflow page of the mode.
///   Only the tiles move; the keyboard never does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    /// Bound everywhere, on and off the card.
    Global,
    /// Reserved on every card, whichever mode is showing.
    Pinned,
    /// Any own-unit selection, orders page.
    Orders,
    /// A selection containing a worker.
    WorkerBuild,
    /// A selection containing a hero (abilities and carried items).
    HeroCast,
    /// One completed own building that trains units or researches ladders.
    ProductionTrain,
    /// One completed own building's own abilities and its tier-up.
    BuildingAbility,
    /// One completed own Shop's shelf.
    ShopShelf,
    /// Quick doctrine toggles, orders page, unit selection.
    DoctrineQuick,
    /// Doctrine page, unit selection.
    DoctrinePage,
    /// Doctrine page, production building (the template).
    DoctrineTemplate,
    /// Free-entry nudges: raw keys on the doctrine page with no tile of their
    /// own. Tagged so the validator checks them against the tiles beside them.
    Nudge,
}

/// One constructible card: a selection type crossed with a mode.
///
/// [`all`](CardContext::all) enumerates every one of them, which is what makes
/// [`validate`] systematic rather than a sample. Overflow pages are deliberately
/// NOT a dimension here: they are the same context by construction (see [`Ctx`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardContext {
    /// Nothing of ours selected — no card, but the global keys still fire.
    Empty,
    /// Own units, orders page.
    Units { worker: bool, hero: bool },
    /// Own units, doctrine page.
    UnitsDoctrine,
    /// Exactly one completed own building, orders page.
    Building(BuildingKind),
    /// Exactly one own production building, doctrine page (the template card).
    BuildingDoctrine,
}

impl CardContext {
    /// Every card a player can actually get on screen.
    pub fn all() -> Vec<CardContext> {
        let mut out = vec![CardContext::Empty, CardContext::UnitsDoctrine];
        for worker in [false, true] {
            for hero in [false, true] {
                out.push(CardContext::Units { worker, hero });
            }
        }
        for kind in ALL_BUILDING_KINDS {
            out.push(CardContext::Building(kind));
        }
        out.push(CardContext::BuildingDoctrine);
        out
    }

    /// Which tags can put a button on THIS card.
    ///
    /// The building arms read the shared tables rather than a list of kinds, so
    /// a Shop that ever learned to train units, or a hall that ever grew a
    /// shelf, would start colliding here the moment the table changed — which is
    /// the whole point of deriving the context instead of asserting it.
    pub fn tags(self) -> Vec<Ctx> {
        let mut tags = vec![Ctx::Global];
        match self {
            CardContext::Empty => {}
            CardContext::Units { worker, hero } => {
                tags.extend([Ctx::Pinned, Ctx::Orders, Ctx::DoctrineQuick]);
                if worker {
                    tags.push(Ctx::WorkerBuild);
                }
                if hero {
                    tags.push(Ctx::HeroCast);
                }
            }
            CardContext::UnitsDoctrine => {
                tags.extend([Ctx::Pinned, Ctx::DoctrinePage, Ctx::Nudge]);
            }
            CardContext::Building(kind) => {
                // The mode toggle is reserved on a building card even when this
                // particular kind cannot reach the doctrine page: [I] must never
                // mean two things, and "which buildings are template-capable" is
                // a content decision that should not be able to break a binding.
                tags.push(Ctx::Pinned);
                if !trainable(kind).is_empty() || !building_researches(kind).is_empty() {
                    tags.push(Ctx::ProductionTrain);
                }
                if !abilities_of_building(kind).is_empty() || building_upgrades_to(kind).is_some() {
                    tags.push(Ctx::BuildingAbility);
                }
                if sells_items(kind) {
                    tags.push(Ctx::ShopShelf);
                }
            }
            CardContext::BuildingDoctrine => {
                tags.extend([Ctx::Pinned, Ctx::DoctrineTemplate]);
            }
        }
        tags
    }

    /// Human name for an error message.
    pub fn name(self) -> String {
        match self {
            CardContext::Empty => "empty selection".to_string(),
            CardContext::Units { worker, hero } => format!(
                "unit selection (worker: {worker}, hero: {hero}), orders page"
            ),
            CardContext::UnitsDoctrine => "unit selection, doctrine page".to_string(),
            CardContext::Building(kind) => format!("{kind:?}, orders page"),
            CardContext::BuildingDoctrine => "production building, doctrine page".to_string(),
        }
    }
}

/// Does this building kind put a shelf of items on its card?
///
/// One place, so the context derivation and ui.rs's `HeroCmds::shop` cannot
/// disagree about which building is a shop.
pub fn sells_items(kind: BuildingKind) -> bool {
    matches!(kind, BuildingKind::Shop)
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One binding: what it does, which key does it, where it can appear.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub action: Action,
    pub key: KeyCode,
    pub ctx: Ctx,
}

const fn b(action: Action, key: KeyCode, ctx: Ctx) -> Binding {
    Binding { action, key, ctx }
}

/// **Every key the game binds.** See the module docs for how to add one.
///
/// Muscle memory is load-bearing and is not up for renegotiation here: [A]
/// attack-move, [S] stop, positional [Q][W][E][R][T] production, [B][F][H] for
/// the classic build trio, [R] for the hero's first spell. A binding in this
/// table may be MOVED only to resolve a collision the validator refuses, and
/// only after the alternative (a different letter for the NEW action) has been
/// ruled out.
pub const REGISTRY: &[Binding] = &[
    // ---- global ---------------------------------------------------------
    b(Action::Cancel, CANCEL, Ctx::Global),
    b(Action::IdleWorker, IDLE_WORKER, Ctx::Global),
    b(Action::FocusAlert, FOCUS_ALERT, Ctx::Global),
    b(Action::ApproveProposal, APPROVE_PROPOSAL, Ctx::Global),
    b(Action::VetoProposal, VETO_PROPOSAL, Ctx::Global),
    b(Action::Surrender, SURRENDER, Ctx::Global),
    b(Action::NextCardPage, NEXT_CARD_PAGE, Ctx::Global),
    b(Action::ControlGroup(1), KeyCode::Digit1, Ctx::Global),
    b(Action::ControlGroup(2), KeyCode::Digit2, Ctx::Global),
    b(Action::ControlGroup(3), KeyCode::Digit3, Ctx::Global),
    // Owned by other modules; registered so the validator sees the whole
    // keyboard rather than ui.rs's corner of it.
    b(Action::AiTakeover, KeyCode::F9, Ctx::Global),
    b(Action::GameSpeed(0), KeyCode::F1, Ctx::Global),
    b(Action::GameSpeed(1), KeyCode::F2, Ctx::Global),
    b(Action::GameSpeed(2), KeyCode::F3, Ctx::Global),
    b(Action::GameSpeed(3), KeyCode::F4, Ctx::Global),
    b(Action::PanNorth, KeyCode::ArrowUp, Ctx::Global),
    b(Action::PanSouth, KeyCode::ArrowDown, Ctx::Global),
    b(Action::PanEast, KeyCode::ArrowRight, Ctx::Global),
    b(Action::PanWest, KeyCode::ArrowLeft, Ctx::Global),
    // ---- pinned ---------------------------------------------------------
    // [I] is the one letter reserved on EVERY card, in every mode, because the
    // doctrine page is the only route to postures and templates and a route a
    // stray worker in the drag box can close is not a route.
    b(Action::ModeToggle, KeyCode::KeyI, Ctx::Pinned),
    // ---- orders ---------------------------------------------------------
    b(Action::AttackMove, KeyCode::KeyA, Ctx::Orders),
    b(Action::Stop, KeyCode::KeyS, Ctx::Orders),
    // ---- worker-build ---------------------------------------------------
    // In card order. [C] and [M] are building-ability letters borrowed by the
    // Blacksmith and the Sanctum: legal because `WorkerBuild` and
    // `BuildingAbility` never share a context, which the validator now checks
    // instead of the comment that used to say it.
    b(Action::Build(BuildingKind::Barracks), KeyCode::KeyB, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Farm), KeyCode::KeyF, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::TownHall), KeyCode::KeyH, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Tower), KeyCode::KeyO, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Wall), KeyCode::KeyL, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Workshop), KeyCode::KeyK, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Shop), KeyCode::KeyN, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Blacksmith), KeyCode::KeyC, Ctx::WorkerBuild),
    b(Action::Build(BuildingKind::Sanctum), KeyCode::KeyM, Ctx::WorkerBuild),
    // ---- hero-cast ------------------------------------------------------
    // [R] is where the one hero ability has always lived. Slot 3 is [D] rather
    // than [U] because the tier-up took [U]; the two are on disjoint cards, but
    // a hotkey the player has to think about is already broken.
    b(Action::HeroAbility(0), KeyCode::KeyR, Ctx::HeroCast),
    b(Action::HeroAbility(1), KeyCode::KeyY, Ctx::HeroCast),
    b(Action::HeroAbility(2), KeyCode::KeyD, Ctx::HeroCast),
    b(Action::ItemSlot(0), KeyCode::KeyZ, Ctx::HeroCast),
    b(Action::ItemSlot(1), KeyCode::KeyX, Ctx::HeroCast),
    // ---- production-train -----------------------------------------------
    // Positional, along one keyboard row. Research rungs reuse them on a forge
    // (which trains nothing) and the shelf reuses them on a Shop (which trains
    // nothing either) — both are `ProductionTrain`-free contexts.
    b(Action::TrainSlot(0), KeyCode::KeyQ, Ctx::ProductionTrain),
    b(Action::TrainSlot(1), KeyCode::KeyW, Ctx::ProductionTrain),
    b(Action::TrainSlot(2), KeyCode::KeyE, Ctx::ProductionTrain),
    b(Action::TrainSlot(3), KeyCode::KeyR, Ctx::ProductionTrain),
    b(Action::TrainSlot(4), KeyCode::KeyT, Ctx::ProductionTrain),
    // ---- building-ability -----------------------------------------------
    b(Action::BuildingAbility(0), KeyCode::KeyC, Ctx::BuildingAbility),
    b(Action::BuildingAbility(1), KeyCode::KeyJ, Ctx::BuildingAbility),
    b(Action::BuildingAbility(2), KeyCode::KeyM, Ctx::BuildingAbility),
    b(Action::TierUp, KeyCode::KeyU, Ctx::BuildingAbility),
    // ---- shop-shelf -----------------------------------------------------
    // The production row again, rung for rung. The fifth rung was [I] until the
    // registry landed: [I] is now reserved on every card for the mode toggle,
    // and a shelf that runs Q W E R and then jumps to I was a pattern break
    // besides. See `docs/` note in the bead — this is the ONE rebind v3 makes.
    b(Action::ShopSlot(0), KeyCode::KeyQ, Ctx::ShopShelf),
    b(Action::ShopSlot(1), KeyCode::KeyW, Ctx::ShopShelf),
    b(Action::ShopSlot(2), KeyCode::KeyE, Ctx::ShopShelf),
    b(Action::ShopSlot(3), KeyCode::KeyR, Ctx::ShopShelf),
    b(Action::ShopSlot(4), KeyCode::KeyT, Ctx::ShopShelf),
    // ---- doctrine quick toggles -----------------------------------------
    b(Action::QuickGuard, KeyCode::KeyG, Ctx::DoctrineQuick),
    b(Action::QuickFallback, KeyCode::KeyV, Ctx::DoctrineQuick),
    b(Action::QuickPriority, KeyCode::KeyP, Ctx::DoctrineQuick),
    b(Action::QuickAutoCast, KeyCode::KeyT, Ctx::DoctrineQuick),
    // ---- doctrine page, units -------------------------------------------
    b(Action::PostureDefend, KeyCode::KeyQ, Ctx::DoctrinePage),
    b(Action::PosturePush, KeyCode::KeyW, Ctx::DoctrinePage),
    b(Action::PostureForage, KeyCode::KeyE, Ctx::DoctrinePage),
    b(Action::PostureEscort, KeyCode::KeyR, Ctx::DoctrinePage),
    b(Action::StandDown, KeyCode::KeyT, Ctx::DoctrinePage),
    b(Action::CycleFallback, KeyCode::KeyF, Ctx::DoctrinePage),
    b(Action::CycleLeash, KeyCode::KeyG, Ctx::DoctrinePage),
    b(Action::CyclePriority, KeyCode::KeyP, Ctx::DoctrinePage),
    b(Action::AutoCastSlot(0), KeyCode::KeyZ, Ctx::DoctrinePage),
    b(Action::AutoCastSlot(1), KeyCode::KeyX, Ctx::DoctrinePage),
    b(Action::AutoCastSlot(2), KeyCode::KeyC, Ctx::DoctrinePage),
    b(Action::HomeGuard, KeyCode::KeyH, Ctx::DoctrinePage),
    // ---- doctrine page, production building ------------------------------
    b(Action::TemplateSquad, KeyCode::KeyQ, Ctx::DoctrineTemplate),
    b(Action::TemplateFallback, KeyCode::KeyW, Ctx::DoctrineTemplate),
    b(Action::TemplatePriority, KeyCode::KeyE, Ctx::DoctrineTemplate),
    b(Action::TemplateAutoCast, KeyCode::KeyR, Ctx::DoctrineTemplate),
    b(Action::TemplateClear, KeyCode::KeyT, Ctx::DoctrineTemplate),
    // ---- free-entry nudges ----------------------------------------------
    // Raw keys with no tile: the card is a menu of what you can do, the nudge is
    // a refinement of a value the card already shows. `-`/`=` and `[`/`]` are
    // the only adjacent unshifted pairs left, and they read as less/more.
    b(Action::NudgeFallbackDown, KeyCode::Minus, Ctx::Nudge),
    b(Action::NudgeFallbackUp, KeyCode::Equal, Ctx::Nudge),
    b(Action::NudgeLeashDown, KeyCode::BracketLeft, Ctx::Nudge),
    b(Action::NudgeLeashUp, KeyCode::BracketRight, Ctx::Nudge),
];

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// The key bound to an action, or `None` when the action has no rung (a fifth
/// train slot on a building with six units, say). Callers treat `None` as "this
/// button cannot be drawn", which is how a card degrades instead of panicking.
pub fn key(action: Action) -> Option<KeyCode> {
    REGISTRY
        .iter()
        .find(|bind| bind.action == action)
        .map(|bind| bind.key)
}

/// Every binding visible on a given card.
pub fn bindings_in(context: CardContext) -> Vec<Binding> {
    let tags = context.tags();
    REGISTRY
        .iter()
        .filter(|bind| tags.contains(&bind.ctx))
        .copied()
        .collect()
}

/// **The one place a key becomes a caption.** Tiles render this from
/// `entry.key` at draw time, so a caption cannot drift from the key that fires
/// it — the old `(KeyCode::KeyQ, "Q")` pairs made that drift a typo away.
pub fn key_caption(k: KeyCode) -> &'static str {
    match k {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F9 => "F9",
        KeyCode::F12 => "F12",
        KeyCode::Escape => "Esc",
        KeyCode::Space => "Space",
        KeyCode::Enter => "Enter",
        KeyCode::Backspace => "Bksp",
        KeyCode::Tab => "Tab",
        KeyCode::Period => ".",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::ArrowLeft => "Left",
        KeyCode::ArrowRight => "Right",
        // Not a placeholder to be filled in later: reaching this arm means a
        // binding was added whose key has no caption, and a tile that shows "?"
        // is a bug the `every_bound_key_has_a_caption` test refuses to ship.
        _ => "?",
    }
}

/// The build card, in card order, as the registry declares it. The row order in
/// [`REGISTRY`] IS the left-to-right order of the build buttons, so a new
/// building's position and its letter are one decision in one place.
pub fn build_order() -> Vec<(BuildingKind, KeyCode)> {
    REGISTRY
        .iter()
        .filter_map(|bind| match bind.action {
            Action::Build(kind) if building_placeable(kind) => Some((kind, bind.key)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The validator
// ---------------------------------------------------------------------------

/// Walk every constructible card context and prove no key appears twice in any
/// one of them.
///
/// This is the structural replacement for the per-site disjointness arguments
/// that used to live in ui.rs comments. It runs as a `debug_assert` when
/// `UiPlugin` is built (so a debug run of the game refuses to start with a
/// broken table) and as a test.
pub fn validate() -> Result<(), String> {
    for context in CardContext::all() {
        let mut seen: Vec<Binding> = Vec::new();
        for bind in bindings_in(context) {
            if let Some(prev) = seen.iter().find(|p| p.key == bind.key) {
                return Err(format!(
                    "{}: {} is bound to both {:?} ({:?}) and {:?} ({:?})",
                    context.name(),
                    key_caption(bind.key),
                    prev.action,
                    prev.ctx,
                    bind.action,
                    bind.ctx,
                ));
            }
            seen.push(bind);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The invariant the whole module exists for.** Every card the player can
    /// get on screen, every key on it, no duplicates — checked by construction
    /// rather than by three paragraphs of "these two selections are disjoint".
    #[test]
    fn the_registry_has_no_collision_in_any_card_context() {
        if let Err(e) = validate() {
            panic!("hotkey registry collision — {e}");
        }
    }

    /// The contexts really do enumerate the cards, rather than quietly covering
    /// none of them: every tag in `Ctx` must appear in at least one context, or
    /// a whole family of bindings is going unchecked.
    #[test]
    fn every_tag_is_reachable_from_some_context() {
        let reachable: Vec<Ctx> = CardContext::all()
            .into_iter()
            .flat_map(|c| c.tags())
            .collect();
        for bind in REGISTRY {
            assert!(
                reachable.contains(&bind.ctx),
                "{:?} sits in {:?}, which no card context includes — it would \
                 never be collision-checked",
                bind.action,
                bind.ctx,
            );
        }
    }

    /// A key with no caption draws a "?" on the tile. Cheaper to fail here.
    #[test]
    fn every_bound_key_has_a_caption() {
        for bind in REGISTRY {
            assert_ne!(
                key_caption(bind.key),
                "?",
                "{:?} is bound to {:?}, which has no caption in `key_caption`",
                bind.action,
                bind.key,
            );
        }
    }

    /// One action, one key. A duplicated `Action` row would make `key()` return
    /// whichever came first and silently orphan the other.
    #[test]
    fn no_action_is_registered_twice() {
        for (i, bind) in REGISTRY.iter().enumerate() {
            assert!(
                !REGISTRY[..i].iter().any(|p| p.action == bind.action),
                "{:?} is registered twice",
                bind.action,
            );
        }
    }

    /// Every placeable building has a build binding. A building with no button
    /// has no route in at all for a player at the keyboard — the failure mode
    /// the old silent `truncate` produced, now impossible to reach by omission
    /// as well.
    #[test]
    fn every_placeable_building_has_a_build_key() {
        for kind in ALL_BUILDING_KINDS {
            if !building_placeable(kind) {
                continue;
            }
            assert!(
                key(Action::Build(kind)).is_some(),
                "{kind:?} is placeable but has no build hotkey",
            );
        }
        // ...and nothing unplaceable claims one, which would waste a letter.
        for (kind, _) in build_order() {
            assert!(building_placeable(kind), "{kind:?} is not placeable");
        }
    }

    /// The muscle-memory bindings the doctrine protects. This test is a
    /// tripwire, not a description: changing any of these is a design decision
    /// that has to be argued for, and the diff should say so out loud.
    #[test]
    fn the_protected_bindings_are_where_they_have_always_been() {
        assert_eq!(key(Action::AttackMove), Some(KeyCode::KeyA));
        assert_eq!(key(Action::Stop), Some(KeyCode::KeyS));
        assert_eq!(key(Action::TrainSlot(0)), Some(KeyCode::KeyQ));
        assert_eq!(key(Action::TrainSlot(1)), Some(KeyCode::KeyW));
        assert_eq!(key(Action::TrainSlot(2)), Some(KeyCode::KeyE));
        assert_eq!(key(Action::TrainSlot(3)), Some(KeyCode::KeyR));
        assert_eq!(key(Action::TrainSlot(4)), Some(KeyCode::KeyT));
        assert_eq!(key(Action::HeroAbility(0)), Some(KeyCode::KeyR));
        assert_eq!(key(Action::Build(BuildingKind::Barracks)), Some(KeyCode::KeyB));
        assert_eq!(key(Action::Build(BuildingKind::Farm)), Some(KeyCode::KeyF));
        assert_eq!(key(Action::Build(BuildingKind::TownHall)), Some(KeyCode::KeyH));
        assert_eq!(key(Action::ModeToggle), Some(KeyCode::KeyI));
        assert_eq!(key(Action::TierUp), Some(KeyCode::KeyU));
    }

    /// The Shop's fifth rung is the one binding v3 moved, and it moved because
    /// [I] became reserved on every card. Pinned here so the change is a fact
    /// with a test rather than a line in a commit message.
    #[test]
    fn the_shop_shelf_runs_the_production_row_end_to_end() {
        for slot in 0..5 {
            assert_eq!(
                key(Action::ShopSlot(slot)),
                key(Action::TrainSlot(slot)),
                "shelf rung {slot} should sit on the production letter",
            );
        }
        assert_ne!(
            key(Action::ShopSlot(4)),
            Some(KeyCode::KeyI),
            "[I] is the mode toggle now, on every card including a Shop's",
        );
    }
}
