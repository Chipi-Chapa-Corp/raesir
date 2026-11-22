//! Explicit provider diagnostic. Downloads only into the required private root;
//! it never launches Noita or any downloaded companion executable.
use raesir_backend::{catalog, mods::Manager};
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let root = PathBuf::from(
        std::env::var_os("RAESIR_MOD_PROBE_DIR")
            .ok_or("Set RAESIR_MOD_PROBE_DIR to a diagnostic directory")?,
    );
    let manager = Manager::new(&root.join("data"), &root.join("cache"));
    let game = catalog::entry("steam", "881100", "Noita", vec![], "diagnostic");
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let snapshot = match args.first().map(String::as_str) {
        Some("--install") => {
            manager.install(&game, args.get(1).ok_or("Supply the catalog mod ID")?)?
        }
        Some("--uninstall") => {
            manager.uninstall(&game, args.get(1).ok_or("Supply the installed mod ID")?)?
        }
        _ => manager.snapshot(&game, false)?,
    };
    println!(
        "{} packages, {} profiles",
        snapshot.packages.len(),
        snapshot.profiles.len()
    );
    for warning in snapshot.warnings {
        println!("Provider warning: {warning}");
    }
    for p in &snapshot.profiles {
        if p.active {
            println!(
                "Selected: {} · {} installed · {}",
                p.name,
                p.installed.len(),
                p.path.display()
            );
            for m in &p.installed {
                println!("  {} · {}", m.id, m.version);
            }
        }
    }
    for source in ["Raesir catalog", "ModWorkshop", "Nexus Mods"] {
        println!(
            "{source}: {}",
            snapshot
                .packages
                .iter()
                .filter(|p| p.source == source)
                .count()
        );
    }
    Ok(())
}
