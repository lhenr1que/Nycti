//! Opt-in smoke tests against a live Hyprland session.

use std::env;

use super::{HyprlandBackend, HyprlandTransport};
use crate::backend::{WindowBackend, WindowId, WindowObservation};
use crate::core::WindowPlacement;

fn observed_placement(
    windows: &[WindowObservation],
    target: WindowId,
) -> Result<WindowPlacement, String> {
    windows
        .iter()
        .find(|window| window.id() == target)
        .map(WindowObservation::placement)
        .ok_or_else(|| "target window is absent from the current snapshot".to_owned())
}

#[test]
#[ignore = "requires a live Hyprland session"]
fn live_read_only_snapshot_smoke_test() {
    let transport = HyprlandTransport::from_env()
        .expect("failed to derive the live Hyprland request transport from the environment");
    let mut backend = HyprlandBackend::new(transport);
    let snapshot = backend
        .snapshot()
        .expect("failed to produce a normalized snapshot from the live Hyprland session");

    let focused_count = snapshot
        .windows
        .iter()
        .filter(|window| window.is_focused())
        .count();

    assert!(focused_count <= 1);

    println!("Nycti Hyprland read-only smoke test:");
    println!("workspaces: {}", snapshot.workspaces.len());
    println!("windows: {}", snapshot.windows.len());
    println!("focused windows: {focused_count}");
}

#[test]
#[ignore = "mutates a live Hyprland window"]
fn live_action_focus_and_restore_placement_smoke_test() {
    assert!(
        matches!(env::var("CLEA_LIVE_ACTION_TEST").as_deref(), Ok("1")),
        "live action smoke test is disabled; set CLEA_LIVE_ACTION_TEST=1 and invoke this ignored test explicitly"
    );

    let mut backend = HyprlandBackend::from_env()
        .expect("failed to create the live Hyprland backend from the environment");
    let initial_windows = backend
        .list_windows()
        .expect("failed to obtain the initial live window snapshot");
    let focused_windows = initial_windows
        .iter()
        .filter(|window| window.is_focused())
        .copied()
        .collect::<Vec<_>>();

    assert_eq!(
        focused_windows.len(),
        1,
        "live action smoke test requires exactly one focused window"
    );

    let target_window = focused_windows[0];
    let target = target_window.id();
    let initial_placement = target_window.placement();
    let initial_focused = target_window.is_focused();
    let initial_fullscreen = target_window.is_fullscreen();

    assert!(initial_focused, "selected live target must be focused");
    assert!(
        !initial_fullscreen,
        "live action smoke test requires a focused non-fullscreen window; no action was executed"
    );
    assert_eq!(
        initial_placement,
        WindowPlacement::Tiled,
        "live action smoke test requires the focused test window to be initially tiled; floating geometry restoration is not yet modeled"
    );

    println!("initial placement: {initial_placement:?}");

    match backend.focus_window(target) {
        Ok(()) => {}
        Err(error) => {
            panic!(
                "focus step failed before placement mutation; no placement action was attempted: {error:?}"
            )
        }
    }

    let temporary_action_result = backend.ensure_floating(target);
    let temporary_verification = match backend.list_windows() {
        Ok(windows) => match observed_placement(&windows, target) {
            Ok(WindowPlacement::Floating) => Ok(()),
            Ok(placement) => Err(format!(
                "temporary placement mismatch: expected Floating, observed {placement:?}"
            )),
            Err(message) => Err(message),
        },
        Err(error) => Err(format!(
            "failed to obtain the temporary-state snapshot: {error:?}"
        )),
    };

    let restoration_result = backend.ensure_tiled(target);
    let final_verification = match backend.list_windows() {
        Ok(windows) => observed_placement(&windows, target),
        Err(error) => Err(format!(
            "failed to obtain the final restoration snapshot: {error:?}"
        )),
    };

    match restoration_result {
        Ok(()) => {}
        Err(restoration_error) => panic!(
            "CRITICAL: failed to restore the original placement {initial_placement:?}; temporary action result: {temporary_action_result:?}; temporary verification: {temporary_verification:?}; restoration error: {restoration_error:?}; final verification: {final_verification:?}"
        ),
    }

    let final_placement = match final_verification {
        Ok(placement) => placement,
        Err(final_error) => panic!(
            "CRITICAL: restoration action completed but final placement could not be verified; temporary action result: {temporary_action_result:?}; temporary verification: {temporary_verification:?}; final verification error: {final_error}"
        ),
    };

    assert_eq!(
        final_placement,
        WindowPlacement::Tiled,
        "CRITICAL: final placement does not match the original placement; temporary action result: {temporary_action_result:?}; temporary verification: {temporary_verification:?}"
    );

    match temporary_action_result {
        Ok(()) => {}
        Err(temporary_error) => panic!(
            "temporary placement action failed after the original placement was restored and verified: {temporary_error:?}; temporary verification: {temporary_verification:?}"
        ),
    }

    match temporary_verification {
        Ok(()) => {}
        Err(temporary_error) => panic!(
            "temporary placement verification failed after the original placement was restored and verified: {temporary_error}"
        ),
    }
}
