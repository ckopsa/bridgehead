use bevy::prelude::*;

mod ai;
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

/// `WC3_WINDOW=800x600` opens the game at a given logical size.
///
/// It exists because the HUD has a *narrow* failure mode and no way to
/// reproduce it: a tiling WM hands the game whatever the tile is, and "does
/// the console still fit at 800 wide?" was previously a question you could
/// only answer by resizing a window with a mouse. Unset keeps Bevy's default.
fn window_resolution() -> bevy::window::WindowResolution {
    use bevy::window::WindowResolution;
    let default = WindowResolution::default();
    let Ok(raw) = std::env::var("WC3_WINDOW") else {
        return default;
    };
    let parsed = raw.trim().split_once(['x', 'X']).and_then(|(w, h)| {
        Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
    });
    match parsed {
        Some((w, h)) if w >= 320.0 && h >= 240.0 => WindowResolution::new(w, h),
        _ => {
            eprintln!("WC3_WINDOW=\"{raw}\" is not a WxH of at least 320x240 — ignoring");
            default
        }
    }
}

fn main() {
    // WC3_HEADLESS=1: full-fidelity simulation with no window, no renderer,
    // no GPU — for agents, CI, and balance testing. Combine with WC3_SPEED,
    // WC3_AI_BOTH, and WC3_BRIDGE; exits on game over or WC3_MAX_GAME_SECS.
    let headless = env_truthy("WC3_HEADLESS");

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
                title: "WC3 Clone — Human vs Claude".into(),
                resolution: window_resolution(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(ui::UiPlugin);
        // A windowed run normally ends when the player closes it. Setting the
        // cap opts a windowed run into the same self-termination headless has,
        // which is what lets an unattended session open a window, photograph
        // itself (`WC3_SHOT_AT`) and get out of the way. Registered only when
        // the variable is set, so an ordinary game is untouched.
        if std::env::var("WC3_MAX_GAME_SECS").is_ok() {
            app.add_systems(Update, headless_exit.in_set(shared::SimSet::Feed));
        }
    }

    // WC3_FIXED_DT=0.05: every frame advances the clock by exactly 50ms rather
    // than by however long the frame took. Together with WC3_SEED this is what
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
                 (WC3_SPEED is ignored)",
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
        // WC3_COMMAND_LATENCY is set, so v1 behaviour is the default.
        command::CommandPlugin,
        terrain::TerrainPlugin { headless },
        units::UnitsPlugin,
        combat::CombatPlugin,
        economy::EconomyPlugin,
        ai::AiPlugin,
        bridge::BridgePlugin,
        // Co-command: the negotiation layer between a co-commander's wire and
        // the compiler. Inert unless `WC3_BRIDGE=copilot` seats one.
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
        ))
        .add_systems(Update, headless_exit.in_set(shared::SimSet::Feed));

        // Two frames, not one: the first builds and validates the schedule,
        // the second proves the world it left behind is one the same schedule
        // can step again.
        app.update();
        app.update();
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
    // WC3_MAX_GAME_SECS opts an automated run into a safety cap (with a
    // score-based verdict) so unattended sims can't spin forever.
    let Some(cap) = std::env::var("WC3_MAX_GAME_SECS")
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
