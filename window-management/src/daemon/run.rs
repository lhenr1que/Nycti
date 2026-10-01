//! Process-level entry logic of the daemon: startup, the coordinator, a shutdown
//! source, exit codes, and diagnostics on standard error.
//!
//! The binary stays thin and only provides the real shutdown source. Everything
//! else lives here so it can be tested with an injected source and a fake
//! backend. See ADR 0008. Exit codes and message text are provisional and
//! promise no stability.

use std::io::Write;
use std::thread;

use super::{Coordinator, CoordinatorReport, ShutdownError, StopReason};
use crate::backend::WindowBackend;
use crate::backend::hyprland::HyprlandBackend;
use crate::core::{WindowManager, WorkspaceMode};
use crate::runtime::{UnixRuntimeError, UnixRuntimeListener};
use crate::service::WindowManagementService;

/// Shutdown requested and every shutdown step ended cleanly.
pub const EXIT_CLEAN: u8 = 0;
/// A startup step failed.
pub const EXIT_STARTUP_FAILED: u8 = 1;
/// Another daemon is already running.
pub const EXIT_ALREADY_RUNNING: u8 = 2;
/// The authority ended while the daemon was running.
pub const EXIT_AUTHORITY_FAILED: u8 = 3;
/// Accepting a connection failed.
pub const EXIT_ACCEPT_FAILED: u8 = 4;
/// The coordinator thread panicked.
pub const EXIT_PANICKED: u8 = 5;
/// Shutdown was requested, but a shutdown step failed.
pub const EXIT_UNCLEAN_SHUTDOWN: u8 = 6;
/// A second signal forced an immediate exit.
pub const EXIT_FORCED: u8 = 7;

/// Where shutdown requests come from, such as process signals.
///
/// The real source lives in the binary. Tests inject their own, so no real
/// signal handler is ever installed in a test process.
pub trait ShutdownSource: Send + 'static {
    /// Blocks until the next shutdown request. Returns `None` when the source
    /// ended or was closed.
    fn wait(&mut self) -> Option<()>;

    /// Returns a function that makes a blocked [`ShutdownSource::wait`] return
    /// `None`, so the thread waiting on this source can be joined.
    fn closer(&self) -> Box<dyn FnOnce() + Send + 'static>;
}

/// Reads shutdown requests and acts on them. This is the body of the signal
/// thread.
///
/// The first request asks for an orderly shutdown through `request`. A second
/// request forces an immediate exit through `force_exit`, because an orderly
/// shutdown can stall behind a slow backend operation. Requests of the same kind
/// can be merged before they are read, so this counts the requests it reads. When
/// the source ends, it asks for a shutdown, because a daemon that can no longer
/// be stopped by a request should not keep running.
fn signal_loop<S, R, F, W>(mut source: S, mut request: R, force_exit: F, mut diag: W)
where
    S: ShutdownSource,
    R: FnMut() -> Result<(), ShutdownError>,
    F: Fn(u8),
    W: Write,
{
    let mut seen = 0_u32;
    while source.wait().is_some() {
        seen += 1;
        if seen == 1 {
            let _ = writeln!(diag, "clea-windowd: shutdown requested");
            if request() == Err(ShutdownError::WakeFailed) {
                let _ = writeln!(
                    diag,
                    "clea-windowd: warning: the accept loop could not be woken; \
                     it stops when the next client connects"
                );
            }
        } else {
            let _ = writeln!(diag, "clea-windowd: second signal, forcing exit");
            force_exit(EXIT_FORCED);
            return;
        }
    }
    // The source ended. Asking again after the coordinator stopped is harmless.
    let _ = request();
}

/// Maps a startup listener error to an exit code.
pub(crate) fn startup_exit_code(error: &UnixRuntimeError) -> u8 {
    match error {
        UnixRuntimeError::AlreadyRunning => EXIT_ALREADY_RUNNING,
        _ => EXIT_STARTUP_FAILED,
    }
}

/// Maps the coordinator's report to an exit code.
pub fn exit_code(report: &CoordinatorReport) -> u8 {
    match report.reason {
        StopReason::ShutdownRequested => {
            if report.authority.is_err()
                || report.listener_cleanup.is_err()
                || report.workers_panicked > 0
            {
                EXIT_UNCLEAN_SHUTDOWN
            } else {
                EXIT_CLEAN
            }
        }
        StopReason::AuthorityFailed => EXIT_AUTHORITY_FAILED,
        StopReason::AcceptFailed(_) => EXIT_ACCEPT_FAILED,
    }
}

