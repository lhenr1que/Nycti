//! Helpers shared by the authority and worker tests.

use std::env;
use std::fs::{self, DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::AuthorityClient;
use crate::backend::fake::FakeBackend;
use crate::backend::{
    BackendError, WindowBackend, WindowId, WindowObservation, WorkspaceObservation,
};
use crate::core::{WindowManager, WindowPlacement, WorkspaceMode};
use crate::runtime::UnixRuntimeListener;
use crate::service::WindowManagementService;

/// Protection against a hung test only. It is never used to synchronize.
pub(crate) const GUARD: Duration = Duration::from_secs(30);

/// Runs a test body on its own thread so a hang fails instead of blocking
/// `cargo test`. A panic in the body is propagated unchanged.
pub(crate) fn guarded<F: FnOnce() + Send + 'static>(body: F) {
    let (done, finished) = mpsc::channel();
    let handle = thread::spawn(move || {
        body();
        let _ = done.send(());
    });

    match finished.recv_timeout(GUARD) {
        Ok(()) => handle.join().expect("guarded test body should not panic"),
        Err(RecvTimeoutError::Timeout) => panic!("test did not finish within the guard limit"),
        Err(RecvTimeoutError::Disconnected) => match handle.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("test body ended without reporting completion"),
        },
    }
}

pub(crate) fn service_with(backend: FakeBackend) -> WindowManagementService<FakeBackend> {
    WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling))
}

pub(crate) fn request(id: &str, method: &str, params: Value) -> Vec<u8> {
    json!({"version": 1, "id": id, "method": method, "params": params})
        .to_string()
        .into_bytes()
}

pub(crate) fn lined(request: &[u8]) -> Vec<u8> {
    let mut line = request.to_vec();
    line.push(b'\n');
    line
}

pub(crate) fn response(line: &str) -> Value {
    assert!(
        line.ends_with('\n'),
        "response must be one LF-terminated line"
    );
    serde_json::from_str(line).expect("response should be JSON")
}

pub(crate) fn call(client: &AuthorityClient, id: &str, method: &str, params: Value) -> Value {
    let line = client
        .submit(&request(id, method, params))
        .expect("authority should respond");
    response(&line)
}

pub(crate) fn fake_with_workspace_and_window() -> FakeBackend {
    let mut backend = FakeBackend::new();
    let workspace = backend.add_workspace(true);
    backend
        .add_window(workspace, WindowPlacement::Tiled, false, false)
        .expect("known workspace should accept window");
    backend
}

pub(crate) fn workspace_tokens(client: &AuthorityClient) -> Vec<String> {
    call(client, "workspaces", "list_workspaces", json!({}))["result"]["workspaces"]
        .as_array()
        .expect("workspaces should be an array")
        .iter()
        .map(|workspace| {
            workspace["workspace_id"]
                .as_str()
                .expect("workspace token should be text")
                .to_owned()
        })
        .collect()
}

/// Fake backend whose placement actions signal entry and exit and wait for
/// an explicit release, so tests can observe serialization without timing.
pub(crate) struct GatedBackend {
    pub(crate) inner: FakeBackend,
    pub(crate) events: mpsc::Sender<GateEvent>,
    pub(crate) release: mpsc::Receiver<()>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GateEvent {
    Enter,
    Exit,
}

impl WindowBackend for GatedBackend {
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceObservation>, BackendError> {
        self.inner.list_workspaces()
    }

    fn list_windows(&mut self) -> Result<Vec<WindowObservation>, BackendError> {
        self.inner.list_windows()
    }

    fn ensure_tiled(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.inner.ensure_tiled(window_id)
    }

    fn ensure_floating(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        let _ = self.events.send(GateEvent::Enter);
        self.release
            .recv_timeout(GUARD)
            .map_err(|_| BackendError::ActionFailed)?;
        let result = self.inner.ensure_floating(window_id);
        let _ = self.events.send(GateEvent::Exit);
        result
    }

    fn focus_window(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.inner.focus_window(window_id)
    }
}

/// Backend that panics when windows are listed, to end the authority abnormally.
pub(crate) struct PanickingBackend;

impl WindowBackend for PanickingBackend {
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceObservation>, BackendError> {
        Ok(Vec::new())
    }

    fn list_windows(&mut self) -> Result<Vec<WindowObservation>, BackendError> {
        panic!("intentional authority failure in test");
    }

    fn ensure_tiled(&mut self, _window_id: WindowId) -> Result<(), BackendError> {
        Err(BackendError::ActionFailed)
    }

    fn ensure_floating(&mut self, _window_id: WindowId) -> Result<(), BackendError> {
        Err(BackendError::ActionFailed)
    }

    fn focus_window(&mut self, _window_id: WindowId) -> Result<(), BackendError> {
        Err(BackendError::ActionFailed)
    }
}

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

/// A private runtime root (mode `0700`) that is removed when dropped.
pub(crate) struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub(crate) fn new() -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "nycti-windowd-daemon-test-{}-{sequence}",
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

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Binds the real runtime listener inside a private test directory.
pub(crate) fn bound_listener(directory: &TestDirectory) -> UnixRuntimeListener {
    UnixRuntimeListener::bind_at(directory.path()).expect("listener should bind in the test root")
}
