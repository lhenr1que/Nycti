//! The `nycti` executable. It stays thin: the grammar, the exchange with the
//! daemon, and the exit codes live in the library (`nycti_windowd::client`).

use std::ffi::OsString;
use std::io;
use std::process::ExitCode;

use nycti_windowd::client::run::run_from_env;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let code = run_from_env(&args, &mut io::stdout().lock(), &mut io::stderr());
    ExitCode::from(code)
}
