//! Offline compositor backend for tests and development.

use super::{
    BackendError, WindowBackend, WindowId, WindowObservation, WorkspaceId, WorkspaceObservation,
};
use crate::core::WindowPlacement;

/// An action requested through [`FakeBackend`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordedBackendAction {
    EnsureTiled(WindowId),
    EnsureFloating(WindowId),
    Focus(WindowId),
}

/// An in-memory backend that requires no compositor.
#[derive(Default)]
pub struct FakeBackend {
    next_workspace_id: u64,
    next_window_id: u64,
    workspaces: Vec<WorkspaceObservation>,
    windows: Vec<WindowObservation>,
    recorded_actions: Vec<RecordedBackendAction>,
    action_failure: Option<(WindowId, BackendError)>,
}

impl FakeBackend {
    /// Creates an empty fake backend.
    pub const fn new() -> Self {
        Self {
            next_workspace_id: 0,
            next_window_id: 0,
            workspaces: Vec::new(),
            windows: Vec::new(),
            recorded_actions: Vec::new(),
            action_failure: None,
        }
    }

    /// Adds a workspace and returns its generated opaque identity.
    pub fn add_workspace(&mut self, present: bool) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        self.workspaces.push(WorkspaceObservation::new(id, present));
        id
    }

    /// Adds a window to a known workspace and returns its opaque identity.
    pub fn add_window(
        &mut self,
        workspace_id: WorkspaceId,
        placement: WindowPlacement,
        fullscreen: bool,
        focused: bool,
    ) -> Result<WindowId, BackendError> {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return Err(BackendError::UnknownWorkspace(workspace_id));
        }

        if focused {
            for window in &mut self.windows {
                window.focused = false;
            }
        }

        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        self.windows.push(WindowObservation::new(
            id,
            workspace_id,
            placement,
            fullscreen,
            focused,
        ));
        Ok(id)
    }

    /// Returns the actions requested through this backend.
    pub fn recorded_actions(&self) -> &[RecordedBackendAction] {
        &self.recorded_actions
    }

    /// Configures actions for one known window to return the supplied error.
    pub fn fail_actions_for(&mut self, window_id: WindowId, error: BackendError) {
        self.action_failure = Some((window_id, error));
    }

    fn prepare_action(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.window_mut(window_id)?;

        match self.action_failure {
            Some((failed_window, error)) if failed_window == window_id => Err(error),
            _ => Ok(()),
        }
    }

    fn window_mut(&mut self, window_id: WindowId) -> Result<&mut WindowObservation, BackendError> {
        self.windows
            .iter_mut()
            .find(|window| window.id == window_id)
            .ok_or(BackendError::UnknownWindow(window_id))
    }
}

impl WindowBackend for FakeBackend {
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceObservation>, BackendError> {
        Ok(self.workspaces.clone())
    }

    fn list_windows(&mut self) -> Result<Vec<WindowObservation>, BackendError> {
        Ok(self.windows.clone())
    }

    fn ensure_tiled(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.prepare_action(window_id)?;
        self.window_mut(window_id)?.placement = WindowPlacement::Tiled;
        self.recorded_actions
            .push(RecordedBackendAction::EnsureTiled(window_id));
        Ok(())
    }

    fn ensure_floating(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.prepare_action(window_id)?;
        self.window_mut(window_id)?.placement = WindowPlacement::Floating;
        self.recorded_actions
            .push(RecordedBackendAction::EnsureFloating(window_id));
        Ok(())
    }

    fn focus_window(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.prepare_action(window_id)?;

        for window in &mut self.windows {
            window.focused = window.id == window_id;
        }
        self.recorded_actions
            .push(RecordedBackendAction::Focus(window_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeBackend, RecordedBackendAction};
    use crate::backend::{BackendError, WindowBackend, apply_window_placement_action};
    use crate::core::{
        ObservedWindowState, WindowPlacement, WindowPlacementAction, WorkspaceMode,
        plan_window_placement,
    };

    fn listed_window(
        backend: &mut FakeBackend,
        window_id: super::WindowId,
    ) -> super::WindowObservation {
        backend
            .list_windows()
            .expect("fake listing should succeed")
            .into_iter()
            .find(|window| window.id() == window_id)
            .expect("configured window should be listed")
    }

    #[test]
    fn provides_distinct_opaque_workspace_ids() {
        let mut backend = FakeBackend::new();
        let first = backend.add_workspace(true);
        let second = backend.add_workspace(false);

        assert_ne!(first, second);
        let workspaces = backend
            .list_workspaces()
            .expect("fake listing should succeed");
        assert_eq!(workspaces.len(), 2);
        assert_eq!(workspaces[0].id(), first);
        assert!(workspaces[0].is_present());
        assert_eq!(workspaces[1].id(), second);
        assert!(!workspaces[1].is_present());
    }

    #[test]
    fn lists_window_observation_and_workspace_membership() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Floating, true, true)
            .expect("known workspace should accept a window");

        let observation = listed_window(&mut backend, window);

        assert_eq!(observation.workspace_id(), workspace);
        assert_eq!(observation.placement(), WindowPlacement::Floating);
        assert!(observation.is_fullscreen());
        assert!(observation.is_focused());
    }

