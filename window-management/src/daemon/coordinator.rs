//! The daemon coordinator: accept loop, connection workers, shutdown, and
//! fatal authority failure.
//!
//! One accept/lifecycle thread owns the listener, the [`Authority`], and every
//! [`WorkerHandle`]. Decisions are recorded in ADR 0007, which refines ADR 0006.
//!
//! The blocked `accept` is woken by connecting to the daemon's own service
//! socket after a lifecycle event has been queued. Events come from a
//! [`ShutdownHandle`] and from the authority's exit hook. No polling, timeout,
//! or shared state is used.

use std::fmt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use super::{
    Authority, AuthorityLifecycleError, WorkerExit, WorkerHandle, WorkerLifecycleError,
    spawn_worker,
};
use crate::backend::WindowBackend;
use crate::runtime::{UnixRuntimeError, UnixRuntimeListener};
use crate::service::WindowManagementService;

/// Something the accept thread must react to.
#[derive(Debug, PartialEq, Eq)]
enum LifecycleEvent {
    Shutdown,
    AuthorityExited,
}

/// Why a shutdown request could not reach the coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownError {
    /// The coordinator has already stopped, so nothing was sent.
    AlreadyStopped,
    /// The request was queued, but the connection that wakes the accept loop
    /// failed, so the loop may stay blocked until the next client connects.
    WakeFailed,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::AlreadyStopped => "window management coordinator has already stopped",
            Self::WakeFailed => "window management accept loop could not be woken",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ShutdownError {}

/// Queues lifecycle events and wakes the blocked accept loop.
#[derive(Clone)]
struct Waker {
    socket_path: PathBuf,
    events: Sender<LifecycleEvent>,
}

impl Waker {
    /// Delivers `event` and then connects to the service socket so the blocked
    /// `accept` returns.
    ///
    /// The event is delivered first, and the connection is made only if it was
    /// delivered. When the receiver is gone the coordinator has stopped and this
    /// returns `AlreadyStopped` without connecting, so it cannot block on a
    /// listener that nobody accepts from any more. Every failure is returned,
    /// never raised, so this is safe to call from an exit hook.
    fn wake(&self, event: LifecycleEvent) -> Result<(), ShutdownError> {
        self.events
            .send(event)
            .map_err(|_| ShutdownError::AlreadyStopped)?;
        UnixStream::connect(&self.socket_path)
            .map(drop)
            .map_err(|_| ShutdownError::WakeFailed)
    }
}

/// Requests an orderly shutdown of the coordinator.
///
/// This is the point where signal handling will connect later. It can be cloned
/// and used from any thread.
#[derive(Clone)]
pub struct ShutdownHandle {
    waker: Waker,
}

impl ShutdownHandle {
    /// Asks the coordinator to stop and wakes its accept loop.
    ///
    /// Returns `ShutdownError::AlreadyStopped` when the coordinator has already
    /// stopped, and `ShutdownError::WakeFailed` when the request was queued but
    /// the accept loop could not be woken.
    pub fn request_shutdown(&self) -> Result<(), ShutdownError> {
        self.waker.wake(LifecycleEvent::Shutdown)
    }
}

/// A failure of the coordinator itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorError {
    /// The authority could not be started. The listener is dropped and cleaned.
    AuthorityStartFailed(AuthorityLifecycleError),
    /// The accept thread could not be created.
    SpawnFailed,
    /// The accept thread panicked. See [`Coordinator::wait`].
    Panicked,
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthorityStartFailed(error) => error.fmt(formatter),
            Self::SpawnFailed => formatter.write_str("coordinator thread could not be created"),
            Self::Panicked => formatter.write_str("coordinator thread panicked"),
        }
    }
}

impl std::error::Error for CoordinatorError {}

