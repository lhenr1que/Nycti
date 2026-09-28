//! Opt-in smoke tests against a live Hyprland session.

use super::{HyprlandBackend, HyprlandTransport};

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

    println!("CLEA Hyprland read-only smoke test:");
    println!("workspaces: {}", snapshot.workspaces.len());
    println!("windows: {}", snapshot.windows.len());
    println!("focused windows: {focused_count}");
}
