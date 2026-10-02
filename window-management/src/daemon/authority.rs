//! The single service authority thread and its worker-facing client.
//!
//! One thread exclusively owns the `WindowManagementService` and therefore the
//! `WindowManager`, the external identity registry, and the backend. Workers
//! hold only an [`AuthorityClient`], which can submit complete request bytes
//! and receive one serialized response line. Requests are admitted through a
//! zero-capacity rendezvous channel and processed one at a time.

use std::fmt;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use crate::backend::WindowBackend;
use crate::service::WindowManagementService;

/// One request submitted to the authority: the complete request bytes, not
/// interpreted by the submitter, and a sender dedicated to this call's response.
pub(crate) struct ServiceCall {
    request: Vec<u8>,
    response: mpsc::Sender<String>,
}

/// A failure of the authority as observed by a connection worker.
///
/// These are internal daemon/runtime failures. They are never converted into
/// protocol responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityError {
    /// The central request channel is closed. The request was not admitted.
    Unavailable,
    /// The request was admitted, but the authority ended without responding.
    /// The request may have taken effect.
    ResponseLost,
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unavailable => "window management authority is unavailable",
            Self::ResponseLost => "window management authority ended without a response",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for AuthorityError {}

/// A failure in the authority thread's lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityLifecycleError {
    /// The authority thread could not be created.
    SpawnFailed,
    /// The authority thread ended by panicking.
    Panicked,
}

impl fmt::Display for AuthorityLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::SpawnFailed => "window management authority thread could not be created",
            Self::Panicked => "window management authority thread panicked",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for AuthorityLifecycleError {}

/// The worker-facing side of the authority.
///
/// It can only submit request bytes. It gives no access to the service, the
/// manager, or the backend, and it is cheap to clone for each worker.
#[derive(Clone)]
pub struct AuthorityClient {
    calls: mpsc::SyncSender<ServiceCall>,
}

impl AuthorityClient {
    /// Submits one complete request line, without its LF, and waits for the
    /// authority's response.
    ///
    /// Blocks until the authority admits the call, then until it hands back the
    /// response line. The response is already terminated by LF, so this method
    /// can serve directly as a transport handler.
    ///
    /// `AuthorityError::Unavailable` means the request was not admitted.
    /// `AuthorityError::ResponseLost` means it was admitted but no response
    /// arrived, so a mutating request may have taken effect.
    pub fn submit(&self, request: &[u8]) -> Result<String, AuthorityError> {
        let (response, receiver) = mpsc::channel();
        self.calls
            .send(ServiceCall {
                request: request.to_vec(),
                response,
            })
            .map_err(|_| AuthorityError::Unavailable)?;
        receiver.recv().map_err(|_| AuthorityError::ResponseLost)
    }
}

/// Owns the authority thread and the means to observe and join it.
pub struct Authority {
    client: AuthorityClient,
    thread: JoinHandle<()>,
}

impl Authority {
    /// Moves the service, and with it the backend, into a new authority thread.
    ///
    /// This is a boundary that moves the backend between threads, so it requires
    /// the backend to be `Send + 'static`. [`Authority::spawn_with_exit_hook`]
    /// and the coordinator are the other such boundaries.
    pub fn spawn<B>(service: WindowManagementService<B>) -> Result<Self, AuthorityLifecycleError>
    where
        B: WindowBackend + Send + 'static,
    {
        Self::spawn_inner(service, None)
    }

    /// Like [`Authority::spawn`], and also runs `hook` once on the authority
    /// thread when it ends, whether it ended orderly or by panicking.
    ///
    /// The hook runs after the service and the call receiver have been dropped.
    /// It runs from a drop guard, possibly while a panic is unwinding, so it must
    /// not panic: it must ignore every error and must not use `unwrap` or
    /// `expect`. A panic in a hook that runs during unwinding would abort the
    /// process, so the guard also contains a panicking hook. It must not block
    /// indefinitely either, because [`Authority::shutdown`] joins this thread.
    ///
    /// Like `spawn`, this is a boundary that moves the backend between threads,
    /// so it requires `B: Send + 'static`.
    pub fn spawn_with_exit_hook<B, H>(
        service: WindowManagementService<B>,
        hook: H,
    ) -> Result<Self, AuthorityLifecycleError>
    where
        B: WindowBackend + Send + 'static,
        H: FnOnce() + Send + 'static,
    {
        Self::spawn_inner(service, Some(Box::new(hook)))
    }

