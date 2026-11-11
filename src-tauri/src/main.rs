#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    {
        use workshop_downloader::downloads::sandbox;
        let mut args = std::env::args_os().skip(1);
        if args.next().is_some_and(|arg| arg == sandbox::CHILD_FLAG) {
            if let Err(error) = sandbox::exec(args) {
                eprintln!("{} {error}", sandbox::START_ERROR);
                return std::process::ExitCode::FAILURE;
            }
            return std::process::ExitCode::SUCCESS;
        }
    }
    match workshop_downloader::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Workshop Downloader could not start: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
