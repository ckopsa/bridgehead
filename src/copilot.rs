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
//! `BH_BRIDGE=copilot` opens **one** bridge seat attached to `Team::Human` —
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
//! | **neither** | `ready` | not a third tier so much as a verb from before the match: it is a statement about the *clock*, not about the army, and it is intercepted in `bridge.rs::poll_commands` ahead of the copilot branch. Routing it here would ask a player to approve their partner's willingness to begin — and would hold the match at t=0 until they did. See `shared::ReadyGate`. |
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
//! `BH_COPILOT_TRUST` moves the line for experiments: `full` (everything
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
//!
//! ## Answering back: the veto has a reason
//!
//! A veto that says only "no" makes the negotiation one-sided. The
//! co-commander proposed *with an argument*; it gets back a bare refusal and
//! has to guess which of three completely different things happened — bad
//! timing, bad idea, or bad aim — and the three call for opposite next moves.
//! Guessing wrong is how a partner becomes a nag.
//!
//! So a veto carries one of three [`VetoReason`]s, and the human picks it in
//! the same keystroke that gives it:
//!
//! | key | reason | what it asks of the proposer |
//! |---|---|---|
//! | `[Bksp]` | `NotNow` | the idea is fine, the moment is not — re-propose when conditions change |
//! | `[Shift]+[Bksp]` | `WrongTarget` | the idea is right, the aim is wrong — re-propose elsewhere |
//! | `[Ctrl]+[Bksp]` | `Never` | drop it; do not raise it again this match |
//!
//! Plain `[Bksp]` is `NotNow` because the fast path must stay one key, and
//! because the softest of the three is the right thing to mean when the human
//! was too busy to modify. `Never` is **etiquette, not enforcement**: nothing
//! here refuses a re-proposal. That is the same rule the rest of co-command
//! follows — *source is descriptive, never authoritative* — and a partner that
//! could silently ban its partner's ideas would be arbitration by the back
//! door.
//!
//! ## Urgency: the queue is answered in the order that matters
//!
//! `severity: "urgent"` on the wrapper puts a proposal at the FRONT of the
//! queue rather than the back. It changes nothing about what may be proposed
//! and nothing about the cap — four is still four, because the cap is about
//! how many questions a human can hold, and urgency does not add attention.
//! It changes only *which question is asked first*, which is exactly the thing
//! oldest-first got wrong: "they are flanking, pull back" and "we should
//! expand" are not equally answerable at second 40 of a fight.
//!
//! Because insertion keeps `pending` in **answer order**, index 0 is still
//! "the card `[Enter]` takes" for ui.rs and still "#1 in the queue" for the
//! snapshot. Neither had to learn what urgency is.
//!
//! ## Measuring any of this
//!
//! Approval is a human act, so a headless sim has nobody to give it and every
//! proposal lapses — correct, and useless for measurement. Two knobs make the
//! loop observable without a person:
//!
//! * `BH_COPILOT_TRUST=full` — no loop at all, the control case.
//! * `BH_COPILOT_AUTOAPPROVE=1` — a scripted stand-in that approves each
//!   proposal `BH_COPILOT_APPROVE_DELAY` seconds after it arrives (default
//!   [`DEFAULT_APPROVE_DELAY`]), modelling an attentive human's reading time.
//!
//! The delay is the point. Zero-delay approval would measure a co-commander
//! with a rubber stamp; a real partner costs *seconds between the idea and the
//! act*, and whether the plan still fits the board after those seconds is the
//! question the proposal loop actually raises.

use crate::intent::{IntentApply, LateBind};
use crate::shared::*;
use bevy::prelude::*;
use serde::Deserialize;
use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Tuning knobs
// ---------------------------------------------------------------------------

/// How the co-commander's direct (non-proposal) commands are treated.
const TRUST_ENV: &str = "BH_COPILOT_TRUST";

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

/// Turn on the scripted approver that stands in for a human in sims.
const AUTOAPPROVE_ENV: &str = "BH_COPILOT_AUTOAPPROVE";
/// Game seconds the scripted approver waits before saying yes.
const APPROVE_DELAY_ENV: &str = "BH_COPILOT_APPROVE_DELAY";

/// The scripted approver's default reading time. Chosen to be a plausible
/// *attentive* human — long enough that a proposal is a real commitment of
/// tempo, short enough to sit well inside [`PROPOSAL_TTL`] so a sim measures
/// the approval path rather than the expiry path.
pub const DEFAULT_APPROVE_DELAY: f32 = 3.0;

/// How many answered proposals a co-commander can still read about.
///
/// A tail rather than a one-cycle grace period on `pending`: a seat that polls
/// slower than the snapshot ticks would miss a status that lives for exactly
/// one write, and "did my partner ever answer #3?" is precisely the question
/// you ask when you have *not* been keeping up.
pub const RESOLUTION_TAIL: usize = 8;

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
            // Declared once, so anything later tagged only `.in_set(CopilotSet)`
            // inherits the frame order rather than floating outside it.
            .configure_sets(Update, CopilotSet.in_set(crate::shared::SimSet::CoCommand))
            .add_systems(
                Update,
                // `auto_approve` sits between the two for the same reason
                // ui.rs's `proposal_input` runs before `CopilotSet`: a verdict
                // is only useful in the frame it can still be acted on. A
                // proposal that came of age this frame is approved, compiled
                // and snapshot in that one frame.
                (ingest_wire, auto_approve, resolve_proposals)
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

/// How badly this wants answering first.
///
/// Deliberately two values, not five. A scale a proposer can game is a scale
/// that becomes all-urgent within a match; two values make the choice a real
/// one, because marking everything urgent marks nothing urgent and the human
/// finds out immediately.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ProposalSeverity {
    /// The default and the overwhelming majority: answer it when you can.
    #[default]
    Routine,
    /// Jumps the queue and wears the Warning tint. For the window that closes.
    Urgent,
}

impl ProposalSeverity {
    /// The two values the wrapper accepts, in the order the snapshot lists
    /// them — advertised so a co-commander learns the vocabulary by reading.
    pub const NAMES: [&'static str; 2] = ["routine", "urgent"];

    fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "routine" | "normal" => Some(ProposalSeverity::Routine),
            "urgent" => Some(ProposalSeverity::Urgent),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ProposalSeverity::Routine => "routine",
            ProposalSeverity::Urgent => "urgent",
        }
    }
}

