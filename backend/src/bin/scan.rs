//! Read-only scanner diagnostics, without opening a desktop or accessing the API.
use exposed_games_backend::{
    inventory,
    model::{Os, OwnedLibrary, ScanOptions, Source},
};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--download-worker" {
        exposed_games_backend::downloads::run_worker(std::path::Path::new(&args[2]));
        return;
    }
    if args.len() == 6 && args[1] == "--steam-control" {
        let appid = args[4].to_string_lossy().parse().unwrap_or(0);
        exposed_games_backend::steam_control::helper(
            std::path::Path::new(&args[2]),
            appid,
            &args[3].to_string_lossy(),
            std::path::Path::new(&args[5]),
        );
        return;
    }
    if args.len() == 6 && args[1] == "--game-details" {
        let game = exposed_games_backend::catalog::entry(
            &args[2].to_string_lossy(),
            &args[3].to_string_lossy(),
            &args[4].to_string_lossy(),
            vec![],
            "diagnostic",
        );
        let result = exposed_games_backend::details::fetch(&game, std::path::Path::new(&args[5]));
        match result {
            Ok(data) => println!("{}", serde_json::to_string_pretty(&data).unwrap()),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() == 3 && args[1] == "--steam-catalog" {
        let result = exposed_games_backend::steam_ipc::catalog(std::path::Path::new(&args[2]));
        println!("{}", serde_json::to_string(&result).unwrap());
        return;
    }
    let mut options = ScanOptions::default();
    for path in std::env::args().skip(1) {
        options.sources.push(Source {
            path,
            os: Os::Unknown,
        });
    }
    let report = inventory::scan(&options, &OwnedLibrary::default(), &[], None);
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
