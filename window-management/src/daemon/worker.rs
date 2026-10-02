//! Connection workers: one synchronous thread per accepted Unix connection.
//!
//! A worker owns one accepted `UnixStream` and one [`AuthorityClient`]. It
//! serves the connection through the existing handler-based Unix runtime
//! function, forwarding each complete request line to the authority. It never
//! sees the service, the manager, or the backend, and it implements no framing.

use std::fmt;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::thread::{self, JoinHandle};

use super::{AuthorityClient, AuthorityError};
use crate::runtime::{UnixRuntimeError, UnixServeError, serve_unix_connection_with_handler};
use crate::transport::TransportError;

/// How a connection worker ended.
///
/// Only `Completed` is a normal end. The other variants are internal
/// daemon/runtime outcomes for this one connection; none of them is written to
/// the client as a protocol response, and the client observes only EOF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerExit {
    /// The client closed the connection (clean EOF, or an incomplete final line).
    Completed,
    /// Reading, writing, or flushing failed on this connection.
    Transport(TransportError),
    /// The accepted stream could not be prepared; no request was handled.
    Preparation(UnixRuntimeError),
    /// The authority was unavailable or ended without a response. This is a
    /// sign of a fatal authority failure, but the coordinator must still
    /// observe the authority itself, through [`super::Authority::is_finished`].
    Authority(AuthorityError),
}

/// A failure in a worker's lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerLifecycleError {
    /// A duplicate of the connection stream needed for control or cleanup
    /// could not be created.
    ControlHandleFailed,
    /// The worker thread could not be created.
    SpawnFailed,
    /// The worker thread ended by panicking.
    Panicked,
}

impl fmt::Display for WorkerLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ControlHandleFailed => "connection control handle could not be created",
            Self::SpawnFailed => "connection worker thread could not be created",
            Self::Panicked => "connection worker thread panicked",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WorkerLifecycleError {}

/// Shuts the socket down when dropped, including while unwinding from a panic.
///
/// The coordinator keeps a duplicate descriptor of the same socket so it can
/// close the connection, and that duplicate keeps the socket open after the
/// worker ends. Shutting the socket down here, rather than relying on dropping
/// descriptors, lets the client observe EOF as soon as the worker ends.
struct CloseOnDrop(UnixStream);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// The coordinator's handle to one running connection worker.
///
/// Dropping a `WorkerHandle` without calling [`WorkerHandle::join`] detaches
/// the worker thread. It does not close the connection: the connection stays
/// open until the worker terminates on its own, so call
/// [`WorkerHandle::close_connection`] first when the worker must end.
pub struct WorkerHandle {
    control: UnixStream,
    thread: JoinHandle<WorkerExit>,
}

impl WorkerHandle {
    /// Returns whether the worker thread has ended, without blocking.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Shuts down the connection's socket in both directions, best effort.
    ///
    /// A worker blocked reading from the client wakes with EOF and ends with
    /// [`WorkerExit::Completed`]; a worker blocked writing to a slow client
    /// fails and ends with a transport error. This does not wake a worker that
    /// is waiting for the authority inside a request: it ends after the
    /// authority answers or ends, when its next read observes the closed
    /// socket. A "not connected" failure is expected when the client already
    /// closed, and every failure is ignored.
    pub fn close_connection(&self) {
        let _ = self.control.shutdown(Shutdown::Both);
    }

    /// Waits for the worker thread and returns how it ended.
    ///
    /// Blocks until the worker ends. Returns only `WorkerLifecycleError::Panicked`
    /// as an error; a worker that ended normally or through a connection or
    /// authority failure returns its [`WorkerExit`].
    pub fn join(self) -> Result<WorkerExit, WorkerLifecycleError> {
        self.thread
            .join()
            .map_err(|_| WorkerLifecycleError::Panicked)
    }
}

/// Starts a worker thread that serves `stream` through the authority.
///
/// The worker owns `client`, so the authority's shutdown waits for the worker
/// to end. The coordinator must not serve the connection itself if this fails.
///
/// Returns `WorkerLifecycleError::ControlHandleFailed` when a stream duplicate
/// cannot be created, or `WorkerLifecycleError::SpawnFailed` when the thread
/// cannot be created. It never returns `Panicked`. On either error the stream
/// is closed and the authority and existing workers are unaffected.
pub fn spawn_worker(
    stream: UnixStream,
    client: AuthorityClient,
) -> Result<WorkerHandle, WorkerLifecycleError> {
    spawn_worker_with_handler(stream, move |line| client.submit(line))
}