/// Writes the coordinator's report as plain text. The format is not stable.
pub fn write_report(report: &CoordinatorReport, out: &mut dyn Write) {
    let reason = match report.reason {
        StopReason::ShutdownRequested => "shutdown requested".to_owned(),
        StopReason::AuthorityFailed => "authority failed".to_owned(),
        StopReason::AcceptFailed(error) => format!("accept failed ({error})"),
    };
    let authority = match report.authority {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("failed ({error})"),
    };
    let cleanup = match report.listener_cleanup {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("failed ({error})"),
    };

    let _ = writeln!(out, "clea-windowd: stopped: {reason}");
    let _ = writeln!(
        out,
        "clea-windowd: workers: completed={} failed={} panicked={} refused={}",
        report.workers_completed,
        report.workers_failed,
        report.workers_panicked,
        report.connections_refused
    );
    let _ = writeln!(out, "clea-windowd: authority: {authority}");
    let _ = writeln!(out, "clea-windowd: listener cleanup: {cleanup}");
}

/// Runs the daemon for the current user session and returns its exit code.
///
/// Performs startup steps 1 to 3 of ADR 0006: bind the listener from
/// `XDG_RUNTIME_DIR`, construct the Hyprland backend from the session
/// environment, and construct the manager and the service. The default mode is
/// fixed to `Tiling` (provisional). It then runs the coordinator through
/// [`run_with`]. A step that fails is reported on `diag` and ends the run; there
/// is no retry, and a bound listener is dropped, which removes its socket by
/// recorded device and inode.
///
/// The compositor is not probed: the backend only resolves its socket path, so a
/// stopped compositor shows up as `compositor_unavailable` on each request.
pub fn run_from_env<S, F, W>(source: S, force_exit: F, diag: &mut dyn Write, signal_diag: W) -> u8
where
    S: ShutdownSource,
    F: Fn(u8) + Send + 'static,
    W: Write + Send + 'static,
{
    let listener = match UnixRuntimeListener::bind_from_env() {
        Ok(listener) => listener,
        Err(error) => {
            let _ = writeln!(
                diag,
                "clea-windowd: error: cannot bind the service socket: {error}"
            );
            return startup_exit_code(&error);
        }
    };
    let backend = match HyprlandBackend::from_env() {
        Ok(backend) => backend,
        Err(error) => {
            let _ = writeln!(
                diag,
                "clea-windowd: error: cannot use the Hyprland session: {error} \
                 (check XDG_RUNTIME_DIR and HYPRLAND_INSTANCE_SIGNATURE)"
            );
            return EXIT_STARTUP_FAILED;
        }
    };
    let service = WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling));

    run_with(listener, service, source, force_exit, diag, signal_diag)
}