    #[test]
    fn ensure_tiled_is_declarative_and_recorded() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");

        backend.ensure_tiled(window).expect("window should exist");
        backend.ensure_tiled(window).expect("window should exist");

        assert_eq!(
            listed_window(&mut backend, window).placement(),
            WindowPlacement::Tiled
        );
        assert_eq!(
            backend.recorded_actions(),
            &[
                RecordedBackendAction::EnsureTiled(window),
                RecordedBackendAction::EnsureTiled(window),
            ]
        );
    }

    #[test]
    fn ensure_floating_is_declarative_and_recorded() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");

        backend
            .ensure_floating(window)
            .expect("window should exist");
        backend
            .ensure_floating(window)
            .expect("window should exist");

        assert_eq!(
            listed_window(&mut backend, window).placement(),
            WindowPlacement::Floating
        );
        assert_eq!(
            backend.recorded_actions(),
            &[
                RecordedBackendAction::EnsureFloating(window),
                RecordedBackendAction::EnsureFloating(window),
            ]
        );
    }

    #[test]
    fn focus_window_focuses_target_and_unfocuses_others() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let first = backend
            .add_window(workspace, WindowPlacement::Tiled, false, true)
            .expect("known workspace should accept a window");
        let second = backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");

        backend
            .focus_window(second)
            .expect("target window should exist");

        assert!(!listed_window(&mut backend, first).is_focused());
        assert!(listed_window(&mut backend, second).is_focused());
        assert_eq!(
            backend.recorded_actions(),
            &[RecordedBackendAction::Focus(second)]
        );
    }

    #[test]
    fn unknown_window_actions_return_an_error() {
        let mut source = FakeBackend::new();
        let source_workspace = source.add_workspace(true);
        let unknown_window = source
            .add_window(source_workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");
        let mut backend = FakeBackend::new();

        assert_eq!(
            backend.ensure_tiled(unknown_window),
            Err(BackendError::UnknownWindow(unknown_window))
        );
        assert_eq!(
            backend.ensure_floating(unknown_window),
            Err(BackendError::UnknownWindow(unknown_window))
        );
        assert_eq!(
            backend.focus_window(unknown_window),
            Err(BackendError::UnknownWindow(unknown_window))
        );
        assert!(backend.recorded_actions().is_empty());
    }

    #[test]
    fn adding_window_to_unknown_workspace_returns_an_error() {
        let mut source = FakeBackend::new();
        let unknown_workspace = source.add_workspace(true);
        let mut backend = FakeBackend::new();

        assert_eq!(
            backend.add_window(unknown_workspace, WindowPlacement::Tiled, false, false),
            Err(BackendError::UnknownWorkspace(unknown_workspace))
        );
    }

    #[test]
    fn applying_make_tiled_calls_backend() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");

        apply_window_placement_action(&mut backend, window, WindowPlacementAction::MakeTiled)
            .expect("fake action should succeed");

        assert_eq!(
            backend.recorded_actions(),
            &[RecordedBackendAction::EnsureTiled(window)]
        );
        assert_eq!(
            listed_window(&mut backend, window).placement(),
            WindowPlacement::Tiled
        );
    }

    #[test]
    fn applying_make_floating_calls_backend() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");

        apply_window_placement_action(&mut backend, window, WindowPlacementAction::MakeFloating)
            .expect("fake action should succeed");

        assert_eq!(
            backend.recorded_actions(),
            &[RecordedBackendAction::EnsureFloating(window)]
        );
        assert_eq!(
            listed_window(&mut backend, window).placement(),
            WindowPlacement::Floating
        );
    }

    #[test]
    fn applying_keep_does_not_call_backend() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");

        apply_window_placement_action(&mut backend, window, WindowPlacementAction::Keep)
            .expect("keep should succeed");

        assert!(backend.recorded_actions().is_empty());
        assert_eq!(
            listed_window(&mut backend, window).placement(),
            WindowPlacement::Tiled
        );
    }

    #[test]
    fn fullscreen_planner_flow_makes_no_backend_call() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Floating, true, false)
            .expect("known workspace should accept a window");
        let observation = listed_window(&mut backend, window);
        let observed_state =
            ObservedWindowState::new(observation.placement(), observation.is_fullscreen());

        let action = plan_window_placement(WorkspaceMode::Tiling, &observed_state);
        apply_window_placement_action(&mut backend, window, action)
            .expect("keep should succeed without a compositor");

        assert_eq!(action, WindowPlacementAction::Keep);
        assert!(backend.recorded_actions().is_empty());
        assert_eq!(
            listed_window(&mut backend, window).placement(),
            WindowPlacement::Floating
        );
    }
}
