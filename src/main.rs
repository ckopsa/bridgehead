use bevy::prelude::*;

mod ai;
mod alarm;
mod bounty;
mod bridge;
mod combat;
mod command;
mod copilot;
mod data;
mod doctrine;
mod economy;
mod hotkeys;
mod intent;
mod plan;
mod shared;
mod terrain;
mod trigger;
mod ui;
mod units;

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// `BH_WINDOW=800x600` opens the game at a given logical size.
///
/// It exists because the HUD has a *narrow* failure mode and no way to
/// reproduce it: a tiling WM hands the game whatever the tile is, and "does
/// the console still fit at 800 wide?" was previously a question you could
/// only answer by resizing a window with a mouse. Unset keeps Bevy's default.
fn window_resolution() -> bevy::window::WindowResolution {
    use bevy::window::WindowResolution;
    let default = WindowResolution::default();
    let Ok(raw) = std::env::var("BH_WINDOW") else {
        return default;
    };
    let parsed = raw.trim().split_once(['x', 'X']).and_then(|(w, h)| {
        Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
    });
    match parsed {
        Some((w, h)) if w >= 320.0 && h >= 240.0 => WindowResolution::new(w, h),
        _ => {
            eprintln!("BH_WINDOW=\"{raw}\" is not a WxH of at least 320x240 — ignoring");
            default
        }
    }
}

/// **What paces a windowed frame: the display, or a clock we own.**
///
/// Two settings — the swapchain's present mode and winit's update mode —
/// decided together because they answer one question. `Display` is Bevy's
/// default pair and the right one for a human at a mouse: `AutoVsync` (FIFO)
/// hands each frame to the compositor's queue and waits its turn, and
/// `Continuous` re-runs the app as fast as the redraws come back. `Unblocked`
/// is for a run nobody is holding the mouse for: `AutoNoVsync` resolves to
/// `Immediate` (falling back through `Mailbox` to `Fifo`; the two `Auto` modes
/// are the only ones wgpu promises not to *crash* on when the surface does not
/// support them), so presenting is fire-and-forget rather than a slot in a
/// queue somebody else has to drain — and the update loop is woken by a 60 Hz
/// timer instead of by a redraw round-trip through the window system.
///
/// **Why it exists: arena r32.** A windowed round on Hyprland/XWayland wedged
/// at t=1495.7 of an 1800s cap with the window parked on an inactive
/// workspace: every thread futex-parked, ~zero CPU, the game clock stopped,
/// the snapshot five minutes stale — and no recovery when the workspace came
/// back. It had to be killed by PID. The shape is a presenter whose consumer
/// stopped consuming: under FIFO the frame *after* the queue fills blocks, and
/// because Bevy's pipelined renderer hands the render app back to the main
/// schedule once per frame, a blocked present stops the **simulation** — a
/// windowed arena round therefore stalls silently whenever the compositor
/// stops taking frames. `Unblocked` removes the queue that fills and removes
/// the dependency on redraw delivery; it does not make the GPU path
/// unwedgeable, which is what `BH_WATCHDOG` below is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WindowPacing {
    Display,
    Unblocked,
}

/// `BH_PRESENT=vsync|novsync` — force one pacing or the other.
const PRESENT_ENV: &str = "BH_PRESENT";

/// `Unblocked` when nobody is watching, `Display` when somebody is.
///
/// The default turns on `BH_MAX_GAME_SECS`, which is already this file's mark
/// of an unattended windowed run (it is what registers `headless_exit` for a
/// window, so the session can open, photograph itself and get out of the way).
/// A human's game keeps vsync — tearing and an uncapped GPU are a real cost
/// and a human is present to notice a freeze; an arena round is neither.
fn window_pacing(unattended: bool) -> WindowPacing {
    window_pacing_of(&std::env::var(PRESENT_ENV).unwrap_or_default(), unattended)
}

/// The decision itself, with the environment lifted out — env is process-global
/// and this suite runs in parallel, so the testable half takes the string.
fn window_pacing_of(raw: &str, unattended: bool) -> WindowPacing {
    let default = if unattended {
        WindowPacing::Unblocked
    } else {
        WindowPacing::Display
    };
    match raw.trim() {
        "" => default,
        "vsync" | "display" => WindowPacing::Display,
        "novsync" | "unblocked" => WindowPacing::Unblocked,
        other => {
            eprintln!(
                "{PRESENT_ENV}=\"{other}\" is not vsync or novsync — ignoring \
                 (vsync: present in step with the display; novsync: never \
                 block the update loop on a present)"
            );
            default
        }
    }
}

impl WindowPacing {
    fn present_mode(self) -> bevy::window::PresentMode {
        use bevy::window::PresentMode;
        match self {
            WindowPacing::Display => PresentMode::AutoVsync,
            WindowPacing::Unblocked => PresentMode::AutoNoVsync,
        }
    }

    /// `None` keeps Bevy's own default (`WinitSettings::game()`).
    ///
    /// The `Unblocked` pair mirrors that default with one substitution:
    /// `Continuous` becomes a 60 Hz `Reactive`. Continuous on Linux parks the
    /// event loop in `ControlFlow::Wait` and relies on its own redraw request
    /// coming back to it; `Reactive` arms a `WaitUntil`, which is a wakeup the
    /// window system cannot fail to deliver. It also caps an unattended round
    /// at 60 updates a second, which vsync used to do and `AutoNoVsync` no
    /// longer will.
    fn winit_settings(self) -> Option<bevy::winit::WinitSettings> {
        use bevy::winit::{UpdateMode, WinitSettings};
        let frame = std::time::Duration::from_secs_f64(1.0 / 60.0);
        match self {
            WindowPacing::Display => None,
            WindowPacing::Unblocked => Some(WinitSettings {
                focused_mode: UpdateMode::reactive(frame),
                unfocused_mode: UpdateMode::reactive_low_power(frame),
            }),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            WindowPacing::Display => "vsync — presents in step with the display",
            WindowPacing::Unblocked => {
                "novsync — presents never block the update loop, updates on a 60Hz timer"
            }
        }
    }
}

