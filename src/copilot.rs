//! copilot.rs — co-command: one faction, two authors, negotiated directives.
//!
//! THESIS.md's last paragraph is the reason this file exists:
//!
//! > If this works, "playing with an AI" stops meaning *against a bot* or
//! > *carried by an aimbot* and starts meaning what it means with a friend:
//! > shared language, negotiated plans, complementary strengths, mutual
//! > legibility.
//!
//! Every earlier bead built the shared language. This one puts two speakers on
//! the same side of it.
//!
//! ## The seat
//!
//! `WC3_BRIDGE=copilot` opens **one** bridge seat attached to `Team::Human` —
//! not as the opponent, as a co-commander sitting next to the player. Its
//! snapshot is a `Team::Human` snapshot: same fog, same knowability, same
//! everything. There is no privileged view, because there is no second team to
//! have one of. That is the whole reason a co-commander is cheap: the hard
//! parts (one vocabulary, one compiler, one fog rule, provenance on every
//! order) were already paid for, and a second author is a second
//! `SubmitIntent` producer with a different `IntentSource`.
//!
//! bridge.rs still owns transport. This file owns the one thing that is
//! genuinely new: **what a second author is allowed to do without asking.**
//!
//! ## The real design question is not plumbing, it is conflict policy
//!
//! At the engine, conflict policy is already settled and does not change here:
//! `Order` is a component, so the last writer wins, and it has always been
//! overwrite-tolerant — that is how a human's right-click overrides doctrine's
//! push a second later. Making a co-commander's orders lose to the human's (or
//! win) would need a priority field on every order and would break the one
//! rule that makes the seats equitable: *source is descriptive, never
//! authoritative.*
//!
//! So the deliverable is not arbitration. It is **visibility plus consent**:
//!
//! 1. **Consent, before the fact.** Things that cannot be un-done cheaply —
//!    spending the human's gold, committing their army — arrive as
//!    **proposals**, not actions. They wait in a queue, on the human's screen,
//!    with the co-commander's stated reason and the compiled English of every
//!    command in the batch. The human approves, vetoes, or lets them lapse.
//! 2. **Visibility, after the fact.** Whatever the co-commander *does* do is
//!    stamped `by copilot` in `units[].why`, the selection panel and
//!    `intent_log.jsonl`, because `Cause::Order { source }` was already there.
//!    "Did my partner re-task my push?" is answered by selecting the push.
//!
//! ### Which half is which: the direct/propose split
//!
//! | | verbs | why |
//! |---|---|---|
//! | **direct** | `squad` `posture` `template` `priority` `retreat` `leash` `autocast` | doctrine is *advice-shaped already* |
//! | **propose** | every unit order, all production, all spending, `autopilot`, `surrender` | irreversible, or spends what is not the proposer's |
//!
//! The line is drawn where **the cost of being wrong** is. Vetoing a posture is
//! trivial: you set another one, and the squad re-tasks within a second — a
//! standing order is a *disposition*, and the engine is re-reading it
//! continuously anyway. Vetoing a spent 400 gold is impossible. So the
//! co-commander may keep the army fighting, holding fords, falling back at 35%
//! and focusing siege — all the machine-speed work THESIS.md's tempo argument
//! says belongs to the engine — while it may not empty the treasury unasked.
//!
//! That split is what makes it a partner rather than either a nag (propose
//! everything, and it cannot help during a fight) or a stranger with your
//! wallet (do everything, and "co-command" means "handed over").
//!
//! `WC3_COPILOT_TRUST` moves the line for experiments: `full` (everything
//! direct — the "I trust you, drive" mode) or `strict` (everything proposed,
//! including doctrine — the "show me your reasoning" mode).
//!
//! ### What a proposal is *not*
//!
//! It is not a queued order. Approval submits the batch through the ordinary
//! compiler, at approval time, against the world as it is *then* — so a
//! proposal whose units have since died is refused exactly as any stale
//! command is, with the same strings, into the same `errors` array. There is
//! no second execution path, and therefore no second set of rules to drift.

use crate::intent::IntentApply;
use crate::shared::*;
use bevy::prelude::*;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Tuning knobs
// ---------------------------------------------------------------------------

/// How the co-commander's direct (non-proposal) commands are treated.
const TRUST_ENV: &str = "WC3_COPILOT_TRUST";

/// Game seconds a proposal waits before lapsing. Long enough to read three
/// sentences mid-fight, short enough that an approval is still about the
/// battle the co-commander was looking at.
pub const PROPOSAL_TTL: f32 = 20.0;

/// Most proposals pending at once. The human has six alert rows and a battle
/// to fight; a co-commander that queues faster than its partner can read is
/// told so, rather than silently filling a buffer nobody will reach the
/// bottom of.
pub const MAX_PENDING: usize = 4;

/// A human direct order this recent still counts as "what my partner is doing
/// right now" when a proposal would overwrite it.
const CONFLICT_RECENT_S: f32 = 30.0;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CopilotPlugin;

/// Everything co-command does in a frame, as one set: it reads the wire
/// bridge.rs just polled and submits into the compiler bridge.rs is about to
/// snapshot, so a proposal approved this frame is applied this frame and its
/// errors ride out in the same `state.json`.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CopilotSet;