/// The human's half of the argument: *why* the answer was no.
///
/// Three, because three is how many genuinely different next moves there are.
/// A fourth would have to be a shade of one of these, and a co-commander that
/// has to distinguish shades is back to guessing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VetoReason {
    /// Good idea, wrong moment. The default, and what a plain `[Bksp]` means —
    /// the softest answer belongs on the key pressed under pressure.
    #[default]
    NotNow,
    /// Drop it. Etiquette only: nothing in this file refuses a re-proposal,
    /// because a partner who could ban ideas would be a referee.
    Never,
    /// The idea is right and the aim is wrong. The one veto that is really a
    /// request: send it again, pointed somewhere else.
    WrongTarget,
}

impl VetoReason {
    /// What the snapshot calls it. `snake_case` like every other wire enum
    /// here, so a co-commander matches on it rather than parsing prose.
    pub fn wire(self) -> &'static str {
        match self {
            VetoReason::NotNow => "not_now",
            VetoReason::Never => "never",
            VetoReason::WrongTarget => "wrong_target",
        }
    }

    /// What the human's alert stack calls it.
    pub fn phrase(self) -> &'static str {
        match self {
            VetoReason::NotNow => "not now",
            VetoReason::Never => "never",
            VetoReason::WrongTarget => "wrong target",
        }
    }

    /// The etiquette in one clause, carried in the event line AND the wire.
    ///
    /// Duplicating the brief here is on purpose: a model reading `events`
    /// mid-match should not need a second document to know whether it may try
    /// again. Same reason `needs_proposal_error` prints the wrapper.
    pub fn advice(self) -> &'static str {
        match self {
            VetoReason::NotNow => "re-propose when conditions change",
            VetoReason::Never => "do not re-propose this match",
            VetoReason::WrongTarget => "re-propose with a different target",
        }
    }

    /// Every reason and its advice, for the snapshot to teach the vocabulary.
    pub fn all() -> [VetoReason; 3] {
        [
            VetoReason::NotNow,
            VetoReason::Never,
            VetoReason::WrongTarget,
        ]
    }
}

/// How a proposal left the queue.
///
/// There is no `Pending` variant, and that absence is the wire design: being
/// in `Copilot::pending` *is* pending. A status field that restates a list
/// membership is a second source of truth, and the two disagree the first time
/// somebody forgets to update one of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Approved,
    Vetoed(VetoReason),
    /// Nobody answered inside [`PROPOSAL_TTL`]. Distinct from `Vetoed` because
    /// silence and refusal call for opposite responses.
    Expired,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Approved => "approved",
            Outcome::Vetoed(_) => "vetoed",
            Outcome::Expired => "expired",
        }
    }

    pub fn reason(self) -> Option<VetoReason> {
        match self {
            Outcome::Vetoed(reason) => Some(reason),
            _ => None,
        }
    }
}

/// One answered proposal, kept just long enough for its proposer to read it.
pub struct Resolution {
    pub id: u32,
    /// Game seconds at which it was answered.
    pub at: f32,
    /// The proposer's own note, echoed back — so a resolution identifies the
    /// idea by what it was FOR, not just by a number the model has to have
    /// remembered.
    pub note: String,
    pub severity: ProposalSeverity,
    pub outcome: Outcome,
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
    /// Where it sits in the queue, and how the card is tinted. Never affects
    /// what the batch is allowed to do — urgency buys attention, not trust.
    pub severity: ProposalSeverity,
    pub proposed_at: f32,
    pub expires_at: f32,
    /// Somewhere on the map this is about, for `[Space]` to focus.
    pub pos: Option<Vec3>,
}

impl Proposal {
    pub fn expires_in(&self, now: f32) -> f32 {
        (self.expires_at - now).max(0.0)
    }

    pub fn is_urgent(&self) -> bool {
        self.severity == ProposalSeverity::Urgent
    }
}

/// Where a newly arrived proposal goes: ahead of every routine one, behind
/// every urgent one already waiting.
///
/// This is the whole of "urgent jumps the queue", and it is deliberately here
/// rather than a sort at read time. Keeping `pending` permanently in ANSWER
/// order means index 0 is the card `[Enter]` takes, the card the panel
/// brightens, and the first entry of the snapshot's `proposals` — three
/// readers that between them needed to learn nothing about severity.
///
/// Urgent-then-oldest, not urgent-only: two urgent proposals still answer in
/// the order they were asked, because the second one did not become more
/// important by being later.
fn insert_index(pending: &[Proposal], severity: ProposalSeverity) -> usize {
    match severity {
        ProposalSeverity::Routine => pending.len(),
        ProposalSeverity::Urgent => pending
            .iter()
            .position(|p| !p.is_urgent())
            .unwrap_or(pending.len()),
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
    /// **Answer order**, which is urgent-then-oldest: index 0 is the one the
    /// human's approve/veto keys act on, the one the panel brightens, and the
    /// first entry of the snapshot's `proposals`. See `insert_index`.
    pub pending: Vec<Proposal>,
    /// The last [`RESOLUTION_TAIL`] answered proposals, oldest first — the
    /// only place a co-commander can read *why* a veto was a veto.
    pub resolved: VecDeque<Resolution>,
    /// `Some(delay)` when a scripted approver is standing in for the human
    /// (`BH_COPILOT_AUTOAPPROVE`). `None` in every real match, which is what
    /// keeps this inert outside sims.
    pub auto_approve: Option<f32>,
    next_id: u32,
}

impl Copilot {
    fn from_env() -> Self {
        Copilot {
            seat: None,
            policy: TrustPolicy::from_env(),
            pending: Vec::new(),
            resolved: VecDeque::new(),
            auto_approve: auto_approve_from_env(),
            next_id: 1,
        }
    }

    /// Called by bridge.rs when it opens a copilot seat.
    pub fn seat(&mut self, team: Team) {
        self.seat = Some(team);
    }

    /// File an answered proposal in the tail, evicting the oldest.
    fn resolve(&mut self, resolution: Resolution) {
        if self.resolved.len() == RESOLUTION_TAIL {
            self.resolved.pop_front();
        }
        self.resolved.push_back(resolution);
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
            resolved: VecDeque::new(),
            auto_approve: None,
            next_id: 1,
        }
    }
}

/// Read the scripted approver's configuration once, at startup.
///
/// Off unless asked for, and a delay that cannot be negative — a "delay" of
/// `-1` would be a rubber stamp wearing a stopwatch, which is the one thing
/// this knob exists to avoid measuring.
fn auto_approve_from_env() -> Option<f32> {
    let on = std::env::var(AUTOAPPROVE_ENV)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);
    if !on {
        return None;
    }
    let delay = std::env::var(APPROVE_DELAY_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|d| d.is_finite() && *d >= 0.0)
        .unwrap_or(DEFAULT_APPROVE_DELAY);
    Some(delay)
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

