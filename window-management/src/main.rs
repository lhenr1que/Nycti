//! The `nycti-windowd` executable. It stays thin: the startup, coordinator, and
//! exit-code logic lives in the library (`nycti_windowd::daemon`).

mod signals;

use std::io;
use std::process::ExitCode;

use nycti_windowd::daemon::{self, EXIT_STARTUP_FAILED};

fn main() -> ExitCode {
    // Signals are registered before anything is bound, so a signal during
    // startup is not lost.
    let source = match signals::SignalSource::register() {
        Ok(source) => source,
        Err(error) => {
            eprintln!("nycti-windowd: error: cannot register the signal handlers: {error}");
            return ExitCode::from(EXIT_STARTUP_FAILED);
        }
    };

    let code = daemon::run_from_env(
        source,
        |code| std::process::exit(i32::from(code)),
        &mut io::stderr(),
        io::stderr(),
    );
    ExitCode::from(code)
}
