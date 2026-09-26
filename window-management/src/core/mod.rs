//! Compositor-independent Window Management policy.

/// A workspace's window-management mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceMode {
    Tiling,
    Windows,
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

#[cfg(test)]
mod tests {
    use super::{WorkspaceMode, WorkspaceModeResolution};

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
}
