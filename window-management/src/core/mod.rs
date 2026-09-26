//! Compositor-independent Window Management policy.

/// A workspace's window-management mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceMode {
    Tiling,
    Windows,
}

/// The observed placement of an ordinary window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowPlacement {
    Tiled,
    Floating,
}

/// The observed state needed to plan an ordinary window's placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservedWindowState {
    placement: WindowPlacement,
    fullscreen: bool,
}

impl ObservedWindowState {
    /// Creates an observed window state.
    pub const fn new(placement: WindowPlacement, fullscreen: bool) -> Self {
        Self {
            placement,
            fullscreen,
        }
    }

    /// Returns the observed placement.
    pub const fn placement(&self) -> WindowPlacement {
        self.placement
    }

    /// Returns whether the window is fullscreen.
    pub const fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }
}

/// A compositor-independent action needed to satisfy workspace placement policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowPlacementAction {
    Keep,
    MakeTiled,
    MakeFloating,
}

/// The inputs used to resolve a workspace's effective mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceModeResolution {
    default_mode: WorkspaceMode,
    explicit_mode: Option<WorkspaceMode>,
}

impl WorkspaceModeResolution {
    /// Creates a mode resolution from the default and optional explicit mode.
    pub const fn new(default_mode: WorkspaceMode, explicit_mode: Option<WorkspaceMode>) -> Self {
        Self {
            default_mode,
            explicit_mode,
        }
    }

    /// Returns the default workspace mode.
    pub const fn default_mode(&self) -> WorkspaceMode {
        self.default_mode
    }

    /// Returns the workspace-specific mode, when one is set.
    pub const fn explicit_mode(&self) -> Option<WorkspaceMode> {
        self.explicit_mode
    }

    /// Returns the explicit mode when present, or the default mode otherwise.
    pub const fn effective_mode(&self) -> WorkspaceMode {
        match self.explicit_mode {
            Some(mode) => mode,
            None => self.default_mode,
        }
    }
}

/// Plans an ordinary window's placement action for an effective workspace mode.
pub const fn plan_window_placement(
    effective_workspace_mode: WorkspaceMode,
    observed_window: &ObservedWindowState,
) -> WindowPlacementAction {
    if observed_window.is_fullscreen() {
        return WindowPlacementAction::Keep;
    }

    match (effective_workspace_mode, observed_window.placement()) {
        (WorkspaceMode::Tiling, WindowPlacement::Floating) => WindowPlacementAction::MakeTiled,
        (WorkspaceMode::Windows, WindowPlacement::Tiled) => WindowPlacementAction::MakeFloating,
        (WorkspaceMode::Tiling, WindowPlacement::Tiled)
        | (WorkspaceMode::Windows, WindowPlacement::Floating) => WindowPlacementAction::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ObservedWindowState, WindowPlacement, WindowPlacementAction, WorkspaceMode,
        WorkspaceModeResolution, plan_window_placement,
    };

    #[test]
    fn workspace_without_explicit_mode_uses_default() {
        let resolution = WorkspaceModeResolution::new(WorkspaceMode::Tiling, None);

        assert_eq!(resolution.default_mode(), WorkspaceMode::Tiling);
        assert_eq!(resolution.explicit_mode(), None);
        assert_eq!(resolution.effective_mode(), WorkspaceMode::Tiling);
    }

    #[test]
    fn explicit_tiling_overrides_default_windows() {
        let resolution =
            WorkspaceModeResolution::new(WorkspaceMode::Windows, Some(WorkspaceMode::Tiling));

        assert_eq!(resolution.default_mode(), WorkspaceMode::Windows);
        assert_eq!(resolution.explicit_mode(), Some(WorkspaceMode::Tiling));
        assert_eq!(resolution.effective_mode(), WorkspaceMode::Tiling);
    }

    #[test]
    fn explicit_windows_overrides_default_tiling() {
        let resolution =
            WorkspaceModeResolution::new(WorkspaceMode::Tiling, Some(WorkspaceMode::Windows));

        assert_eq!(resolution.default_mode(), WorkspaceMode::Tiling);
        assert_eq!(resolution.explicit_mode(), Some(WorkspaceMode::Windows));
        assert_eq!(resolution.effective_mode(), WorkspaceMode::Windows);
    }

    #[test]
    fn querying_effective_mode_does_not_modify_state() {
        let resolution =
            WorkspaceModeResolution::new(WorkspaceMode::Tiling, Some(WorkspaceMode::Windows));
        let original = resolution;

        assert_eq!(resolution.effective_mode(), WorkspaceMode::Windows);
        assert_eq!(resolution, original);
    }

    #[test]
    fn tiling_mode_makes_floating_window_tiled() {
        let observed = ObservedWindowState::new(WindowPlacement::Floating, false);

        assert_eq!(
            plan_window_placement(WorkspaceMode::Tiling, &observed),
            WindowPlacementAction::MakeTiled
        );
    }

    #[test]
    fn tiling_mode_keeps_tiled_window() {
        let observed = ObservedWindowState::new(WindowPlacement::Tiled, false);

        assert_eq!(
            plan_window_placement(WorkspaceMode::Tiling, &observed),
            WindowPlacementAction::Keep
        );
    }

    #[test]
    fn windows_mode_makes_tiled_window_floating() {
        let observed = ObservedWindowState::new(WindowPlacement::Tiled, false);

        assert_eq!(
            plan_window_placement(WorkspaceMode::Windows, &observed),
            WindowPlacementAction::MakeFloating
        );
    }

    #[test]
    fn windows_mode_keeps_floating_window() {
        let observed = ObservedWindowState::new(WindowPlacement::Floating, false);

        assert_eq!(
            plan_window_placement(WorkspaceMode::Windows, &observed),
            WindowPlacementAction::Keep
        );
    }

    #[test]
    fn fullscreen_floating_window_is_kept_in_tiling_mode() {
        let observed = ObservedWindowState::new(WindowPlacement::Floating, true);

        assert_eq!(
            plan_window_placement(WorkspaceMode::Tiling, &observed),
            WindowPlacementAction::Keep
        );
    }

    #[test]
    fn fullscreen_tiled_window_is_kept_in_windows_mode() {
        let observed = ObservedWindowState::new(WindowPlacement::Tiled, true);

        assert_eq!(
            plan_window_placement(WorkspaceMode::Windows, &observed),
            WindowPlacementAction::Keep
        );
    }

    #[test]
    fn planning_placement_does_not_modify_observed_state() {
        let observed = ObservedWindowState::new(WindowPlacement::Floating, false);
        let original = observed;

        assert_eq!(
            plan_window_placement(WorkspaceMode::Tiling, &observed),
            WindowPlacementAction::MakeTiled
        );
        assert_eq!(observed, original);
    }
}
