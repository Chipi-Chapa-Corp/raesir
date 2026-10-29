// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 4 && args[1] == "--steam-library-probe" {
        raesir_lib::steam_ipc::helper(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
        );
        return;
    }
    raesir_lib::run()
}
