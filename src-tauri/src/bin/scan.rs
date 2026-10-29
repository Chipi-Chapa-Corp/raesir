//! Read-only scanner diagnostics, without opening a desktop or accessing the API.
use raesir_lib::{
    model::{Os, OwnedLibrary, ScanOptions, Source},
    steam,
};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3 && args[1] == "--steam-catalog" {
        let result = raesir_lib::steam_ipc::catalog(std::path::Path::new(&args[2]));
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
    let report = steam::scan(&options, &OwnedLibrary::default(), None);
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
