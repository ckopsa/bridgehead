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
mod shared;
mod terrain;
mod ui;
mod units;

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
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
        .add_systems(Update, headless_exit);
    } else {
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "WC3 Clone — Human vs Claude".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(ui::UiPlugin);
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
        bounty::BountyPlugin,
    ))
    .run();
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