/// Runs the coordinator with an already bound listener and built service.
///
/// Starts the coordinator, prints where it listens, starts the signal thread,
/// and waits for the coordinator on the calling thread. When the coordinator
/// stops, it closes the source and joins the signal thread, prints the report,
/// and returns the exit code.
///
/// `diag` belongs to the calling thread. The signal thread writes its own
/// messages to `signal_diag`, which must be `Send`; the binary passes standard
/// error for both.
pub(crate) fn run_with<B, S, F, W>(
    listener: UnixRuntimeListener,
    service: WindowManagementService<B>,
    source: S,
    force_exit: F,
    diag: &mut dyn Write,
    signal_diag: W,
) -> u8
where
    B: WindowBackend + Send + 'static,
    S: ShutdownSource,
    F: Fn(u8) + Send + 'static,
    W: Write + Send + 'static,
{
    let socket_path = listener.socket_path().to_path_buf();
    let coordinator = match Coordinator::start(listener, service) {
        Ok(coordinator) => coordinator,
        Err(error) => {
            let _ = writeln!(
                diag,
                "clea-windowd: error: cannot start the coordinator: {error}"
            );
            return EXIT_STARTUP_FAILED;
        }
    };
    let _ = writeln!(diag, "clea-windowd: listening on {}", socket_path.display());

    let handle = coordinator.shutdown_handle();
    let fallback = handle.clone();
    let close_source = source.closer();
    let spawned = thread::Builder::new()
        .name("clea-windowd-signals".to_owned())
        .spawn(move || {
            signal_loop(
                source,
                move || handle.request_shutdown(),
                force_exit,
                signal_diag,
            );
        });
    let signal_thread = match spawned {
        Ok(thread) => thread,
        Err(_) => {
            let _ = writeln!(diag, "clea-windowd: error: cannot start the signal thread");
            let _ = fallback.request_shutdown();
            let _ = coordinator.wait();
            return EXIT_STARTUP_FAILED;
        }
    };

    let result = coordinator.wait();
    close_source();
    let _ = signal_thread.join();

    match result {
        Ok(report) => {
            write_report(&report, diag);
            exit_code(&report)
        }
        Err(error) => {
            let _ = writeln!(
                diag,
                "clea-windowd: error: {error}; the workers and the authority are left running"
            );
            EXIT_PANICKED
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::thread::JoinHandle;

    use serde_json::json;

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::daemon::AuthorityLifecycleError;
    use crate::daemon::test_support::{
        GUARD, PanickingBackend, TestDirectory, bound_listener, guarded, lined, request, response,
        service_with,
    };

    enum Message {
        Signal,
        Close,
    }

    /// A shutdown source fed by the test through a channel.
    struct ChannelSource {
        receiver: Receiver<Message>,
        closer: Sender<Message>,
    }

    impl ShutdownSource for ChannelSource {
        fn wait(&mut self) -> Option<()> {
            match self.receiver.recv() {
                Ok(Message::Signal) => Some(()),
                Ok(Message::Close) | Err(_) => None,
            }
        }

        fn closer(&self) -> Box<dyn FnOnce() + Send + 'static> {
            let closer = self.closer.clone();
            Box::new(move || {
                let _ = closer.send(Message::Close);
            })
        }
    }

    fn channel_source() -> (Sender<Message>, ChannelSource) {
        let (sender, receiver) = mpsc::channel();
        let source = ChannelSource {
            receiver,
            closer: sender.clone(),
        };
        (sender, source)
    }

    /// Forwards everything written to it, so another thread can read it.
    struct ChannelWriter(Sender<Vec<u8>>);

    impl Write for ChannelWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let _ = self.0.send(buffer.to_vec());
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Reads the text written to a [`ChannelWriter`], a line at a time.
    struct Lines {
        receiver: Receiver<Vec<u8>>,
        pending: String,
    }

    impl Lines {
        fn new(receiver: Receiver<Vec<u8>>) -> Self {
            Self {
                receiver,
                pending: String::new(),
            }
        }

        /// Waits for the next complete line, or `None` once the writer is gone.
        fn next_line(&mut self) -> Option<String> {
            loop {
                if let Some(end) = self.pending.find('\n') {
                    let line = self.pending[..end].to_owned();
                    self.pending.drain(..=end);
                    return Some(line);
                }
                match self.receiver.recv_timeout(GUARD) {
                    Ok(bytes) => self.pending.push_str(&String::from_utf8_lossy(&bytes)),
                    Err(RecvTimeoutError::Disconnected) => return None,
                    Err(RecvTimeoutError::Timeout) => panic!("no output within the guard limit"),
                }
            }
        }

        /// Waits for a line containing `text` and returns it.
        fn expect_line_containing(&mut self, text: &str) -> String {
            while let Some(line) = self.next_line() {
                if line.contains(text) {
                    return line;
                }
            }
            panic!("output ended before a line containing {text:?}");
        }

        /// Reads every remaining line, until the writer is gone.
        fn remaining(&mut self) -> Vec<String> {
            let mut lines = Vec::new();
            while let Some(line) = self.next_line() {
                lines.push(line);
            }
            lines
        }
    }

    /// A daemon running `run_with` on its own thread.
    struct Running {
        exit: JoinHandle<u8>,
        diag: Lines,
        signal_diag: Lines,
        socket_path: PathBuf,
        signal: Sender<Message>,
        _directory: TestDirectory,
    }

    fn start_daemon<B>(
        service: WindowManagementService<B>,
        force_exit: impl Fn(u8) + Send + 'static,
    ) -> Running
    where
        B: WindowBackend + Send + 'static,
    {
        let directory = TestDirectory::new();
        let listener = bound_listener(&directory);
        let socket_path = listener.socket_path().to_path_buf();
        let (diag_sender, diag_receiver) = mpsc::channel();
        let (signal_diag_sender, signal_diag_receiver) = mpsc::channel();
        let (signal, source) = channel_source();

        let exit = thread::spawn(move || {
            let mut diag = ChannelWriter(diag_sender);
            run_with(
                listener,
                service,
                source,
                force_exit,
                &mut diag,
                ChannelWriter(signal_diag_sender),
            )
        });
        let mut running = Running {
            exit,
            diag: Lines::new(diag_receiver),
            signal_diag: Lines::new(signal_diag_receiver),
            socket_path,
            signal,
            _directory: directory,
        };
        running.diag.expect_line_containing("listening on");
        running
    }

    fn status_round_trip(socket_path: &PathBuf) {
        use std::io::{BufRead, BufReader};

        let mut stream = UnixStream::connect(socket_path).expect("client should connect");
        stream
            .write_all(&lined(&request("status", "status", json!({}))))
            .expect("request should be written");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("response should be read");
        assert_eq!(response(&line)["result"]["service"], "clea-windowd");
    }

    fn report_with(
        reason: StopReason,
        authority: Result<(), AuthorityLifecycleError>,
        listener_cleanup: Result<(), UnixRuntimeError>,
        workers_panicked: usize,
    ) -> CoordinatorReport {
        CoordinatorReport {
            reason,
            workers_completed: 2,
            workers_failed: 1,
            workers_panicked,
            connections_refused: 3,
            authority,
            listener_cleanup,
        }
    }

    #[test]
    fn a_shutdown_request_stops_the_daemon_cleanly_and_removes_the_socket() {
        guarded(|| {
            let mut running = start_daemon(service_with(FakeBackend::new()), |_| {});
            status_round_trip(&running.socket_path);

            running
                .signal
                .send(Message::Signal)
                .expect("the source should be listening");
            running
                .signal_diag
                .expect_line_containing("shutdown requested");
            let code = running
                .exit
                .join()
                .expect("the daemon thread should not panic");

            assert_eq!(code, EXIT_CLEAN);
            let report = running.diag.remaining();
            assert!(
                report
                    .iter()
                    .any(|line| line.contains("stopped: shutdown requested"))
            );
            assert!(
                report
                    .iter()
                    .any(|line| line.contains("workers: completed=1"))
            );
            assert!(!running.socket_path.exists());
        });
    }

    #[test]
    fn a_source_that_ends_requests_a_shutdown() {
        guarded(|| {
            let running = start_daemon(service_with(FakeBackend::new()), |_| {});

            running
                .signal
                .send(Message::Close)
                .expect("the source should be listening");
            let code = running
                .exit
                .join()
                .expect("the daemon thread should not panic");

            assert_eq!(code, EXIT_CLEAN);
            assert!(!running.socket_path.exists());
        });
    }

    #[test]
    fn authority_failure_exits_with_code_three_and_joins_the_signal_thread() {
        guarded(|| {
            let mut running = start_daemon(
                WindowManagementService::new(WindowManager::new(
                    PanickingBackend,
                    WorkspaceMode::Tiling,
                )),
                |_| {},
            );
            let mut stream =
                UnixStream::connect(&running.socket_path).expect("client should connect");
            stream
                .write_all(&lined(&request("boom", "list_windows", json!({}))))
                .expect("request should be written");

            // The source is never fired: only the authority failure stops the
            // daemon, and run_with must still close and join the signal thread.
            let code = running
                .exit
                .join()
                .expect("the daemon thread should not panic");

            assert_eq!(code, EXIT_AUTHORITY_FAILED);
            let report = running.diag.remaining();
            assert!(
                report
                    .iter()
                    .any(|line| line.contains("stopped: authority failed"))
            );
            assert!(report.iter().any(|line| line.contains("authority: failed")));
            assert!(!running.socket_path.exists());
            // The signal thread was joined, so it dropped its writer.
            assert!(running.signal_diag.remaining().is_empty());
        });
    }

    #[test]
    fn first_request_asks_for_shutdown_and_second_forces_exit() {
        guarded(|| {
            let (signal, source) = channel_source();
            let (requests, requested) = mpsc::channel();
            let (forced, forced_with) = mpsc::channel();
            let (diag_sender, diag_receiver) = mpsc::channel();
            for _ in 0..2 {
                signal
                    .send(Message::Signal)
                    .expect("source should accept requests");
            }

            signal_loop(
                source,
                move || {
                    let _ = requests.send(());
                    Ok(())
                },
                move |code| {
                    let _ = forced.send(code);
                },
                ChannelWriter(diag_sender),
            );

            assert_eq!(requested.try_iter().count(), 1);
            assert_eq!(forced_with.try_recv(), Ok(EXIT_FORCED));
            let lines = Lines::new(diag_receiver).remaining();
            assert_eq!(
                lines,
                vec![
                    "clea-windowd: shutdown requested".to_owned(),
                    "clea-windowd: second signal, forcing exit".to_owned(),
                ]
            );
        });
    }

    #[test]
    fn a_failed_wake_warns_and_an_already_stopped_request_stays_quiet() {
        guarded(|| {
            let (signal, source) = channel_source();
            let (diag_sender, diag_receiver) = mpsc::channel();
            signal
                .send(Message::Signal)
                .expect("source should accept requests");
            signal
                .send(Message::Close)
                .expect("source should accept requests");
            let mut answers = vec![
                Err(ShutdownError::WakeFailed),
                Err(ShutdownError::AlreadyStopped),
            ]
            .into_iter();

            signal_loop(
                source,
                move || answers.next().unwrap_or(Ok(())),
                |_| {},
                ChannelWriter(diag_sender),
            );

            let lines = Lines::new(diag_receiver).remaining();
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0], "clea-windowd: shutdown requested");
            assert!(lines[1].contains("warning: the accept loop could not be woken"));
        });
    }

    #[test]
    fn exit_codes_follow_the_stop_reason() {
        let clean = report_with(StopReason::ShutdownRequested, Ok(()), Ok(()), 0);
        let authority_step_failed = report_with(
            StopReason::ShutdownRequested,
            Err(AuthorityLifecycleError::Panicked),
            Ok(()),
            0,
        );
        let cleanup_failed = report_with(
            StopReason::ShutdownRequested,
            Ok(()),
            Err(UnixRuntimeError::SocketCleanupFailed),
            0,
        );
        let worker_panicked = report_with(StopReason::ShutdownRequested, Ok(()), Ok(()), 1);
        let authority_failed = report_with(StopReason::AuthorityFailed, Ok(()), Ok(()), 0);
        let accept_failed = report_with(
            StopReason::AcceptFailed(UnixRuntimeError::AcceptFailed),
            Ok(()),
            Ok(()),
            0,
        );

        assert_eq!(exit_code(&clean), EXIT_CLEAN);
        assert_eq!(exit_code(&authority_step_failed), EXIT_UNCLEAN_SHUTDOWN);
        assert_eq!(exit_code(&cleanup_failed), EXIT_UNCLEAN_SHUTDOWN);
        assert_eq!(exit_code(&worker_panicked), EXIT_UNCLEAN_SHUTDOWN);
        assert_eq!(exit_code(&authority_failed), EXIT_AUTHORITY_FAILED);
        assert_eq!(exit_code(&accept_failed), EXIT_ACCEPT_FAILED);
    }

    #[test]
    fn exit_codes_are_distinct() {
        let codes = [
            EXIT_CLEAN,
            EXIT_STARTUP_FAILED,
            EXIT_ALREADY_RUNNING,
            EXIT_AUTHORITY_FAILED,
            EXIT_ACCEPT_FAILED,
            EXIT_PANICKED,
            EXIT_UNCLEAN_SHUTDOWN,
            EXIT_FORCED,
        ];

        for (index, code) in codes.iter().enumerate() {
            assert!(!codes[..index].contains(code));
        }
    }

    #[test]
    fn the_report_names_the_reason_the_counts_and_each_step() {
        let mut shutdown = Vec::new();
        write_report(
            &report_with(StopReason::ShutdownRequested, Ok(()), Ok(()), 0),
            &mut shutdown,
        );
        let mut failure = Vec::new();
        write_report(
            &report_with(
                StopReason::AcceptFailed(UnixRuntimeError::AcceptFailed),
                Err(AuthorityLifecycleError::Panicked),
                Err(UnixRuntimeError::SocketCleanupFailed),
                1,
            ),
            &mut failure,
        );

        let shutdown = String::from_utf8(shutdown).expect("report should be text");
        let failure = String::from_utf8(failure).expect("report should be text");
        assert!(shutdown.contains("stopped: shutdown requested"));
        assert!(shutdown.contains("completed=2 failed=1 panicked=0 refused=3"));
        assert!(shutdown.contains("authority: ok"));
        assert!(shutdown.contains("listener cleanup: ok"));
        assert!(failure.contains("stopped: accept failed"));
        assert!(failure.contains("panicked=1"));
        assert!(failure.contains("authority: failed"));
        assert!(failure.contains("listener cleanup: failed"));
    }

    #[test]
    fn only_an_active_daemon_has_its_own_startup_exit_code() {
        assert_eq!(
            startup_exit_code(&UnixRuntimeError::AlreadyRunning),
            EXIT_ALREADY_RUNNING
        );
        for error in [
            UnixRuntimeError::RuntimeDirUnavailable,
            UnixRuntimeError::InvalidRuntimeDir,
            UnixRuntimeError::UnsafeSocketPath,
            UnixRuntimeError::StaleSocketCleanupFailed,
            UnixRuntimeError::BindFailed,
        ] {
            assert_eq!(startup_exit_code(&error), EXIT_STARTUP_FAILED);
        }
    }
}
