#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use clap::Parser;
#[derive(Parser)]
struct Args {
    #[arg(long)]
    data_dir: Option<std::path::PathBuf>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<std::ffi::OsString>,
}
fn main() {
    if let Err(e) = run() {
        eprintln!("CraftLauncher: {e}");
        std::process::exit(1);
    }
}
fn run() -> craftlauncher_core::Result<()> {
    let args = Args::parse();
    let root = args
        .data_dir
        .unwrap_or(craftlauncher_core::Manager::default_root()?);
    let executable =
        if let Some(executable) = craftlauncher_core::self_update::current_executable(&root)? {
            executable
        } else {
            let own = std::env::current_exe()?;
            own.parent()
                .ok_or_else(|| craftlauncher_core::fail("Invalid launcher location."))?
                .join(if cfg!(windows) {
                    "craftlauncher.exe"
                } else {
                    "craftlauncher"
                })
        };
    let status = std::process::Command::new(executable)
        .arg("--data-dir")
        .arg(root)
        .args(args.args)
        .status()?;
    if !status.success() {
        return Err(craftlauncher_core::fail(
            "The launcher exited unsuccessfully.",
        ));
    }
    Ok(())
}