impl Plugin for CopilotPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Copilot::from_env())
            .add_event::<CopilotWire>()
            .add_event::<ProposalVerdict>()
            .add_systems(
                Update,
                (ingest_wire, resolve_proposals)
                    .chain()
                    .in_set(CopilotSet)
                    .after(crate::bridge::BridgePoll)
                    .before(IntentApply)
                    .run_if(copilot_seated),
            );
    }
}

fn copilot_seated(copilot: Res<Copilot>) -> bool {
    copilot.seat.is_some()
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// How much of the language the co-commander may speak without asking first.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TrustPolicy {
    /// The default, and the one the design argues for: doctrine direct,
    /// everything else proposed.
    #[default]
    Split,
    /// Everything direct. For experiments in what a fully trusted partner
    /// plays like — and, honestly, for measuring what the proposal loop costs.
    Full,
    /// Everything proposed, doctrine included. For watching a co-commander
    /// reason, one directive at a time.
    Strict,
}

impl TrustPolicy {
    fn from_env() -> Self {
        match std::env::var(TRUST_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "full" | "1" | "all" => TrustPolicy::Full,
            "strict" | "propose" | "ask" => TrustPolicy::Strict,
            _ => TrustPolicy::Split,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TrustPolicy::Split => "split",
            TrustPolicy::Full => "full",
            TrustPolicy::Strict => "strict",
        }
    }
}

/// One directive awaiting the human's verdict.
pub struct Proposal {
    /// Small, monotonic, and what the human's hotkeys and the co-commander's
    /// snapshot both name it by.
    pub id: u32,
    /// The co-commander's stated reason. This is the field that makes a
    /// proposal a *negotiation* rather than a confirmation dialog: the
    /// sentences say what would happen, the note says why it is worth doing.
    pub note: String,
    /// The batch, exactly as it will be submitted on approval.
    pub intents: Vec<Intent>,
    /// `Intent::sentence()` for each, compiled once at arrival. Free from the
    /// intent layer — the human reads the same English the replay log will.
    pub sentences: Vec<String>,
    /// What this batch would disturb, in the human's terms: their squads,
    /// their recent orders. See `conflict_tags`.
    pub conflicts: Vec<String>,
    pub proposed_at: f32,
    pub expires_at: f32,
    /// Somewhere on the map this is about, for `[Space]` to focus.
    pub pos: Option<Vec3>,
}

impl Proposal {
    pub fn expires_in(&self, now: f32) -> f32 {
        (self.expires_at - now).max(0.0)
    }
}

/// The co-command seat and its pending queue.
#[derive(Resource)]
pub struct Copilot {
    /// Which team a co-commander is seated on. `None` — the overwhelmingly
    /// common case — turns every system in this file off before it touches
    /// anything.
    pub seat: Option<Team>,
    pub policy: TrustPolicy,
    /// Oldest first: index 0 is the one closest to lapsing, and the one the
    /// human's approve/veto keys act on.
    pub pending: Vec<Proposal>,
    next_id: u32,
}

impl Copilot {
    fn from_env() -> Self {
        Copilot {
            seat: None,
            policy: TrustPolicy::from_env(),
            pending: Vec::new(),
            next_id: 1,
        }
    }

    /// Called by bridge.rs when it opens a copilot seat.
    pub fn seat(&mut self, team: Team) {
        self.seat = Some(team);
    }

    /// A seated co-command state holding `pending`, for tests in the modules
    /// that read this resource — ui.rs draws it and answers it, and wants to
    /// do so against a queue it built rather than one it had to drive a whole
    /// bridge to produce.
    #[cfg(test)]
    pub fn seated_with(team: Team, pending: Vec<Proposal>) -> Self {
        Copilot {
            seat: Some(team),
            policy: TrustPolicy::Split,
            pending,
            next_id: 1,
        }
    }
}

/// One raw command off a copilot seat's `commands.json`, still JSON.
///
/// bridge.rs does not parse these itself, because a copilot's wire has one
/// shape the ordinary seat's does not — the `propose` wrapper — and teaching
/// transport about negotiation would put half of this file in that one. The
/// bridge reads the file, honours `seq`, and hands the values over.
#[derive(Event, Clone, Debug)]
pub struct CopilotWire {
    pub team: Team,
    /// The historical `cmd <i>` prefix, so a co-commander greps its errors the
    /// same way every other seat does.
    pub tag: String,
    pub raw: serde_json::Value,
}

/// The human's answer to a proposal. Written by ui.rs (hotkey or click); this
/// file is the only reader.
#[derive(Event, Clone, Copy, Debug)]
pub struct ProposalVerdict {
    pub id: u32,
    pub approve: bool,
}

/// The `propose` wrapper, on the wire.
///
/// Deliberately not an `Intent` variant: a proposal is not something a player
/// can *mean*, it is a thing one author says to another about a batch of
/// meanings. Putting it in the vocabulary would have handed the human's
/// interface a verb with nothing to compile and given the compiler a case that
/// changes no game state.
#[derive(Deserialize)]
struct ProposeWire {
    #[serde(default)]
    commands: Vec<serde_json::Value>,
    #[serde(default)]
    note: String,
}

// ---------------------------------------------------------------------------
// Which verbs need asking
// ---------------------------------------------------------------------------