/// Yes, or no-and-here-is-why.
///
/// An enum rather than a `bool` plus an `Option<VetoReason>`, because "an
/// approval that carries a veto reason" is not a state anything should have to
/// decide what to do with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Approve,
    Veto(VetoReason),
}

/// The human's answer to a proposal. Written by ui.rs (hotkey or click) and by
/// the scripted approver; this file is the only reader.
#[derive(Event, Clone, Copy, Debug)]
pub struct ProposalVerdict {
    pub id: u32,
    pub verdict: Verdict,
}

impl ProposalVerdict {
    pub fn approve(id: u32) -> Self {
        ProposalVerdict {
            id,
            verdict: Verdict::Approve,
        }
    }

    pub fn veto(id: u32, reason: VetoReason) -> Self {
        ProposalVerdict {
            id,
            verdict: Verdict::Veto(reason),
        }
    }
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
    /// `"routine"` (default) or `"urgent"`. A `String` rather than a typed
    /// enum so an unknown value produces an error naming both accepted words
    /// instead of serde's "unknown variant" — the same reason every other
    /// refusal here teaches the shape it wanted.
    #[serde(default)]
    severity: Option<String>,
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
            // A stance is the other seven in one word, so it is doctrine by
            // construction: it installs a disposition, spends nothing and moves
            // nobody. Excluding it would have been the odd choice — a partner
            // trusted to set a posture and a leash separately but not to set
            // both at once.
            | Intent::Stance { .. }
            | Intent::Template { .. }
    )
}