    fn spawn_inner<B>(
        service: WindowManagementService<B>,
        hook: Option<ExitHook>,
    ) -> Result<Self, AuthorityLifecycleError>
    where
        B: WindowBackend + Send + 'static,
    {
        let (calls, receiver) = mpsc::sync_channel(0);
        let thread = thread::Builder::new()
            .name("nycti-windowd-authority".to_owned())
            .spawn(move || {
                let _exit = ExitGuard(hook);
                run_authority(service, receiver);
            })
            .map_err(|_| AuthorityLifecycleError::SpawnFailed)?;

        Ok(Self {
            client: AuthorityClient { calls },
            thread,
        })
    }

    /// Returns a new client for one connection worker.
    pub fn client(&self) -> AuthorityClient {
        self.client.clone()
    }

    /// Returns whether the authority thread has ended, without blocking.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Drops this owner's client and joins the authority thread.
    ///
    /// The authority ends only when every `AuthorityClient` has been dropped,
    /// so this returns only after all clones handed out by [`Authority::client`]
    /// are gone. A worker blocked on an idle client holds a clone and keeps the
    /// authority alive, and therefore blocks this call, until that worker
    /// terminates. Whoever calls this must first end the workers, for example
    /// by closing their connection streams.
    ///
    /// Returns `Panicked` if the authority thread ended by panicking.
    pub fn shutdown(self) -> Result<(), AuthorityLifecycleError> {
        let Self { client, thread } = self;
        drop(client);
        thread.join().map_err(|_| AuthorityLifecycleError::Panicked)
    }
}

type ExitHook = Box<dyn FnOnce() + Send + 'static>;

/// Runs the exit hook when the authority thread ends, including by panic.
struct ExitGuard(Option<ExitHook>);

impl Drop for ExitGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            // A panic escaping a drop that runs during unwinding aborts the
            // process, so a misbehaving hook is contained here.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(hook));
        }
    }
}

