//! Helpers shared by the authority and worker tests.

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
use crate::service::WindowManagementService;

/// Protection against a hung test only. It is never used to synchronize.
pub(super) const GUARD: Duration = Duration::from_secs(30);

/// Runs a test body on its own thread so a hang fails instead of blocking
/// `cargo test`. A panic in the body is propagated unchanged.
pub(super) fn guarded<F: FnOnce() + Send + 'static>(body: F) {
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

pub(super) fn service_with(backend: FakeBackend) -> WindowManagementService<FakeBackend> {
    WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling))
}

pub(super) fn request(id: &str, method: &str, params: Value) -> Vec<u8> {
    json!({"version": 1, "id": id, "method": method, "params": params})
        .to_string()
        .into_bytes()
}

pub(super) fn lined(request: &[u8]) -> Vec<u8> {
    let mut line = request.to_vec();
    line.push(b'\n');
    line
}

pub(super) fn response(line: &str) -> Value {
    assert!(
        line.ends_with('\n'),
        "response must be one LF-terminated line"
    );
    serde_json::from_str(line).expect("response should be JSON")
}

pub(super) fn call(client: &AuthorityClient, id: &str, method: &str, params: Value) -> Value {
    let line = client
        .submit(&request(id, method, params))
        .expect("authority should respond");
    response(&line)
}

pub(super) fn fake_with_workspace_and_window() -> FakeBackend {
    let mut backend = FakeBackend::new();
    let workspace = backend.add_workspace(true);
    backend
        .add_window(workspace, WindowPlacement::Tiled, false, false)
        .expect("known workspace should accept window");
    backend
}

pub(super) fn workspace_tokens(client: &AuthorityClient) -> Vec<String> {
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
pub(super) struct GatedBackend {
    pub(super) inner: FakeBackend,
    pub(super) events: mpsc::Sender<GateEvent>,
    pub(super) release: mpsc::Receiver<()>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum GateEvent {
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
pub(super) struct PanickingBackend;

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
