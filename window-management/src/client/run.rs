//! The `nycti` command: parse, build one request, exchange it with the daemon,
//! and render the outcome. The executable only calls [`run_from_env`].

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use super::args::{self, ChangeCommand, Command, Invocation, Parsed, USAGE, UsageError};
use super::connection::{self, ConnectionError, ExchangeConfig};
use super::output;
use super::wire::{self, Reply, ResponseError};
use crate::protocol::ProtocolError;

/// Success.
pub const EXIT_OK: u8 = 0;
/// A failure of the client itself.
pub const EXIT_INTERNAL: u8 = 1;
/// Incorrect usage.
pub const EXIT_USAGE: u8 = 2;
/// The daemon cannot be reached.
pub const EXIT_DAEMON_UNAVAILABLE: u8 = 3;
/// The daemon answered with a protocol error.
pub const EXIT_DAEMON_ERROR: u8 = 4;
/// The exchange failed: timeout, closed connection, or malformed response.
pub const EXIT_COMMUNICATION: u8 = 5;

/// The correlation ID of the only request of a connection.
const REQUEST_ID: &str = "1";

/// Why a command did not succeed.
#[derive(Debug)]
enum ClientError {
    Usage(UsageError),
    Connection(ConnectionError),
    Response(ResponseError),
    Daemon(ProtocolError),
    /// Standard output could not be written.
    Output,
}

impl ClientError {
    const fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => EXIT_USAGE,
            Self::Daemon(_) => EXIT_DAEMON_ERROR,
            Self::Response(_) => EXIT_COMMUNICATION,
            Self::Output => EXIT_INTERNAL,
            Self::Connection(error) => match error {
                ConnectionError::RuntimeDirUnset
                | ConnectionError::RuntimeDirNotAbsolute
                | ConnectionError::DaemonNotRunning
                | ConnectionError::StaleSocket
                | ConnectionError::PermissionDenied
                | ConnectionError::ConnectFailed(_)
                | ConnectionError::ConnectTimedOut => EXIT_DAEMON_UNAVAILABLE,
                ConnectionError::SendFailed(_)
                | ConnectionError::Closed
                | ConnectionError::ResponseTimedOut
                | ConnectionError::ReceiveFailed(_)
                | ConnectionError::IncompleteResponse
                | ConnectionError::ResponseTooLarge => EXIT_COMMUNICATION,
                ConnectionError::Internal(_) => EXIT_INTERNAL,
            },
        }
    }

    /// The machine-readable code of the `--json` error line. A daemon error
    /// keeps the protocol's own code.
    fn code(&self) -> &'static str {
        match self {
            Self::Usage(error) => error.code(),
            Self::Daemon(error) => output::error_code_name(error.code()),
            Self::Response(_) => "malformed_response",
            Self::Output => "output_failed",
            Self::Connection(error) => match error {
                ConnectionError::RuntimeDirUnset | ConnectionError::RuntimeDirNotAbsolute => {
                    "runtime_dir_unavailable"
                }
                ConnectionError::DaemonNotRunning => "daemon_not_running",
                ConnectionError::StaleSocket => "stale_socket",
                ConnectionError::PermissionDenied => "permission_denied",
                ConnectionError::ConnectFailed(_) => "connect_failed",
                ConnectionError::ConnectTimedOut => "connect_timed_out",
                ConnectionError::SendFailed(_) => "send_failed",
                ConnectionError::Closed => "connection_closed",
                ConnectionError::ResponseTimedOut => "response_timed_out",
                ConnectionError::ReceiveFailed(_) => "receive_failed",
                ConnectionError::IncompleteResponse => "incomplete_response",
                ConnectionError::ResponseTooLarge => "response_too_large",
                ConnectionError::Internal(_) => "internal_client_error",
            },
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Usage(error) => error.to_string(),
            Self::Daemon(error) => error.message().to_owned(),
            Self::Response(error) => error.to_string(),
            Self::Output => "cannot write to standard output".to_owned(),
            Self::Connection(error) => error.to_string(),
        }
    }

    /// Whether the request was completely sent, so the daemon may have acted on
    /// it even though no valid response was obtained.
    const fn request_was_sent(&self) -> bool {
        match self {
            Self::Response(_) => true,
            Self::Connection(error) => error.request_was_sent(),
            _ => false,
        }
    }
}

