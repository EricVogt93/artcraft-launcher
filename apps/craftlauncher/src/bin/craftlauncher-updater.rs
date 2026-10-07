#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use clap::Parser;
#[derive(Parser)]
struct Args {
    #[arg(long)]
    data_dir: std::path::PathBuf,
}
fn main() {
    let args = Args::parse();
    match craftlauncher_core::self_update::apply_job(&args.data_dir) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("Launcher startup failed; the previous version was restored.");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("Update failed: {e}");
            std::process::exit(1);
        }
    }
}
