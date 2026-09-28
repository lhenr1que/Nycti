//! In-memory workspace-mode authority and placement orchestration.

use std::collections::HashMap;

use super::{ObservedWindowState, WorkspaceMode, WorkspaceModeResolution, plan_window_placement};
use crate::backend::{BackendError, WindowBackend, WorkspaceId, apply_window_placement_action};

/// Owns workspace modes and applies their placement policy through a backend.
pub struct WindowManager<B: WindowBackend> {
    backend: B,
    default_mode: WorkspaceMode,
    explicit_modes: HashMap<WorkspaceId, WorkspaceMode>,
}

impl<B: WindowBackend> WindowManager<B> {
    /// Creates an in-memory manager with no explicit workspace modes.
    pub fn new(backend: B, default_mode: WorkspaceMode) -> Self {
        Self {
            backend,
            default_mode,
            explicit_modes: HashMap::new(),
        }
    }

    /// Returns the mode inherited by workspaces without an explicit mode.
    pub const fn default_mode(&self) -> WorkspaceMode {
        self.default_mode
    }

    /// Changes the in-memory default without applying placement actions.
    pub fn set_default_mode(&mut self, mode: WorkspaceMode) {
        self.default_mode = mode;
    }

    /// Resolves the default, explicit, and effective modes for a workspace.
    pub fn workspace_mode(&self, workspace_id: WorkspaceId) -> WorkspaceModeResolution {
        WorkspaceModeResolution::new(
            self.default_mode,
            self.explicit_modes.get(&workspace_id).copied(),
        )
    }

    /// Sets an explicit in-memory mode without applying placement actions.
    pub fn set_workspace_mode(&mut self, workspace_id: WorkspaceId, mode: WorkspaceMode) {
        self.explicit_modes.insert(workspace_id, mode);
    }

    /// Clears an explicit mode without applying placement actions.
    pub fn clear_workspace_mode(&mut self, workspace_id: WorkspaceId) {
        self.explicit_modes.remove(&workspace_id);
    }

    /// Applies the workspace's effective placement mode to its current windows.
    ///
    /// A failure is returned immediately without retry or rollback, so actions
    /// completed before the failure may remain applied.
    pub fn apply_workspace_mode(&mut self, workspace_id: WorkspaceId) -> Result<(), BackendError> {
        let effective_mode = self.workspace_mode(workspace_id).effective_mode();
        let workspace_is_present = self
            .backend
            .list_workspaces()?
            .iter()
            .any(|workspace| workspace.id() == workspace_id && workspace.is_present());

        if !workspace_is_present {
            return Err(BackendError::UnknownWorkspace(workspace_id));
        }

        let windows = self.backend.list_windows()?;
        for window in windows
            .iter()
            .filter(|window| window.workspace_id() == workspace_id)
        {
            let observed = ObservedWindowState::new(window.placement(), window.is_fullscreen());
            let action = plan_window_placement(effective_mode, &observed);
            apply_window_placement_action(&mut self.backend, window.id(), action)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::WindowManager;
    use crate::backend::fake::{FakeBackend, RecordedBackendAction};
    use crate::backend::{BackendError, WindowBackend, WindowId};
    use crate::core::{WindowPlacement, WorkspaceMode};

    fn listed_placement(
        manager: &mut WindowManager<FakeBackend>,
        window_id: WindowId,
    ) -> WindowPlacement {
        manager
            .backend
            .list_windows()
            .expect("fake listing should succeed")
            .into_iter()
            .find(|window| window.id() == window_id)
            .expect("configured window should be listed")
            .placement()
    }

    #[test]
    fn workspace_without_override_uses_default() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        let resolution = manager.workspace_mode(workspace);

        assert_eq!(resolution.default_mode(), WorkspaceMode::Tiling);
        assert_eq!(resolution.explicit_mode(), None);
        assert_eq!(resolution.effective_mode(), WorkspaceMode::Tiling);
    }

    #[test]
    fn explicit_override_wins_over_default() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        manager.set_workspace_mode(workspace, WorkspaceMode::Windows);
        let resolution = manager.workspace_mode(workspace);

        assert_eq!(resolution.default_mode(), WorkspaceMode::Tiling);
        assert_eq!(resolution.explicit_mode(), Some(WorkspaceMode::Windows));
        assert_eq!(resolution.effective_mode(), WorkspaceMode::Windows);
    }

