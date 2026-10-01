//! Backend boundary between Window Management policy and compositors.

pub mod fake;
pub mod hyprland;

use std::fmt;

use crate::core::{WindowPlacement, WindowPlacementAction};

/// An opaque workspace identity assigned by a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkspaceId(u64);

/// An opaque window identity assigned by a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

/// A normalized observation of a compositor workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceObservation {
    id: WorkspaceId,
    present: bool,
}

impl WorkspaceObservation {
    pub(crate) const fn new(id: WorkspaceId, present: bool) -> Self {
        Self { id, present }
    }

    /// Returns the workspace's opaque identity.
    pub const fn id(&self) -> WorkspaceId {
        self.id
    }

    /// Returns whether the workspace is currently present.
    pub const fn is_present(&self) -> bool {
        self.present
    }
}

/// A normalized observation of a compositor window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowObservation {
    id: WindowId,
    workspace_id: WorkspaceId,
    placement: WindowPlacement,
    fullscreen: bool,
    focused: bool,
}

impl WindowObservation {
    pub(crate) const fn new(
        id: WindowId,
        workspace_id: WorkspaceId,
        placement: WindowPlacement,
        fullscreen: bool,
        focused: bool,
    ) -> Self {
        Self {
            id,
            workspace_id,
            placement,
            fullscreen,
            focused,
        }
    }

    /// Returns the window's opaque identity.
    pub const fn id(&self) -> WindowId {
        self.id
    }

    /// Returns the containing workspace's opaque identity.
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns the observed window placement.
    pub const fn placement(&self) -> WindowPlacement {
        self.placement
    }

    /// Returns whether the window is fullscreen.
    pub const fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Returns whether the window is focused.
    pub const fn is_focused(&self) -> bool {
        self.focused
    }
}

/// A failure reported at the compositor backend boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    UnknownWindow(WindowId),
    UnknownWorkspace(WorkspaceId),
    CompositorUnavailable,
    ActionFailed,
    ObservedStateUnavailable,
    InconsistentObservedState,
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnknownWindow(_) => "window is not known to the backend",
            Self::UnknownWorkspace(_) => "workspace is not known to the backend",
            Self::CompositorUnavailable => "compositor is unavailable",
            Self::ActionFailed => "compositor action did not establish the requested outcome",
            Self::ObservedStateUnavailable => "observed compositor state is unavailable",
            Self::InconsistentObservedState => "observed compositor state is inconsistent",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for BackendError {}

/// The synchronous boundary between Window Management policy and a compositor.
pub trait WindowBackend {
    /// Returns a snapshot of known workspaces.
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceObservation>, BackendError>;

    /// Returns a snapshot of known windows.
    fn list_windows(&mut self) -> Result<Vec<WindowObservation>, BackendError>;

    /// Ensures that the identified window is tiled.
    fn ensure_tiled(&mut self, window_id: WindowId) -> Result<(), BackendError>;

    /// Ensures that the identified window is floating.
    fn ensure_floating(&mut self, window_id: WindowId) -> Result<(), BackendError>;

    /// Focuses the identified window.
    fn focus_window(&mut self, window_id: WindowId) -> Result<(), BackendError>;
}

/// Applies a compositor-independent placement action through a backend.
pub fn apply_window_placement_action<B: WindowBackend + ?Sized>(
    backend: &mut B,
    window_id: WindowId,
    action: WindowPlacementAction,
) -> Result<(), BackendError> {
    match action {
        WindowPlacementAction::Keep => Ok(()),
        WindowPlacementAction::MakeTiled => backend.ensure_tiled(window_id),
        WindowPlacementAction::MakeFloating => backend.ensure_floating(window_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected message of each variant. The match is exhaustive on purpose:
    /// adding a variant to `BackendError` fails to compile here until it is
    /// listed, and `every_variant` below must then include it.
    fn expected_message(error: &BackendError) -> &'static str {
        match error {
            BackendError::UnknownWindow(_) => "window is not known to the backend",
            BackendError::UnknownWorkspace(_) => "workspace is not known to the backend",
            BackendError::CompositorUnavailable => "compositor is unavailable",
            BackendError::ActionFailed => {
                "compositor action did not establish the requested outcome"
            }
            BackendError::ObservedStateUnavailable => "observed compositor state is unavailable",
            BackendError::InconsistentObservedState => "observed compositor state is inconsistent",
        }
    }

    fn every_variant() -> [BackendError; 6] {
        [
            BackendError::UnknownWindow(WindowId(1)),
            BackendError::UnknownWorkspace(WorkspaceId(1)),
            BackendError::CompositorUnavailable,
            BackendError::ActionFailed,
            BackendError::ObservedStateUnavailable,
            BackendError::InconsistentObservedState,
        ]
    }

    #[test]
    fn every_variant_displays_its_message() {
        for error in every_variant() {
            assert_eq!(error.to_string(), expected_message(&error));
        }
    }

    #[test]
    fn variant_messages_are_distinct_and_do_not_expose_identities() {
        let messages = every_variant().map(|error| error.to_string());

        for (index, message) in messages.iter().enumerate() {
            assert!(!message.is_empty());
            assert!(!message.chars().any(|character| character.is_ascii_digit()));
            assert!(!messages[..index].contains(message));
        }
    }

    #[test]
    fn backend_error_is_a_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}

        assert_error(&BackendError::ActionFailed);
    }
}
