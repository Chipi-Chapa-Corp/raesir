//! Read-only diagnostic for the real Thunderstore catalog and Exposed Games profiles.
use exposed_games_backend::{catalog, mods};
use std::path::PathBuf;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let id = args.get(1).expect("Steam app ID");
    let query = args.get(2).map(|s| s.to_lowercase()).unwrap_or_default();
    let root = PathBuf::from(
        std::env::var_os("EXPOSED_GAMES_MOD_PROBE_DIR")
            .expect("Set EXPOSED_GAMES_MOD_PROBE_DIR to a diagnostic directory"),
    );
    let manager = mods::Manager::new(&root.join("data"), &root.join("cache"));
    let game = catalog::entry("steam", id, "Mod diagnostic", vec![], "diagnostic");
    let snapshot = manager
        .snapshot(&game, false)
        .expect("Thunderstore catalog");
    println!(
        "Community: {}, {} packages, selected {}",
        snapshot.community,
        snapshot.packages.len(),
        snapshot.selected
    );
    for p in &snapshot.profiles {
        println!(
            "Profile: {} / {}, {} mods",
            p.source,
            p.name,
            p.installed.len()
        );
    }
    for p in snapshot
        .packages
        .iter()
        .filter(|p| {
            format!("{} {}", p.id, p.description)
                .to_lowercase()
                .contains(&query)
        })
        .take(30)
    {
        println!("{} {}: {} ({})", p.id, p.version, p.description, p.page);
    }
}