/// Processes calls one at a time until every sender has been dropped.
fn run_authority<B: WindowBackend>(
    mut service: WindowManagementService<B>,
    calls: mpsc::Receiver<ServiceCall>,
) {
    while let Ok(call) = calls.recv() {
        let response = service.handle_json_bytes(&call.request);
        // A departed worker may have dropped its receiver. The operation has
        // already happened, so discard the response and keep serving.
        let _ = call.response.send(response);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufRead, BufReader, Cursor, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::thread;

    use serde_json::json;

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::backend::hyprland::HyprlandBackend;
    use crate::core::{WindowManager, WindowPlacement, WorkspaceMode};
    use crate::daemon::test_support::{
        GUARD, GateEvent, GatedBackend, PanickingBackend, call, fake_with_workspace_and_window,
        guarded, lined, request, response, service_with, workspace_tokens,
    };
    use crate::runtime::{UnixServeError, serve_unix_connection_with_handler};
    use crate::transport::{ServeError, TransportError, serve_connection_with_handler};

    #[test]
    fn real_and_fake_backends_can_move_to_the_authority_thread() {
        fn requires_send<B: WindowBackend + Send + 'static>() {}

        requires_send::<FakeBackend>();
        requires_send::<HyprlandBackend>();
    }

    #[test]
    fn two_clients_share_default_mode_through_one_authority() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let client_a = authority.client();
            let client_b = authority.client();

            let before = call(&client_b, "before", "get_default_mode", json!({}));
            let set = call(
                &client_a,
                "set",
                "set_default_mode",
                json!({"mode": "windows"}),
            );
            let after = call(&client_b, "after", "get_default_mode", json!({}));

            assert_eq!(before["result"]["mode"], "tiling");
            assert_eq!(set["result"]["mode"], "windows");
            assert_eq!(after["result"]["mode"], "windows");

            drop((client_a, client_b));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn explicit_workspace_mode_set_by_one_client_is_observed_by_another() {
        guarded(|| {
            let authority = Authority::spawn(service_with(fake_with_workspace_and_window()))
                .expect("authority should start");
            let client_a = authority.client();
            let client_b = authority.client();
            let token = workspace_tokens(&client_a).remove(0);

            let set = call(
                &client_a,
                "set",
                "set_workspace_mode",
                json!({"workspace_id": token, "mode": "windows"}),
            );
            let observed = call(
                &client_b,
                "get",
                "get_workspace_mode",
                json!({"workspace_id": token}),
            );

            assert_eq!(set["result"]["explicit_mode"], "windows");
            assert_eq!(observed["result"]["explicit_mode"], "windows");
            assert_eq!(observed["result"]["effective_mode"], "windows");

            drop((client_a, client_b));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn external_workspace_and_window_tokens_are_the_same_for_every_client() {
        guarded(|| {
            let authority = Authority::spawn(service_with(fake_with_workspace_and_window()))
                .expect("authority should start");
            let client_a = authority.client();
            let client_b = authority.client();

            let workspaces_a = workspace_tokens(&client_a);
            let workspaces_b = workspace_tokens(&client_b);
            let windows_a = call(&client_a, "windows-a", "list_windows", json!({}));
            let windows_b = call(&client_b, "windows-b", "list_windows", json!({}));

            assert_eq!(workspaces_a.len(), 1);
            assert_eq!(workspaces_a, workspaces_b);
            assert_eq!(
                windows_a["result"]["windows"][0]["window_id"],
                windows_b["result"]["windows"][0]["window_id"]
            );
            assert_eq!(
                windows_a["result"]["windows"][0]["workspace_id"],
                workspaces_a[0]
            );

            drop((client_a, client_b));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn invalid_json_and_protocol_errors_do_not_affect_other_clients() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let client_a = authority.client();
            let client_b = authority.client();

            let invalid_json = response(
                &client_a
                    .submit(b"bad-json")
                    .expect("invalid JSON is a normal protocol response"),
            );
            let invalid_params = call(
                &client_a,
                "invalid",
                "set_default_mode",
                json!({"mode": "floating"}),
            );
            let other = call(&client_b, "other", "status", json!({}));

            assert_eq!(invalid_json["error"]["code"], "invalid_request");
            assert_eq!(invalid_params["error"]["code"], "invalid_params");
            assert_eq!(other["ok"], true);

            drop((client_a, client_b));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn requests_on_one_connection_keep_their_order() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let client = authority.client();
            let mut input = Vec::new();
            for id in ["first", "second", "third"] {
                input.extend(lined(&request(id, "status", json!({}))));
            }
            let mut output = Vec::new();

            serve_connection_with_handler(Cursor::new(input), &mut output, |line| {
                client.submit(line)
            })
            .expect("connection should be served");

            let ids = String::from_utf8(output)
                .expect("responses should be UTF-8")
                .lines()
                .map(|line| response(&format!("{line}\n"))["id"].clone())
                .collect::<Vec<_>>();
            assert_eq!(ids, vec![json!("first"), json!("second"), json!("third")]);

            drop(client);
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn each_response_returns_to_the_client_that_submitted_it() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let workers = ["a", "b", "c"]
                .into_iter()
                .map(|name| {
                    let client = authority.client();
                    thread::spawn(move || {
                        for sequence in 0..50 {
                            let id = format!("{name}-{sequence}");
                            let reply = call(&client, &id, "status", json!({}));
                            assert_eq!(reply["id"], id.as_str());
                        }
                    })
                })
                .collect::<Vec<_>>();

            for worker in workers {
                worker
                    .join()
                    .expect("worker should receive its own responses");
            }
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn concurrent_mutations_are_processed_one_at_a_time() {
        guarded(|| {
            let mut fake = FakeBackend::new();
            for _ in 0..2 {
                let workspace = fake.add_workspace(true);
                fake.add_window(workspace, WindowPlacement::Tiled, false, false)
                    .expect("known workspace should accept window");
            }
            let (events, observed) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let backend = GatedBackend {
                inner: fake,
                events,
                release: gate,
            };
            let authority = Authority::spawn(WindowManagementService::new(WindowManager::new(
                backend,
                WorkspaceMode::Windows,
            )))
            .expect("authority should start");
            let tokens = workspace_tokens(&authority.client());
            assert_eq!(tokens.len(), 2);

            let workers = tokens
                .into_iter()
                .map(|token| {
                    let client = authority.client();
                    thread::spawn(move || {
                        call(
                            &client,
                            "apply",
                            "apply_workspace_mode",
                            json!({"workspace_id": token}),
                        )
                    })
                })
                .collect::<Vec<_>>();

            // Each admitted call must fully enter and exit before the next
            // one enters; any overlap would break this exact sequence.
            for _ in 0..2 {
                assert_eq!(observed.recv_timeout(GUARD), Ok(GateEvent::Enter));
                release.send(()).expect("backend should await release");
                assert_eq!(observed.recv_timeout(GUARD), Ok(GateEvent::Exit));
            }
            for worker in workers {
                let reply = worker.join().expect("worker should finish");
                assert_eq!(reply["ok"], true);
            }

            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn side_effect_of_an_admitted_call_survives_a_departed_requester() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let departed = authority.client();
            let observer = authority.client();

            // Admit a mutation whose requester has already gone away.
            let (response_sender, response_receiver) = mpsc::channel();
            drop(response_receiver);
            departed
                .calls
                .send(ServiceCall {
                    request: request("departed", "set_default_mode", json!({"mode": "windows"})),
                    response: response_sender,
                })
                .expect("authority should admit the call");

            // The authority discards the undeliverable response and keeps serving,
            // and the mutation it admitted is visible to others.
            let observed = call(&observer, "observe", "get_default_mode", json!({}));
            assert_eq!(observed["result"]["mode"], "windows");
            assert!(!authority.is_finished());

            drop((departed, observer));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn idle_connection_does_not_block_other_clients() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let client_a = authority.client();
            let (server, mut peer) = UnixStream::pair().expect("stream pair should be created");
            let (done, finished) = mpsc::channel();
            let worker = thread::spawn(move || {
                let result =
                    serve_unix_connection_with_handler(server, |line| client_a.submit(line));
                let _ = done.send(result);
            });

            // Serve one request on A, leaving A's connection open and idle.
            peer.write_all(&lined(&request("a", "status", json!({}))))
                .expect("request should be written");
            let mut reader = BufReader::new(peer.try_clone().expect("peer should clone"));
            let mut reply = String::new();
            reader.read_line(&mut reply).expect("reply should be read");
            assert_eq!(response(&reply)["id"], "a");

            let other = call(&authority.client(), "b", "status", json!({}));
            assert_eq!(other["ok"], true);

            // Closing A's connection ends only A's worker.
            drop((reader, peer));
            assert_eq!(finished.recv_timeout(GUARD), Ok(Ok(())));
            worker.join().expect("worker should finish");
            authority.shutdown().expect("authority should shut down");
        });
    }

    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("intentional read failure"))
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("intentional write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn connection_failures_do_not_terminate_the_authority_or_other_clients() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let client_a = authority.client();
            let client_b = authority.client();

            let (server, peer) = UnixStream::pair().expect("stream pair should be created");
            drop(peer);
            let eof = serve_unix_connection_with_handler(server, |line| client_a.submit(line));
            let read_failure =
                serve_connection_with_handler(BufReader::new(FailingReader), Vec::new(), |line| {
                    client_a.submit(line)
                });
            // The authority handles this request; only writing its response fails.
            let write_failure = serve_connection_with_handler(
                Cursor::new(lined(&request(
                    "gone",
                    "set_default_mode",
                    json!({"mode": "windows"}),
                ))),
                FailingWriter,
                |line| client_a.submit(line),
            );

            assert_eq!(eof, Ok(()));
            assert_eq!(
                read_failure,
                Err(ServeError::Transport(TransportError::ReadFailed))
            );
            assert_eq!(
                write_failure,
                Err(ServeError::Transport(TransportError::WriteFailed))
            );

            let other = call(&client_b, "other", "get_default_mode", json!({}));
            assert_eq!(other["result"]["mode"], "windows");
            assert!(!authority.is_finished());

            drop((client_a, client_b));
            authority.shutdown().expect("authority should shut down");
        });
    }

    #[test]
    fn closed_authority_channel_is_unavailable_and_writes_no_protocol_response() {
        guarded(|| {
            let (calls, receiver) = mpsc::sync_channel(0);
            drop(receiver);
            let client = AuthorityClient { calls };
            let mut output = Vec::new();

            let direct = client.submit(&request("closed", "status", json!({})));
            let served = serve_connection_with_handler(
                Cursor::new(lined(&request("closed", "status", json!({})))),
                &mut output,
                |line| client.submit(line),
            );
            let unix = {
                let (server, mut peer) = UnixStream::pair().expect("stream pair should be created");
                peer.write_all(&lined(&request("closed", "status", json!({}))))
                    .expect("request should be written");
                peer.shutdown(std::net::Shutdown::Write)
                    .expect("write half should close");
                serve_unix_connection_with_handler(server, |line| client.submit(line))
            };

            assert_eq!(direct, Err(AuthorityError::Unavailable));
            assert_eq!(
                served,
                Err(ServeError::Handler(AuthorityError::Unavailable))
            );
            assert!(output.is_empty());
            assert_eq!(
                unix,
                Err(UnixServeError::Handler(AuthorityError::Unavailable))
            );
        });
    }

    #[test]
    fn authority_panic_is_fatal_and_never_replaced() {
        guarded(|| {
            let authority = Authority::spawn(WindowManagementService::new(WindowManager::new(
                PanickingBackend,
                WorkspaceMode::Tiling,
            )))
            .expect("authority should start");
            let client = authority.client();

            // The admitted call gets no response because the authority panicked.
            let lost = client.submit(&request("boom", "list_windows", json!({})));
            // No replacement authority or service exists to admit later calls.
            let after = client.submit(&request("after", "status", json!({})));
            let mut output = Vec::new();
            let served = serve_connection_with_handler(
                Cursor::new(lined(&request("served", "status", json!({})))),
                &mut output,
                |line| client.submit(line),
            );

            assert_eq!(lost, Err(AuthorityError::ResponseLost));
            assert_eq!(after, Err(AuthorityError::Unavailable));
            assert_eq!(
                served,
                Err(ServeError::Handler(AuthorityError::Unavailable))
            );
            assert!(output.is_empty());

            drop(client);
            assert_eq!(authority.shutdown(), Err(AuthorityLifecycleError::Panicked));
        });
    }

    #[test]
    fn running_authority_is_not_finished_and_shuts_down_after_clients_drop() {
        guarded(|| {
            let authority =
                Authority::spawn(service_with(FakeBackend::new())).expect("authority should start");
            let client = authority.client();
            assert!(!authority.is_finished());
            assert_eq!(call(&client, "status", "status", json!({}))["ok"], true);

            drop(client);

            assert_eq!(authority.shutdown(), Ok(()));
        });
    }

    #[test]
    fn exit_hook_runs_exactly_once_after_an_orderly_shutdown() {
        guarded(|| {
            let (ran, observed) = mpsc::channel();
            let authority =
                Authority::spawn_with_exit_hook(service_with(FakeBackend::new()), move || {
                    let _ = ran.send(());
                })
                .expect("authority should start");
            let client = authority.client();
            assert_eq!(call(&client, "status", "status", json!({}))["ok"], true);
            assert!(observed.try_recv().is_err());

            drop(client);
            assert_eq!(authority.shutdown(), Ok(()));

            assert_eq!(observed.recv_timeout(GUARD), Ok(()));
            // The hook consumed its sender, so a second run is impossible.
            assert!(observed.recv_timeout(GUARD).is_err());
        });
    }

    #[test]
    fn exit_hook_runs_when_the_authority_panics() {
        guarded(|| {
            let (ran, observed) = mpsc::channel();
            let authority = Authority::spawn_with_exit_hook(
                WindowManagementService::new(WindowManager::new(
                    PanickingBackend,
                    WorkspaceMode::Tiling,
                )),
                move || {
                    let _ = ran.send(());
                },
            )
            .expect("authority should start");
            let client = authority.client();

            let lost = client.submit(&request("boom", "list_windows", json!({})));

            assert_eq!(lost, Err(AuthorityError::ResponseLost));
            assert_eq!(observed.recv_timeout(GUARD), Ok(()));
            drop(client);
            assert_eq!(authority.shutdown(), Err(AuthorityLifecycleError::Panicked));
        });
    }

    #[test]
    fn panicking_exit_hook_is_contained_instead_of_failing_the_authority() {
        guarded(|| {
            let authority =
                Authority::spawn_with_exit_hook(service_with(FakeBackend::new()), || {
                    panic!("intentional exit hook failure in test")
                })
                .expect("authority should start");

            assert_eq!(authority.shutdown(), Ok(()));
        });
    }
}
