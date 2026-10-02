//! Process-level tests for the `nycti` binary.
//!
//! The CLI runs as a child process with a cleared environment against the real
//! `nycti-windowd` binary, which runs with a private runtime directory and a
//! fake Hyprland socket that serves the recorded fixtures. These tests never
//! touch a real Hyprland session. The helpers repeat the ones of
//! `daemon_process.rs`, because each integration test is its own crate.
//!
//! They synchronize with the daemon through its `listening on` line on standard
//! error, which is provisional text (ADR 0008).

use std::env;
use std::fs::{self, DirBuilder, Permissions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::Value;

/// Protection against a hung test only. It is never used to synchronize.
const GUARD: Duration = Duration::from_secs(30);
const SIGNATURE: &str = "test-instance";
const WORKSPACES: &str = include_str!("fixtures/hyprland/workspaces.json");
const CLIENTS: &str = include_str!("fixtures/hyprland/clients.json");
const ACTIVE_WINDOW: &str = include_str!("fixtures/hyprland/activewindow.json");

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

/// A private runtime directory that is removed when dropped.
struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "nycti-cli-process-test-{}-{sequence}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&path)
            .expect("unique test directory should be created");
        fs::set_permissions(&path, Permissions::from_mode(0o700))
            .expect("test directory mode should be set");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Serves the fixtures on a fake Hyprland request socket. Anything else, such
/// as a `/dispatch` action, is refused with an `error:` reply, as Hyprland
/// refuses a command it cannot run. The server thread is detached and ends with
/// the test process.
fn start_fake_hyprland(runtime: &Path) {
    let directory = runtime.join("hypr").join(SIGNATURE);
    fs::create_dir_all(&directory).expect("fake Hyprland directory should be created");
    let listener = UnixListener::bind(directory.join(".socket.sock"))
        .expect("fake Hyprland socket should bind");

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0_u8; 1024];
            let Ok(length) = stream.read(&mut buffer) else {
                continue;
            };
            let reply: &[u8] = match &buffer[..length] {
                b"j/workspaces" => WORKSPACES.as_bytes(),
                b"j/clients" => CLIENTS.as_bytes(),
                b"j/activewindow" => ACTIVE_WINDOW.as_bytes(),
                _ => b"error: not served by the fake",
            };
            let _ = stream.write_all(reply);
        }
    });
}

/// The daemon as a child process. It is killed when dropped, including when a
/// test panics, so no daemon is left running.
struct Daemon {
    child: Child,
}

impl Daemon {
    fn spawn(runtime: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nycti-windowd"))
            .env_clear()
            .env("XDG_RUNTIME_DIR", runtime)
            .env("HYPRLAND_INSTANCE_SIGNATURE", SIGNATURE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the daemon should start");

        let stderr = child.stderr.take().expect("stderr should be piped");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = sender.send(line);
            }
        });
        let daemon = Self { child };
        wait_for_line(&lines, "listening on");
        daemon
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_line(lines: &Receiver<String>, text: &str) {
    loop {
        match lines.recv_timeout(GUARD) {
            Ok(line) if line.contains(text) => return,
            Ok(_) => {}
            Err(RecvTimeoutError::Disconnected) => {
                panic!("the daemon ended before a line containing {text:?}")
            }
            Err(RecvTimeoutError::Timeout) => {
                panic!("no line containing {text:?} within the guard limit")
            }
        }
    }
}

/// Starts a daemon in a fresh runtime directory with a fake Hyprland. The
/// directory is declared first so it outlives the daemon's last use.
fn running_daemon() -> (Daemon, TestDirectory) {
    let directory = TestDirectory::new();
    start_fake_hyprland(directory.path());
    let daemon = Daemon::spawn(directory.path());
    (daemon, directory)
}

/// Runs `nycti` with a cleared environment and the given runtime directory.
fn nycti(runtime: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nycti"));
    command.env_clear().args(args).stdin(Stdio::null());
    if let Some(runtime) = runtime {
        command.env("XDG_RUNTIME_DIR", runtime);
    }
    command.output().expect("nycti should run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout should be UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr should be UTF-8")
}

fn code(output: &Output) -> Option<i32> {
    output.status.code()
}

fn json_stdout(output: &Output) -> Value {
    assert_eq!(code(output), Some(0), "stderr: {}", stderr(output));
    serde_json::from_str(&stdout(output)).expect("stdout should be one JSON line")
}

fn fixture_count(json: &str) -> usize {
    serde_json::from_str::<Vec<Value>>(json)
        .expect("fixture should be a JSON array")
        .len()
}

/// The token of the workspace that holds the one tiled window of the fixtures.
fn tiled_workspace(runtime: &Path) -> String {
    let windows = json_stdout(&nycti(
        Some(runtime),
        &["--json", "wm", "query", "list-windows"],
    ));
    let tiled: Vec<&Value> = windows["windows"]
        .as_array()
        .expect("windows should be an array")
        .iter()
        .filter(|window| window["placement"] == "tiled")
        .collect();
    assert_eq!(tiled.len(), 1, "the fixtures hold one tiled window");
    tiled[0]["workspace_id"]
        .as_str()
        .expect("workspace token should be text")
        .to_owned()
}