/// Is this a doctrine verb — standing policy the engine executes at machine
/// speed for whoever set it?
///
/// The list is the same seven docs/INTENT.md groups under "Doctrine", and that
/// is not a coincidence: the grouping already encodes the property the trust
/// split needs, which is that these verbs install a *disposition* rather than
/// perform an act. Nothing here spends a resource or moves a unit; doctrine.rs
/// reads them next tick and will read whatever replaces them the tick after.
pub fn is_doctrine_verb(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::Priority { .. }
            | Intent::Retreat { .. }
            | Intent::Leash { .. }
            | Intent::Autocast { .. }
            | Intent::Squad { .. }
            | Intent::Posture { .. }
            | Intent::Template { .. }
    )
}

/// The seven doctrine verbs by name, for the snapshot to tell the seat what it
/// may say without asking. Kept honest against `is_doctrine_verb` by
/// `the_advertised_direct_verbs_are_the_ones_that_pass`.
pub const DOCTRINE_VERBS: [&str; 7] = [
    "priority", "retreat", "leash", "autocast", "squad", "posture", "template",
];

/// What this seat may send directly, as the snapshot reports it. A
/// co-commander should learn its own etiquette by reading, not by being told
/// out of band — the same principle that makes the catalog the tech tree.
pub fn direct_verbs(policy: TrustPolicy) -> Vec<&'static str> {
    match policy {
        TrustPolicy::Full => vec!["*"],
        TrustPolicy::Strict => Vec::new(),
        TrustPolicy::Split => DOCTRINE_VERBS.to_vec(),
    }
}

/// May the co-commander submit this without the human's approval?
pub fn direct_allowed(policy: TrustPolicy, intent: &Intent) -> bool {
    match policy {
        TrustPolicy::Full => true,
        TrustPolicy::Strict => false,
        TrustPolicy::Split => is_doctrine_verb(intent),
    }
}

/// The refusal a direct command gets when the policy wants it proposed.
///
/// It names the verb and shows the wrapper, because the co-commander is a
/// model reading an error string mid-match and "permission denied" would cost
/// it a round trip to the brief.
fn needs_proposal_error(tag: &str, intent: &Intent, policy: TrustPolicy) -> String {
    let why = match policy {
        TrustPolicy::Strict => "this seat is in strict trust mode",
        _ => "it spends or commits what your partner owns",
    };
    format!(
        "{tag}: '{}' needs the human's approval — {why}. Wrap it: \
         {{\"type\":\"propose\",\"commands\":[…],\"note\":\"why\"}}",
        intent.verb()
    )
}

// ---------------------------------------------------------------------------
// Ingest: wire -> direct submission or pending proposal
// ---------------------------------------------------------------------------

type CopilotUnits<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Team,
        Option<&'static SquadId>,
        Option<&'static Provenance>,
    ),
    With<Unit>,
>;

#[allow(clippy::too_many_arguments)]
fn ingest_wire(
    mut wire: EventReader<CopilotWire>,
    time: Res<Time>,
    mut copilot: ResMut<Copilot>,
    mut errors: ResMut<IntentErrors>,
    mut feed: ResMut<GameEvents>,
    mut submissions: EventWriter<SubmitIntent>,
    squad_orders: Res<SquadOrders>,
    units: CopilotUnits,
) {
    let now = time.elapsed_secs();
    let policy = copilot.policy;
    for CopilotWire { team, tag, raw } in wire.read().cloned() {
        // A `propose` wrapper, or a bare command? One field decides, and an
        // ordinary command is untouched by the check — which is what keeps
        // `tools/bridge_send.py` working at this seat with no changes.
        let is_propose = raw.get("type").and_then(|t| t.as_str()) == Some("propose");
        if !is_propose {
            match serde_json::from_value::<Intent>(raw) {
                Ok(intent) if direct_allowed(policy, &intent) => {
                    submissions.write(SubmitIntent {
                        team,
                        source: IntentSource::Copilot,
                        tag,
                        intent,
                    });
                }
                Ok(intent) => {
                    errors
                        .get_mut(team)
                        .push(needs_proposal_error(&tag, &intent, policy));
                }
                Err(err) => errors
                    .get_mut(team)
                    .push(format!("{tag}: unrecognized command ({err})")),
            }
            continue;
        }

        let wrapper: ProposeWire = match serde_json::from_value(raw) {
            Ok(wrapper) => wrapper,
            Err(err) => {
                errors
                    .get_mut(team)
                    .push(format!("{tag}: malformed proposal ({err})"));
                continue;
            }
        };
        if wrapper.commands.is_empty() {
            errors
                .get_mut(team)
                .push(format!("{tag}: proposal carries no commands"));
            continue;
        }
        if copilot.pending.len() >= MAX_PENDING {
            errors.get_mut(team).push(format!(
                "{tag}: proposal queue full ({MAX_PENDING} pending) — \
                 your partner has not answered the earlier ones yet"
            ));
            continue;
        }
        // One malformed command sinks only itself, exactly as in an ordinary
        // batch. What is left still makes a proposal: a co-commander that
        // fumbled its third command should not lose the other two, and the
        // human sees the sentences of what actually survived.
        let mut intents: Vec<Intent> = Vec::with_capacity(wrapper.commands.len());
        for (j, sub) in wrapper.commands.iter().enumerate() {
            match serde_json::from_value::<Intent>(sub.clone()) {
                Ok(intent) => intents.push(intent),
                Err(err) => errors
                    .get_mut(team)
                    .push(format!("{tag}.{j}: unrecognized command ({err})")),
            }
        }
        if intents.is_empty() {
            continue;
        }

        let id = copilot.next_id;
        copilot.next_id += 1;
        let sentences: Vec<String> = intents.iter().map(Intent::sentence).collect();
        let conflicts = conflict_tags(team, &intents, &squad_orders, &units, now);
        let pos = intents.iter().find_map(intent_pos);
        let note = if wrapper.note.trim().is_empty() {
            "(no reason given)".to_string()
        } else {
            wrapper.note.trim().to_string()
        };
        // The arrival is news, on the channel the human already watches — and
        // the same push lands in the co-commander's own `events`, which is how
        // it learns the proposal was received rather than dropped.
        feed.push(
            team,
            now,
            format!("copilot proposes #{id}: {note}"),
            EventSeverity::Info,
            pos,
        );
        copilot.pending.push(Proposal {
            id,
            note,
            intents,
            sentences,
            conflicts,
            proposed_at: now,
            expires_at: now + PROPOSAL_TTL,
            pos,
        });
    }
}