    #[test]
    fn clearing_explicit_mode_returns_to_default() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);
        manager.set_workspace_mode(workspace, WorkspaceMode::Windows);

        manager.clear_workspace_mode(workspace);

        let resolution = manager.workspace_mode(workspace);
        assert_eq!(resolution.explicit_mode(), None);
        assert_eq!(resolution.effective_mode(), WorkspaceMode::Tiling);
    }

    #[test]
    fn changing_default_does_not_change_explicit_override() {
        let mut backend = FakeBackend::new();
        let inherited = backend.add_workspace(true);
        let explicit = backend.add_workspace(true);
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);
        manager.set_workspace_mode(explicit, WorkspaceMode::Windows);

        manager.set_default_mode(WorkspaceMode::Windows);

        assert_eq!(
            manager.workspace_mode(inherited).effective_mode(),
            WorkspaceMode::Windows
        );
        assert_eq!(
            manager.workspace_mode(explicit).explicit_mode(),
            Some(WorkspaceMode::Windows)
        );
        assert_eq!(
            manager.workspace_mode(explicit).effective_mode(),
            WorkspaceMode::Windows
        );

        manager.set_workspace_mode(explicit, WorkspaceMode::Tiling);

        assert_eq!(manager.default_mode(), WorkspaceMode::Windows);
        assert_eq!(
            manager.workspace_mode(explicit).explicit_mode(),
            Some(WorkspaceMode::Tiling)
        );
        assert_eq!(
            manager.workspace_mode(explicit).effective_mode(),
            WorkspaceMode::Tiling
        );

        manager.clear_workspace_mode(explicit);

        assert_eq!(manager.workspace_mode(explicit).explicit_mode(), None);
        assert_eq!(
            manager.workspace_mode(explicit).effective_mode(),
            WorkspaceMode::Windows
        );
    }

    #[test]
    fn mode_changes_do_not_apply_actions_implicitly() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        manager.set_default_mode(WorkspaceMode::Windows);
        manager.set_workspace_mode(workspace, WorkspaceMode::Tiling);
        manager.clear_workspace_mode(workspace);

        assert!(manager.backend.recorded_actions().is_empty());
    }

    #[test]
    fn applying_absent_workspace_returns_unknown_without_actions() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(false);
        let historical_window = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known absent workspace should accept historical test state");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        assert_eq!(
            manager.apply_workspace_mode(workspace),
            Err(BackendError::UnknownWorkspace(workspace))
        );
        assert!(manager.backend.recorded_actions().is_empty());
        assert_eq!(
            listed_placement(&mut manager, historical_window),
            WindowPlacement::Floating
        );
    }

    #[test]
    fn applying_tiling_uses_planner_for_all_transition_states() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let tiled = backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");
        let floating = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let fullscreen = backend
            .add_window(workspace, WindowPlacement::Floating, true, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        manager
            .apply_workspace_mode(workspace)
            .expect("fake actions should succeed");

        assert_eq!(
            manager.backend.recorded_actions(),
            &[RecordedBackendAction::EnsureTiled(floating)]
        );
        assert_eq!(
            listed_placement(&mut manager, tiled),
            WindowPlacement::Tiled
        );
        assert_eq!(
            listed_placement(&mut manager, floating),
            WindowPlacement::Tiled
        );
        assert_eq!(
            listed_placement(&mut manager, fullscreen),
            WindowPlacement::Floating
        );
    }

    #[test]
    fn applying_windows_uses_planner_for_all_transition_states() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let floating = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let tiled = backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");
        let fullscreen = backend
            .add_window(workspace, WindowPlacement::Tiled, true, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Windows);

        manager
            .apply_workspace_mode(workspace)
            .expect("fake actions should succeed");

        assert_eq!(
            manager.backend.recorded_actions(),
            &[RecordedBackendAction::EnsureFloating(tiled)]
        );
        assert_eq!(
            listed_placement(&mut manager, floating),
            WindowPlacement::Floating
        );
        assert_eq!(
            listed_placement(&mut manager, tiled),
            WindowPlacement::Floating
        );
        assert_eq!(
            listed_placement(&mut manager, fullscreen),
            WindowPlacement::Tiled
        );
    }

    #[test]
    fn windows_already_in_effective_placement_produce_no_actions() {
        let mut backend = FakeBackend::new();
        let tiling_workspace = backend.add_workspace(true);
        let windows_workspace = backend.add_workspace(true);
        backend
            .add_window(tiling_workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");
        backend
            .add_window(windows_workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);
        manager.set_workspace_mode(windows_workspace, WorkspaceMode::Windows);

        manager
            .apply_workspace_mode(tiling_workspace)
            .expect("keep should succeed");
        manager
            .apply_workspace_mode(windows_workspace)
            .expect("keep should succeed");

        assert!(manager.backend.recorded_actions().is_empty());
    }

    #[test]
    fn fullscreen_windows_produce_no_placement_actions() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let floating = backend
            .add_window(workspace, WindowPlacement::Floating, true, false)
            .expect("known workspace should accept a window");
        let tiled = backend
            .add_window(workspace, WindowPlacement::Tiled, true, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        manager
            .apply_workspace_mode(workspace)
            .expect("fullscreen keep should succeed");
        manager.set_workspace_mode(workspace, WorkspaceMode::Windows);
        manager
            .apply_workspace_mode(workspace)
            .expect("fullscreen keep should succeed");

        assert!(manager.backend.recorded_actions().is_empty());
        assert_eq!(
            listed_placement(&mut manager, floating),
            WindowPlacement::Floating
        );
        assert_eq!(
            listed_placement(&mut manager, tiled),
            WindowPlacement::Tiled
        );
    }

    #[test]
    fn applying_workspace_only_changes_its_own_windows() {
        let mut backend = FakeBackend::new();
        let target_workspace = backend.add_workspace(true);
        let other_workspace = backend.add_workspace(true);
        let target = backend
            .add_window(target_workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let other = backend
            .add_window(other_workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        manager
            .apply_workspace_mode(target_workspace)
            .expect("fake action should succeed");

        assert_eq!(
            manager.backend.recorded_actions(),
            &[RecordedBackendAction::EnsureTiled(target)]
        );
        assert_eq!(
            listed_placement(&mut manager, target),
            WindowPlacement::Tiled
        );
        assert_eq!(
            listed_placement(&mut manager, other),
            WindowPlacement::Floating
        );
    }

    #[test]
    fn two_workspaces_keep_distinct_modes_and_apply_independently() {
        let mut backend = FakeBackend::new();
        let workspace_a = backend.add_workspace(true);
        let workspace_b = backend.add_workspace(true);
        let window_a = backend
            .add_window(workspace_a, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let window_b = backend
            .add_window(workspace_b, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);
        manager.set_workspace_mode(workspace_a, WorkspaceMode::Tiling);
        manager.set_workspace_mode(workspace_b, WorkspaceMode::Windows);

        manager
            .apply_workspace_mode(workspace_a)
            .expect("workspace A should apply");
        assert_eq!(
            listed_placement(&mut manager, window_a),
            WindowPlacement::Tiled
        );
        assert_eq!(
            listed_placement(&mut manager, window_b),
            WindowPlacement::Tiled
        );
        assert_eq!(
            manager.workspace_mode(workspace_b).explicit_mode(),
            Some(WorkspaceMode::Windows)
        );

        manager
            .apply_workspace_mode(workspace_b)
            .expect("workspace B should apply");

        assert_eq!(
            manager.workspace_mode(workspace_a).effective_mode(),
            WorkspaceMode::Tiling
        );
        assert_eq!(
            manager.workspace_mode(workspace_b).effective_mode(),
            WorkspaceMode::Windows
        );
        assert_eq!(
            listed_placement(&mut manager, window_a),
            WindowPlacement::Tiled
        );
        assert_eq!(
            listed_placement(&mut manager, window_b),
            WindowPlacement::Floating
        );
    }

    #[test]
    fn changing_one_workspace_mode_does_not_change_another_mode_or_windows() {
        let mut backend = FakeBackend::new();
        let workspace_a = backend.add_workspace(true);
        let workspace_b = backend.add_workspace(true);
        let window_b = backend
            .add_window(workspace_b, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);
        manager.set_workspace_mode(workspace_b, WorkspaceMode::Windows);

        manager.set_workspace_mode(workspace_a, WorkspaceMode::Windows);
        manager
            .apply_workspace_mode(workspace_a)
            .expect("empty workspace should apply");

        assert_eq!(
            manager.workspace_mode(workspace_b).explicit_mode(),
            Some(WorkspaceMode::Windows)
        );
        assert_eq!(
            listed_placement(&mut manager, window_b),
            WindowPlacement::Floating
        );
        assert!(manager.backend.recorded_actions().is_empty());
    }

    #[test]
    fn backend_action_failure_stops_without_retry_or_rollback() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let first = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let failed = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        let unattempted = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept a window");
        backend.fail_actions_for(failed, BackendError::ActionFailed);
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        assert_eq!(
            manager.apply_workspace_mode(workspace),
            Err(BackendError::ActionFailed)
        );

        assert_eq!(
            manager.backend.recorded_actions(),
            &[RecordedBackendAction::EnsureTiled(first)]
        );
        assert_eq!(
            listed_placement(&mut manager, first),
            WindowPlacement::Tiled
        );
        assert_eq!(
            listed_placement(&mut manager, failed),
            WindowPlacement::Floating
        );
        assert_eq!(
            listed_placement(&mut manager, unattempted),
            WindowPlacement::Floating
        );
    }

    #[test]
    fn applying_mode_never_requests_focus() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Floating, false, true)
            .expect("known workspace should accept a window");
        let mut manager = WindowManager::new(backend, WorkspaceMode::Tiling);

        manager
            .apply_workspace_mode(workspace)
            .expect("fake action should succeed");

        assert_eq!(
            manager.backend.recorded_actions(),
            &[RecordedBackendAction::EnsureTiled(window)]
        );
        assert!(
            manager
                .backend
                .recorded_actions()
                .iter()
                .all(|action| { !matches!(action, RecordedBackendAction::Focus(_)) })
        );
    }
}
