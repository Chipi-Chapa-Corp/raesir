//! Explicit real-game diagnostic. Run Linux GUI diagnostics only through
//! isolated-gui, with a separate Exposed Games profile prepared by the mod browser.
use exposed_games_backend::{model::Report, mods::Manager};
use std::{fs, path::PathBuf};

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 5 && args[1] == "--steam-mods" {
        exposed_games_backend::mods::steam_helper(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
            std::path::Path::new(&args[4]),
        );
        return Ok(());
    }
    let root = PathBuf::from(
        std::env::var_os("EXPOSED_GAMES_MOD_PROBE_DIR")
            .ok_or("Set EXPOSED_GAMES_MOD_PROBE_DIR to the diagnostic profile directory")?,
    );
    let report: Report =
        serde_json::from_slice(&fs::read(root.join("library.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let key = std::env::args().nth(1).ok_or("Supply the game key")?;
    let game = report
        .catalog
        .iter()
        .find(|g| g.key == key)
        .ok_or("Game is not in the diagnostic library")?;
    let manager = Manager::new(&root.join("data"), &root.join("cache"));
    let extra_args: Vec<_> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let slug = exposed_games_backend::mods::community(game).ok_or("No Thunderstore community")?;
    let profiles = manager.profiles(slug);
    println!(
        "Game: {} · community {slug}, {} profiles",
        game.title,
        profiles.len()
    );
    if std::env::args().any(|a| a == "--launch") {
        if std::env::args().any(|a| a == "--vanilla") {
            manager.launch_vanilla(&report, game, &extra_args)?.wait()?;
        } else {
            manager.launch_modded(&report, game, &extra_args)?.wait()?;
        }
        println!("Game exited and loader staging was restored");
    }
    Ok(())
}