// ---------------------------------------------------------------------------
// Conflict visibility
// ---------------------------------------------------------------------------

/// Which of the issuer's own units a verb is about.
enum Scope {
    Units(Vec<IntentId>),
    Squad(u8),
    Nothing,
}

fn scope_of(intent: &Intent) -> Scope {
    match intent {
        Intent::Move { units, .. }
        | Intent::AttackMove { units, .. }
        | Intent::Attack { units, .. }
        | Intent::Harvest { units, .. }
        | Intent::Return { units }
        | Intent::Follow { units, .. }
        | Intent::Stop { units }
        | Intent::Priority { units, .. }
        | Intent::Retreat { units, .. }
        | Intent::Leash { units, .. }
        | Intent::Autocast { units, .. }
        | Intent::Squad { units, .. } => Scope::Units(units.clone()),
        Intent::Build { worker, .. } => Scope::Units(vec![*worker]),
        Intent::Posture { id, .. } => Scope::Squad(*id),
        _ => Scope::Nothing,
    }
}

/// Somewhere on the map a proposal is about, so `[Space]` can go look at it.
fn intent_pos(intent: &Intent) -> Option<Vec3> {
    let (x, z) = match intent {
        Intent::Move { x, z, .. }
        | Intent::AttackMove { x, z, .. }
        | Intent::Build { x, z, .. }
        | Intent::Leash {
            x: Some(x),
            z: Some(z),
            ..
        }
        | Intent::Retreat {
            x: Some(x),
            z: Some(z),
            ..
        } => (*x, *z),
        Intent::Posture {
            posture: Some(posture),
            ..
        } => match posture {
            PostureIntent::Defend { x, z, .. }
            | PostureIntent::Push { x, z }
            | PostureIntent::Forage { x, z } => (*x, *z),
            PostureIntent::Escort { .. } => return None,
        },
        _ => return None,
    };
    Some(Vec3::new(x, 0.0, z))
}

/// What this batch would step on, phrased in the human's own terms.
///
/// **This is the deliverable of the conflict policy**, not arbitration. The
/// engine's rule stays last-writer-wins, because that is what it has always
/// been and what makes a right-click able to override doctrine at all. What
/// was missing was not a referee — it was *knowing*. A partner who re-tasks
/// your push is a partner; a partner who re-tasks your push invisibly is a
/// bug you will spend the next minute misdiagnosing.
///
/// Provenance is what makes this nearly free. Every unit already carries who
/// gave it its current reason and when (`Cause::Order { source, .. }`), and
/// every squad already carries its standing posture, so "would this disturb
/// something the human set?" is a lookup rather than a new bookkeeping system.
///
/// Computed once, when the proposal arrives — it describes the board the
/// co-commander was looking at when it wrote the directive, which is the thing
/// the note is arguing about. (A tag can therefore go stale during the 20s
/// window; the sentences cannot, and approval re-validates everything against
/// the live world anyway.)
fn conflict_tags(
    me: Team,
    intents: &[Intent],
    squad_orders: &SquadOrders,
    units: &CopilotUnits,
    now: f32,
) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut squads_touched: Vec<u8> = Vec::new();
    // Human orders this batch would overwrite: verb -> how many units, and the
    // freshest one, because "4 seconds ago" is the part that stings.
    let mut overridden: Vec<(&'static str, usize, f32)> = Vec::new();

    for intent in intents {
        match scope_of(intent) {
            Scope::Squad(id) => {
                if let Some(old) = squad_orders.0.get(&(me, id)) {
                    let new = match intent {
                        Intent::Posture {
                            posture: Some(p), ..
                        } => posture_word_intent(p),
                        _ => "cleared",
                    };
                    let old = posture_word(old);
                    if old != new {
                        push_unique(
                            &mut tags,
                            format!("changes squad {id}: {old} -> {new}"),
                        );
                    }
                }
            }
            Scope::Units(ids) => {
                for id in ids {
                    let Some(entity) = intent_entity(id) else {
                        continue;
                    };
                    let Ok((_, team, squad, why)) = units.get(entity) else {
                        continue;
                    };
                    if *team != me {
                        continue;
                    }
                    // In a squad the human gave a standing posture to.
                    if let Some(SquadId(squad)) = squad {
                        if squad_orders.0.contains_key(&(me, *squad))
                            && !squads_touched.contains(squad)
                        {
                            squads_touched.push(*squad);
                        }
                    }
                    // Under a direct order the human gave recently.
                    if let Some(Provenance {
                        cause:
                            Cause::Order {
                                verb,
                                source: IntentSource::Ui,
                            },
                        at,
                    }) = why
                    {
                        let age = now - at;
                        if age <= CONFLICT_RECENT_S {
                            match overridden.iter_mut().find(|(v, _, _)| v == verb) {
                                Some((_, n, freshest)) => {
                                    *n += 1;
                                    *freshest = freshest.min(age);
                                }
                                None => overridden.push((verb, 1, age)),
                            }
                        }
                    }
                }
            }
            Scope::Nothing => {}
        }
    }

    squads_touched.sort_unstable();
    for squad in squads_touched {
        let posture = squad_orders
            .0
            .get(&(me, squad))
            .map(posture_word)
            .unwrap_or("no posture");
        push_unique(&mut tags, format!("re-tasks squad {squad} ({posture})"));
    }
    overridden.sort_by(|a, b| a.2.total_cmp(&b.2));
    for (verb, n, age) in overridden {
        push_unique(
            &mut tags,
            format!("overrides your {verb} on {n} unit(s), {age:.0}s ago"),
        );
    }
    tags
}