fn main() {
    // BH_HEADLESS=1: full-fidelity simulation with no window, no renderer,
    // no GPU — for agents, CI, and balance testing. Combine with BH_SPEED,
    // BH_AI_BOTH, and BH_BRIDGE; exits on game over or BH_MAX_GAME_SECS.
    let headless = env_truthy("BH_HEADLESS");

    let mut app = App::new();
    if headless {
        app.add_plugins((
            MinimalPlugins,
            bevy::log::LogPlugin::default(),
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
        ))
        // Gameplay systems allocate meshes/materials; without a renderer the
        // handles are inert data, but the Assets stores must exist.
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        // Hotkey systems (speed keys, F9) read input resources normally
        // provided by InputPlugin; empty ones keep them satisfied and inert.
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        // Reads the finished frame and decides whether there is another one;
        // that is reporting, not simulation.
        .add_systems(Update, headless_exit.in_set(shared::SimSet::Feed));
        // Opt-in only without a window: a headless run has no presenter to
        // wedge, so the default is off and `BH_WATCHDOG=<secs>` is for the
        // person who suspects something else has stopped the loop.
        if let Some(stall) = watchdog_stall_secs(false) {
            app.add_systems(Update, watchdog_heartbeat.in_set(shared::SimSet::Feed));
            arm_watchdog(stall, watchdog_abort_secs());
        }
    } else {
        // A windowed run normally ends when the player closes it. Setting the
        // cap opts a windowed run into the same self-termination headless has,
        // which is what lets an unattended session open a window, photograph
        // itself (`BH_SHOT_AT`) and get out of the way. It is therefore also
        // the flag that says "nobody is watching this window", which is what
        // `window_pacing` reads it as.
        let unattended = std::env::var("BH_MAX_GAME_SECS").is_ok();
        let pacing = window_pacing(unattended);
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bridgehead — Human vs Claude".into(),
                resolution: window_resolution(),
                present_mode: pacing.present_mode(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(ui::UiPlugin);
        // After the plugin group, for the same reason `TimeUpdateStrategy` is
        // below it: `WinitPlugin` has by now run `init_resource::<WinitSettings>`,
        // and `insert_resource` overwrites that default — which is the
        // intended direction.
        if let Some(settings) = pacing.winit_settings() {
            app.insert_resource(settings);
        }
        info!("{PRESENT_ENV}: {}", pacing.describe());
        // Registered only when the variable is set, so an ordinary game is
        // untouched.
        if unattended {
            app.add_systems(Update, headless_exit.in_set(shared::SimSet::Feed));
        }
        // The stall detector, armed by default for exactly the runs that have
        // nobody to notice a freeze. See `watchdog_stall_secs`.
        if let Some(stall) = watchdog_stall_secs(unattended) {
            app.add_systems(Update, watchdog_heartbeat.in_set(shared::SimSet::Feed));
            arm_watchdog(stall, watchdog_abort_secs());
        }
    }

    // BH_FIXED_DT=0.05: every frame advances the clock by exactly 50ms rather
    // than by however long the frame took. Together with BH_SEED this is what
    // makes a run reproducible — without it the sim integrates a wall-clock
    // delta, and no two runs (let alone two machines) agree on it.
    //
    // Headless only. A windowed run is paced by the display and its frames are
    // wall-clock events by definition; a fixed step there would only decouple
    // the game from the screen drawing it.
    //
    // After the plugin groups, for two reasons: the log line needs `LogPlugin`
    // to exist, and `TimePlugin` has by now run its own
    // `init_resource::<TimeUpdateStrategy>()` — `insert_resource` overwrites
    // that default, which is exactly the intended direction.
    match (shared::fixed_time_strategy(), headless) {
        (Some(strategy), true) => {
            info!(
                "{}: fixed tick — the clock advances a constant step per frame \
                 (BH_SPEED is ignored)",
                shared::FIXED_DT_ENV
            );
            app.insert_resource(strategy);
        }
        (Some(_), false) => warn!(
            "{} ignored: the fixed tick is a headless-only mode",
            shared::FIXED_DT_ENV
        ),
        (None, _) => {}
    }

    app.add_plugins((
        shared::CorePlugin,
        // The intent compiler: the one path from a player's meaning to game
        // state. Registered before every interface plugin that submits to it.
        intent::IntentPlugin,
        // Chain of Command (docs/TEMPO.md §3): direct orders take time to
        // reach a unit far from your halls and your hero. Inert unless
        // BH_COMMAND_LATENCY is set, so v1 behaviour is the default.
        command::CommandPlugin,
        terrain::TerrainPlugin { headless },
        units::UnitsPlugin,
        combat::CombatPlugin,
        economy::EconomyPlugin,
        ai::AiPlugin,
        bridge::BridgePlugin,
        // Co-command: the negotiation layer between a co-commander's wire and
        // the compiler. Inert unless `BH_BRIDGE=copilot` seats one.
        copilot::CopilotPlugin,
        doctrine::DoctrinePlugin,
        // Triggers: the CONTINGENT half of doctrine — a condition the engine
        // watches and an intent it submits the moment it holds. Beside
        // DoctrinePlugin because it shares its frame slot and its argument.
        trigger::TriggerPlugin,
        // Sequenced standing policy. Ordered against TriggerPlugin from inside
        // plan.rs, where the reasoning for the edge lives.
        plan::PlanPlugin,
        bounty::BountyPlugin,
        alarm::AlarmPlugin,
    ))
    .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one test that can only live here: every other module tests its own
    /// systems in a hand-built app, so nothing else ever sees the real
    /// composition. `SimSet` spans eleven plugins, and a set ordering is only
    /// checked for cycles when the schedule is first built — a contradiction
    /// (say, a system filed in `Input` that also declares `.after(CopilotSet)`)
    /// compiles fine and panics the first time anyone runs the game. This
    /// assembles the headless app exactly as `main` does and steps it, so that
    /// panic happens in CI instead of in a match.
    #[test]
    fn the_whole_game_schedules_without_a_cycle() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_plugins((
            shared::CorePlugin,
            intent::IntentPlugin,
            command::CommandPlugin,
            terrain::TerrainPlugin { headless: true },
            units::UnitsPlugin,
            combat::CombatPlugin,
            economy::EconomyPlugin,
            ai::AiPlugin,
            bridge::BridgePlugin,
            copilot::CopilotPlugin,
            doctrine::DoctrinePlugin,
            trigger::TriggerPlugin,
            plan::PlanPlugin,
            bounty::BountyPlugin,
            alarm::AlarmPlugin,
        ))
        .add_systems(Update, headless_exit.in_set(shared::SimSet::Feed));

        // Two frames, not one: the first builds and validates the schedule,
        // the second proves the world it left behind is one the same schedule
        // can step again.
        app.update();
        app.update();
    }

    // -----------------------------------------------------------------------
    // The ready handshake (wc3clone-t0d)
    // -----------------------------------------------------------------------

    /// Seconds of `Time<Real>` each test frame advances. A hand-driven clock,
    /// for the reason `BH_FIXED_DT` exists: the whole claim below is about
    /// what the clock did, and a wall-clock delta would make the numbers a
    /// property of the CI box.
    const TEST_DT: f64 = 0.05;

    /// The real game, whole, on a hand-driven clock — the same composition
    /// `the_whole_game_schedules_without_a_cycle` builds, because an inertness
    /// claim is only worth as much as the number of systems it was tested
    /// against. `BH_BRIDGE` is never set in tests (env is process-global and
    /// the suite runs in parallel), so `bridge_startup` early-returns and the
    /// `ReadyGate` handed in here is the only thing that gates.
    fn handshake_app(gate: shared::ReadyGate) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_plugins((
            shared::CorePlugin,
            intent::IntentPlugin,
            command::CommandPlugin,
            terrain::TerrainPlugin { headless: true },
            units::UnitsPlugin,
            combat::CombatPlugin,
            economy::EconomyPlugin,
            ai::AiPlugin,
            bridge::BridgePlugin,
            copilot::CopilotPlugin,
            doctrine::DoctrinePlugin,
            trigger::TriggerPlugin,
            plan::PlanPlugin,
            bounty::BountyPlugin,
            alarm::AlarmPlugin,
        ));
        // After the plugins, so it overwrites `CorePlugin`'s `init_resource`
        // default rather than being overwritten by it.
        app.insert_resource(gate);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(TEST_DT),
        ));
        app
    }

    fn gate_of(seats: &[(&'static str, shared::Team)], timeout: f32) -> shared::ReadyGate {
        shared::ReadyGate {
            seats: seats
                .iter()
                .map(|(name, team)| shared::ReadySeat {
                    name,
                    team: *team,
                    ready: false,
                })
                .collect(),
            timeout,
            ..Default::default()
        }
    }

    /// A cheap fingerprint of everything that is supposed to stand still: the
    /// two banks, and every unit's and building's position and health.
    fn world_state(app: &mut App) -> Vec<String> {
        let mut out = Vec::new();
        let economies = app.world().resource::<shared::Economies>();
        for team in [shared::Team::Human, shared::Team::Claude] {
            let e = economies.get(team);
            out.push(format!(
                "{team:?} gold={} lumber={} supply={}/{}",
                e.gold, e.lumber, e.supply_used, e.supply_cap
            ));
        }
        let mut units: Vec<String> = app
            .world_mut()
            .query::<(Entity, &Transform, &shared::Health)>()
            .iter(app.world())
            .map(|(e, tf, hp)| {
                format!(
                    "{:?} {:.4},{:.4},{:.4} hp={:.4}",
                    e, tf.translation.x, tf.translation.y, tf.translation.z, hp.current
                )
            })
            .collect();
        units.sort();
        out.extend(units);
        out
    }

    /// **A held match is inert.** The hold is a paused `Time<Virtual>`, and the
    /// claim that buys is total: not "the clock reads zero" but "no accumulator
    /// in the sim advanced". Worker mining, construction, training queues,
    /// spawns, doctrine, the scripted AI's macro decisions — all of them
    /// integrate a virtual delta, so all of them integrate zero.
    ///
    /// Two hundred frames at a hand-driven 0.05s: ten game-seconds of world if
    /// the pause failed, which is enough for five workers to have walked to a
    /// mine and banked gold. The economies and the transforms are compared
    /// byte-for-byte against the opening position.
    #[test]
    fn a_held_match_is_inert() {
        let mut app = handshake_app(gate_of(
            &[("red", shared::Team::Claude), ("blue", shared::Team::Human)],
            600.0,
        ));
        // Frame one builds the world (`initial_spawns` is `Startup`), so the
        // baseline is taken after it — otherwise this would only prove the
        // world is empty, which it is, before anything has spawned.
        app.update();
        let opening = world_state(&mut app);
        assert!(
            opening.len() > 4,
            "the opening position should have spawned units and halls, got {opening:?}"
        );

        for _ in 0..200 {
            app.update();
        }

        assert_eq!(
            app.world().resource::<Time>().elapsed_secs(),
            0.0,
            "the game clock moved while the match was held"
        );
        assert!(
            app.world().resource::<Time<Virtual>>().is_paused(),
            "virtual time should still be paused"
        );
        assert_eq!(
            world_state(&mut app),
            opening,
            "the world moved while the match was held"
        );
        // ...and the real clock DID advance, which is what lets the bridge keep
        // writing snapshots and reading commands through the hold. A test that
        // froze both clocks would prove inertness by proving nothing ran.
        let real = app.world().resource::<Time<Real>>().elapsed_secs();
        assert!(
            real > 9.0,
            "real time should have advanced through the hold, got {real}"
        );
        assert!(
            app.world().resource::<shared::ReadyGate>().holding(),
            "nobody readied, so the match should still be held"
        );

        // THE POSITIVE CONTROL, and the half of this test that makes the other
        // half mean anything. An inert world is also what a broken harness
        // produces — no terrain, no nav grid, nothing spawned, a schedule that
        // silently does not run. So: release the same app, step it the same
        // number of frames, and require that it moves. If this assertion ever
        // fails the one above is worthless, and they fail together.
        app.world_mut().send_event(shared::MatchReady {
            team: shared::Team::Claude,
        });
        app.world_mut().send_event(shared::MatchReady {
            team: shared::Team::Human,
        });
        app.update();
        for _ in 0..200 {
            app.update();
        }
        assert!(
            app.world().resource::<Time>().elapsed_secs() > 9.0,
            "the released clock should have run ten seconds"
        );
        assert_ne!(
            world_state(&mut app),
            opening,
            "the world did not move even after the hold was released — this \
             harness cannot detect motion, so the inertness claim above is empty"
        );
    }

    /// **The last seat to speak starts the match**, and it starts from t=0 —
    /// the whole point of the hold. Red readies, the match stays held; blue
    /// readies, the clock runs.
    #[test]
    fn the_last_seat_to_ready_starts_the_match() {
        let mut app = handshake_app(gate_of(
            &[("red", shared::Team::Claude), ("blue", shared::Team::Human)],
            600.0,
        ));
        app.update();

        app.world_mut().send_event(shared::MatchReady {
            team: shared::Team::Claude,
        });
        app.update();
        let gate = app.world().resource::<shared::ReadyGate>();
        assert!(gate.holding(), "one seat of two readied — still held");
        assert_eq!(gate.waiting_for(), vec!["blue"], "red has been heard");
        assert_eq!(app.world().resource::<Time>().elapsed_secs(), 0.0);

        app.world_mut().send_event(shared::MatchReady {
            team: shared::Team::Human,
        });
        app.update();
        let gate = app.world().resource::<shared::ReadyGate>();
        assert!(gate.started, "both seats readied — the match should start");
        assert!(!gate.started_by_timeout, "this was a handshake, not a timeout");
        assert!(gate.waiting_for().is_empty());
        assert!(!app.world().resource::<Time<Virtual>>().is_paused());
        // The clock is released on the frame the last seat speaks, so it is
        // still reading zero: play begins at t=0 for both sides, which is the
        // fairness claim the whole mechanism exists to make.
        assert_eq!(
            app.world().resource::<Time>().elapsed_secs(),
            0.0,
            "the match must begin at t=0, not at the wall time the seats took"
        );

        // Both feeds carry the same line — neither side has to infer the start
        // from the other's behaviour.
        let feed = app.world().resource::<shared::GameEvents>();
        for team in [shared::Team::Human, shared::Team::Claude] {
            assert!(
                feed.feed(team).iter().any(|e| e.message.contains("match start")),
                "{team:?} was never told the match started"
            );
        }

        // ...and now it runs.
        for _ in 0..20 {
            app.update();
        }
        let t = app.world().resource::<Time>().elapsed_secs();
        assert!(t > 0.9 && t < 1.1, "the clock should run from 0, got t={t}");
    }

    /// **A dead seat cannot hang a match.** The timeout is the whole reason an
    /// unattended arena round can use this mechanic at all: an agent that
    /// crashes before it connects costs the round its opening, not its
    /// existence. The start is recorded as a timeout so the log, the feed and
    /// any after-action report agree about how this match began.
    #[test]
    fn a_dead_seat_cannot_hang_the_match() {
        // One second of wall clock = 20 frames at TEST_DT.
        let mut app = handshake_app(gate_of(
            &[("red", shared::Team::Claude), ("blue", shared::Team::Human)],
            1.0,
        ));
        app.update();
        app.world_mut().send_event(shared::MatchReady {
            team: shared::Team::Claude,
        });
        app.update();
        assert!(app.world().resource::<shared::ReadyGate>().holding());

        for _ in 0..25 {
            app.update();
        }
        let gate = app.world().resource::<shared::ReadyGate>();
        assert!(gate.started, "the timeout should have started the match");
        assert!(
            gate.started_by_timeout,
            "a timeout start must not be recorded as a clean handshake"
        );
        let feed = app.world().resource::<shared::GameEvents>();
        let line = feed
            .feed(shared::Team::Claude)
            .iter()
            .find(|e| e.message.contains("match start"))
            .map(|e| e.message.clone())
            .expect("the feed should carry a match start line");
        assert!(
            line.contains("timeout") && line.contains("blue"),
            "the note must name the timeout and the silent seat, got {line:?}"
        );
    }

    /// **Scripted and autopilot seats are born ready.** A faction in the
    /// scripted AI's hands has no map to read, so it gates nothing — otherwise
    /// `BH_BRIDGE=red` (one commander against the scripted AI) could never
    /// start, and a commander could hang a match by autopiloting and walking
    /// away. Checked live rather than at startup, so the release works whenever
    /// the handback happens.
    #[test]
    fn a_scripted_seat_is_born_ready() {
        // The gate lists both sides, but only Human has a live commander:
        // `AiControlled` defaults to Claude-is-scripted.
        let mut app = handshake_app(gate_of(
            &[("red", shared::Team::Claude), ("blue", shared::Team::Human)],
            600.0,
        ));
        app.update();
        let gate = app.world().resource::<shared::ReadyGate>();
        assert!(gate.holding(), "the human seat has not readied yet");
        assert_eq!(
            gate.waiting_for(),
            vec!["blue"],
            "the scripted side should already be ready"
        );

        app.world_mut().send_event(shared::MatchReady {
            team: shared::Team::Human,
        });
        app.update();
        assert!(app.world().resource::<shared::ReadyGate>().started);
        assert_eq!(app.world().resource::<Time>().elapsed_secs(), 0.0);
    }

    // -----------------------------------------------------------------------
    // The exit waits for the bridge (wc3clone-0i9)
    // -----------------------------------------------------------------------

    /// `headless_exit` alone, on the hand-driven clock, with whatever bridge
    /// the caller wants. Deliberately *not* the whole game: the claim is about
    /// one system's exit condition, and a full match would decide its own game
    /// over on its own schedule.
    fn exit_app(bridge: Option<bridge::Bridge>) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<shared::GameOver>()
            .init_resource::<shared::Economies>()
            .add_systems(Update, headless_exit);
        if let Some(bridge) = bridge {
            app.insert_resource(bridge);
        }
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(TEST_DT),
        ));
        app
    }

    fn decide(app: &mut App) {
        app.world_mut()
            .resource_mut::<shared::GameOver>()
            .decide(shared::Team::Claude, shared::GameOverReason::Razed);
    }

    /// Frames of `TEST_DT` each. The two tests below run 300 of them after the
    /// verdict — fifteen game-seconds, three times `check_decided`'s
    /// post-verdict window — so "did not exit" cannot be read as "has not got
    /// there yet".
    fn run_frames(app: &mut App, frames: usize) {
        for _ in 0..frames {
            app.update();
        }
    }

    /// The historical behaviour, unchanged: no seat, so nothing to wait for.
    #[test]
    fn an_unbridged_run_still_exits_shortly_after_the_verdict() {
        let mut app = exit_app(Some(bridge::Bridge::default()));
        run_frames(&mut app, 20);
        assert!(app.should_exit().is_none(), "no verdict, no exit");
        decide(&mut app);
        run_frames(&mut app, 300);
        assert!(
            app.should_exit().is_some(),
            "a decided match with no bridged seat must still terminate"
        );
    }

    /// **The bug.** A seat that has not yet been handed a snapshot carrying
    /// `game_over` holds the process open. Without this the engine could exit
    /// first and the seat's last `state.json` would say `game_over: null`
    /// forever — which is not a missing file but a hung commander, since the
    /// documented loop is "repeat until `game_over` is non-null".
    #[test]
    fn a_seat_that_has_not_been_told_the_verdict_holds_the_exit_open() {
        let mut app = exit_app(Some(bridge::one_unpublished_seat()));
        decide(&mut app);
        run_frames(&mut app, 300);
        assert!(
            app.should_exit().is_none(),
            "the process may not stop while a commander is still owed the result"
        );
    }

    // -----------------------------------------------------------------------
    // The time cap decides the match it stops (wc3clone-j84)
    // -----------------------------------------------------------------------

    /// **The referee's rule, and the sentence it says.** Both halves are a
    /// contract with `tools/arena_run.py`: it reads the winner out of this
    /// phrase (`TIMECAP`), and it has read `"dead even"` as "no winner" since
    /// round 10. The rule itself is `shared::asset_score`, unchanged — this
    /// test pins that the engine's own verdict agrees with the ledger's.
    #[test]
    fn the_cap_verdict_follows_the_assets_and_says_so_in_the_arena_phrase() {
        assert_eq!(cap_verdict(900, 400), (Some(shared::Team::Human), "Human wins on score"));
        assert_eq!(cap_verdict(400, 900), (Some(shared::Team::Claude), "Claude wins on score"));
        // A tie names nobody — docs/ARENA.md: "a draw is an absent winner, not
        // a sentinel team".
        assert_eq!(cap_verdict(700, 700), (None, "dead even"));
    }

    fn draw(app: &mut App) {
        app.world_mut()
            .resource_mut::<shared::GameOver>()
            .decide_draw(shared::GameOverReason::Score);
    }

    /// **A draw ends the match for the exit, too.** The whole point of
    /// deciding a capped run is that the process stops *after* the seats have
    /// been told; a verdict the exit path did not recognise as a verdict would
    /// leave the run spinning until the harness's wall timeout instead.
    #[test]
    fn a_drawn_match_exits_like_any_other_ending() {
        let mut app = exit_app(Some(bridge::Bridge::default()));
        run_frames(&mut app, 20);
        assert!(app.should_exit().is_none(), "no verdict, no exit");
        draw(&mut app);
        run_frames(&mut app, 300);
        assert!(
            app.should_exit().is_some(),
            "a drawn match must terminate exactly like a won one"
        );
    }

    /// ...and it owes the commanders the same telling first (wc3clone-0i9). A
    /// draw is the ending most likely to reach a bridged seat — every ladder
    /// round runs under a cap.
    #[test]
    fn a_seat_is_owed_the_draw_as_much_as_a_win() {
        let mut app = exit_app(Some(bridge::one_unpublished_seat()));
        draw(&mut app);
        run_frames(&mut app, 300);
        assert!(
            app.should_exit().is_none(),
            "a commander is owed the draw before the process may stop"
        );
    }

    /// **No bridge seat, no handshake.** The default that keeps every existing
    /// sim, every fingerprint run and the whole determinism harness byte-identical:
    /// an empty gate never holds, and the clock runs from the first frame.
    #[test]
    fn a_match_with_no_bridged_seat_is_never_held() {
        let mut app = handshake_app(shared::ReadyGate::default());
        app.update();
        assert!(!app.world().resource::<shared::ReadyGate>().holding());
        assert!(!app.world().resource::<Time<Virtual>>().is_paused());
        for _ in 0..20 {
            app.update();
        }
        assert!(
            app.world().resource::<Time>().elapsed_secs() > 0.9,
            "an ungated match must run from the first frame"
        );
    }

    // -----------------------------------------------------------------------
    // Windowed pacing and the stall detector (wc3clone-yom)
    // -----------------------------------------------------------------------

    /// **A human's window is unchanged.** The whole mitigation is opt-in for
    /// the seat that has somebody sitting in it: vsync, Bevy's own
    /// `WinitSettings`, no watchdog thread. If this test ever flips, an
    /// ordinary game started tearing.
    #[test]
    fn a_human_game_keeps_the_display_pacing_bevy_ships() {
        let pacing = window_pacing_of("", false);
        assert_eq!(pacing, WindowPacing::Display);
        assert_eq!(pacing.present_mode(), bevy::window::PresentMode::AutoVsync);
        assert!(
            pacing.winit_settings().is_none(),
            "Display must leave WinitSettings to Bevy"
        );
        assert_eq!(watchdog_stall_of(None, false), None);
    }

    /// **An unattended window defends itself.** `BH_MAX_GAME_SECS` is this
    /// file's existing mark of a run nobody is watching — an arena round, a
    /// screenshot session — and that is the run r32 froze.
    #[test]
    fn an_unattended_window_never_blocks_its_update_loop_on_a_present() {
        let pacing = window_pacing_of("", true);
        assert_eq!(pacing, WindowPacing::Unblocked);
        assert_eq!(
            pacing.present_mode(),
            bevy::window::PresentMode::AutoNoVsync,
            "the queue that fills under FIFO is the one r32 wedged behind"
        );
        let settings = pacing.winit_settings().expect("Unblocked sets its own");
        // Continuous is the mode whose only wakeup is a redraw round-trip
        // through the window system; Reactive arms a timer instead.
        assert!(
            !matches!(settings.focused_mode, bevy::winit::UpdateMode::Continuous),
            "a timer wakeup is the point"
        );
        assert!(!matches!(
            settings.unfocused_mode,
            bevy::winit::UpdateMode::Continuous
        ));
        assert_eq!(watchdog_stall_of(None, true), Some(WATCHDOG_DEFAULT_SECS));
    }

    /// Both directions can be forced, because the first thing anyone will want
    /// to do with a freeze report is run the same round the other way.
    #[test]
    fn the_present_env_forces_either_pacing_and_shrugs_off_a_typo() {
        assert_eq!(window_pacing_of("novsync", false), WindowPacing::Unblocked);
        assert_eq!(window_pacing_of(" vsync ", true), WindowPacing::Display);
        assert_eq!(
            window_pacing_of("mailbox", true),
            WindowPacing::Unblocked,
            "an unreadable value must fall back to the default, not to a mode \
             wgpu crashes on when the surface lacks it"
        );
    }

    /// An explicit `0` beats the default: somebody debugging a slow machine
    /// must be able to turn the alarm off without also turning off the run.
    #[test]
    fn the_watchdog_can_be_set_disabled_or_mistyped() {
        assert_eq!(watchdog_stall_of(Some("90"), false), Some(90.0));
        assert_eq!(watchdog_stall_of(Some("0"), true), None);
        assert_eq!(watchdog_stall_of(Some(" 12.5 "), false), Some(12.5));
        assert_eq!(
            watchdog_stall_of(Some("soon"), true),
            Some(WATCHDOG_DEFAULT_SECS),
            "a typo falls back to the default rather than silently disarming"
        );
    }

    /// **The heartbeat is a report, not a simulation.** It counts frames and
    /// copies the clock; it takes no `&mut` anything. The claim that it cannot
    /// move the sim is `tools/verify.sh identity`, but this pins the shape.
    #[test]
    fn the_heartbeat_counts_frames_and_touches_nothing_else() {
        use std::sync::atomic::Ordering;
        let before = WATCHDOG_FRAMES.load(Ordering::Relaxed);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f64(TEST_DT),
            ))
            .add_systems(Update, watchdog_heartbeat);
        app.update();
        app.update();
        assert!(
            WATCHDOG_FRAMES.load(Ordering::Relaxed) >= before + 2,
            "two frames, two beats"
        );
        assert!(
            watchdog_game_secs() > 0.0,
            "the stall report names the game second the match stopped at"
        );
    }
}

