//! One connection per command: connect, send one line, read one line, close.
//!
//! The socket path is only `$XDG_RUNTIME_DIR/nycti/window-management.sock`,
//! with a variable that is set, non-empty, and absolute. There is no flag and
//! no fallback, and the client creates nothing. The daemon owns the path.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const RUNTIME_ENVIRONMENT_VARIABLE: &str = "XDG_RUNTIME_DIR";
const RUNTIME_DIRECTORY: &str = "nycti";
const SOCKET_FILE: &str = "window-management.sock";

/// How long to wait to connect and to write the request.
pub const CONNECT_AND_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the response unless `--timeout` says otherwise.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest response line accepted, not counting its LF.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Timeouts and limits of one exchange. Every duration must be non-zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExchangeConfig {
    pub connect: Duration,
    pub write: Duration,
    /// The total time allowed for the whole response line to arrive.
    pub response: Duration,
    pub max_response_bytes: usize,
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        Self {
            connect: CONNECT_AND_WRITE_TIMEOUT,
            write: CONNECT_AND_WRITE_TIMEOUT,
            response: DEFAULT_RESPONSE_TIMEOUT,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
}

/// Why an exchange with the daemon failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionError {
    /// `XDG_RUNTIME_DIR` is not set or is empty.
    RuntimeDirUnset,
    /// `XDG_RUNTIME_DIR` is not an absolute path.
    RuntimeDirNotAbsolute,
    /// The socket does not exist.
    DaemonNotRunning,
    /// The socket file exists but nothing listens on it.
    StaleSocket,
    PermissionDenied,
    ConnectFailed(io::ErrorKind),
    ConnectTimedOut,
    /// The request could not be written. It was not processed.
    SendFailed(io::ErrorKind),
    /// The daemon closed the connection without a response.
    Closed,
    ResponseTimedOut,
    ReceiveFailed(io::ErrorKind),
    /// The connection ended in the middle of a line, with no LF.
    IncompleteResponse,
    /// The line is longer than the limit.
    ResponseTooLarge,
    /// A failure of the client itself.
    Internal(&'static str),
}

impl ConnectionError {
    /// Returns whether the request was completely sent, so the daemon may have
    /// acted on it even though no valid response was obtained.
    pub const fn request_was_sent(&self) -> bool {
        matches!(
            self,
            Self::Closed
                | Self::ResponseTimedOut
                | Self::ReceiveFailed(_)
                | Self::IncompleteResponse
                | Self::ResponseTooLarge
        )
    }
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeDirUnset => formatter
                .write_str("XDG_RUNTIME_DIR is not set, so the daemon socket cannot be located"),
            Self::RuntimeDirNotAbsolute => {
                formatter.write_str("XDG_RUNTIME_DIR is not an absolute path")
            }
            Self::DaemonNotRunning => {
                formatter.write_str("the daemon is not running (the socket does not exist)")
            }
            Self::StaleSocket => formatter.write_str(
                "the socket exists but nobody is listening on it; it is stale \
                 (the daemon is not running)",
            ),
            Self::PermissionDenied => formatter
                .write_str("permission denied on the daemon socket; is XDG_RUNTIME_DIR yours?"),
            Self::ConnectFailed(kind) => {
                write!(formatter, "cannot connect to the daemon socket ({kind})")
            }
            Self::ConnectTimedOut => formatter.write_str("timed out connecting to the daemon"),
            Self::SendFailed(kind) => {
                write!(
                    formatter,
                    "cannot send the request ({kind}); nothing was processed"
                )
            }
            Self::Closed => formatter
                .write_str("internal failure: the daemon closed the connection without responding"),
            Self::ResponseTimedOut => formatter.write_str("timed out waiting for the response"),
            Self::ReceiveFailed(kind) => {
                write!(formatter, "cannot read the response ({kind})")
            }
            Self::IncompleteResponse => formatter
                .write_str("malformed response: the connection ended before the end of the line"),
            Self::ResponseTooLarge => {
                formatter.write_str("malformed response: the line exceeds the size limit")
            }
            Self::Internal(reason) => write!(formatter, "internal failure: {reason}"),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// Builds the socket path from the value of `XDG_RUNTIME_DIR`.
pub fn socket_path(runtime_dir: Option<OsString>) -> Result<PathBuf, ConnectionError> {
    let runtime_dir = runtime_dir
        .filter(|value| !value.is_empty())
        .ok_or(ConnectionError::RuntimeDirUnset)?;
    let runtime_dir = PathBuf::from(runtime_dir);
    if !runtime_dir.is_absolute() {
        return Err(ConnectionError::RuntimeDirNotAbsolute);
    }
    Ok(runtime_dir.join(RUNTIME_DIRECTORY).join(SOCKET_FILE))
}

/// Builds the socket path from the process environment.
pub fn socket_path_from_env() -> Result<PathBuf, ConnectionError> {
    socket_path(std::env::var_os(RUNTIME_ENVIRONMENT_VARIABLE))
}

fn classify_connect_error(kind: io::ErrorKind) -> ConnectionError {
    match kind {
        io::ErrorKind::NotFound => ConnectionError::DaemonNotRunning,
        io::ErrorKind::ConnectionRefused => ConnectionError::StaleSocket,
        io::ErrorKind::PermissionDenied => ConnectionError::PermissionDenied,
        other => ConnectionError::ConnectFailed(other),
    }
}

/// Connects with a timeout.
///
/// The standard library has no connect timeout for Unix sockets, so the
/// connection is made on a helper thread and waited for with `recv_timeout`.
/// After a timeout the helper thread is left to finish on its own, which is
/// harmless in a process that ends right after the command. This is a
/// provisional workaround (ADR 0010).
fn connect(path: &Path, timeout: Duration) -> Result<UnixStream, ConnectionError> {
    let (sender, receiver) = mpsc::channel();
    let target = path.to_path_buf();
    thread::Builder::new()
        .name("nycti-connect".to_owned())
        .spawn(move || {
            let _ = sender.send(UnixStream::connect(target));
        })
        .map_err(|_| ConnectionError::Internal("cannot create the connection thread"))?;

    match receiver.recv_timeout(timeout) {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => Err(classify_connect_error(error.kind())),
        Err(RecvTimeoutError::Timeout) => Err(ConnectionError::ConnectTimedOut),
        Err(RecvTimeoutError::Disconnected) => Err(ConnectionError::Internal(
            "the connection thread ended unexpectedly",
        )),
    }
}

/// Sends `request` (one JSON line without LF) and returns the response line
/// without its LF.
///
/// Anything after the first LF of the response is ignored: the protocol answers
/// one request with exactly one line.
pub fn exchange(
    path: &Path,
    request: &str,
    config: &ExchangeConfig,
) -> Result<Vec<u8>, ConnectionError> {
    let mut stream = connect(path, config.connect)?;

    stream
        .set_write_timeout(Some(config.write))
        .map_err(|_| ConnectionError::Internal("cannot set the write timeout"))?;
    let mut line = Vec::with_capacity(request.len() + 1);
    line.extend_from_slice(request.as_bytes());
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|error| ConnectionError::SendFailed(error.kind()))?;

    read_line(&mut stream, config)
}

fn read_line(stream: &mut UnixStream, config: &ExchangeConfig) -> Result<Vec<u8>, ConnectionError> {
    // The timeout is a deadline for the whole line, not for each read, so a
    // peer that sends one byte at a time cannot extend it.
    let deadline = Instant::now() + config.response;
    let mut line = Vec::new();
    let mut chunk = [0_u8; 8192];

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(ConnectionError::ResponseTimedOut)?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| ConnectionError::Internal("cannot set the read timeout"))?;

        match stream.read(&mut chunk) {
            Ok(0) if line.is_empty() => return Err(ConnectionError::Closed),
            Ok(0) => return Err(ConnectionError::IncompleteResponse),
            Ok(count) => {
                let received = &chunk[..count];
                let end = received.iter().position(|byte| *byte == b'\n');
                line.extend_from_slice(&received[..end.unwrap_or(count)]);
                if line.len() > config.max_response_bytes {
                    return Err(ConnectionError::ResponseTooLarge);
                }
                if end.is_some() {
                    return Ok(line);
                }
            }
            Err(error) => match error.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                    return Err(ConnectionError::ResponseTimedOut);
                }
                io::ErrorKind::ConnectionReset => return Err(ConnectionError::Closed),
                kind => return Err(ConnectionError::ReceiveFailed(kind)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    use super::*;
    use crate::client::test_support::{Behavior, FakeServer, ignores_file_permissions};
    use crate::daemon::test_support::{TestDirectory, bound_listener, guarded};

    /// A short response timeout, so the tests that wait for it stay fast.
    const SHORT: Duration = Duration::from_millis(150);

    fn config() -> ExchangeConfig {
        ExchangeConfig::default()
    }

    #[test]
    fn the_documented_timeouts_and_limit_are_the_defaults() {
        let config = ExchangeConfig::default();

        assert_eq!(config.connect, Duration::from_secs(5));
        assert_eq!(config.write, Duration::from_secs(5));
        assert_eq!(config.response, Duration::from_secs(10));
        assert_eq!(config.max_response_bytes, 8 * 1024 * 1024);
    }

    #[test]
    fn the_socket_path_comes_only_from_a_valid_runtime_directory() {
        assert_eq!(
            socket_path(Some("/run/user/1000".into())),
            Ok(PathBuf::from("/run/user/1000/nycti/window-management.sock"))
        );
        assert_eq!(socket_path(None), Err(ConnectionError::RuntimeDirUnset));
        assert_eq!(
            socket_path(Some(OsString::new())),
            Err(ConnectionError::RuntimeDirUnset)
        );
        assert_eq!(
            socket_path(Some("run/user/1000".into())),
            Err(ConnectionError::RuntimeDirNotAbsolute)
        );
        assert_eq!(
            socket_path(Some(".".into())),
            Err(ConnectionError::RuntimeDirNotAbsolute)
        );
    }

    #[test]
    fn the_client_path_is_the_one_the_daemon_binds() {
        let directory = TestDirectory::new();
        let listener = bound_listener(&directory);

        let path = socket_path(Some(directory.path().as_os_str().to_os_string()));

        assert_eq!(path.as_deref(), Ok(listener.socket_path()));
    }

    #[test]
    fn one_request_line_is_sent_and_one_response_line_is_returned_without_its_lf() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::line(r#"{"a":1}"#));

            let response = exchange(&server.path(), r#"{"request":true}"#, &config());

            assert_eq!(response, Ok(br#"{"a":1}"#.to_vec()));
            assert_eq!(server.request_line(), r#"{"request":true}"#);
        });
    }

    #[test]
    fn bytes_after_the_first_line_feed_are_ignored() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::raw("first\nsecond\n"));

            let response = exchange(&server.path(), "{}", &config());

            assert_eq!(response, Ok(b"first".to_vec()));
        });
    }

    #[test]
    fn a_missing_socket_means_the_daemon_is_not_running() {
        let directory = TestDirectory::new();

        let result = exchange(&directory.path().join("absent.sock"), "{}", &config());

        assert_eq!(result, Err(ConnectionError::DaemonNotRunning));
    }

    #[test]
    fn a_socket_file_nobody_listens_on_is_stale() {
        let directory = TestDirectory::new();
        let path = directory.path().join("stale.sock");
        // A listener that is gone leaves its socket file on disk.
        drop(UnixListener::bind(&path).expect("stale socket should bind"));
        assert!(path.exists());

        let result = exchange(&path, "{}", &config());

        assert_eq!(result, Err(ConnectionError::StaleSocket));
    }

    #[test]
    fn a_socket_with_mode_000_is_permission_denied() {
        let directory = TestDirectory::new();
        if ignores_file_permissions(&directory) {
            // A privileged user connects to any socket, so the case cannot be
            // simulated; the other tests still cover the classification.
            return;
        }
        let path = directory.path().join("closed.sock");
        let _listener = UnixListener::bind(&path).expect("socket should bind");
        fs::set_permissions(&path, Permissions::from_mode(0o000))
            .expect("socket mode should be set");

        let result = exchange(&path, "{}", &config());

        assert_eq!(result, Err(ConnectionError::PermissionDenied));
    }

    #[test]
    fn a_daemon_that_closes_without_responding_is_reported_as_closed() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::Close);

            let result = exchange(&server.path(), "{}", &config());

            assert_eq!(result, Err(ConnectionError::Closed));
            assert_eq!(server.request_line(), "{}");
        });
    }

    /// This test waits for the real response timeout, so it is the one
    /// time-dependent test of the module. `guarded` only protects it from a hang.
    #[test]
    fn a_daemon_that_never_responds_ends_with_a_response_timeout() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::Mute);
            let config = ExchangeConfig {
                response: SHORT,
                ..config()
            };

            let result = exchange(&server.path(), "{}", &config);

            assert_eq!(result, Err(ConnectionError::ResponseTimedOut));
        });
    }

    #[test]
    fn a_line_without_a_line_feed_is_incomplete() {
        guarded(|| {
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::raw(r#"{"version":1"#));

            let result = exchange(&server.path(), "{}", &config());

            assert_eq!(result, Err(ConnectionError::IncompleteResponse));
        });
    }

    #[test]
    fn a_line_above_the_limit_is_refused_and_one_at_the_limit_is_accepted() {
        guarded(|| {
            let config = ExchangeConfig {
                max_response_bytes: 16,
                ..config()
            };

            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::line(&"x".repeat(16)));
            assert_eq!(exchange(&server.path(), "{}", &config), Ok(vec![b'x'; 16]));
            drop(server);

            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::line(&"x".repeat(17)));
            assert_eq!(
                exchange(&server.path(), "{}", &config),
                Err(ConnectionError::ResponseTooLarge)
            );
            drop(server);

            // Without any line feed the limit is hit as well.
            let directory = TestDirectory::new();
            let server = FakeServer::start(&directory, Behavior::raw("x".repeat(64)));
            assert_eq!(
                exchange(&server.path(), "{}", &config),
                Err(ConnectionError::ResponseTooLarge)
            );
        });
    }

    #[test]
    fn only_failures_after_the_request_was_sent_say_so() {
        let after = [
            ConnectionError::Closed,
            ConnectionError::ResponseTimedOut,
            ConnectionError::ReceiveFailed(io::ErrorKind::Other),
            ConnectionError::IncompleteResponse,
            ConnectionError::ResponseTooLarge,
        ];
        let before = [
            ConnectionError::RuntimeDirUnset,
            ConnectionError::RuntimeDirNotAbsolute,
            ConnectionError::DaemonNotRunning,
            ConnectionError::StaleSocket,
            ConnectionError::PermissionDenied,
            ConnectionError::ConnectFailed(io::ErrorKind::Other),
            ConnectionError::ConnectTimedOut,
            ConnectionError::SendFailed(io::ErrorKind::BrokenPipe),
            ConnectionError::Internal("x"),
        ];

        assert!(after.iter().all(ConnectionError::request_was_sent));
        assert!(!before.iter().any(ConnectionError::request_was_sent));
    }
}
