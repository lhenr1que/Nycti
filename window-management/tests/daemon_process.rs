//! Process-level tests for the `clea-windowd` binary.
//!
//! The daemon runs as a child process with a cleared environment, a private
//! runtime directory, and a fake Hyprland socket that serves the recorded
//! fixtures. These tests never touch a real Hyprland session and send signals
//! only to their own child process, through `/usr/bin/kill`, which is a
//! dependency of the test environment.
//!
//! They synchronize with the daemon through lines it writes on standard error
//! (`listening on` and `shutdown requested`). That text is provisional, so these
//! tests are tied to it. See ADR 0008.

use std::env;
use std::fs::{self, DirBuilder, Permissions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// Protection against a hung test only. It is never used to synchronize.
const GUARD: Duration = Duration::from_secs(30);
const KILL: &str = "/usr/bin/kill";
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
    fn new(mode: u32) -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "clea-windowd-process-test-{}-{sequence}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        builder.mode(mode);
        builder
            .create(&path)
            .expect("unique test directory should be created");
        fs::set_permissions(&path, Permissions::from_mode(mode))
            .expect("test directory mode should be set");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn socket(&self) -> PathBuf {
        self.path.join("clea").join("window-management.sock")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Serves the fixtures on a fake Hyprland request socket.
///
/// With `hold`, every request waits for the test to send a release (or to drop
/// the sender) before it is answered. `received` is told when a request arrives.
/// The server thread is detached and ends with the test process.
fn start_fake_hyprland(runtime: &Path, hold: Option<Receiver<()>>, received: Option<Sender<()>>) {
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
            if let Some(received) = &received {
                let _ = received.send(());
            }
            if let Some(hold) = &hold {
                let _ = hold.recv_timeout(GUARD);
            }
            let reply: &[u8] = match &buffer[..length] {
                b"j/workspaces" => WORKSPACES.as_bytes(),
                b"j/clients" => CLIENTS.as_bytes(),
                b"j/activewindow" => ACTIVE_WINDOW.as_bytes(),
                _ => b"error: not served by the fake",
            };
            let _ = stream.write_all(reply);
            // Dropping the stream ends the response, as Hyprland does.
        }
    });
}

/// The daemon as a child process. It is killed when dropped, including when a
/// test panics, so no daemon is left running.
struct Daemon {
    child: Child,
    lines: Receiver<Option<String>>,
}

impl Daemon {
    /// Starts the daemon with a cleared environment. Without `signature`, no
    /// Hyprland session variable is set.
    fn spawn(runtime: &Path, signature: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clea-windowd"));
        command
            .env_clear()
            .env("XDG_RUNTIME_DIR", runtime)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(signature) = signature {
            command.env("HYPRLAND_INSTANCE_SIGNATURE", signature);
        }
        let mut child = command.spawn().expect("the daemon should start");

        let stderr = child.stderr.take().expect("stderr should be piped");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = sender.send(Some(line));
            }
            let _ = sender.send(None);
        });
        Self { child, lines }
    }

    /// Waits for a line of standard error containing `text`.
    fn expect_line_containing(&mut self, text: &str) -> String {
        loop {
            match self.lines.recv_timeout(GUARD) {
                Ok(Some(line)) if line.contains(text) => return line,
                Ok(Some(_)) => {}
                Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                    panic!("the daemon ended before a line containing {text:?}")
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no line containing {text:?} within the guard limit")
                }
            }
        }
    }

    /// Sends a signal to the child process only.
    fn signal(&self, name: &str) {
        let status = Command::new(KILL)
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("these tests need /usr/bin/kill");
        assert!(status.success(), "kill -{name} should reach the daemon");
    }

    /// Waits for the daemon to end and returns its exit code and the standard
    /// error lines it wrote after the ones already read.
    fn finish(&mut self) -> (Option<i32>, Vec<String>) {
        let mut lines = Vec::new();
        loop {
            match self.lines.recv_timeout(GUARD) {
                Ok(Some(line)) => lines.push(line),
                Ok(None) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("the daemon did not end in time"),
            }
        }
        let status = self.child.wait().expect("the daemon should be reaped");
        (status.code(), lines)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A client connected to the daemon's service socket.
struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("client should connect");
        stream
            .set_read_timeout(Some(GUARD))
            .expect("read timeout should be set");
        let reader = BufReader::new(stream.try_clone().expect("client stream should clone"));
        Self {
            writer: stream,
            reader,
        }
    }

    fn send(&mut self, method: &str) {
        let line = format!(
            "{}\n",
            json!({"version": 1, "id": method, "method": method, "params": {}})
        );
        self.writer
            .write_all(line.as_bytes())
            .expect("request should be written");
    }

    fn call(&mut self, method: &str) -> Value {
        self.send(method);
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .expect("response line should be read");
        serde_json::from_str(&line).expect("response should be JSON")
    }

    /// Reads until the daemon closes the connection.
    fn read_to_eof(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.reader
            .read_to_end(&mut bytes)
            .expect("the connection should end cleanly");
        bytes
    }
}

fn fixture_count(json: &str) -> usize {
    serde_json::from_str::<Vec<Value>>(json)
        .expect("fixture should be a JSON array")
        .len()
}