fn push_unique(tags: &mut Vec<String>, tag: String) {
    if !tags.contains(&tag) {
        tags.push(tag);
    }
}

fn posture_word(posture: &SquadPosture) -> &'static str {
    match posture {
        SquadPosture::Defend { .. } => "defend",
        SquadPosture::Push { .. } => "push",
        SquadPosture::Escort { .. } => "escort",
        SquadPosture::Forage { .. } => "forage",
    }
}

fn posture_word_intent(posture: &PostureIntent) -> &'static str {
    match posture {
        PostureIntent::Defend { .. } => "defend",
        PostureIntent::Push { .. } => "push",
        PostureIntent::Escort { .. } => "escort",
        PostureIntent::Forage { .. } => "forage",
    }
}

// ---------------------------------------------------------------------------
// Resolve: approve, veto, lapse
// ---------------------------------------------------------------------------

/// Apply the human's verdicts, then expire whatever they did not answer.
///
/// Approval submits through the ordinary compiler with `IntentSource::Copilot`
/// — the same path, the same validation, the same fog rule, the same error
/// strings. That is the point of having spent four beads collapsing the
/// mutation paths into one: co-command needed no new applier, and therefore
/// has no second set of rules that can drift from the first.
fn resolve_proposals(
    mut verdicts: EventReader<ProposalVerdict>,
    time: Res<Time>,
    game_over: Res<GameOver>,
    mut copilot: ResMut<Copilot>,
    mut feed: ResMut<GameEvents>,
    mut submissions: EventWriter<SubmitIntent>,
) {
    let now = time.elapsed_secs();
    let Some(team) = copilot.seat else { return };

    // A finished match answers every outstanding question at once.
    if game_over.0.is_some() {
        copilot.pending.clear();
        return;
    }

    for verdict in verdicts.read().copied() {
        let Some(i) = copilot.pending.iter().position(|p| p.id == verdict.id) else {
            // Already approved, vetoed or lapsed — a double-tap on a key, not
            // an error worth telling anyone about.
            continue;
        };
        let proposal = copilot.pending.remove(i);
        if !verdict.approve {
            feed.push(
                team,
                now,
                format!("proposal #{} vetoed: {}", proposal.id, proposal.note),
                EventSeverity::Info,
                None,
            );
            continue;
        }
        for (j, intent) in proposal.intents.into_iter().enumerate() {
            submissions.write(SubmitIntent {
                team,
                source: IntentSource::Copilot,
                // Names the proposal AND the command inside it, so a rejection
                // 20 seconds after the batch was written is still traceable to
                // the directive that asked for it.
                tag: format!("prop {} cmd {j}", proposal.id),
                intent,
            });
        }
        feed.push(
            team,
            now,
            format!(
                "proposal #{} approved ({} order(s)): {}",
                proposal.id,
                proposal.sentences.len(),
                proposal.note
            ),
            EventSeverity::Info,
            proposal.pos,
        );
    }

    // Lapsing is a real answer, and it is the *safe* one: silence never spends
    // gold. It is reported rather than silent, because a co-commander that
    // cannot tell "vetoed" from "not seen" will either nag or give up.
    let mut lapsed: Vec<(u32, String)> = Vec::new();
    copilot.pending.retain(|p| {
        if p.expires_at > now {
            return true;
        }
        lapsed.push((p.id, p.note.clone()));
        false
    });
    for (id, note) in lapsed {
        feed.push(
            team,
            now,
            format!("proposal #{id} expired unanswered: {note}"),
            EventSeverity::Warning,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{IntentLog, IntentPlugin};

    /// The whole co-command loop over a real world: the negotiation layer
    /// AND the compiler it submits into. Nothing here is a stand-in — an
    /// approved proposal takes exactly the path a live match takes.
    fn co_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
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
            .add_event::<CastAbility>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .add_plugins((IntentPlugin, CopilotPlugin));
        // A unit test must not depend on `WC3_INTENT_LOG` or leave a file.
        app.insert_resource(IntentLog::disabled());
        // Nor on `WC3_COPILOT_TRUST`: the policy under test is set per test.
        {
            let mut copilot = app.world_mut().resource_mut::<Copilot>();
            copilot.policy = TrustPolicy::Split;
            copilot.seat(Team::Human);
        }
        app
    }

    fn footman(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(at),
                Health::new(100.0),
                Order::Idle,
            ))
            .id()
    }

    /// Put one raw wire command in front of the seat and run a frame.
    fn wire(app: &mut App, json: &str) {
        let raw: serde_json::Value = serde_json::from_str(json).expect("test json");
        app.world_mut().send_event(CopilotWire {
            team: Team::Human,
            tag: "cmd 0".to_string(),
            raw,
        });
        app.update();
    }

    fn errors(app: &App) -> Vec<String> {
        app.world().resource::<IntentErrors>().get(Team::Human).clone()
    }

    fn pending(app: &App) -> &[Proposal] {
        &app.world().resource::<Copilot>().pending
    }

    fn why_of(app: &App, entity: Entity) -> String {
        app.world()
            .entity(entity)
            .get::<Provenance>()
            .map(Provenance::why)
            .unwrap_or_else(|| NO_PROVENANCE.to_string())
    }

    /// **The bead, end to end.** A co-commander proposes; the batch waits with
    /// its note and its compiled English; the human approves; the order lands
    /// on a real unit carrying a real provenance stamp that names the partner.
    ///
    /// Every assertion here is a link in the chain the design claims: the
    /// sentence the human read is the sentence the log writes, and the unit's
    /// answer to "why are you doing that?" says which of the two authors moved
    /// it — which is the whole readout co-command needed and, because
    /// `Cause::Order { source }` already existed, cost nothing to build.
    #[test]
    fn a_proposal_waits_then_lands_stamped_by_the_copilot() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));

        wire(
            &mut app,
            &format!(
                r#"{{"type":"propose","note":"the ford is open — take it now",
                     "commands":[{{"type":"move","units":[{}],"x":20.0,"z":30.0}}]}}"#,
                intent_id(unit)
            ),
        );

        // Nothing happened to the world yet. That is the point of a proposal:
        // it is a sentence, not an act.
        assert_eq!(pending(&app).len(), 1, "one directive is waiting");
        assert!(
            app.world().entity(unit).get::<Order>().is_some_and(|o| matches!(o, Order::Idle)),
            "a pending proposal must not move anything"
        );
        let proposal = &pending(&app)[0];
        assert_eq!(proposal.id, 1);
        assert_eq!(proposal.note, "the ford is open — take it now");
        assert_eq!(
            proposal.sentences,
            vec![format!("move unit {} to (20.0, 30.0)", intent_id(unit))],
            "the human reads the same English the replay log will write"
        );
        assert!(proposal.conflicts.is_empty(), "it steps on nothing yet");
        // The arrival is news on the human's channel, not a silent queue push.
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .any(|e| e.message.contains("copilot proposes #1")),
            "the human is told a directive arrived"
        );

        // The human says yes.
        app.world_mut().send_event(ProposalVerdict { id: 1, approve: true });
        app.update();

        assert!(pending(&app).is_empty(), "answered proposals leave the queue");
        assert!(
            matches!(app.world().entity(unit).get::<Order>(), Some(Order::Move(_))),
            "approval submits through the ordinary compiler"
        );
        // The attribution that makes two authors legible to each other.
        assert_eq!(why_of(&app, unit), "order:move by copilot t=0");
        assert!(errors(&app).is_empty(), "a clean batch is refused nothing");
    }

    /// A veto is not a delay. Nothing is submitted, ever — and the seat learns
    /// the answer through the ordinary event feed rather than by inferring it
    /// from a queue that quietly emptied.
    #[test]
    fn a_veto_submits_nothing_and_says_so() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        wire(
            &mut app,
            &format!(
                r#"{{"type":"propose","note":"push mid","commands":[
                     {{"type":"move","units":[{}],"x":5.0,"z":5.0}}]}}"#,
                intent_id(unit)
            ),
        );
        app.world_mut().send_event(ProposalVerdict { id: 1, approve: false });
        app.update();

        assert!(pending(&app).is_empty());
        assert_eq!(why_of(&app, unit), NO_PROVENANCE, "the unit was never touched");
        assert!(app
            .world()
            .resource::<GameEvents>()
            .feed(Team::Human)
            .iter()
            .any(|e| e.message.contains("proposal #1 vetoed")));
    }

    /// Silence is the safe answer, and it is still an answer: a proposal that
    /// lapses is reported, because a co-commander that cannot tell "vetoed"
    /// from "not seen" will either nag or give up.
    #[test]
    fn an_unanswered_proposal_lapses_and_is_reported() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        wire(
            &mut app,
            &format!(
                r#"{{"type":"propose","note":"expand north","commands":[
                     {{"type":"move","units":[{}],"x":5.0,"z":5.0}}]}}"#,
                intent_id(unit)
            ),
        );
        assert_eq!(pending(&app).len(), 1);

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PROPOSAL_TTL + 1.0));
        app.update();

        assert!(pending(&app).is_empty(), "it lapsed");
        assert_eq!(why_of(&app, unit), NO_PROVENANCE, "lapsing spends nothing");
        assert!(app
            .world()
            .resource::<GameEvents>()
            .feed(Team::Human)
            .iter()
            .any(|e| e.message.contains("proposal #1 expired unanswered")));
    }

    /// The trust split, live. A posture is advice and goes straight through; a
    /// `train` spends the human's gold and comes back with the wrapper.
    #[test]
    fn doctrine_goes_direct_and_production_is_bounced() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        app.world_mut().entity_mut(unit).insert(SquadId(1));

        wire(
            &mut app,
            r#"{"type":"posture","id":1,"posture":{"type":"push","x":40.0,"z":40.0}}"#,
        );
        assert!(pending(&app).is_empty(), "doctrine does not queue up");
        assert!(
            app.world()
                .resource::<SquadOrders>()
                .0
                .contains_key(&(Team::Human, 1)),
            "the posture is installed, this second, with no approval"
        );

        wire(&mut app, r#"{"type":"train","building":1,"unit":"Footman"}"#);
        assert!(pending(&app).is_empty(), "a bounced command is not a proposal");
        let err = errors(&app).join(" | ");
        assert!(
            err.contains("'train' needs the human's approval"),
            "got {err}"
        );
    }

    /// Full trust removes the loop entirely — the experiment mode, and the
    /// control the split has to be measured against.
    #[test]
    fn full_trust_lets_production_through() {
        let mut app = co_app();
        app.world_mut().resource_mut::<Copilot>().policy = TrustPolicy::Full;
        let unit = footman(&mut app, Vec3::ZERO);

        wire(
            &mut app,
            &format!(
                r#"{{"type":"move","units":[{}],"x":8.0,"z":9.0}}"#,
                intent_id(unit)
            ),
        );
        assert!(pending(&app).is_empty());
        assert_eq!(why_of(&app, unit), "order:move by copilot t=0");
    }

    /// **The conflict readout.** A proposal that would overwrite what the human
    /// is already doing has to SAY SO, in the human's terms, before they
    /// approve it — that is the deliverable of the conflict policy, since the
    /// engine's own rule (last writer wins) is deliberately unchanged.
    #[test]
    fn a_proposal_names_the_squad_and_the_order_it_would_overwrite() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        // The human put this unit in squad 1, gave squad 1 a push, and
        // right-clicked it somewhere five seconds ago.
        app.world_mut().entity_mut(unit).insert((
            SquadId(1),
            Provenance::new(
                Cause::Order { verb: "move", source: IntentSource::Ui },
                0.0,
            ),
        ));
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: Vec3::ZERO });

        wire(
            &mut app,
            &format!(
                r#"{{"type":"propose","note":"fall back, we are losing this",
                     "commands":[{{"type":"move","units":[{}],"x":-70.0,"z":-70.0}}]}}"#,
                intent_id(unit)
            ),
        );

        let conflicts = &pending(&app)[0].conflicts;
        assert!(
            conflicts.iter().any(|c| c == "re-tasks squad 1 (push)"),
            "got {conflicts:?}"
        );
        assert!(
            conflicts
                .iter()
                .any(|c| c.starts_with("overrides your move on 1 unit(s)")),
            "got {conflicts:?}"
        );
    }

    /// A posture swap names both ends, because "changes squad 1" without
    /// saying from what is not enough to decide on.
    #[test]
    fn changing_a_posture_names_what_it_replaces() {
        let mut app = co_app();
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 2), SquadPosture::Push { pos: Vec3::ZERO });
        app.world_mut().resource_mut::<Copilot>().policy = TrustPolicy::Strict;

        wire(
            &mut app,
            r#"{"type":"propose","note":"they countered — hold instead","commands":[
                 {"type":"posture","id":2,"posture":{"type":"defend","x":0.0,"z":0.0,"radius":20.0}}]}"#,
        );
        assert_eq!(
            pending(&app)[0].conflicts,
            vec!["changes squad 2: push -> defend"]
        );
    }

    /// One fumbled command in a batch must not cost the human the other two —
    /// the same rule an ordinary batch has always followed.
    #[test]
    fn a_malformed_command_sinks_only_itself() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        wire(
            &mut app,
            &format!(
                r#"{{"type":"propose","note":"two of these are fine","commands":[
                     {{"type":"move","units":[{u}],"x":1.0,"z":1.0}},
                     {{"type":"nonsense"}},
                     {{"type":"stop","units":[{u}]}}]}}"#,
                u = intent_id(unit)
            ),
        );
        assert_eq!(pending(&app)[0].sentences.len(), 2, "the survivors are proposed");
        assert!(errors(&app).iter().any(|e| e.starts_with("cmd 0.1:")));
    }

    /// A partner that queues faster than its partner can read is told so,
    /// rather than filling a buffer nobody reaches the bottom of.
    #[test]
    fn the_queue_has_a_floor_and_it_reports_it() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        let one = format!(
            r#"{{"type":"propose","note":"again","commands":[
                 {{"type":"move","units":[{}],"x":1.0,"z":1.0}}]}}"#,
            intent_id(unit)
        );
        for _ in 0..MAX_PENDING + 1 {
            wire(&mut app, &one);
        }
        assert_eq!(pending(&app).len(), MAX_PENDING);
        assert!(errors(&app)
            .iter()
            .any(|e| e.contains("proposal queue full")));
    }

    /// The trust split, stated as a test so a new verb cannot quietly land on
    /// the wrong side of it. Doctrine is advice; everything else is an act.
    #[test]
    fn doctrine_is_direct_and_spending_is_not() {
        let posture = Intent::Posture {
            id: 1,
            posture: Some(PostureIntent::Push { x: 0.0, z: 0.0 }),
        };
        let retreat = Intent::Retreat {
            units: vec![1],
            below: Some(0.35),
            x: Some(0.0),
            z: Some(0.0),
        };
        let train = Intent::Train {
            building: 1,
            unit: "Footman".to_string(),
        };
        let attack = Intent::Attack {
            units: vec![1],
            target: 2,
        };
        let surrender = Intent::Surrender;

        for advice in [&posture, &retreat] {
            assert!(is_doctrine_verb(advice), "{} is doctrine", advice.verb());
            assert!(direct_allowed(TrustPolicy::Split, advice));
        }
        for act in [&train, &attack, &surrender] {
            assert!(!is_doctrine_verb(act), "{} is an act", act.verb());
            assert!(
                !direct_allowed(TrustPolicy::Split, act),
                "{} must be proposed",
                act.verb()
            );
        }

        // The two experiment modes move the whole line, not part of it.
        assert!(direct_allowed(TrustPolicy::Full, &train));
        assert!(!direct_allowed(TrustPolicy::Strict, &posture));
    }

    /// The snapshot advertises `direct_verbs`; `is_doctrine_verb` enforces it.
    /// Two lists is one list too many, so this is the seam that checks they
    /// have not drifted — a new doctrine verb that lands in the predicate but
    /// not in the advertisement is a co-commander told to ask about something
    /// it is allowed to do.
    #[test]
    fn the_advertised_direct_verbs_are_the_ones_that_pass() {
        let samples: Vec<Intent> = vec![
            Intent::Priority { units: vec![1], classes: Vec::new() },
            Intent::Retreat { units: vec![1], below: None, x: None, z: None },
            Intent::Leash { units: vec![1], x: None, z: None, radius: None },
            Intent::Autocast { units: vec![1], min_enemies: None, ability: None },
            Intent::Squad { units: vec![1], id: None },
            Intent::Posture { id: 1, posture: None },
            Intent::Template {
                building: 1,
                squad: None,
                retreat: None,
                priority: None,
                autocast: None,
            },
        ];
        let passing: Vec<&'static str> = samples
            .iter()
            .filter(|i| is_doctrine_verb(i))
            .map(Intent::verb)
            .collect();
        assert_eq!(passing, DOCTRINE_VERBS.to_vec());
        assert_eq!(direct_verbs(TrustPolicy::Split), DOCTRINE_VERBS.to_vec());
        assert!(direct_verbs(TrustPolicy::Strict).is_empty());
    }

    /// The refusal has to teach: a model reading it mid-match must be able to
    /// resend without going back to the brief.
    #[test]
    fn a_refusal_shows_the_wrapper() {
        let intent = Intent::Train {
            building: 1,
            unit: "Footman".to_string(),
        };
        let err = needs_proposal_error("cmd 0", &intent, TrustPolicy::Split);
        assert!(err.starts_with("cmd 0: 'train' needs the human's approval"));
        assert!(err.contains(r#"{"type":"propose","commands":[…],"note":"why"}"#));
    }

    /// Every verb that names units must be able to say which ones, or the
    /// conflict readout has a hole exactly where the surprises are.
    #[test]
    fn every_unit_verb_has_a_scope() {
        let cases: Vec<(Intent, usize)> = vec![
            (
                Intent::Move {
                    units: vec![7, 8],
                    x: 1.0,
                    z: 2.0,
                },
                2,
            ),
            (
                Intent::Attack {
                    units: vec![7],
                    target: 9,
                },
                1,
            ),
            (
                Intent::Build {
                    worker: 7,
                    kind: "Farm".to_string(),
                    x: 0.0,
                    z: 0.0,
                },
                1,
            ),
            (Intent::Stop { units: vec![7, 8, 9] }, 3),
        ];
        for (intent, want) in cases {
            match scope_of(&intent) {
                Scope::Units(ids) => assert_eq!(ids.len(), want, "{}", intent.verb()),
                _ => panic!("{} should scope to units", intent.verb()),
            }
        }
        assert!(matches!(
            scope_of(&Intent::Posture { id: 2, posture: None }),
            Scope::Squad(2)
        ));
        assert!(matches!(
            scope_of(&Intent::Surrender),
            Scope::Nothing
        ));
    }

    /// `[Space]` should be able to send the camera to what a proposal is
    /// about — but only when the batch actually names a place.
    #[test]
    fn a_proposal_about_ground_carries_that_ground() {
        let push = Intent::Posture {
            id: 1,
            posture: Some(PostureIntent::Push { x: 12.0, z: -34.0 }),
        };
        assert_eq!(intent_pos(&push), Some(Vec3::new(12.0, 0.0, -34.0)));
        assert_eq!(
            intent_pos(&Intent::Train {
                building: 1,
                unit: "Footman".to_string()
            }),
            None
        );
    }
}