/// Runs the CLI against the daemon socket of the process environment and
/// returns the exit code.
pub fn run_from_env(args: &[OsString], stdout: &mut impl Write, stderr: &mut impl Write) -> u8 {
    run(
        args,
        connection::socket_path_from_env,
        &ExchangeConfig::default(),
        stdout,
        stderr,
    )
}

/// Runs the CLI and returns the exit code.
///
/// `locate_socket` is called only when a connection is needed, so `--dry-run`
/// works without `XDG_RUNTIME_DIR`. `config` holds the timeouts and the limit;
/// `--timeout` overrides its response timeout. Tests inject the socket path and
/// short timeouts here.
pub fn run(
    args: &[OsString],
    locate_socket: impl FnOnce() -> Result<PathBuf, ConnectionError>,
    config: &ExchangeConfig,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> u8 {
    let invocation = match args::parse(args) {
        Ok(Parsed::Help) => {
            return match stdout.write_all(USAGE.as_bytes()) {
                Ok(()) => EXIT_OK,
                Err(_) => EXIT_INTERNAL,
            };
        }
        Ok(Parsed::Run(invocation)) => invocation,
        Err(error) => {
            // The options may not have parsed, so look for the flag directly.
            let json = args.iter().any(|arg| arg == "--json");
            let code = report(stderr, json, &ClientError::Usage(error), false);
            if !json {
                let _ = writeln!(stderr, "Try `nycti --help` for usage.");
            }
            return code;
        }
    };

    let json = invocation.options.json;
    match execute(&invocation, locate_socket, config, stdout, stderr) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            let may_have_taken_effect = invocation.command.is_change() && error.request_was_sent();
            report(stderr, json, &error, may_have_taken_effect)
        }
    }
}

fn execute(
    invocation: &Invocation,
    locate_socket: impl FnOnce() -> Result<PathBuf, ConnectionError>,
    config: &ExchangeConfig,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), ClientError> {
    let method = invocation.command.method();
    let request = wire::serialize_request(REQUEST_ID, &method);

    if invocation.options.dry_run {
        return write_out(stdout, &format!("{request}\n"));
    }

    let socket = locate_socket().map_err(ClientError::Connection)?;
    let mut config = *config;
    if let Some(timeout) = invocation.options.timeout {
        config.response = timeout;
    }

    if matches!(
        invocation.command,
        Command::Change(ChangeCommand::ApplyWorkspaceMode { .. })
    ) {
        let _ = writeln!(
            stderr,
            "nycti: warning: applying the workspace mode changes the placement of real windows"
        );
    }

    let response =
        connection::exchange(&socket, &request, &config).map_err(ClientError::Connection)?;
    match wire::parse_response(&response, REQUEST_ID, &method).map_err(ClientError::Response)? {
        Reply::Failure(error) => Err(ClientError::Daemon(error)),
        Reply::Success(result) => {
            let text = if invocation.options.json {
                output::render_json(&result)
            } else {
                output::render_human(&result)
            };
            write_out(stdout, &text)
        }
    }
}

fn write_out(stdout: &mut impl Write, text: &str) -> Result<(), ClientError> {
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|_| ClientError::Output)
}