/// The eight doctrine verbs by name, for the snapshot to tell the seat what it
/// may say without asking. Kept honest against `is_doctrine_verb` by
/// `the_advertised_direct_verbs_are_the_ones_that_pass`.
pub const DOCTRINE_VERBS: [&str; 8] = [
    "priority", "retreat", "leash", "autocast", "squad", "posture", "stance", "template",
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
    // The team's own named geography, so a proposal that says "push to
    // north-pass" still has somewhere for `[Space]` to fly to — and, since the
    // conflict preview resolves the batch, the same names the compiler would
    // look up on approval.
    regions: Res<Regions>,
    units: CopilotUnits,
    // What the one resolver is allowed to see, so the preview can expand a
    // `"select"` phrase by asking it rather than by learning the vocabulary a
    // second time. Read-only, and read only for the preview: nothing here is
    // submitted, and no resolved id is kept.
    nav: Res<NavGrid>,
    bind_world: crate::intent::LateBindWorld,
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
                        trigger: None,
                        plan: None,
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
        // A misspelt severity sinks the whole proposal rather than quietly
        // downgrading to routine. Silent downgrade is the worse failure: the
        // proposer believes it jumped the queue, the human never sees it jump,
        // and nothing anywhere says why.
        let severity = match wrapper.severity.as_deref() {
            None => ProposalSeverity::Routine,
            Some(word) if word.trim().is_empty() => ProposalSeverity::Routine,
            Some(word) => match ProposalSeverity::parse(word) {
                Some(severity) => severity,
                None => {
                    errors.get_mut(team).push(format!(
                        "{tag}: unknown severity '{}' — use \"routine\" (the \
                         default) or \"urgent\" (jumps the queue)",
                        word.trim()
                    ));
                    continue;
                }
            },
        };
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
        let conflicts = conflict_tags(
            team,
            &intents,
            &squad_orders,
            &units,
            now,
            &LateBind::new(team, &regions, &nav, &bind_world),
        );
        let pos = intents
            .iter()
            .find_map(|i| intent_pos(i, team, &regions));
        let note = if wrapper.note.trim().is_empty() {
            "(no reason given)".to_string()
        } else {
            wrapper.note.trim().to_string()
        };
        // The arrival is news, on the channel the human already watches — and
        // the same push lands in the co-commander's own `events`, which is how
        // it learns the proposal was received rather than dropped. An urgent
        // one arrives in the alert stack's Warning colour, so it is the louder
        // line before the human's eye ever reaches the panel.
        let (headline, loudness) = match severity {
            ProposalSeverity::Routine => {
                (format!("copilot proposes #{id}: {note}"), EventSeverity::Info)
            }
            ProposalSeverity::Urgent => (
                format!("copilot proposes #{id} (urgent): {note}"),
                EventSeverity::Warning,
            ),
        };
        feed.push(team, now, headline, loudness, pos);
        let at = insert_index(&copilot.pending, severity);
        copilot.pending.insert(
            at,
            Proposal {
                id,
                note,
                intents,
                sentences,
                conflicts,
                severity,
                proposed_at: now,
                expires_at: now + PROPOSAL_TTL,
                pos,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Conflict visibility
// ---------------------------------------------------------------------------

/// Which of the issuer's own units a verb is about.
///
/// **It reads a RESOLVED intent.** A proposal arrives holding phrases —
/// `"select":"all army"` — and a phrase names no ids, so this function used to
/// scope a selector-written batch to nobody and the human was told it
/// overrides nothing while it was in fact about to take the whole army. The
/// fix is not to teach this function the selector vocabulary a second time; it
/// is `conflict_tags` running the batch through `intent::resolve_places` first,
/// so what this matches on is a copy of the intent with its roles already
/// expanded into ids by the one resolver.
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
        | Intent::Return { units, .. }
        | Intent::Follow { units, .. }
        | Intent::Stop { units, .. }
        | Intent::Priority { units, .. }
        | Intent::Retreat { units, .. }
        | Intent::Leash { units, .. }
        | Intent::Autocast { units, .. }
        | Intent::Squad { units, .. } => Scope::Units(units.clone()),
        Intent::Build { worker, .. } => Scope::Units(worker.iter().copied().collect()),
        Intent::Posture { id, .. } => Scope::Squad(*id),
        Intent::Stance { squad, .. } => Scope::Squad(*squad),
        _ => Scope::Nothing,
    }
}

/// Somewhere on the map a proposal is about, so `[Space]` can go look at it.
///
/// A proposal is held UNRESOLVED — it has not been through the compiler and may
/// never be — so a partner that proposed "push to north-pass" carries a name
/// here rather than coordinates. `regions` is the reviewing seat's own
/// vocabulary, which is the right one to look the name up in: the two
/// co-commanders share a team, and therefore share its regions.
fn intent_pos(intent: &Intent, team: Team, regions: &Regions) -> Option<Vec3> {
    /// Coordinates if given, else the named region's centre. Same precedence
    /// the compiler's `resolve_places` uses — region wins — so the camera flies
    /// to the ground the order would actually act on.
    fn spot(
        x: &Option<f32>,
        z: &Option<f32>,
        region: &Option<String>,
        team: Team,
        regions: &Regions,
    ) -> Option<(f32, f32)> {
        if let Some(name) = region {
            let found = regions.find(team, name)?;
            return Some((found.center.x, found.center.z));
        }
        match (x, z) {
            (Some(x), Some(z)) => Some((*x, *z)),
            _ => None,
        }
    }
    let (x, z) = match intent {
        Intent::Move { x, z, region, .. }
        | Intent::AttackMove { x, z, region, .. }
        | Intent::Build { x, z, region, .. }
        | Intent::Leash { x, z, region, .. }
        | Intent::Retreat { x, z, region, .. }
        // A stance whose anchor was omitted means the team's base, which
        // `spot` reports as "no place" — correct here: the camera has nothing
        // to fly to that the reviewer is not already looking at.
        | Intent::Stance { x, z, region, .. } => spot(x, z, region, team, regions)?,
        Intent::Posture {
            posture: Some(posture),
            ..
        } => match posture {
            PostureIntent::Defend { x, z, region, .. }
            | PostureIntent::Push { x, z, region }
            | PostureIntent::Forage { x, z, region } => spot(x, z, region, team, regions)?,
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
///
/// **Roles are expanded here, through the one resolver, and the expansion is a
/// PREVIEW.** `bind` is the same [`intent::LateBind`] the compiler builds, so
/// `"select":"all army"` becomes the ids it stands for by the identical rule
/// that will expand it again on approval — no second vocabulary, no second
/// spelling of the empty-match refusal. But the two resolutions happen at
/// different moments, and between them units die, join squads and are trained,
/// so the answers can honestly differ. That is why every tag this expansion
/// produces is dated `as of now`: the reviewer is reading a scope measured at
/// arrival, not a promise about approval. The advisory list has always had this
/// property (`overrides your move on 4 unit(s), 6s ago` was already a
/// measurement); selectors just make it visible, because a phrase can change
/// what it means while a list of ids cannot.
fn conflict_tags(
    me: Team,
    intents: &[Intent],
    squad_orders: &SquadOrders,
    units: &CopilotUnits,
    now: f32,
    bind: &LateBind,
) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut squads_touched: Vec<u8> = Vec::new();
    // Human orders this batch would overwrite: verb -> how many units, and the
    // freshest one, because "4 seconds ago" is the part that stings.
    let mut overridden: Vec<(&'static str, usize, f32)> = Vec::new();
    // Distinct own units the batch reaches by NAMING A ROLE rather than by
    // listing ids — the blast radius the sentences alone do not give a size to.
    // `"move all army to the ford"` reads the same whether the army is two
    // units or twenty.
    let mut reached_by_role: Vec<IntentId> = Vec::new();

    for intent in intents {
        // The one resolver, asked a question instead of told to act. An `Err`
        // here is the refusal this command would earn if the human approved it
        // this instant — an unknown phrase, a role that currently matches
        // nobody, a region this seat cannot name. Worth saying: a reviewer
        // about to spend a keystroke on a batch that would refuse is exactly
        // the person who wants to know.
        let resolved = match crate::intent::resolve_places(intent.clone(), bind) {
            Ok(resolved) => resolved,
            Err(why) => {
                push_unique(&mut tags, format!("as of now this would refuse: {why}"));
                continue;
            }
        };
        // Anything the resolver added to the unit channel came from a role:
        // the ids the co-commander wrote are the ids that survive resolution.
        let scope = scope_of(&resolved);
        if let (Scope::Units(before), Scope::Units(after)) = (scope_of(intent), &scope) {
            if &before != after {
                for id in after {
                    if !reached_by_role.contains(id) {
                        reached_by_role.push(*id);
                    }
                }
            }
        }
        // Everything below weighs the RESOLVED batch, which is the one the
        // human would be approving.
        let intent = &resolved;
        match scope {
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

    // First line of the readout when a role was named, because it is the size
    // of the thing being agreed to. `move all army to the ford` reads the same
    // at two units and at twenty, and the number is the part a human weighs.
    if !reached_by_role.is_empty() {
        tags.insert(
            0,
            format!(
                "the roles named reach {} unit(s) as of now",
                reached_by_role.len()
            ),
        );
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
    if game_over.decided() {
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
        if let Verdict::Veto(reason) = verdict.verdict {
            // The reason AND what it asks for, on the line the co-commander
            // reads anyway. A partner told only "vetoed" has to guess between
            // three opposite next moves; this is the whole point of the bead.
            feed.push(
                team,
                now,
                format!(
                    "proposal #{} vetoed ({} - {}): {}",
                    proposal.id,
                    reason.phrase(),
                    reason.advice(),
                    proposal.note
                ),
                EventSeverity::Info,
                None,
            );
            copilot.resolve(Resolution {
                id: proposal.id,
                at: now,
                note: proposal.note,
                severity: proposal.severity,
                outcome: Outcome::Vetoed(reason),
            });
            continue;
        }
        copilot.resolve(Resolution {
            id: proposal.id,
            at: now,
            note: proposal.note.clone(),
            severity: proposal.severity,
            outcome: Outcome::Approved,
        });
        for (j, intent) in proposal.intents.into_iter().enumerate() {
            submissions.write(SubmitIntent {
                team,
                source: IntentSource::Copilot,
                // Names the proposal AND the command inside it, so a rejection
                // 20 seconds after the batch was written is still traceable to
                // the directive that asked for it.
                tag: format!("prop {} cmd {j}", proposal.id),
                intent,
                trigger: None,
                plan: None,
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
    let mut lapsed: Vec<(u32, String, ProposalSeverity)> = Vec::new();
    copilot.pending.retain(|p| {
        if p.expires_at > now {
            return true;
        }
        lapsed.push((p.id, p.note.clone(), p.severity));
        false
    });
    for (id, note, severity) in lapsed {
        feed.push(
            team,
            now,
            format!("proposal #{id} expired unanswered: {note}"),
            EventSeverity::Warning,
            None,
        );
        copilot.resolve(Resolution {
            id,
            at: now,
            note,
            severity,
            outcome: Outcome::Expired,
        });
    }
}

/// The scripted approver: a stand-in for an attentive human, for sims.
///
/// It exists because the proposal loop is the one part of co-command a
/// headless run cannot exercise — approval is a human act, and headless has no
/// human, so every proposal lapses and the measurement is of nothing. With
/// this, an AI-vs-(AI+AI) match runs the *real* path: the same queue, the same
/// compiler, the same errors, just with the verdict arriving on a timer
/// instead of a keystroke.
///
/// It approves rather than judges, and that is a stated limitation, not an
/// oversight. A scripted approver that vetoed on some heuristic would be
/// measuring the heuristic. This one measures the only thing a human's
/// presence reliably adds to the loop: **delay** — the seconds between a good
/// idea and its execution, and whether the board still rewards it afterwards.
///
/// Queue order is honoured for free: `pending` is already urgent-then-oldest,
/// so an urgent proposal is approved before a routine one that arrived first.
fn auto_approve(
    time: Res<Time>,
    copilot: Res<Copilot>,
    mut verdicts: EventWriter<ProposalVerdict>,
) {
    let Some(delay) = copilot.auto_approve else {
        return;
    };
    let now = time.elapsed_secs();
    for proposal in &copilot.pending {
        if now - proposal.proposed_at >= delay {
            verdicts.write(ProposalVerdict::approve(proposal.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandLatency, CommandNodes};
    use crate::intent::{IntentLog, IntentPlugin};

    /// The whole co-command loop over a real world: the negotiation layer
    /// AND the compiler it submits into. Nothing here is a stand-in — an
    /// approved proposal takes exactly the path a live match takes.
    fn co_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
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
            // Chain of Command's two resources (docs/TEMPO.md §3), defaulted
            // to `on: false` — an approved batch compiles the instant the
            // human says yes, so nothing here is about propagation. Under
            // latency a co-commander's approved order would travel exactly as
            // any other direct order does, which is the point of it going
            // through the same compiler.
            .init_resource::<CommandNodes>()
            .init_resource::<CommandLatency>()
            .add_plugins((IntentPlugin, CopilotPlugin));
        // A unit test must not depend on `BH_INTENT_LOG` or leave a file.
        app.insert_resource(IntentLog::disabled());
        // Nor on `BH_COPILOT_TRUST`: the policy under test is set per test.
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
        app.world_mut().send_event(ProposalVerdict::approve(1));
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

    /// **A proposed PLAN is one proposal, and one reviewable line.**
    ///
    /// This is the thing co-command wanted and could not have before: a
    /// partner's opening used to arrive as five separate commands, each its own
    /// queue entry, each approvable on its own — so the human could approve the
    /// barracks and veto the keep and end up with an incoherent half-sequence
    /// nobody proposed. `plan_set` makes the whole sequence ONE `Intent`, so it
    /// is one queue entry with one sentence and one `[Enter]`.
    ///
    /// Nothing in copilot.rs knew a plan was coming. It wraps any command, and
    /// this test is the confirmation rather than the mechanism — which is the
    /// choke point (docs/INTENT.md) paying for itself again.
    #[test]
    fn a_proposed_plan_is_one_proposal_with_the_whole_sequence_on_its_line() {
        let mut app = co_app();
        wire(
            &mut app,
            r#"{"type":"propose","note":"the boomer opening — sanctum before army",
                 "commands":[{"type":"plan_set","name":"boomer","steps":[
                    {"intent":{"type":"build","worker":7,"kind":"Barracks","x":-60.0,"z":-60.0},
                     "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}},
                    {"intent":{"type":"train","building":9,"unit":"Sorcerer"}}]}]}"#,
        );

        let proposal = &pending(&app)[0];
        assert_eq!(pending(&app).len(), 1, "a five-step opening is ONE decision");
        assert_eq!(
            proposal.sentences.len(),
            1,
            "and one line to answer it on: {:?}",
            proposal.sentences
        );
        assert_eq!(
            proposal.sentences[0],
            "plan boomer (2 steps): worker 7 builds Barracks at (-60.0, -60.0), \
             then when we reach tier 2: building 9 trains Sorcerer",
            "the whole sequence, in the English the replay log will write"
        );

        // A plan is not a doctrine verb, so `split` trust makes the human
        // decide — which is correct: its steps spend money.
        app.world_mut().send_event(ProposalVerdict::approve(1));
        app.update();
        assert!(pending(&app).is_empty());
        let plans = app.world().resource::<Plans>();
        assert_eq!(plans.get(Team::Human).len(), 1, "approval set it running");
        assert_eq!(plans.get(Team::Human)[0].name.as_str(), "boomer");
        assert_eq!(
            plans.get(Team::Human)[0].source,
            IntentSource::Copilot,
            "attributed to the partner who proposed it, not to the human who \
             approved it — approval is consent, not authorship"
        );
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
        app.world_mut()
            .send_event(ProposalVerdict::veto(1, VetoReason::NotNow));
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

    /// **The negotiation, made two-sided.** A veto carries which of three
    /// answers it was, and both channels a co-commander reads say so: the
    /// event line it sees mid-match, and the resolution tail its snapshot
    /// carries. Each reason is paired with what it asks for next, because the
    /// three call for opposite moves and a partner that has to guess between
    /// them will pick wrong and become a nag.
    #[test]
    fn a_veto_reason_reaches_both_the_feed_and_the_tail() {
        let cases = [
            (VetoReason::NotNow, "not now", "re-propose when conditions change"),
            (VetoReason::Never, "never", "do not re-propose this match"),
            (
                VetoReason::WrongTarget,
                "wrong target",
                "re-propose with a different target",
            ),
        ];
        for (reason, phrase, advice) in cases {
            let mut app = co_app();
            let unit = footman(&mut app, Vec3::ZERO);
            wire(
                &mut app,
                &format!(
                    r#"{{"type":"propose","note":"hit their siege","commands":[
                         {{"type":"move","units":[{}],"x":5.0,"z":5.0}}]}}"#,
                    intent_id(unit)
                ),
            );
            app.world_mut().send_event(ProposalVerdict::veto(1, reason));
            app.update();

            let line = app
                .world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .map(|e| e.message.clone())
                .find(|m| m.contains("vetoed"))
                .expect("the veto is announced");
            assert!(
                line.contains(phrase) && line.contains(advice),
                "the feed must say which no it was AND what it asks for: {line}"
            );

            let copilot = app.world().resource::<Copilot>();
            let resolution = copilot.resolved.back().expect("it left a resolution");
            assert_eq!(resolution.id, 1);
            assert_eq!(resolution.outcome, Outcome::Vetoed(reason));
            assert_eq!(resolution.outcome.reason(), Some(reason));
            assert_eq!(
                resolution.note, "hit their siege",
                "a resolution names the idea, not just its number"
            );
            assert!(copilot.pending.is_empty(), "it still left the queue");
        }
    }

    /// The three terminal states are distinguishable in the tail, and only the
    /// veto carries a reason — an approval with a veto reason is a state the
    /// enum makes unrepresentable, and this is the wire half of that.
    #[test]
    fn every_terminal_state_lands_in_the_tail_and_only_vetoes_have_reasons() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        let one = format!(
            r#"{{"type":"propose","note":"n","commands":[
                 {{"type":"move","units":[{}],"x":5.0,"z":5.0}}]}}"#,
            intent_id(unit)
        );
        wire(&mut app, &one);
        wire(&mut app, &one);
        wire(&mut app, &one);
        app.world_mut().send_event(ProposalVerdict::approve(1));
        app.world_mut()
            .send_event(ProposalVerdict::veto(2, VetoReason::Never));
        app.update();
        // #3 is left to the clock.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PROPOSAL_TTL + 1.0));
        app.update();

        let outcomes: Vec<Outcome> = app
            .world()
            .resource::<Copilot>()
            .resolved
            .iter()
            .map(|r| r.outcome)
            .collect();
        assert_eq!(
            outcomes,
            vec![
                Outcome::Approved,
                Outcome::Vetoed(VetoReason::Never),
                Outcome::Expired
            ],
            "oldest first, one entry per answered proposal"
        );
        assert_eq!(outcomes[0].reason(), None, "an approval has no reason");
        assert_eq!(outcomes[2].reason(), None, "nor does a lapse");
    }

    /// The tail is a tail: it forgets, so a long match cannot grow a snapshot
    /// field without bound.
    #[test]
    fn the_resolution_tail_forgets_the_oldest() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        let one = format!(
            r#"{{"type":"propose","note":"n","commands":[
                 {{"type":"move","units":[{}],"x":5.0,"z":5.0}}]}}"#,
            intent_id(unit)
        );
        // MAX_PENDING at a time, answered immediately, until the tail overflows.
        let mut id = 1;
        while id <= RESOLUTION_TAIL as u32 + 2 {
            wire(&mut app, &one);
            app.world_mut()
                .send_event(ProposalVerdict::veto(id, VetoReason::NotNow));
            app.update();
            id += 1;
        }
        let resolved = &app.world().resource::<Copilot>().resolved;
        assert_eq!(resolved.len(), RESOLUTION_TAIL);
        assert_eq!(
            resolved.front().map(|r| r.id),
            Some(3),
            "the two oldest were evicted"
        );
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

    /// **The bug this bead is about (`wc3clone-brq`).** A proposal written with
    /// a role — `"select":"all army"` — used to scope to nobody, so the human
    /// was shown an empty conflict list and approved a batch that took the
    /// whole army. The preview now runs the batch through the one resolver, so
    /// a phrase is weighed exactly as the ids it stands for would be.
    #[test]
    fn a_selector_proposal_previews_the_units_it_would_actually_take() {
        let mut app = co_app();
        // Three footmen the human put in squad 1, gave a push, and right-
        // clicked. None of them is named by the proposal below.
        for _ in 0..3 {
            let unit = footman(&mut app, Vec3::ZERO);
            app.world_mut().entity_mut(unit).insert((
                SquadId(1),
                Provenance::new(
                    Cause::Order { verb: "move", source: IntentSource::Ui },
                    0.0,
                ),
            ));
        }
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: Vec3::ZERO });

        wire(
            &mut app,
            r#"{"type":"propose","note":"the ford is open","commands":[
                 {"type":"move","select":"all army","x":-70.0,"z":-70.0}]}"#,
        );

        let conflicts = &pending(&app)[0].conflicts;
        assert_eq!(
            conflicts[0], "the roles named reach 3 unit(s) as of now",
            "the size of the thing being agreed to leads the readout: {conflicts:?}"
        );
        assert!(
            conflicts.iter().any(|c| c == "re-tasks squad 1 (push)"),
            "got {conflicts:?}"
        );
        assert!(
            conflicts
                .iter()
                .any(|c| c.starts_with("overrides your move on 3 unit(s)")),
            "got {conflicts:?}"
        );
    }

    /// A role that currently matches nobody previews as nobody, in the
    /// resolver's own words — the same sentence approval would refuse with.
    /// Silence here would read as "this disturbs nothing", which is true and
    /// deeply misleading: it does nothing at all.
    #[test]
    fn a_selector_that_matches_nobody_previews_as_such() {
        let mut app = co_app();
        wire(
            &mut app,
            r#"{"type":"propose","note":"push with what we have","commands":[
                 {"type":"move","select":"all army","x":-70.0,"z":-70.0}]}"#,
        );
        assert_eq!(
            pending(&app)[0].conflicts,
            vec![
                "as of now this would refuse: move: 'all army' matches none of \
                 your units right now — nothing was ordered"
            ]
        );
    }

    /// **Preview and apply are two moments, and the tags say so.** The scope is
    /// measured when the proposal arrives; the order is resolved again when the
    /// human approves. Between them the army can change — here two of the three
    /// die — and the two answers legitimately differ. The tag is dated `as of
    /// now` for exactly this reason: it is advice about a board, not a promise
    /// about an outcome, and what actually lands is whatever the one compiler
    /// resolves at approval time.
    #[test]
    fn the_preview_is_dated_because_approval_resolves_again() {
        let mut app = co_app();
        let squad: Vec<Entity> = (0..3).map(|_| footman(&mut app, Vec3::ZERO)).collect();

        wire(
            &mut app,
            r#"{"type":"propose","note":"take the ford","commands":[
                 {"type":"move","select":"all army","x":-70.0,"z":-70.0}]}"#,
        );
        assert_eq!(
            pending(&app)[0].conflicts,
            vec!["the roles named reach 3 unit(s) as of now"],
            "three, as of the moment it arrived"
        );

        // The fight the co-commander was worried about happens.
        app.world_mut().despawn(squad[0]);
        app.world_mut().despawn(squad[1]);
        app.world_mut().send_event(ProposalVerdict::approve(1));
        app.update();

        assert!(
            matches!(app.world().entity(squad[2]).get::<Order>(), Some(Order::Move(_))),
            "approval re-resolves 'all army' against the world as it is THEN"
        );
        assert!(
            errors(&app).is_empty(),
            "a survivor is still an army: {:?}",
            errors(&app)
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
            posture: Some(PostureIntent::Push { x: Some(0.0), z: Some(0.0), region: None }),
        };
        let retreat = Intent::Retreat {
            units: vec![1],
            below: Some(0.35),
            x: Some(0.0),
            z: Some(0.0),
            region: None,
            select: None,
        };
        let train = Intent::Train {
            building: Some(1),
            unit: "Footman".to_string(),
            select: None,
        };
        let attack = Intent::Attack {
            units: vec![1],
            target: 2,
            select: None,
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
            Intent::Priority {
                units: vec![1],
                classes: Vec::new(),
                select: None,
            },
            Intent::Retreat {
                units: vec![1],
                below: None,
                x: None,
                z: None,
                region: None,
                select: None,
            },
            Intent::Leash {
                units: vec![1],
                x: None,
                z: None,
                region: None,
                radius: None,
                select: None,
            },
            Intent::Autocast {
                units: vec![1],
                min_enemies: None,
                ability: None,
                select: None,
            },
            Intent::Squad {
                units: vec![1],
                id: None,
                select: None,
            },
            Intent::Posture {
                id: 1,
                posture: None,
            },
            Intent::Stance {
                squad: 1,
                stance: "turtle".to_string(),
                x: None,
                z: None,
                region: None,
            },
            Intent::Template {
                building: Some(1),
                squad: None,
                retreat: None,
                priority: None,
                autocast: None,
                select: None,
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

    /// **A co-commander must ask before arming a trigger** (`wc3clone-pec`).
    ///
    /// Triggers look like doctrine — standing policy, engine-executed, cheap to
    /// overwrite — and the split still puts them on the propose side, because
    /// the line sits where the COST OF BEING WRONG is rather than where the
    /// verb's shape is. A trigger whose `then` is `train` or `attack` is an
    /// irreversible act that has merely been postponed, and it is *harder* to
    /// veto than the immediate version because it happens when nobody is
    /// looking. `trigger_clear` rides along: silently disarming the rule your
    /// partner is relying on is the same surprise in the other direction.
    #[test]
    fn a_copilot_proposes_triggers_rather_than_arming_them() {
        for intent in [
            Intent::TriggerSet {
                name: "home-guard".to_string(),
                when: TriggerWhen::BaseUnderAttack,
                then: Box::new(Intent::Train {
                    building: Some(1),
                    unit: "Footman".to_string(),
                    select: None,
                }),
                repeat: None,
            },
            Intent::TriggerClear { name: None },
        ] {
            assert!(
                !is_doctrine_verb(&intent),
                "{} must not go direct",
                intent.verb()
            );
            assert!(!direct_allowed(TrustPolicy::Split, &intent));
            assert!(
                !DOCTRINE_VERBS.contains(&intent.verb()),
                "and must not be advertised as direct"
            );
            // `full` is the experiment knob and still means everything direct.
            assert!(direct_allowed(TrustPolicy::Full, &intent));
        }
    }

    /// The refusal has to teach: a model reading it mid-match must be able to
    /// resend without going back to the brief.
    #[test]
    fn a_refusal_shows_the_wrapper() {
        let intent = Intent::Train {
            building: Some(1),
            unit: "Footman".to_string(),
            select: None,
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
                    x: Some(1.0),
                    z: Some(2.0),
                    region: None,
                    select: None,
                },
                2,
            ),
            (
                Intent::Attack {
                    units: vec![7],
                    target: 9,
                    select: None,
                },
                1,
            ),
            (
                Intent::Build {
                    worker: Some(7),
                    kind: "Farm".to_string(),
                    x: Some(0.0),
                    z: Some(0.0),
                    region: None,
                    select: None,
                    site: None,
                },
                1,
            ),
            (
                Intent::Stop {
                    units: vec![7, 8, 9],
                    select: None,
                },
                3,
            ),
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
            posture: Some(PostureIntent::Push { x: Some(12.0), z: Some(-34.0), region: None }),
        };
        assert_eq!(
            intent_pos(&push, Team::Human, &Regions::default()),
            Some(Vec3::new(12.0, 0.0, -34.0))
        );
        assert_eq!(
            intent_pos(
                &Intent::Train {
                    building: Some(1),
                    unit: "Footman".to_string(),
                    select: None,
                },
                Team::Human,
                &Regions::default()
            ),
            None
        );
    }

    // -----------------------------------------------------------------------
    // Severity
    // -----------------------------------------------------------------------

    fn propose(app: &mut App, unit: Entity, note: &str, severity: Option<&str>) {
        let sev = severity
            .map(|s| format!(r#","severity":"{s}""#))
            .unwrap_or_default();
        wire(
            app,
            &format!(
                r#"{{"type":"propose","note":"{note}"{sev},"commands":[
                     {{"type":"move","units":[{}],"x":5.0,"z":5.0}}]}}"#,
                intent_id(unit)
            ),
        );
    }

    fn queue_ids(app: &App) -> Vec<u32> {
        pending(app).iter().map(|p| p.id).collect()
    }

    /// **Urgency buys position, nothing else.** An urgent proposal lands ahead
    /// of every routine one already waiting and behind every urgent one — so
    /// the queue is permanently in ANSWER order and `[Enter]`, which still
    /// just takes index 0, answers the most-urgent-oldest without ui.rs
    /// knowing severity exists.
    #[test]
    fn urgent_proposals_jump_the_queue_and_keep_their_own_order() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);

        propose(&mut app, unit, "expand north", None); // #1 routine
        propose(&mut app, unit, "tech up", Some("routine")); // #2 routine
        propose(&mut app, unit, "they are flanking", Some("urgent")); // #3 urgent
        propose(&mut app, unit, "and the hero is low", Some("urgent")); // #4 urgent

        assert_eq!(
            queue_ids(&app),
            vec![3, 4, 1, 2],
            "urgent first, and among equals still oldest first"
        );
        assert!(pending(&app)[0].is_urgent());
        assert_eq!(pending(&app)[2].severity, ProposalSeverity::Routine);

        // What `[Enter]` takes is index 0 — the urgent one, not the oldest.
        let top = pending(&app)[0].id;
        assert_eq!(top, 3);
        app.world_mut().send_event(ProposalVerdict::approve(top));
        app.update();
        assert_eq!(queue_ids(&app), vec![4, 1, 2]);
    }

    /// Urgency does not buy trust and does not buy room: the cap is about how
    /// many questions a human can hold, and an urgent fifth is still a fifth.
    #[test]
    fn urgency_does_not_raise_the_cap() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        for _ in 0..MAX_PENDING {
            propose(&mut app, unit, "routine", None);
        }
        propose(&mut app, unit, "urgent", Some("urgent"));
        assert_eq!(pending(&app).len(), MAX_PENDING);
        assert!(errors(&app).iter().any(|e| e.contains("proposal queue full")));
    }

    /// Absent severity is routine; a misspelt one is refused with both words,
    /// because a silent downgrade would leave the proposer believing it jumped
    /// a queue it never jumped.
    #[test]
    fn severity_defaults_to_routine_and_a_typo_is_taught() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);

        propose(&mut app, unit, "no severity given", None);
        assert_eq!(pending(&app)[0].severity, ProposalSeverity::Routine);

        propose(&mut app, unit, "typo", Some("urgnet"));
        assert_eq!(pending(&app).len(), 1, "the typo did not queue");
        let err = errors(&app).join(" | ");
        assert!(
            err.contains("unknown severity 'urgnet'")
                && err.contains("routine")
                && err.contains("urgent"),
            "got {err}"
        );
        assert_eq!(ProposalSeverity::NAMES, ["routine", "urgent"]);
    }

    /// An urgent arrival is the louder line in the alert stack, so it is seen
    /// before the human's eye ever reaches the panel.
    #[test]
    fn an_urgent_arrival_is_announced_loudly() {
        let mut app = co_app();
        let unit = footman(&mut app, Vec3::ZERO);
        propose(&mut app, unit, "they are flanking", Some("urgent"));
        let event = app
            .world()
            .resource::<GameEvents>()
            .feed(Team::Human)
            .iter()
            .find(|e| e.message.contains("copilot proposes #1"))
            .cloned()
            .expect("announced");
        assert!(event.message.contains("(urgent)"), "got {}", event.message);
        assert_eq!(event.severity, EventSeverity::Warning);
    }

    // -----------------------------------------------------------------------
    // The scripted approver
    // -----------------------------------------------------------------------

    /// **What makes co-command measurable.** Headless has no human, so without
    /// this every proposal in a sim lapses and the loop is unobservable. The
    /// approver waits its delay — that wait is the thing being modelled, since
    /// delay is what a human's presence actually costs the loop — and then the
    /// batch goes through the ordinary compiler onto a real unit, stamped by
    /// the partner exactly as a keystroke approval would be.
    #[test]
    fn the_scripted_approver_waits_its_delay_then_approves() {
        let mut app = co_app();
        app.world_mut().resource_mut::<Copilot>().auto_approve = Some(3.0);
        let unit = footman(&mut app, Vec3::new(-10.0, 0.0, -10.0));
        propose(&mut app, unit, "take the ford", None);
        assert_eq!(pending(&app).len(), 1);

        // Just short of the delay: still waiting. A rubber stamp would have
        // fired here, and a rubber stamp measures nothing.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(2.5));
        app.update();
        assert_eq!(pending(&app).len(), 1, "not yet — the delay is the point");
        assert_eq!(why_of(&app, unit), NO_PROVENANCE);

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.5));
        app.update();
        assert!(pending(&app).is_empty(), "answered once it came of age");
        assert!(
            matches!(app.world().entity(unit).get::<Order>(), Some(Order::Move(_))),
            "and it went through the ordinary compiler"
        );
        // Stamped by the PARTNER, not by some third author — a scripted
        // approver stands in for the human's keystroke, it does not become a
        // new source, so the provenance a replay shows is the ordinary one.
        assert_eq!(why_of(&app, unit), "order:move by copilot t=4");
        assert_eq!(
            app.world().resource::<Copilot>().resolved.back().map(|r| r.outcome),
            Some(Outcome::Approved),
            "a scripted verdict is a verdict, and lands in the same tail"
        );
    }

    /// The approver honours the queue's answer order, so a sim measuring
    /// urgency measures the same ordering a human would have answered in.
    #[test]
    fn the_scripted_approver_takes_the_urgent_one_first() {
        let mut app = co_app();
        app.world_mut().resource_mut::<Copilot>().auto_approve = Some(5.0);
        let unit = footman(&mut app, Vec3::ZERO);

        propose(&mut app, unit, "routine, asked first", None); // #1
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(4.0));
        app.update();
        propose(&mut app, unit, "urgent, asked second", Some("urgent")); // #2
        assert_eq!(queue_ids(&app), vec![2, 1]);

        // At t=5+ the routine one is of age and the urgent one is not, so the
        // approver takes only what is ripe — and when both are, order holds.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.5));
        app.update();
        assert_eq!(queue_ids(&app), vec![2], "#1 came of age and was answered");
    }

    /// Off unless asked for. A real match must never have a robot answering
    /// for the human, so the default is the absence of this system's effect.
    #[test]
    fn the_scripted_approver_is_off_by_default() {
        let mut app = co_app();
        assert_eq!(
            app.world().resource::<Copilot>().auto_approve,
            None,
            "co_app() builds the resource the way a real match does"
        );
        let unit = footman(&mut app, Vec3::ZERO);
        propose(&mut app, unit, "spend it all", None);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(10.0));
        app.update();
        assert_eq!(pending(&app).len(), 1, "nobody answered, and nobody should");
        assert_eq!(why_of(&app, unit), NO_PROVENANCE);
    }

    /// The wire vocabulary is a contract: the snapshot advertises these words
    /// and a co-commander matches on them. Renaming one silently is the bug
    /// this catches.
    #[test]
    fn the_reason_vocabulary_is_stable() {
        let wire: Vec<&str> = VetoReason::all().iter().map(|r| r.wire()).collect();
        assert_eq!(wire, vec!["not_now", "never", "wrong_target"]);
        for reason in VetoReason::all() {
            assert!(!reason.advice().is_empty(), "{reason:?} must ask for something");
            assert!(!reason.phrase().is_empty());
        }
        assert_eq!(VetoReason::default(), VetoReason::NotNow);
        assert_eq!(Outcome::Approved.name(), "approved");
        assert_eq!(Outcome::Vetoed(VetoReason::Never).name(), "vetoed");
        assert_eq!(Outcome::Expired.name(), "expired");
    }
}
