//! Flooring Partners App. With no arguments it opens the window; with
//! arguments it runs a command-line action (see `FPTools help`).

// A GUI program: no console window flashes up when it is started from the
// Start menu. Command-line use attaches to the caller's console instead.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os().skip(1).map(|a| a.into_string()).collect() {
        Ok(args) => args,
        Err(_) => {
            attach_console();
            eprintln!("Arguments must be valid Unicode text.");
            return ExitCode::from(2);
        }
    };
    #[cfg(windows)]
    if args.is_empty() {
        return fp_tools::gui::run(None);
    }
    // A scan dropped on the program's icon (or opened with it): open the
    // window on Scan to Excel and read it.
    #[cfg(windows)]
    if let [only] = args.as_slice() {
        let path = std::path::Path::new(only);
        if path.is_file() && fp_tools::scan::looks_like_scan(path) {
            return fp_tools::gui::run(Some(path.to_path_buf()));
        }
    }
    attach_console();
    fp_tools::cli::run(&args)
}

fn attach_console() {
    #[cfg(windows)]
    fp_tools::gui::attach_parent_console();
}