/// Starts a worker thread with an arbitrary request handler.
///
/// The `Send + 'static` bound applies to the handler because this is where it
/// moves to another thread. Production code passes the authority client through
/// [`spawn_worker`]; tests use this to inject a failing handler.
pub(crate) fn spawn_worker_with_handler<H>(
    stream: UnixStream,
    handler: H,
) -> Result<WorkerHandle, WorkerLifecycleError>
where
    H: FnMut(&[u8]) -> Result<String, AuthorityError> + Send + 'static,
{
    let control = stream
        .try_clone()
        .map_err(|_| WorkerLifecycleError::ControlHandleFailed)?;
    let guard = CloseOnDrop(
        stream
            .try_clone()
            .map_err(|_| WorkerLifecycleError::ControlHandleFailed)?,
    );
    let thread = thread::Builder::new()
        .name("nycti-windowd-worker".to_owned())
        .spawn(move || run_worker(stream, guard, handler))
        .map_err(|_| WorkerLifecycleError::SpawnFailed)?;

    Ok(WorkerHandle { control, thread })
}

fn run_worker<H>(stream: UnixStream, _guard: CloseOnDrop, handler: H) -> WorkerExit
where
    H: FnMut(&[u8]) -> Result<String, AuthorityError>,
{
    match serve_unix_connection_with_handler(stream, handler) {
        Ok(()) => WorkerExit::Completed,
        Err(UnixServeError::Runtime(error)) => WorkerExit::Preparation(error),
        Err(UnixServeError::Transport(error)) => WorkerExit::Transport(error),
        Err(UnixServeError::Handler(error)) => WorkerExit::Authority(error),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::mpsc;

    use serde_json::{Value, json};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::core::{WindowManager, WorkspaceMode};
    use crate::daemon::Authority;
    use crate::daemon::test_support::{
        GUARD, GateEvent, GatedBackend, PanickingBackend, call, fake_with_workspace_and_window,
        guarded, lined, request, response, service_with, workspace_tokens,
    };
    use crate::service::WindowManagementService;

    /// The client end of a connection served by a worker.
    struct TestClient {
        writer: UnixStream,
        reader: BufReader<UnixStream>,
    }

    impl TestClient {
        fn new(stream: UnixStream) -> Self {
            let reader = BufReader::new(stream.try_clone().expect("client stream should clone"));
            Self {
                writer: stream,
                reader,
            }
        }

        fn send(&mut self, bytes: &[u8]) {
            self.writer
                .write_all(bytes)
                .expect("request should be written");
        }

        fn read_response(&mut self) -> Value {
            let mut line = String::new();
            self.reader
                .read_line(&mut line)
                .expect("response line should be read");
            response(&line)
        }

        fn round_trip(&mut self, id: &str, method: &str, params: Value) -> Value {
            self.send(&lined(&request(id, method, params)));
            self.read_response()
        }

        /// Reads until the server closes the connection.
        fn read_to_eof(&mut self) -> Vec<u8> {
            let mut bytes = Vec::new();
            self.reader
                .read_to_end(&mut bytes)
                .expect("connection should end cleanly");
            bytes
        }
    }

    fn start_worker(client: &AuthorityClient) -> (WorkerHandle, TestClient) {
        let (server, peer) = UnixStream::pair().expect("stream pair should be created");
        let handle = spawn_worker(server, client.clone()).expect("worker should start");
        (handle, TestClient::new(peer))
    }

    fn start_authority() -> Authority {
        Authority::spawn(service_with(FakeBackend::new())).expect("authority should start")
    }

    #[test]
    fn worker_serves_a_request_and_completes_when_the_client_closes() {
        guarded(|| {
            let authority = start_authority();
            let (worker, mut client) = start_worker(&authority.client());

            let reply = client.round_trip("status", "status", json!({}));
            assert_eq!(reply["id"], "status");
            assert_eq!(reply["result"]["service"], "clea-windowd");
            assert!(!worker.is_finished());

            drop(client);
            assert_eq!(worker.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn requests_on_one_worker_connection_keep_their_order() {
        guarded(|| {
            let authority = start_authority();
            let (worker, mut client) = start_worker(&authority.client());
            let mut input = Vec::new();
            for id in ["first", "second", "third"] {
                input.extend(lined(&request(id, "status", json!({}))));
            }

            client.send(&input);
            let ids = (0..3)
                .map(|_| client.read_response()["id"].clone())
                .collect::<Vec<_>>();

            assert_eq!(ids, vec![json!("first"), json!("second"), json!("third")]);
            drop(client);
            assert_eq!(worker.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn two_workers_share_default_mode_explicit_mode_and_tokens() {
        guarded(|| {
            let authority = Authority::spawn(service_with(fake_with_workspace_and_window()))
                .expect("authority should start");
            let (worker_a, mut client_a) = start_worker(&authority.client());
            let (worker_b, mut client_b) = start_worker(&authority.client());

            let set = client_a.round_trip("set", "set_default_mode", json!({"mode": "windows"}));
            let default = client_b.round_trip("get", "get_default_mode", json!({}));
            assert_eq!(set["result"]["mode"], "windows");
            assert_eq!(default["result"]["mode"], "windows");

            let workspaces_a = client_a.round_trip("ws-a", "list_workspaces", json!({}));
            let workspaces_b = client_b.round_trip("ws-b", "list_workspaces", json!({}));
            let token = workspaces_a["result"]["workspaces"][0]["workspace_id"].clone();
            assert_eq!(
                token,
                workspaces_b["result"]["workspaces"][0]["workspace_id"]
            );

            let windows_a = client_a.round_trip("win-a", "list_windows", json!({}));
            let windows_b = client_b.round_trip("win-b", "list_windows", json!({}));
            assert_eq!(
                windows_a["result"]["windows"][0]["window_id"],
                windows_b["result"]["windows"][0]["window_id"]
            );

            client_a.round_trip(
                "explicit",
                "set_workspace_mode",
                json!({"workspace_id": token, "mode": "tiling"}),
            );
            let observed = client_b.round_trip(
                "observed",
                "get_workspace_mode",
                json!({"workspace_id": token}),
            );
            assert_eq!(observed["result"]["explicit_mode"], "tiling");

            drop((client_a, client_b));
            assert_eq!(worker_a.join(), Ok(WorkerExit::Completed));
            assert_eq!(worker_b.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn idle_worker_does_not_block_another_worker() {
        guarded(|| {
            let authority = start_authority();
            let (worker_a, mut client_a) = start_worker(&authority.client());
            let (worker_b, mut client_b) = start_worker(&authority.client());

            // A is served once and then left connected and idle.
            client_a.round_trip("a", "status", json!({}));
            let other = client_b.round_trip("b", "status", json!({}));
            assert_eq!(other["ok"], true);
            assert!(!worker_a.is_finished());

            drop(client_a);
            assert_eq!(worker_a.join(), Ok(WorkerExit::Completed));
            drop(client_b);
            assert_eq!(worker_b.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn client_disconnect_on_one_worker_does_not_end_another() {
        guarded(|| {
            let authority = start_authority();
            let (worker_a, client_a) = start_worker(&authority.client());
            let (worker_b, mut client_b) = start_worker(&authority.client());

            drop(client_a);
            assert_eq!(worker_a.join(), Ok(WorkerExit::Completed));

            let other = client_b.round_trip("b", "status", json!({}));
            assert_eq!(other["ok"], true);
            drop(client_b);
            assert_eq!(worker_b.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn protocol_errors_on_one_worker_affect_neither_it_nor_another_worker() {
        guarded(|| {
            let authority = start_authority();
            let (worker_a, mut client_a) = start_worker(&authority.client());
            let (worker_b, mut client_b) = start_worker(&authority.client());

            client_a.send(b"bad-json\n");
            let invalid_json = client_a.read_response();
            let invalid_params =
                client_a.round_trip("bad", "set_default_mode", json!({"mode": "floating"}));
            let still_served = client_a.round_trip("a", "status", json!({}));
            let other = client_b.round_trip("b", "status", json!({}));

            assert_eq!(invalid_json["error"]["code"], "invalid_request");
            assert_eq!(invalid_params["error"]["code"], "invalid_params");
            assert_eq!(still_served["ok"], true);
            assert_eq!(other["ok"], true);

            drop((client_a, client_b));
            assert_eq!(worker_a.join(), Ok(WorkerExit::Completed));
            assert_eq!(worker_b.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn write_failure_after_admission_keeps_the_effect_and_the_authority() {
        guarded(|| {
            let (events, observed) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let backend = GatedBackend {
                inner: fake_with_workspace_and_window(),
                events,
                release: gate,
            };
            let authority = Authority::spawn(WindowManagementService::new(WindowManager::new(
                backend,
                WorkspaceMode::Windows,
            )))
            .expect("authority should start");
            let token = workspace_tokens(&authority.client()).remove(0);
            let (worker_a, mut client_a) = start_worker(&authority.client());

            // The request is admitted and is blocked inside the backend.
            client_a.send(&lined(&request(
                "apply",
                "apply_workspace_mode",
                json!({"workspace_id": token}),
            )));
            assert_eq!(observed.recv_timeout(GUARD), Ok(GateEvent::Enter));

            // Only now does the requester leave, and only then is the backend released.
            drop(client_a);
            release.send(()).expect("backend should await release");

            assert_eq!(
                worker_a.join(),
                Ok(WorkerExit::Transport(TransportError::WriteFailed))
            );
            assert_eq!(observed.recv_timeout(GUARD), Ok(GateEvent::Exit));

            // The admitted mutation took effect and the authority keeps serving.
            let (worker_b, mut client_b) = start_worker(&authority.client());
            let windows = client_b.round_trip("windows", "list_windows", json!({}));
            assert_eq!(windows["result"]["windows"][0]["placement"], "floating");
            assert!(!authority.is_finished());

            drop(client_b);
            assert_eq!(worker_b.join(), Ok(WorkerExit::Completed));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn authority_failure_ends_only_that_connection_without_a_protocol_response() {
        guarded(|| {
            let authority = Authority::spawn(WindowManagementService::new(WindowManager::new(
                PanickingBackend,
                WorkspaceMode::Tiling,
            )))
            .expect("authority should start");
            let (worker_a, mut client_a) = start_worker(&authority.client());

            // The authority panics while handling A's admitted request.
            client_a.send(&lined(&request("boom", "list_windows", json!({}))));
            // The client sees EOF and no response while the worker handle is
            // still alive, so the connection was closed by the worker itself.
            assert!(client_a.read_to_eof().is_empty());
            assert_eq!(
                worker_a.join(),
                Ok(WorkerExit::Authority(AuthorityError::ResponseLost))
            );

            // Only after A ended does B submit to the now-dead authority.
            let (worker_b, mut client_b) = start_worker(&authority.client());
            client_b.send(&lined(&request("after", "status", json!({}))));
            assert!(client_b.read_to_eof().is_empty());
            assert_eq!(
                worker_b.join(),
                Ok(WorkerExit::Authority(AuthorityError::Unavailable))
            );

            assert_eq!(
                authority.shutdown(),
                Err(crate::daemon::AuthorityLifecycleError::Panicked)
            );
        });
    }

    #[test]
    fn close_connection_wakes_an_idle_worker_and_lets_the_authority_shut_down() {
        guarded(|| {
            let authority = start_authority();
            let (worker, mut client) = start_worker(&authority.client());
            client.round_trip("status", "status", json!({}));

            // The worker is now blocked reading from an idle client.
            worker.close_connection();

            assert!(client.read_to_eof().is_empty());
            assert_eq!(worker.join(), Ok(WorkerExit::Completed));
            drop(client);
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn worker_panic_closes_only_its_own_connection() {
        guarded(|| {
            let authority = start_authority();
            let (server, peer) = UnixStream::pair().expect("stream pair should be created");
            let panicking =
                spawn_worker_with_handler(server, |_| -> Result<String, AuthorityError> {
                    panic!("intentional worker failure in test");
                })
                .expect("worker should start");
            let mut panicked_client = TestClient::new(peer);
            let (worker, mut client) = start_worker(&authority.client());

            panicked_client.send(&lined(&request("boom", "status", json!({}))));
            assert!(panicked_client.read_to_eof().is_empty());
            assert_eq!(panicking.join(), Err(WorkerLifecycleError::Panicked));

            // Another worker and the authority are unaffected.
            assert_eq!(client.round_trip("ok", "status", json!({}))["ok"], true);
            assert!(!authority.is_finished());

            drop(client);
            assert_eq!(worker.join(), Ok(WorkerExit::Completed));
            let direct = call(&authority.client(), "direct", "status", json!({}));
            assert_eq!(direct["ok"], true);
            authority.shutdown().expect("authority should shut down");
        });
    }
}