#[test]
fn reads_report_the_counts_of_the_fixtures() {
    let (_daemon, directory) = running_daemon();
    let runtime = Some(directory.path());

    let status = nycti(runtime, &["wm", "query", "status"]);
    assert_eq!(code(&status), Some(0), "stderr: {}", stderr(&status));
    assert!(stdout(&status).contains("nycti-windowd"));

    let default = nycti(runtime, &["wm", "query", "get-default-mode"]);
    assert!(stdout(&default).contains("default=tiling"));

    let workspaces = json_stdout(&nycti(
        runtime,
        &["--json", "wm", "query", "list-workspaces"],
    ));
    assert_eq!(
        workspaces["workspaces"].as_array().map(Vec::len),
        Some(fixture_count(WORKSPACES))
    );

    let windows = nycti(runtime, &["wm", "query", "list-windows"]);
    assert_eq!(code(&windows), Some(0));
    assert_eq!(stdout(&windows).lines().count(), fixture_count(CLIENTS));
    assert_eq!(
        stdout(&windows)
            .lines()
            .filter(|line| line.contains("focused=yes"))
            .count(),
        1
    );
}

#[test]
fn an_unknown_token_is_a_daemon_error_with_exit_four() {
    let (_daemon, directory) = running_daemon();

    let output = nycti(
        Some(directory.path()),
        &["wm", "query", "get-workspace-mode", "w:999999"],
    );

    assert_eq!(code(&output), Some(4));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("unknown_workspace"));

    let json = nycti(
        Some(directory.path()),
        &["--json", "wm", "query", "get-workspace-mode", "w:999999"],
    );
    assert_eq!(code(&json), Some(4));
    let error: Value = serde_json::from_str(&stderr(&json)).expect("error line should be JSON");
    assert_eq!(error["error"]["code"], "unknown_workspace");
}

#[test]
fn a_missing_daemon_is_exit_three() {
    let directory = TestDirectory::new();

    let output = nycti(Some(directory.path()), &["wm", "query", "status"]);
    assert_eq!(code(&output), Some(3));
    assert!(stderr(&output).contains("not running"));

    let without_runtime_dir = nycti(None, &["wm", "query", "status"]);
    assert_eq!(code(&without_runtime_dir), Some(3));
    assert!(stderr(&without_runtime_dir).contains("XDG_RUNTIME_DIR"));
}

#[test]
fn apply_without_yes_is_exit_two_and_never_connects() {
    // With no daemon, a connection attempt would end with exit code 3.
    let directory = TestDirectory::new();

    let output = nycti(
        Some(directory.path()),
        &["wm", "change", "apply-workspace-mode", "w:1"],
    );

    assert_eq!(code(&output), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("--yes"));
}

#[test]
fn set_get_and_clear_change_only_the_daemon_policy() {
    let (_daemon, directory) = running_daemon();
    let runtime = Some(directory.path());
    let token = tiled_workspace(directory.path());

    let set = nycti(
        runtime,
        &["wm", "change", "set-workspace-mode", &token, "windows"],
    );
    assert_eq!(code(&set), Some(0), "stderr: {}", stderr(&set));
    assert!(stdout(&set).contains("explicit=windows"));

    let get = nycti(runtime, &["wm", "query", "get-workspace-mode", &token]);
    assert!(stdout(&get).contains("explicit=windows"));
    assert!(stdout(&get).contains("effective=windows"));

    // The setter placed nothing: the window is still tiled.
    assert_eq!(tiled_workspace(directory.path()), token);

    let clear = nycti(runtime, &["wm", "change", "clear-workspace-mode", &token]);
    assert_eq!(code(&clear), Some(0));
    let get = nycti(runtime, &["wm", "query", "get-workspace-mode", &token]);
    assert!(stdout(&get).contains("explicit=none"));
    assert!(stdout(&get).contains("effective=tiling"));

    let default = nycti(runtime, &["wm", "change", "set-default-mode", "windows"]);
    assert_eq!(code(&default), Some(0));
    let get = nycti(runtime, &["wm", "query", "get-default-mode"]);
    assert!(stdout(&get).contains("default=windows"));
}

#[test]
fn apply_with_yes_against_a_hyprland_that_refuses_dispatch_is_action_failed() {
    let (_daemon, directory) = running_daemon();
    let runtime = Some(directory.path());
    let token = tiled_workspace(directory.path());
    let set = nycti(
        runtime,
        &["wm", "change", "set-workspace-mode", &token, "windows"],
    );
    assert_eq!(code(&set), Some(0));

    let output = nycti(
        runtime,
        &["wm", "change", "apply-workspace-mode", &token, "--yes"],
    );

    assert_eq!(code(&output), Some(4), "stderr: {}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("warning"));
    assert!(stderr(&output).contains("action_failed"));
    // The fake refused the action, so nothing moved.
    assert_eq!(tiled_workspace(directory.path()), token);
}

#[test]
fn dry_run_and_help_need_no_daemon_and_no_runtime_directory() {
    let dry = nycti(None, &["wm", "query", "status", "--dry-run"]);
    assert_eq!(code(&dry), Some(0));
    assert_eq!(
        stdout(&dry),
        "{\"version\":1,\"id\":\"1\",\"method\":\"status\",\"params\":{}}\n"
    );

    let help = nycti(None, &["--help"]);
    assert_eq!(code(&help), Some(0));
    assert!(stdout(&help).starts_with("Usage:"));

    let usage = nycti(None, &["wm", "toggle"]);
    assert_eq!(code(&usage), Some(2));
    assert!(stderr(&usage).contains("unknown command"));
}