/// Headless runs terminate themselves: shortly after a game over, or at a
/// game-time cap (`BH_MAX_GAME_SECS`) whose score-based verdict is *recorded*
/// like any other, so even a stalemate ends the match rather than merely
/// stopping the process.
fn headless_exit(
    time: Res<Time>,
    mut game_over: ResMut<shared::GameOver>,
    economies: Res<shared::Economies>,
    units: Query<(&shared::Unit, &shared::Team)>,
    buildings: Query<(&shared::Building, &shared::Team)>,
    // Optional so a harness can register this system without the bridge
    // plugin; the real game always has the resource, empty when no seat is
    // open. See `check_decided`.
    bridge: Option<Res<bridge::Bridge>>,
    decided_at: Local<Option<f32>>,
    exit: EventWriter<AppExit>,
) {
    use shared::{GameOverReason, Team};
    // No time limit by default — matches end when a base falls. Setting
    // BH_MAX_GAME_SECS opts an automated run into a safety cap (with a
    // score-based verdict) so unattended sims can't spin forever.
    let cap = std::env::var("BH_MAX_GAME_SECS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    // `decided()`, not `winner`: past the cap this is true every frame, and the
    // draw it can record names nobody. Without the guard the referee would
    // re-count the assets sixty times a second and overwrite a verdict the
    // seats may already have been told.
    if cap.is_some_and(|cap| time.elapsed_secs() > cap) && !game_over.decided() {
        let cap = cap.unwrap_or_default();
        let score = |team: Team| {
            shared::asset_score(
                economies.get(team),
                units.iter().filter(|(_, t)| **t == team).map(|(u, _)| u.kind),
                buildings.iter().filter(|(_, t)| **t == team).map(|(b, _)| b.kind),
            )
        };
        let (human, claude) = (score(Team::Human), score(Team::Claude));
        // **The verdict goes through `GameOver::decide`, not just into the
        // log.** The log line is how `arena_run.py` has read a capped round
        // since round 10 and it is unchanged; what is new is that the match is
        // now *decided*, which is what puts `game_over` into both seats'
        // final snapshots (`Seat::publishes_now`) and lets a commander's
        // documented poll loop terminate. Before this, a capped round left
        // every bridged seat reading `game_over: null` forever — wc3clone-j84.
        let (winner, verdict) = cap_verdict(human, claude);
        match winner {
            Some(winner) => game_over.decide(winner, GameOverReason::Score),
            // A dead-even cap is a draw, not a silence. It still has to be
            // *said*, or the tie is the one ending that hangs a poller.
            None => game_over.decide_draw(GameOverReason::Score),
        }
        info!(
            "headless: time cap {cap}s — timeout verdict: {verdict} (Human {human} vs Claude {claude})"
        );
    }
    // One exit path for all three endings: whatever decided the match, the
    // bridged seats are told before the process is allowed to stop.
    check_decided(time, &game_over, bridge, decided_at, exit);
}

// ---------------------------------------------------------------------------
// The stall detector (wc3clone-yom)
// ---------------------------------------------------------------------------
//
// Arena r32 froze mid-match and nobody found out for five minutes; the round
// then burned the rest of its wall timeout and had to be killed by PID, and
// the post-mortem got nothing because `ptrace_scope` refused the debugger.
// This is the cheap half of the answer: a thread that is not on the frame's
// critical path, watching the one number that is zero if and only if the
// engine has stopped stepping, and saying so **in the log the round keeps**.
//
// It watches *frames*, not the game clock, deliberately. The game clock is
// legitimately frozen during the ready handshake (up to `BH_READY_TIMEOUT`
// wall seconds), and it is legitimately still while the sim is paused — so a
// game-clock watchdog would cry wolf at the start of every commander round.
// A frame counter that stops means the loop stopped, which is the bug.

/// `BH_WATCHDOG=<wall seconds>` — how long a frozen frame counter is tolerated
/// before the engine says so. `0` disables it.
const WATCHDOG_ENV: &str = "BH_WATCHDOG";

/// `BH_WATCHDOG_ABORT=<wall seconds>` — a longer threshold at which the engine
/// aborts itself. Off (`0`) by default, because killing a live match is the
/// runner's call and not the engine's. When it is on, `abort()` is chosen over
/// `exit()` on purpose: it is the one exit that leaves a **core file**, and a
/// core is a backtrace that does not need `ptrace_scope` to be relaxed — which
/// is exactly the evidence r32 could not produce.
const WATCHDOG_ABORT_ENV: &str = "BH_WATCHDOG_ABORT";

/// Default tolerance for an unattended windowed run. Long enough that a slow
/// asset load, a stop-the-world compositor hiccup or a laptop's lid cannot
/// trip it; far shorter than the five minutes r32 spent frozen before anybody
/// looked.
const WATCHDOG_DEFAULT_SECS: f32 = 45.0;

/// Frames the app has stepped. Written by `watchdog_heartbeat` on the main
/// thread, read by the watchdog thread; `Relaxed` is enough because the only
/// question ever asked of it is "is this the same number as last second".
static WATCHDOG_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The game clock in hundredths, so the stall report can name the moment the
/// match stopped — the number an arena AAR is written in.
static WATCHDOG_GAME_CENTIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn watchdog_stall_secs(default_on: bool) -> Option<f32> {
    watchdog_stall_of(std::env::var(WATCHDOG_ENV).ok().as_deref(), default_on)
}

/// Same split as `window_pacing_of`, for the same reason.
fn watchdog_stall_of(raw: Option<&str>, default_on: bool) -> Option<f32> {
    let default = default_on.then_some(WATCHDOG_DEFAULT_SECS);
    let Some(raw) = raw else { return default };
    match raw.trim().parse::<f32>() {
        // An explicit 0 is "off", and it must beat the default.
        Ok(secs) if secs <= 0.0 => None,
        Ok(secs) => Some(secs),
        // A typo is not a setting: say so rather than silently arming
        // something the author did not ask for.
        Err(_) => {
            eprintln!("{WATCHDOG_ENV}=\"{raw}\" is not a number of seconds — ignoring");
            default
        }
    }
}

fn watchdog_abort_secs() -> Option<f32> {
    std::env::var(WATCHDOG_ABORT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .filter(|secs| *secs > 0.0)
}

/// One `fetch_add` per frame, in `SimSet::Feed` beside the other system that
/// reads a finished frame and reports on it. It mutates nothing in the world,
/// so it cannot move the sim: `tools/verify.sh identity` is the proof.
fn watchdog_heartbeat(time: Res<Time>) {
    use std::sync::atomic::Ordering;
    WATCHDOG_FRAMES.fetch_add(1, Ordering::Relaxed);
    WATCHDOG_GAME_CENTIS.store((time.elapsed_secs().max(0.0) * 100.0) as u64, Ordering::Relaxed);
}

/// Spawn the watcher. Detached and daemon-ish: it holds no lock the frame path
/// wants, which is the whole point — a thread that could be blocked by the
/// wedge it is watching for is not a watchdog.
fn arm_watchdog(stall: f32, abort_after: Option<f32>) {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let abort_note = match abort_after {
        Some(secs) => format!(", aborting at {secs:.0}s ({WATCHDOG_ABORT_ENV})"),
        None => String::new(),
    };
    info!("{WATCHDOG_ENV}: stall detector armed at {stall:.0}s{abort_note}");

    let spawned = std::thread::Builder::new()
        .name("bh-watchdog".into())
        .spawn(move || {
            let poll = Duration::from_secs(1);
            let mut last_frames = 0u64;
            let mut last_change = Instant::now();
            let mut reported = false;
            loop {
                std::thread::sleep(poll);
                let frames = WATCHDOG_FRAMES.load(Ordering::Relaxed);
                // Nothing has been stepped yet: plugin setup, asset loading and
                // GPU init all happen before the first frame, and none of them
                // is the stall this is looking for.
                if frames == 0 {
                    last_change = Instant::now();
                    continue;
                }
                if frames != last_frames {
                    last_frames = frames;
                    last_change = Instant::now();
                    // The exit edge. Told once that something is stuck and
                    // never told it recovered, a reader has to poll — which is
                    // the polling this layer exists to delete
                    // (tools/BUILDER_BRIEF.md §6.11).
                    if reported {
                        reported = false;
                        info!(
                            "{WATCHDOG_ENV}: frames are moving again (t={:.1}s)",
                            watchdog_game_secs()
                        );
                    }
                    continue;
                }
                let idle = last_change.elapsed().as_secs_f32();
                if !reported && idle >= stall {
                    reported = true;
                    error!(
                        "{WATCHDOG_ENV}: the engine has not stepped a frame in {idle:.0}s \
                         — the game clock is stopped at t={:.1}s after {frames} frames. \
                         A windowed run that does this is wedged in the present path \
                         (docs/ARENA.md, \"When a windowed round freezes\"); kill it by \
                         PID and keep the seat snapshots.",
                        watchdog_game_secs()
                    );
                }
                if abort_after.is_some_and(|limit| idle >= limit) {
                    error!(
                        "{WATCHDOG_ABORT_ENV}: {idle:.0}s without a frame — aborting so \
                         this leaves a core file to read"
                    );
                    std::process::abort();
                }
            }
        });
    if let Err(err) = spawned {
        // Not fatal: the game is playable without a watchdog, and a match that
        // refused to start because its *diagnostics* would not start is worse
        // than one that runs unwatched.
        warn!("{WATCHDOG_ENV}: could not spawn the stall detector ({err}) — running unwatched");
    }
}

fn watchdog_game_secs() -> f32 {
    use std::sync::atomic::Ordering;
    WATCHDOG_GAME_CENTIS.load(Ordering::Relaxed) as f32 / 100.0
}

/// **The cap's referee, and the sentence it says.** More assets wins; exactly
/// equal is a draw.
///
/// The rule is not new and is deliberately not re-invented here: it is
/// `shared::asset_score` (bank + the gold-and-lumber worth of every unit and
/// building still standing), which is what the timeout has compared since the
/// cap existed. The *phrase* is not new either — `arena_run.py`'s `TIMECAP`
/// regex has parsed `timeout verdict: <this>` into the arena ledger since
/// round 10, and `"dead even"` is the wording it already reads as "no winner".
/// Keeping both means the ledger a capped round produces is unchanged; what
/// changed in `wc3clone-j84` is only that the engine now *records* the verdict
/// instead of merely printing it.
fn cap_verdict(human: u32, claude: u32) -> (Option<shared::Team>, &'static str) {
    use shared::Team;
    match human.cmp(&claude) {
        std::cmp::Ordering::Greater => (Some(Team::Human), "Human wins on score"),
        std::cmp::Ordering::Less => (Some(Team::Claude), "Claude wins on score"),
        std::cmp::Ordering::Equal => (None, "dead even"),
    }
}

/// Exit shortly after the match is decided — by a raze, a concession, or the
/// cap's referee.
fn check_decided(
    time: Res<Time>,
    game_over: &shared::GameOver,
    bridge: Option<Res<bridge::Bridge>>,
    mut decided_at: Local<Option<f32>>,
    mut exit: EventWriter<AppExit>,
) {
    if game_over.decided() {
        let decided = *decided_at.get_or_insert(time.elapsed_secs());
        // **The commanders learn the result before the process is allowed to
        // stop.** A bridged seat's only channel is its `state.json`, and the
        // documented poll loop is "repeat until `game_over` is non-null" — so
        // an exit that beats the final write does not merely lose a file, it
        // hangs the reader forever (arena r23). `write_snapshot` forces that
        // write on the frame the verdict lands, and this holds the door until
        // it has been attempted for every seat, so the two systems' order
        // inside `SimSet::Feed` is not something anyone has to maintain.
        if bridge.is_some_and(|bridge| bridge.awaiting_verdict()) {
            return;
        }
        if time.elapsed_secs() > decided + 5.0 {
            // The reason, not just the winner: a sim log that cannot tell a
            // razed base from a concession is a log somebody misreads later.
            let reason = game_over
                .reason
                .map(shared::GameOverReason::name)
                .unwrap_or("unknown");
            // The game clock, not the wall clock: an automated run's only
            // record of how long the match was is this line, and every
            // after-action report in the series is written in game seconds.
            //
            // A draw does NOT say "X wins" — `arena_run.py`'s DECISIVE regex
            // is `(\w+) wins`, and a line claiming a winner the record has to
            // spell as `null` (docs/ARENA.md: "a draw is an absent winner") is
            // exactly the kind of disagreement between two readers of one fact
            // this codebase keeps losing days to.
            match game_over.winner {
                Some(winner) => info!(
                    "headless: game over — {winner:?} wins ({reason}) at t={decided:.1}s — exiting"
                ),
                None => info!(
                    "headless: game over — dead even ({reason}) at t={decided:.1}s — exiting"
                ),
            }
            exit.write(AppExit::Success);
        }
    }
}