/// Why the accept loop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// A shutdown was requested through a [`ShutdownHandle`].
    ShutdownRequested,
    /// The authority ended while the coordinator was running. This is fatal.
    AuthorityFailed,
    /// Accepting a connection failed. Accept errors are not classified, because
    /// `UnixRuntimeError::AcceptFailed` does not keep the `io::ErrorKind`.
    AcceptFailed(UnixRuntimeError),
}

/// What happened while the coordinator ran and how each shutdown step ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinatorReport {
    pub reason: StopReason,
    /// Workers that ended because their client closed or the connection was closed.
    pub workers_completed: usize,
    /// Workers that ended with a connection, preparation, or authority failure.
    pub workers_failed: usize,
    /// Workers that panicked.
    pub workers_panicked: usize,
    /// Connections closed because a worker could not be created.
    pub connections_refused: usize,
    /// Result of joining the authority thread.
    pub authority: Result<(), AuthorityLifecycleError>,
    /// Result of the identity-checked listener cleanup.
    pub listener_cleanup: Result<(), UnixRuntimeError>,
}

#[derive(Default)]
struct Tally {
    completed: usize,
    failed: usize,
    panicked: usize,
    refused: usize,
}

impl Tally {
    fn record(&mut self, exit: Result<WorkerExit, WorkerLifecycleError>) {
        match exit {
            Ok(WorkerExit::Completed) => self.completed += 1,
            Ok(_) => self.failed += 1,
            Err(_) => self.panicked += 1,
        }
    }
}

/// Owns the accept/lifecycle thread of the daemon.
///
/// Dropping a `Coordinator` without calling [`Coordinator::wait`] detaches the
/// accept thread, which keeps running until a shutdown is requested.
pub struct Coordinator {
    handle: ShutdownHandle,
    thread: JoinHandle<CoordinatorReport>,
}

impl Coordinator {
    /// Starts the authority and the accept thread.
    ///
    /// The caller has already bound the listener and built the service, in the
    /// startup order of ADR 0006. This moves the service into the authority and
    /// begins accepting. If starting fails, the listener is dropped and its
    /// socket is cleaned up by the listener's own `Drop`.
    ///
    /// The `Send + 'static` bound is here because the service, and with it the
    /// backend, moves to other threads.
    pub fn start<B>(
        listener: UnixRuntimeListener,
        service: WindowManagementService<B>,
    ) -> Result<Self, CoordinatorError>
    where
        B: WindowBackend + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let waker = Waker {
            socket_path: listener.socket_path().to_path_buf(),
            events: sender,
        };
        let exit_waker = waker.clone();
        // The hook must not panic, so it ignores the result of the wake.
        let authority = Authority::spawn_with_exit_hook(service, move || {
            let _ = exit_waker.wake(LifecycleEvent::AuthorityExited);
        })
        .map_err(CoordinatorError::AuthorityStartFailed)?;

        let thread = thread::Builder::new()
            .name("nycti-windowd-accept".to_owned())
            .spawn(move || run_coordinator(listener, authority, receiver))
            .map_err(|_| CoordinatorError::SpawnFailed)?;

        Ok(Self {
            handle: ShutdownHandle { waker },
            thread,
        })
    }

    /// Returns a handle that can request shutdown from any thread.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.handle.clone()
    }

    /// Returns whether the accept thread has ended, without blocking.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Waits for the coordinator to stop and returns its report.
    ///
    /// Returns only `CoordinatorError::Panicked` as an error. A panic of the
    /// coordinator thread is fatal for the process: the workers and the
    /// authority are left running, detached, until the process exits. The
    /// future `main.rs` must observe [`Coordinator::is_finished`] or call this
    /// method, and end the process.
    pub fn wait(self) -> Result<CoordinatorReport, CoordinatorError> {
        self.thread.join().map_err(|_| CoordinatorError::Panicked)
    }
}

