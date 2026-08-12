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
    } else {
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bridgehead — Human vs Claude".into(),
                resolution: window_resolution(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(ui::UiPlugin);
        // A windowed run normally ends when the player closes it. Setting the
        // cap opts a windowed run into the same self-termination headless has,
        // which is what lets an unattended session open a window, photograph
        // itself (`BH_SHOT_AT`) and get out of the way. Registered only when
        // the variable is set, so an ordinary game is untouched.
        if std::env::var("BH_MAX_GAME_SECS").is_ok() {
            app.add_systems(Update, headless_exit.in_set(shared::SimSet::Feed));
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
}

/// Headless runs terminate themselves: shortly after a decisive game over, or
/// at a game-time cap (default 1800s) with a score-based verdict so even a
/// stalemate produces a winner.
fn headless_exit(
    time: Res<Time>,
    game_over: Res<shared::GameOver>,
    economies: Res<shared::Economies>,
    units: Query<(&shared::Unit, &shared::Team)>,
    buildings: Query<(&shared::Building, &shared::Team)>,
    decided_at: Local<Option<f32>>,
    mut exit: EventWriter<AppExit>,
) {
    use shared::Team;
    // No time limit by default — matches end when a base falls. Setting
    // BH_MAX_GAME_SECS opts an automated run into a safety cap (with a
    // score-based verdict) so unattended sims can't spin forever.
    let Some(cap) = std::env::var("BH_MAX_GAME_SECS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    else {
        return check_decided(time, game_over, decided_at, exit);
    };
    if time.elapsed_secs() > cap {
        let score = |team: Team| {
            shared::asset_score(
                economies.get(team),
                units.iter().filter(|(_, t)| **t == team).map(|(u, _)| u.kind),
                buildings.iter().filter(|(_, t)| **t == team).map(|(b, _)| b.kind),
            )
        };
        let (human, claude) = (score(Team::Human), score(Team::Claude));
        let verdict = match human.cmp(&claude) {
            std::cmp::Ordering::Greater => "Human wins on score",
            std::cmp::Ordering::Less => "Claude wins on score",
            std::cmp::Ordering::Equal => "dead even",
        };
        info!(
            "headless: time cap {cap}s — timeout verdict: {verdict} (Human {human} vs Claude {claude})"
        );
        exit.write(AppExit::Success);
        return;
    }
    check_decided(time, game_over, decided_at, exit);
}

/// Exit shortly after a real, decisive game over — the only ending the game
/// itself recognizes.
fn check_decided(
    time: Res<Time>,
    game_over: Res<shared::GameOver>,
    mut decided_at: Local<Option<f32>>,
    mut exit: EventWriter<AppExit>,
) {
    if let Some(winner) = game_over.winner {
        let decided = *decided_at.get_or_insert(time.elapsed_secs());
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
            info!("headless: game over — {winner:?} wins ({reason}) at t={decided:.1}s — exiting");
            exit.write(AppExit::Success);
        }
    }
}