/// Writes the error to standard error and returns its exit code.
fn report(
    stderr: &mut impl Write,
    json: bool,
    error: &ClientError,
    may_have_taken_effect: bool,
) -> u8 {
    let mut message = error.message();
    if may_have_taken_effect {
        message.push_str(
            ". The request may have taken effect: check the state with `nycti wm query` \
             (for example list-workspaces or list-windows)",
        );
    }

    let _ = if json {
        stderr.write_all(output::render_error_json(error.code(), &message).as_bytes())
    } else if let ClientError::Daemon(daemon) = error {
        writeln!(
            stderr,
            "nycti: error: daemon error {}: {message}",
            output::error_code_name(daemon.code())
        )
    } else {
        writeln!(stderr, "nycti: error: {message}")
    };
    error.exit_code()
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::path::Path;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::client::test_support::{Behavior, FakeServer, ignores_file_permissions};
    use crate::core::{WindowManager, WindowPlacement, WorkspaceMode};
    use crate::daemon::Coordinator;
    use crate::daemon::test_support::{TestDirectory, bound_listener, guarded};
    use crate::protocol::{RequestMethod, parse_request};
    use crate::service::WindowManagementService;

    /// A short response timeout, so the tests that wait for it stay fast.
    const SHORT: Duration = Duration::from_millis(150);

    struct Outcome {
        code: u8,
        stdout: String,
        stderr: String,
    }

    fn args(line: &str) -> Vec<OsString> {
        line.split_whitespace().map(OsString::from).collect()
    }

    fn cli_with(line: &str, socket: &Path, config: &ExchangeConfig) -> Outcome {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(
            &args(line),
            || Ok(socket.to_path_buf()),
            config,
            &mut stdout,
            &mut stderr,
        );
        Outcome {
            code,
            stdout: String::from_utf8(stdout).expect("stdout should be UTF-8"),
            stderr: String::from_utf8(stderr).expect("stderr should be UTF-8"),
        }
    }

    fn cli(line: &str, socket: &Path) -> Outcome {
        cli_with(line, socket, &ExchangeConfig::default())
    }

    /// Runs a command that must never locate a socket or connect.
    fn cli_offline(line: &str) -> Outcome {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(
            &args(line),
            || panic!("the command must not look for the socket"),
            &ExchangeConfig::default(),
            &mut stdout,
            &mut stderr,
        );
        Outcome {
            code,
            stdout: String::from_utf8(stdout).expect("stdout should be UTF-8"),
            stderr: String::from_utf8(stderr).expect("stderr should be UTF-8"),
        }
    }

    fn success(result: &str) -> Behavior {
        Behavior::line(&format!(
            r#"{{"version":1,"id":"1","ok":true,"result":{result}}}"#
        ))
    }

    /// A fake daemon that answers like the real service over a `FakeBackend`.
    fn service_behavior(backend: FakeBackend) -> Behavior {
        let mut service =
            WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling));
        Behavior::Respond(Box::new(move |line| {
            let line = std::str::from_utf8(line).expect("request should be UTF-8");
            service
                .handle_json_line(line)
                .expect("response should serialize")
                .into_bytes()
        }))
    }

    fn method_of(line: &str) -> RequestMethod {
        parse_request(line)
            .expect("the CLI should send a valid request")
            .method()
            .clone()
    }

    #[test]
    fn every_query_command_sends_only_a_read_method() {
        guarded(|| {
            let commands = [
                "wm query status",
                "wm query get-default-mode",
                "wm query list-workspaces",
                "wm query get-workspace-mode w:missing",
                "wm query list-windows",
            ];

            for command in commands {
                let directory = TestDirectory::new();
                let server = FakeServer::start(&directory, service_behavior(FakeBackend::new()));

                cli(command, &server.path());

                let method = method_of(&server.request_line());
                assert!(
                    matches!(
                        method,
                        RequestMethod::Status
                            | RequestMethod::GetDefaultMode
                            | RequestMethod::ListWorkspaces
                            | RequestMethod::GetWorkspaceMode { .. }
                            | RequestMethod::ListWindows
                    ),
                    "`{command}` sent {method:?}"
                );
            }
        });
    }

    #[test]
    fn every_change_command_sends_its_own_method() {
        guarded(|| {
            let commands = [
                (
                    "wm change set-default-mode windows",
                    r#"{"version":1,"id":"1","method":"set_default_mode","params":{"mode":"windows"}}"#,
                ),
                (
                    "wm change set-workspace-mode w:2 tiling",
                    r#"{"version":1,"id":"1","method":"set_workspace_mode","params":{"workspace_id":"w:2","mode":"tiling"}}"#,
                ),
                (
                    "wm change clear-workspace-mode w:2",
                    r#"{"version":1,"id":"1","method":"clear_workspace_mode","params":{"workspace_id":"w:2"}}"#,
                ),
                (
                    "wm change apply-workspace-mode w:2 --yes",
                    r#"{"version":1,"id":"1","method":"apply_workspace_mode","params":{"workspace_id":"w:2"}}"#,
                ),
            ];

            for (command, expected) in commands {
                let directory = TestDirectory::new();
                let server = FakeServer::start(&directory, Behavior::Close);

                cli(command, &server.path());

                assert_eq!(server.request_line(), expected, "`{command}`");
            }
        });
    }

    fn json_out(outcome: &Outcome) -> Value {
        assert_eq!(outcome.code, EXIT_OK, "stderr: {}", outcome.stderr);
        assert_eq!(outcome.stdout.matches('\n').count(), 1, "one JSON line");
        serde_json::from_str(&outcome.stdout).expect("stdout should be JSON")
    }

    #[test]
    fn the_cli_drives_the_real_daemon_core_end_to_end() {
        guarded(|| {
            let mut backend = FakeBackend::new();
            let busy = backend.add_workspace(true);
            let other = backend.add_workspace(true);
            for (workspace, fullscreen, focused) in [
                (busy, false, true),
                (busy, false, false),
                (busy, true, false),
                (other, false, false),
            ] {
                backend
                    .add_window(workspace, WindowPlacement::Tiled, fullscreen, focused)
                    .expect("known workspace should accept window");
            }
            let directory = TestDirectory::new();
            let listener = bound_listener(&directory);
            let socket = listener.socket_path().to_path_buf();
            let coordinator = Coordinator::start(
                listener,
                WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling)),
            )
            .expect("coordinator should start");

            let status = cli("wm query status", &socket);
            assert_eq!(status.code, EXIT_OK);
            assert!(status.stdout.contains("nycti-windowd"));

            let default = cli("wm query get-default-mode", &socket);
            assert!(default.stdout.contains("default=tiling"));

            let workspaces = json_out(&cli("--json wm query list-workspaces", &socket));
            assert_eq!(workspaces["workspaces"].as_array().map(Vec::len), Some(2));
            let windows = |socket: &Path| {
                json_out(&cli("--json wm query list-windows", socket))["windows"]
                    .as_array()
                    .expect("windows should be an array")
                    .clone()
            };
            let before = windows(&socket);
            assert_eq!(before.len(), 4);
            // The busy workspace is the one that holds three windows; the
            // token is used exactly as the daemon printed it.
            let token = before
                .iter()
                .map(|window| window["workspace_id"].as_str().expect("token"))
                .find(|candidate| {
                    before
                        .iter()
                        .filter(|window| window["workspace_id"] == *candidate)
                        .count()
                        == 3
                })
                .expect("one workspace should hold three windows")
                .to_owned();

            let get = cli(&format!("wm query get-workspace-mode {token}"), &socket);
            assert!(
                get.stdout.contains("explicit=none") && get.stdout.contains("effective=tiling")
            );

            let set = cli(
                &format!("wm change set-workspace-mode {token} windows"),
                &socket,
            );
            assert_eq!(set.code, EXIT_OK, "stderr: {}", set.stderr);
            assert!(set.stdout.contains("explicit=windows"));
            assert_eq!(windows(&socket), before, "set must not move any window");

            let apply = cli(
                &format!("wm change apply-workspace-mode {token} --yes"),
                &socket,
            );
            assert_eq!(apply.code, EXIT_OK, "stderr: {}", apply.stderr);
            assert!(apply.stdout.contains("applied") && apply.stdout.contains("windows"));
            assert!(apply.stderr.contains("warning"));

            for window in windows(&socket) {
                let in_busy = window["workspace_id"] == token.as_str();
                let fullscreen = window["fullscreen"] == true;
                let expected = if in_busy && !fullscreen {
                    "floating"
                } else {
                    "tiled"
                };
                assert_eq!(window["placement"], expected, "window: {window}");
            }

            let clear = cli(&format!("wm change clear-workspace-mode {token}"), &socket);
            assert_eq!(clear.code, EXIT_OK);
            let get = cli(&format!("wm query get-workspace-mode {token}"), &socket);
            assert!(get.stdout.contains("explicit=none"));

            let unknown = cli("wm query get-workspace-mode w:999999", &socket);
            assert_eq!(unknown.code, EXIT_DAEMON_ERROR);
            assert!(unknown.stderr.contains("unknown_workspace"));

            coordinator
                .shutdown_handle()
                .request_shutdown()
                .expect("shutdown should be accepted");
            let report = coordinator.wait().expect("coordinator should not panic");
            assert_eq!(report.workers_failed, 0);
            assert_eq!(report.listener_cleanup, Ok(()));
        });
    }

    #[test]
    fn a_missing_daemon_exits_with_three() {
        let directory = TestDirectory::new();

        let outcome = cli("wm query status", &directory.path().join("absent.sock"));

        assert_eq!(outcome.code, EXIT_DAEMON_UNAVAILABLE);
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.contains("not running"));
    }

    #[test]
    fn a_stale_socket_exits_with_three_and_says_stale() {
        let directory = TestDirectory::new();
        let path = directory.path().join("stale.sock");
        drop(UnixListener::bind(&path).expect("stale socket should bind"));

        let outcome = cli("wm query status", &path);

        assert_eq!(outcome.code, EXIT_DAEMON_UNAVAILABLE);
        assert!(outcome.stderr.contains("stale"));
    }

    #[test]
    fn a_socket_with_mode_000_exits_with_three_and_says_permission() {
        let directory = TestDirectory::new();
        if ignores_file_permissions(&directory) {
            return;
        }
        let path = directory.path().join("closed.sock");
        let _listener = UnixListener::bind(&path).expect("socket should bind");
        fs::set_permissions(&path, Permissions::from_mode(0o000))
            .expect("socket mode should be set");

        let outcome = cli("wm query status", &path);

        assert_eq!(outcome.code, EXIT_DAEMON_UNAVAILABLE);
        assert!(outcome.stderr.contains("permission denied"));
        assert!(outcome.stderr.contains("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn an_unusable_runtime_directory_exits_with_three() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run(
            &args("wm query status"),
            || Err(ConnectionError::RuntimeDirUnset),
            &ExchangeConfig::default(),
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(code, EXIT_DAEMON_UNAVAILABLE);
        assert!(String::from_utf8_lossy(&stderr).contains("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn a_daemon_that_closes_without_responding_is_a_communication_failure() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::Close);

            let outcome = cli("wm query list-windows", &server.path());

            assert_eq!(outcome.code, EXIT_COMMUNICATION);
            assert!(outcome.stderr.contains("closed the connection"));
            assert!(
                !outcome.stderr.contains("may have taken effect"),
                "a query changes nothing: {}",
                outcome.stderr
            );
        });
    }

    #[test]
    fn the_may_have_taken_effect_notice_appears_only_for_change_commands() {
        guarded(|| {
            for command in [
                "wm change set-default-mode windows",
                "wm change set-workspace-mode w:1 windows",
                "wm change clear-workspace-mode w:1",
                "wm change apply-workspace-mode w:1 --yes",
            ] {
                let directory = TestDirectory::new();
                let server = FakeServer::start(&directory, Behavior::Close);

                let outcome = cli(command, &server.path());

                assert_eq!(outcome.code, EXIT_COMMUNICATION, "`{command}`");
                assert!(
                    outcome.stderr.contains("may have taken effect")
                        && outcome.stderr.contains("wm query"),
                    "`{command}` stderr: {}",
                    outcome.stderr
                );
            }
        });
    }

    #[test]
    fn a_failure_before_sending_does_not_claim_an_effect() {
        let directory = TestDirectory::new();

        let outcome = cli(
            "wm change set-default-mode windows",
            &directory.path().join("absent.sock"),
        );

        assert_eq!(outcome.code, EXIT_DAEMON_UNAVAILABLE);
        assert!(!outcome.stderr.contains("may have taken effect"));
    }

    /// The two tests below wait for a real timeout, so they are time-dependent.
    /// `guarded` only protects them from a hang.
    #[test]
    fn a_daemon_that_never_responds_ends_with_exit_five_after_the_injected_timeout() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::Mute);
            let config = ExchangeConfig {
                response: SHORT,
                ..ExchangeConfig::default()
            };

            let outcome = cli_with("wm query status", &server.path(), &config);

            assert_eq!(outcome.code, EXIT_COMMUNICATION);
            assert!(outcome.stderr.contains("timed out"));
        });
    }

    #[test]
    fn the_timeout_option_overrides_the_response_timeout() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::Mute);

            // The configured timeout is 10 s; only the option makes this short.
            let outcome = cli("wm query status --timeout 0.15", &server.path());

            assert_eq!(outcome.code, EXIT_COMMUNICATION);
            assert!(outcome.stderr.contains("timed out"));
        });
    }

    #[test]
    fn malformed_responses_exit_with_five_and_say_malformed() {
        guarded(|| {
            let wrong_id = r#"{"version":1,"id":"2","ok":true,"result":{"mode":"tiling"}}"#;
            let wrong_shape = r#"{"version":1,"id":"1","ok":true,"result":{"windows":[]}}"#;
            let cases: Vec<(&str, Behavior)> = vec![
                ("not JSON", Behavior::line("hello")),
                ("another id", Behavior::line(wrong_id)),
                ("wrong shape", Behavior::line(wrong_shape)),
                (
                    "version 2",
                    Behavior::line(r#"{"version":2,"id":"1","ok":true}"#),
                ),
            ];

            for (name, behavior) in cases {
                let directory = TestDirectory::new();
                let server = FakeServer::start(&directory, behavior);

                let outcome = cli("--json wm query get-default-mode", &server.path());

                assert_eq!(outcome.code, EXIT_COMMUNICATION, "case: {name}");
                let error: Value =
                    serde_json::from_str(&outcome.stderr).expect("the error line should be JSON");
                assert_eq!(error["error"]["code"], "malformed_response", "case: {name}");
            }
        });
    }

    #[test]
    fn a_response_without_a_line_feed_is_malformed_and_exits_with_five() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::raw(r#"{"version":1"#));

            let outcome = cli("--json wm query status", &server.path());

            assert_eq!(outcome.code, EXIT_COMMUNICATION);
            assert!(outcome.stderr.contains("incomplete_response"));
        });
    }

    #[test]
    fn a_daemon_error_exits_with_four_and_prints_the_protocol_code() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(
                &directory,
                Behavior::line(
                    r#"{"version":1,"id":"1","ok":false,"error":{"code":"unknown_workspace","message":"workspace is not known"}}"#,
                ),
            );

            let outcome = cli("wm query get-workspace-mode w:9", &server.path());

            assert_eq!(outcome.code, EXIT_DAEMON_ERROR);
            assert!(outcome.stdout.is_empty());
            assert!(outcome.stderr.contains("unknown_workspace"));
            assert!(outcome.stderr.contains("workspace is not known"));
        });
    }

    #[test]
    fn json_mode_prints_the_result_object_and_the_error_object() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(
                &directory,
                success(
                    r#"{"workspaces":[{"workspace_id":"w:2","present":true,"default_mode":"tiling","explicit_mode":"windows","effective_mode":"windows"}]}"#,
                ),
            );

            let outcome = cli("--json wm query list-workspaces", &server.path());

            assert_eq!(
                json_out(&outcome),
                json!({"workspaces": [{
                    "workspace_id": "w:2", "present": true, "default_mode": "tiling",
                    "explicit_mode": "windows", "effective_mode": "windows"
                }]})
            );
            assert!(outcome.stderr.is_empty());

            let directory = TestDirectory::new();
            let server = FakeServer::start(
                &directory,
                Behavior::line(
                    r#"{"version":1,"id":"1","ok":false,"error":{"code":"action_failed","message":"nope"}}"#,
                ),
            );
            let outcome = cli("wm query list-windows --json", &server.path());

            assert_eq!(outcome.code, EXIT_DAEMON_ERROR);
            assert!(outcome.stdout.is_empty());
            assert_eq!(
                outcome.stderr,
                "{\"error\":{\"code\":\"action_failed\",\"message\":\"nope\"}}\n"
            );
        });
    }

    #[test]
    fn human_mode_prints_keywords_in_the_received_order() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(
                &directory,
                success(
                    r#"{"windows":[{"window_id":"win:9","workspace_id":"w:1","placement":"tiled","fullscreen":true,"focused":false},{"window_id":"win:7","workspace_id":"w:2","placement":"floating","fullscreen":false,"focused":true}]}"#,
                ),
            );

            let outcome = cli("wm query list-windows", &server.path());

            assert_eq!(outcome.code, EXIT_OK);
            let lines: Vec<&str> = outcome.stdout.lines().collect();
            assert_eq!(lines.len(), 2);
            assert!(lines[0].starts_with("win:9") && lines[0].contains("fullscreen=yes"));
            assert!(lines[1].starts_with("win:7") && lines[1].contains("floating"));
        });
    }

    #[test]
    fn dry_run_prints_the_request_and_does_not_connect() {
        let outcome = cli_offline("wm change set-workspace-mode w:2 windows --dry-run");

        assert_eq!(outcome.code, EXIT_OK);
        assert_eq!(
            outcome.stdout,
            "{\"version\":1,\"id\":\"1\",\"method\":\"set_workspace_mode\",\"params\":{\"workspace_id\":\"w:2\",\"mode\":\"windows\"}}\n"
        );
        assert!(outcome.stderr.is_empty());

        let query = cli_offline("--dry-run wm query status");
        assert_eq!(query.code, EXIT_OK);
        assert_eq!(
            query.stdout,
            "{\"version\":1,\"id\":\"1\",\"method\":\"status\",\"params\":{}}\n"
        );
    }

    #[test]
    fn dry_run_of_apply_needs_yes_and_then_prints_without_a_warning() {
        let refused = cli_offline("wm change apply-workspace-mode w:2 --dry-run");
        assert_eq!(refused.code, EXIT_USAGE);
        assert!(refused.stdout.is_empty());

        let shown = cli_offline("wm change apply-workspace-mode w:2 --yes --dry-run");
        assert_eq!(shown.code, EXIT_OK);
        assert!(shown.stdout.contains("apply_workspace_mode"));
        assert!(shown.stderr.is_empty(), "nothing is sent, so no warning");
    }

    #[test]
    fn apply_without_yes_exits_with_two_explains_and_never_connects() {
        let outcome = cli_offline("wm change apply-workspace-mode w:2");

        assert_eq!(outcome.code, EXIT_USAGE);
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.contains("real windows"));
        assert!(outcome.stderr.contains("--yes"));
        assert!(outcome.stderr.contains("Nothing was sent"));
    }

    #[test]
    fn apply_with_yes_warns_on_standard_error_before_sending() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(
                &directory,
                success(r#"{"workspace_id":"w:2","effective_mode":"windows"}"#),
            );

            let outcome = cli("wm change apply-workspace-mode w:2 --yes", &server.path());

            assert_eq!(outcome.code, EXIT_OK);
            assert!(outcome.stderr.contains("warning"));
            assert!(outcome.stderr.contains("real windows"));
            assert!(outcome.stdout.contains("applied"));
        });
    }

    #[test]
    fn usage_errors_exit_with_two_and_go_to_standard_error() {
        let outcome = cli_offline("wm toggle");

        assert_eq!(outcome.code, EXIT_USAGE);
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.contains("unknown command `toggle`"));
        assert!(outcome.stderr.contains("--help"));

        let json = cli_offline("--json wm change set-default-mode floating");
        assert_eq!(json.code, EXIT_USAGE);
        assert!(json.stdout.is_empty());
        let error: Value = serde_json::from_str(&json.stderr).expect("error line should be JSON");
        assert_eq!(error["error"]["code"], "usage_error");
        assert!(
            error["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("floating"))
        );

        let confirmation = cli_offline("--json wm change apply-workspace-mode w:1");
        let error: Value =
            serde_json::from_str(&confirmation.stderr).expect("error line should be JSON");
        assert_eq!(error["error"]["code"], "confirmation_required");
    }

    #[test]
    fn help_prints_the_usage_on_standard_output_and_exits_with_zero() {
        let outcome = cli_offline("wm query status --help");

        assert_eq!(outcome.code, EXIT_OK);
        assert!(outcome.stdout.starts_with("Usage:"));
        assert!(outcome.stderr.is_empty());
    }

    #[test]
    fn a_failing_standard_output_is_an_internal_failure() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut stderr = Vec::new();

        let code = run(
            &args("wm query status --dry-run"),
            || panic!("the command must not look for the socket"),
            &ExchangeConfig::default(),
            &mut Broken,
            &mut stderr,
        );

        assert_eq!(code, EXIT_INTERNAL);
        assert!(String::from_utf8_lossy(&stderr).contains("standard output"));
    }

    #[test]
    fn exit_codes_have_their_documented_values() {
        assert_eq!(
            [
                EXIT_OK,
                EXIT_INTERNAL,
                EXIT_USAGE,
                EXIT_DAEMON_UNAVAILABLE,
                EXIT_DAEMON_ERROR,
                EXIT_COMMUNICATION
            ],
            [0, 1, 2, 3, 4, 5]
        );
    }
}