/// Reports why the loop must stop, draining every queued event.
fn pending_stop(events: &Receiver<LifecycleEvent>, authority: &Authority) -> Option<StopReason> {
    let mut authority_exited = false;
    let mut shutdown_requested = false;
    while let Ok(event) = events.try_recv() {
        match event {
            LifecycleEvent::AuthorityExited => authority_exited = true,
            LifecycleEvent::Shutdown => shutdown_requested = true,
        }
    }

    if authority_exited || authority.is_finished() {
        Some(StopReason::AuthorityFailed)
    } else if shutdown_requested {
        Some(StopReason::ShutdownRequested)
    } else {
        None
    }
}

/// Joins and forgets the workers that have already ended. This never blocks,
/// because only finished workers are joined.
fn reap_finished(workers: &mut Vec<WorkerHandle>, tally: &mut Tally) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            tally.record(workers.swap_remove(index).join());
        } else {
            index += 1;
        }
    }
}

fn run_coordinator(
    listener: UnixRuntimeListener,
    authority: Authority,
    events: Receiver<LifecycleEvent>,
) -> CoordinatorReport {
    let mut workers: Vec<WorkerHandle> = Vec::new();
    let mut tally = Tally::default();

    let reason = loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(error) => break StopReason::AcceptFailed(error),
        };
        // A wake-up connection and a real client look the same, so the queued
        // events decide. The stream is dropped without service when stopping.
        if let Some(reason) = pending_stop(&events, &authority) {
            drop(stream);
            break reason;
        }
        reap_finished(&mut workers, &mut tally);
        match spawn_worker(stream, authority.client()) {
            Ok(worker) => workers.push(worker),
            Err(_) => tally.refused += 1,
        }
    };

    // From here no waker connects to the socket: a send now fails first, and
    // the authority's exit hook therefore cannot block the joins below.
    drop(events);

    for worker in &workers {
        worker.close_connection();
    }
    for worker in workers {
        tally.record(worker.join());
    }
    // This drops the coordinator's authority client. Every worker's clone is
    // already gone, so the authority can end and be joined.
    let authority = authority.shutdown();
    let listener_cleanup = listener.cleanup();

    CoordinatorReport {
        reason,
        workers_completed: tally.completed,
        workers_failed: tally.failed,
        workers_panicked: tally.panicked,
        connections_refused: tally.refused,
        authority,
        listener_cleanup,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::Path;

    use serde_json::{Value, json};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::core::{WindowManager, WorkspaceMode};
    use crate::daemon::test_support::{
        PanickingBackend, TestDirectory, bound_listener, fake_with_workspace_and_window, guarded,
        lined, request, response, service_with,
    };

    /// A client connected to the coordinator's real service socket.
    struct Client {
        writer: UnixStream,
        reader: BufReader<UnixStream>,
    }

    impl Client {
        fn connect(path: &Path) -> Self {
            let stream = UnixStream::connect(path).expect("client should connect");
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

    fn start_with(
        directory: &TestDirectory,
        service: WindowManagementService<impl WindowBackend + Send + 'static>,
    ) -> (Coordinator, PathBuf) {
        let listener = bound_listener(directory);
        let path = listener.socket_path().to_path_buf();
        let coordinator = Coordinator::start(listener, service).expect("coordinator should start");
        (coordinator, path)
    }

    fn start_fake(directory: &TestDirectory) -> (Coordinator, PathBuf) {
        start_with(directory, service_with(FakeBackend::new()))
    }

    #[test]
    fn client_is_served_over_the_real_socket_and_shutdown_removes_it() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, path) = start_fake(&directory);
            let mut client = Client::connect(&path);

            let reply = client.round_trip("status", "status", json!({}));
            assert_eq!(reply["result"]["service"], "nycti-windowd");
            drop(client);
            coordinator
                .shutdown_handle()
                .request_shutdown()
                .expect("shutdown should be accepted");
            let report = coordinator.wait().expect("coordinator should not panic");

            assert_eq!(report.reason, StopReason::ShutdownRequested);
            assert_eq!(report.authority, Ok(()));
            assert_eq!(report.listener_cleanup, Ok(()));
            assert_eq!(report.workers_completed, 1);
            assert_eq!(report.workers_failed, 0);
            assert_eq!(report.workers_panicked, 0);
            assert_eq!(report.connections_refused, 0);
            assert!(!path.exists());
        });
    }

    #[test]
    fn shutdown_without_any_client_wakes_the_blocked_accept() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, path) = start_fake(&directory);

            coordinator
                .shutdown_handle()
                .request_shutdown()
                .expect("shutdown should be accepted");
            let report = coordinator.wait().expect("coordinator should not panic");

            assert_eq!(report.reason, StopReason::ShutdownRequested);
            assert_eq!(report.workers_completed, 0);
            assert_eq!(report.authority, Ok(()));
            assert_eq!(report.listener_cleanup, Ok(()));
            assert!(!path.exists());
        });
    }

    #[test]
    fn clients_share_state_and_shutdown_closes_their_open_connections() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, path) =
                start_with(&directory, service_with(fake_with_workspace_and_window()));
            let mut client_a = Client::connect(&path);
            let mut client_b = Client::connect(&path);

            let set = client_a.round_trip("set", "set_default_mode", json!({"mode": "windows"}));
            let default = client_b.round_trip("get", "get_default_mode", json!({}));
            assert_eq!(set["result"]["mode"], "windows");
            assert_eq!(default["result"]["mode"], "windows");

            let workspaces_a = client_a.round_trip("ws-a", "list_workspaces", json!({}));
            let workspaces_b = client_b.round_trip("ws-b", "list_workspaces", json!({}));
            assert_eq!(
                workspaces_a["result"]["workspaces"][0]["workspace_id"],
                workspaces_b["result"]["workspaces"][0]["workspace_id"]
            );
            let windows_a = client_a.round_trip("win-a", "list_windows", json!({}));
            let windows_b = client_b.round_trip("win-b", "list_windows", json!({}));
            assert_eq!(
                windows_a["result"]["windows"][0]["window_id"],
                windows_b["result"]["windows"][0]["window_id"]
            );

            // Both clients are still connected and idle when shutdown starts.
            coordinator
                .shutdown_handle()
                .request_shutdown()
                .expect("shutdown should be accepted");
            assert!(client_a.read_to_eof().is_empty());
            assert!(client_b.read_to_eof().is_empty());
            let report = coordinator.wait().expect("coordinator should not panic");

            assert_eq!(report.workers_completed, 2);
            assert_eq!(report.authority, Ok(()));
            assert_eq!(report.listener_cleanup, Ok(()));
        });
    }

    #[test]
    fn idle_connection_does_not_block_another_client() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, path) = start_fake(&directory);
            let mut idle = Client::connect(&path);
            idle.round_trip("idle", "status", json!({}));

            let mut other = Client::connect(&path);
            let reply = other.round_trip("other", "status", json!({}));
            assert_eq!(reply["ok"], true);

            coordinator
                .shutdown_handle()
                .request_shutdown()
                .expect("shutdown should be accepted");
            assert!(idle.read_to_eof().is_empty());
            let report = coordinator.wait().expect("coordinator should not panic");
            assert_eq!(report.workers_completed, 2);
        });
    }

    #[test]
    fn many_short_connections_are_all_accounted_for() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, path) = start_fake(&directory);

            for sequence in 0..5 {
                let mut client = Client::connect(&path);
                let id = format!("status-{sequence}");
                assert_eq!(
                    client.round_trip(&id, "status", json!({}))["id"],
                    id.as_str()
                );
            }
            coordinator
                .shutdown_handle()
                .request_shutdown()
                .expect("shutdown should be accepted");
            let report = coordinator.wait().expect("coordinator should not panic");

            // Workers reaped during accepts and workers joined at shutdown are
            // counted together.
            assert_eq!(report.workers_completed, 5);
            assert_eq!(report.workers_failed, 0);
            assert_eq!(report.workers_panicked, 0);
            assert_eq!(report.connections_refused, 0);
        });
    }

    #[test]
    fn second_shutdown_request_after_stopping_reports_already_stopped() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, _path) = start_fake(&directory);
            let handle = coordinator.shutdown_handle();

            assert_eq!(handle.request_shutdown(), Ok(()));
            coordinator.wait().expect("coordinator should not panic");

            assert_eq!(
                handle.request_shutdown(),
                Err(ShutdownError::AlreadyStopped)
            );
        });
    }

    #[test]
    fn authority_failure_is_detected_without_a_new_client_and_stops_admissions() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (coordinator, path) = start_with(
                &directory,
                WindowManagementService::new(WindowManager::new(
                    PanickingBackend,
                    WorkspaceMode::Tiling,
                )),
            );
            let mut idle = Client::connect(&path);
            idle.round_trip("idle", "status", json!({}));
            let mut trigger = Client::connect(&path);

            // The authority panics while handling this admitted request.
            trigger.send(&lined(&request("boom", "list_windows", json!({}))));
            assert!(trigger.read_to_eof().is_empty());

            // No further client connects: the coordinator notices by itself.
            let report = coordinator.wait().expect("coordinator should not panic");

            assert_eq!(report.reason, StopReason::AuthorityFailed);
            assert_eq!(report.authority, Err(AuthorityLifecycleError::Panicked));
            assert_eq!(report.listener_cleanup, Ok(()));
            assert_eq!(report.workers_completed, 1);
            assert_eq!(report.workers_failed, 1);
            // The idle connection was closed by the shutdown sequence.
            assert!(idle.read_to_eof().is_empty());
            // The socket is gone, so no new connection is admitted.
            assert!(!path.exists());
            assert!(UnixStream::connect(&path).is_err());
        });
    }

    #[test]
    fn wake_does_not_connect_when_the_receiver_is_gone() {
        guarded(|| {
            let directory = TestDirectory::new();
            let socket_path = directory.path().join("wake.sock");
            let listener = UnixListener::bind(&socket_path).expect("test listener should bind");
            listener
                .set_nonblocking(true)
                .expect("test listener should become non-blocking");
            let (events, receiver) = mpsc::channel();
            drop(receiver);
            let waker = Waker {
                socket_path,
                events,
            };

            assert_eq!(
                waker.wake(LifecycleEvent::Shutdown),
                Err(ShutdownError::AlreadyStopped)
            );

            // A connect would already be queued, so accept would succeed.
            let pending = listener
                .accept()
                .expect_err("no connection should be pending");
            assert_eq!(pending.kind(), io::ErrorKind::WouldBlock);
        });
    }

    #[test]
    fn wake_connects_after_delivering_the_event() {
        guarded(|| {
            let directory = TestDirectory::new();
            let socket_path = directory.path().join("wake.sock");
            let listener = UnixListener::bind(&socket_path).expect("test listener should bind");
            let (events, receiver) = mpsc::channel();
            let waker = Waker {
                socket_path,
                events,
            };

            assert_eq!(waker.wake(LifecycleEvent::AuthorityExited), Ok(()));

            listener
                .accept()
                .expect("the wake connection should arrive");
            assert_eq!(receiver.try_recv(), Ok(LifecycleEvent::AuthorityExited));
        });
    }

    #[test]
    fn wake_reports_a_failed_connect_but_keeps_the_queued_event() {
        guarded(|| {
            let directory = TestDirectory::new();
            let (events, receiver) = mpsc::channel();
            let waker = Waker {
                socket_path: directory.path().join("missing.sock"),
                events,
            };

            assert_eq!(
                waker.wake(LifecycleEvent::Shutdown),
                Err(ShutdownError::WakeFailed)
            );
            assert_eq!(receiver.try_recv(), Ok(LifecycleEvent::Shutdown));
        });
    }
}