/// Starts a daemon in a fresh runtime directory with a fake Hyprland.
fn running_daemon() -> (TestDirectory, Daemon) {
    let directory = TestDirectory::new(0o700);
    start_fake_hyprland(directory.path(), None, None);
    let mut daemon = Daemon::spawn(directory.path(), Some(SIGNATURE));
    daemon.expect_line_containing("listening on");
    (directory, daemon)
}

fn serves_read_only_requests_and_stops_cleanly_on(signal: &str) {
    let (directory, mut daemon) = running_daemon();
    let mut client = Client::connect(&directory.socket());

    let status = client.call("status");
    assert_eq!(status["result"]["service"], "clea-windowd");
    let workspaces = client.call("list_workspaces");
    assert_eq!(
        workspaces["result"]["workspaces"]
            .as_array()
            .expect("workspaces should be an array")
            .len(),
        fixture_count(WORKSPACES)
    );
    let windows = client.call("list_windows");
    let windows = windows["result"]["windows"]
        .as_array()
        .expect("windows should be an array");
    assert_eq!(windows.len(), fixture_count(CLIENTS));
    assert_eq!(
        windows
            .iter()
            .filter(|window| window["focused"] == true)
            .count(),
        1
    );

    // The client is still connected and idle when the signal arrives.
    daemon.signal(signal);
    daemon.expect_line_containing("shutdown requested");
    assert!(client.read_to_eof().is_empty());
    let (code, lines) = daemon.finish();

    assert_eq!(code, Some(0));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("stopped: shutdown requested"))
    );
    assert!(!directory.socket().exists());
}

#[test]
fn sigterm_stops_the_daemon_cleanly_and_removes_the_socket() {
    serves_read_only_requests_and_stops_cleanly_on("TERM");
}

#[test]
fn sigint_stops_the_daemon_cleanly_and_removes_the_socket() {
    serves_read_only_requests_and_stops_cleanly_on("INT");
}

#[test]
fn a_missing_hyprland_session_fails_startup_and_leaves_no_socket() {
    let directory = TestDirectory::new(0o700);
    let mut daemon = Daemon::spawn(directory.path(), None);

    let (code, lines) = daemon.finish();

    assert_eq!(code, Some(1));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("cannot use the Hyprland session"))
    );
    assert!(!directory.socket().exists());
}

#[test]
fn a_runtime_directory_with_the_wrong_mode_fails_startup() {
    let directory = TestDirectory::new(0o755);
    start_fake_hyprland(directory.path(), None, None);
    let mut daemon = Daemon::spawn(directory.path(), Some(SIGNATURE));

    let (code, lines) = daemon.finish();

    assert_eq!(code, Some(1));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("cannot bind the service socket"))
    );
    assert!(!directory.socket().exists());
}

#[test]
fn a_second_daemon_exits_with_two_and_leaves_the_first_running() {
    let (directory, mut first) = running_daemon();
    let mut second = Daemon::spawn(directory.path(), Some(SIGNATURE));

    let (code, lines) = second.finish();
    assert_eq!(code, Some(2));
    assert!(lines.iter().any(|line| line.contains("already running")));

    // The first daemon was not disturbed.
    let mut client = Client::connect(&directory.socket());
    assert_eq!(client.call("status")["ok"], true);
    drop(client);
    first.signal("TERM");
    let (code, _) = first.finish();
    assert_eq!(code, Some(0));
}

#[test]
fn a_stale_socket_is_recovered_at_startup() {
    let directory = TestDirectory::new(0o700);
    start_fake_hyprland(directory.path(), None, None);
    fs::create_dir(directory.path().join("clea")).expect("clea directory should be created");
    fs::set_permissions(directory.path().join("clea"), Permissions::from_mode(0o700))
        .expect("clea directory mode should be set");
    // A listener that is gone leaves its socket file on disk.
    drop(UnixListener::bind(directory.socket()).expect("stale socket should bind"));
    assert!(directory.socket().exists());

    let mut daemon = Daemon::spawn(directory.path(), Some(SIGNATURE));
    daemon.expect_line_containing("listening on");
    let mut client = Client::connect(&directory.socket());
    assert_eq!(client.call("status")["ok"], true);
    drop(client);
    daemon.signal("TERM");
    let (code, _) = daemon.finish();

    assert_eq!(code, Some(0));
    assert!(!directory.socket().exists());
}

#[test]
fn a_second_signal_during_a_stalled_shutdown_forces_exit_seven() {
    let directory = TestDirectory::new(0o700);
    let (hold, held) = mpsc::channel();
    let (received, request_arrived) = mpsc::channel();
    start_fake_hyprland(directory.path(), Some(held), Some(received));
    let mut daemon = Daemon::spawn(directory.path(), Some(SIGNATURE));
    daemon.expect_line_containing("listening on");
    let mut client = Client::connect(&directory.socket());

    // The fake Hyprland holds its answer, so the authority stays inside the
    // request and the orderly shutdown cannot finish.
    client.send("list_windows");
    request_arrived
        .recv_timeout(GUARD)
        .expect("the request should reach the fake Hyprland");

    daemon.signal("TERM");
    // The first signal has been read, so the second one cannot be merged with it.
    daemon.expect_line_containing("shutdown requested");
    daemon.signal("TERM");
    let (code, lines) = daemon.finish();

    assert_eq!(code, Some(7));
    assert!(lines.iter().any(|line| line.contains("forcing exit")));
    // A forced exit runs no destructors, so the socket file stays on disk.
    assert!(directory.socket().exists());
    drop(hold);
}
