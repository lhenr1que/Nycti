# Nycti Visual Concept and UX

- Status: Accepted. Rows marked Proposed in the table below still need their own ADR.
- Date: 2026-10-03
- Replaces: the Portuguese working version `nycti_visual_concept.md`

## 1. Purpose

This document records the official interface and user experience decisions for
Nycti. It describes how the Shell is organized, how window management appears to
the user, and what the Shell needs from the daemon.

It is a design document. A decision that changes architecture, adds a dependency,
or changes the protocol needs its own ADR before implementation. Final visual
metrics (sidebar width, spacing, radius, blur, shadows, typography, animation
timing) belong to a future design system and are not set here.

## 2. Decision summary

| # | Decision | Status |
| --- | --- | --- |
| D1 | One vertical sidebar on the left. No top bar and no dock. It is always visible by default. The user can configure it to appear only when the pointer rests on the left edge. | Decided |
| D2 | The taskbar is a module of the sidebar. There is no horizontal taskbar. | Decided |
| D3 | Each workspace keeps its own mode: Window Mode or Tiling Mode. | Decided |
| D4 | Caelestia is a structural and behavioral reference. Nycti has its own visual identity. Pieces of Caelestia may be copied, with origin recorded (section 14). | Decided |
| D5 | Nycti Settings is a program of its own. It does not depend on Nexus (the Caelestia settings application) and does not redirect to it. | Decided |
| D6 | Themes: Light, Dark and Auto. Default pair is Ivory (light) and Night (dark). Sand is a light variant. Colors are direction only. | Decided |
| D7 | The launcher is a searchable, keyboard-first list with categories. The grid seen in the mockup is the Apps view inside the launcher. | Decided |
| D8 | The UI label "Window Mode" corresponds to the protocol token `windows`. No protocol change for this. | Decided |
| D9 | Shell visual code is written in the Nycti tree, starting from selected Caelestia components, and is not a fork. If accepted, it amends ADR 0001, which names Caelestia the primary upstream. | Proposed, needs ADR |
| D10 | The Shell does not depend on `caelestia-cli`. It talks to the daemon (and to future Nycti tools) only. | Proposed, needs ADR |
| D11 | The title bar is implemented with Hyprbars or an equivalent, subject to an ADR. | Open |
| D12 | In Window Mode every window has close, minimize and maximize/restore buttons in its title bar. | Decided |
| D13 | Panels open when the pointer rests on their trigger, as in Caelestia, and always have a button or shortcut as an alternative. | Decided |
| D14 | The power button of the sidebar becomes a system menu button. It opens a panel anchored to the sidebar with the quick toggles and the session actions. Volume has its own icon in System Status, with its own panel. | Decided |
| D15 | The Shell is written with Quickshell and QML, the stack Caelestia uses. | Proposed, needs ADR |
| D16 | The logo in the sidebar is clickable and opens the launcher. | Decided |
| D17 | A Displays icon in System Status opens a panel with one row per monitor. Each row has a brightness slider when that monitor supports it, and the panel links to Settings, section Displays. | Decided |
| D18 | A thin Show Desktop strip at the bottom end of the sidebar hides all windows of the current workspace and, on a second click, restores the windows it hid. | Decided |

## 3. Visual direction

The interface is clean, light on screen space and integrated with the desktop.
It uses a vertical sidebar as the main persistent element, rounded corners,
floating panels, transparency and blur where they fit, smooth animation and low
visual noise. It targets Hyprland.

Caelestia informs how the Shell is organized and how it behaves (sidebar,
launcher, quick settings, notifications, workspaces). The look is Nycti's own:
warm ivory and parchment tones in the light theme, a soft dark theme, and
discreet references to the urutau (Nyctibius) in the logo and icon.

The mockup image produced for the project is a moodboard (maturity phase 3, see
section 17). It shows the mood, palette and one launcher layout. It is not a
specification, and it lacks two elements this document requires: a per-workspace
mode indicator and the Window Mode module.

## 4. Layout

```text
Nycti Sidebar (left edge)
+-- Identity / logo (opens the launcher)
+-- Navigation (Home, Apps, Files)
+-- Taskbar
+-- Workspaces
+-- Window Mode
+-- System Status
+-- Show Desktop (thin strip at the bottom end)
```

The order may change during prototypes. The sidebar is the only component visible by default. Everything else (launcher, quick settings, notifications, calendar,
network, volume, system menu) is a floating panel that appears when requested.

Sidebar behavior. The user chooses between two modes in Settings, section Sidebar:

- Always visible (default). The sidebar keeps its own space and windows do not cover it. Reserving space is a proposal.
- Auto-hide. The sidebar appears when the pointer rests on the left edge and hides when the pointer leaves. It appears over the windows without moving them. The taskbar hides with it, so the launcher shortcut and Alt+Tab matter more in this mode.

Auto-hide needs the edge to be a wall. If another monitor shares that edge, the pointer crosses it, so on that monitor the sidebar opens by shortcut.

Fullscreen: the sidebar does not draw over a fullscreen window. Proposed, to be
confirmed in the fullscreen screen.

Multiple monitors: open (section 18). Proposed starting point is the sidebar on
the primary monitor only.

## 5. Taskbar

The taskbar is a module of the sidebar and shows these application states:

- pinned and closed;
- open;
- active;
- minimized;
- open in another workspace;
- with several windows.

Symbols are provisional. Expected behavior:

| Click on | Result |
| --- | --- |
| Closed application | Open it |
| Open application | Focus its window |
| Focused application | Minimize it |
| Minimized application | Restore it |
| Application with several windows | Show a window list |

Pinning, unpinning and reordering pinned applications are also required.

Most of this needs data and actions that protocol v1 does not offer (section 13).
The taskbar cannot be implemented in full until the protocol grows.

## 6. Workspaces and modes

Each workspace keeps its own mode.

```text
1  Window Mode
2  Tiling Mode
3  Tiling Mode
```

The sidebar shows the mode of every workspace in a compact form (for example an
icon next to the workspace number). The final form is decided in the wireframe
phase.

Terminology. The UI says "Window Mode" and "Tiling Mode". The protocol and the
CLI use the tokens `windows` and `tiling`. The mapping is fixed:

| UI label | Protocol token |
| --- | --- |
| Window Mode | `windows` |
| Tiling Mode | `tiling` |

## 7. Window Mode and Tiling Mode

Window Mode: windows float, can be moved and resized, minimized, restored and
maximized, and have a title bar. Windows opened in a Window Mode workspace open
as floating windows.

Tiling Mode: windows follow the tiling behavior of Hyprland.

Switching mode. The Shell control for a workspace maps to two daemon calls:
`set_workspace_mode` (policy only) and `apply_workspace_mode` (changes real
windows). The daemon has no atomic toggle (ADR 0010). The Shell sends both calls
in sequence and reports a failure of either one. An atomic operation in the
daemon is a candidate for a future protocol version.

What the switch does today. `apply_workspace_mode` only changes windows between
tiled and floating. Where each window lands, and whether earlier positions are
remembered, is decided by Hyprland. The Tiling to Window figure in this concept
is illustrative of the idea, not a promise of final geometry. Whether Nycti
should control geometry is open (section 18).

New windows. Applying a mode affects windows that exist at that moment. The rule
that new windows open floating in Window Mode is not covered by the daemon and
needs a decision (section 18).

## 8. Windows and title bar

The interface must represent these window states: floating, tiled, maximized,
minimized, fullscreen, focused and unfocused.

Window Mode (D12). Every window in a Window Mode workspace has a title bar with
three buttons:

| Button | Meaning |
| --- | --- |
| Close | Asks the application to close. |
| Maximize / Restore | Maximize fills the usable area (the sidebar stays visible). Restore returns the window to its earlier size and position. |
| Minimize | Hides the window and keeps it in the taskbar as minimized. A click on the taskbar item restores it. |

Hyprland has no minimize in the sense of a traditional desktop. The usual
technique is to move the window to a special workspace, which the ADR must
confirm. Nycti therefore has to define minimize and maximize/restore itself. The daemon owns
this state, because the Shell is a client and not an authority (ADR 0002). Protocol
v1 has no such actions, so the buttons need a protocol extension and an ADR.

Tiling Mode. Minimize and maximize have little meaning for tiled windows. The
proposal is no title bar buttons in Tiling Mode, with close by keyboard. Whether
a thin title bar appears there is open.

Implementation candidates: Hyprbars with its buttons wired to Nycti commands, or
a decoration of Nycti's own. Hyprbars is a Hyprland plugin tied to the compositor
version (Hyprland 0.56.2 is the current target), so using it needs an ADR on the
dependency and on what happens when the plugin breaks.

Show Desktop (D18). A thin, low-contrast strip at the bottom end of the sidebar, below the system menu button, hides every window of the current workspace and shows the desktop. A second click restores exactly the windows it hid, in their earlier state. It also has a keyboard shortcut, which matters when the sidebar is in auto-hide mode. It uses the same hiding mechanism as minimize, so it depends on the protocol extension for minimize and restore and on knowing the current workspace.

## 9. Floating panels and hover reveal

Panels open when the pointer rests on their trigger, as in Caelestia (D13).
Moving away closes the panel after a short delay, a click pins it open, and
Escape closes it. Every panel also has a button or a keyboard shortcut, so no
function depends on hover alone. Animation is a smooth reveal; durations belong
to the design system.

Panels and triggers, following the Caelestia screens used as reference, except
for the system menu:

| Panel | Trigger | Position |
| --- | --- | --- |
| Dashboard (clock, weather, calendar, media, performance) | Top edge | Top, centered |
| System menu (quick toggles, session actions) | Menu button at the bottom of the sidebar | Anchored to the sidebar, over the work area |
| Network, Bluetooth | Icon in System Status | Beside the sidebar |
| Volume | Volume icon in System Status | Beside the sidebar |
| Displays | Displays icon in System Status | Beside the sidebar |
| Application menu or window list | Taskbar item | Beside the sidebar |
| Launcher | Logo, shortcut or Navigation button | Centered |
| Notifications | Badge in System Status | Beside the sidebar |

A panel that opens from the top edge is not a top bar (D1). It exists only while
it is shown.

System menu (D14). The power button of the sidebar becomes a menu button with a different icon. It opens a panel anchored to the sidebar, which extends over the work area from the button. The sidebar keeps its width and no window is rearranged, because widening the sidebar would resize tiled windows every time the menu opens. The panel holds the quick toggles (keep awake, screen recording, Wi-Fi, Bluetooth, microphone, do not disturb, VPN, Game Mode) and the session actions (logout, lock, restart, shut down). Shut down, restart and logout ask for confirmation. The confirmation is a proposal.

Volume. The volume icon in System Status opens its own panel with the slider, mute and output device selection. The output device selection is a proposal.

Displays (D17). The Displays icon in System Status opens a panel with one row per monitor. Each row shows the monitor name and a brightness slider when that monitor's brightness can be controlled, and the panel links to Settings, section Displays. Brightness belongs to one monitor, so listing every monitor lets the user choose explicitly. Laptop panels usually can be adjusted; an external monitor depends on hardware support (for example DDC/CI), to be checked on the real devices. A monitor that cannot be adjusted shows its name with a short note instead of a slider.

Multiple monitors. Three points decide how these panels behave:

- Volume is system-wide, so the volume panel is the same on every monitor.
- Brightness belongs to one monitor and is handled in the Displays panel, which lists every monitor.
- Because these triggers live in the sidebar, the shared-edge problem does not affect them. It affects the auto-hide sidebar and the top-edge dashboard: the pointer crosses an edge shared with a neighboring monitor instead of stopping there, so on shared edges those panels open by shortcut.

The Shell gets the list of monitors from the Wayland session. Protocol v1 has no
monitor information.

System tray and notification indicator. Both live in the System Status module of
the sidebar. Tray items open as a panel from that module. Pending notifications
show as a badge on the module. This is a proposal; the mockup and the earlier
documents do not place them.

## 10. Launcher

Search, applications, commands, files, system actions and keyboard navigation.
Categories organize the list. The Apps view shows applications as a grid, as in
the mockup. The launcher opens as a panel and follows the Nycti visual language.
The logo opens the launcher. The Apps item in Navigation opens it on the Apps view.

## 11. Nycti Settings

Nycti Settings is a program of its own (D5). Nexus, the settings application
developed by the Caelestia team, is a reference and a source of ideas. It is not
a dependency.

Sections:

```text
Appearance
Desktop
Sidebar
Taskbar
Window Management
Workspaces
Displays
Input
Notifications
Power
System
```

Candidates by section: wallpaper in Desktop; accent color and density in
Appearance (both appear in the mockup); system information and credits under
System.

Ideas taken from the Caelestia settings screen: a search field at the top of the
section list, one line of description under each section name, a scope selector
for settings that apply globally or per monitor, and an About page with system
information, component versions and loaded plugins.

Settings that change window management go through the daemon. Settings is a
client like the Shell and the CLI (ADR 0002).

## 12. Themes

Light, Dark and Auto.

Light (Ivory) direction: ivory, cream, sand, taupe, charcoal. Colors observed in
the mockup, as direction only: #FAF8F2, #EFEBDB, #DCCFB9, #B7A99A, text #3F3A36.

Dark (Night) direction: soft black, graphite, grey, beige, discreet amber. The
mockup shows #242225 as a base.

Sand is a light variant of the same family. The identity stays recognizable in
all themes.

## 13. Window Management dependencies on protocol v1

What the Shell can do today and what it cannot:

| Need | Protocol v1 |
| --- | --- |
| List workspaces with their mode | Yes (`list_workspaces`) |
| Set the policy of a workspace | Yes (`set_workspace_mode`) |
| Apply a mode to real windows | Yes (`apply_workspace_mode`), tiled or floating only |
| List windows with workspace, floating, fullscreen, focused | Yes (`list_windows`) |
| Current workspace | No explicit field. It can be inferred from the focused window only while a window is focused. |
| Minimized state | No |
| Application identity (pinning, icons, grouping) | No |
| Focus, minimize, maximize, close actions | No |
| Change events | No. The Shell would have to poll. |
| Stable tokens across daemon restarts | No. Tokens are valid only while the daemon lives. |
| Monitor layout and per-monitor data | No. The Shell reads monitors from the Wayland session. |
| Show Desktop (hide and restore all windows of a workspace) | No. It needs minimize and restore and the current workspace. |

Closing these gaps requires a new protocol version, with its own ADR.

## 14. Component origin

Each component is classified by origin before work starts. When code or assets
are copied, the file, upstream commit and license are recorded next to the
copy.

Caelestia (reference, possible copy): sidebar, launcher, notifications, quick
settings, workspaces, system status.

Midnight Shell (reference, possible copy): integrated taskbar, application
behavior, window management related elements.

Nycti (own): Window Mode, Tiling Mode per workspace, switching between modes,
minimize and restore integration, unified taskbar behavior, Settings, visual
identity, integration between Shell and daemon.

Technology. Reusing Caelestia components implies the same stack. The Caelestia
settings screen reports Quickshell 0.3.1 and Qt 6.11.2. The proposal (D15) is to
write the Shell with Quickshell and QML, which needs an ADR. How QML talks to the
daemon is open: directly through the Unix socket, or by running `nycti --json`
as a process. The CLI exists already and is the reference client, so the second
option is the safe fallback.

## 15. Licenses and assets

Caelestia, Midnight Shell and Nycti are GPL-3.0. Copying code between them is
compatible only if the exact license of the copied file allows it, so each copy
records its license.

Open: whether Nycti is "GPL-3.0-only" or "GPL-3.0-or-later". If upstream code is
copied under "only", the project cannot claim "or later" for that code. The
choice is made before the first copy enters the repository.

Assets (wallpapers, icons, the mockup art): no asset enters the repository
before its author and license are recorded in the repository. The origin of the
urutau art in the mockup is not recorded yet, so it stays out until it is.

## 16. Screens for the first design round

1. Empty desktop.
2. Desktop with applications open.
3. Workspace in Tiling Mode.
4. The same workspace in Window Mode.
5. Minimized application.
6. Maximized application.
7. Fullscreen application.
8. Launcher.
9. Quick Settings.
10. Notifications.
11. Alt+Tab.
12. Nycti Settings.
13. Light theme.
14. Dark theme.
15. Several workspaces.
16. Two monitors.

## 17. Visual maturity levels

1. Wireframe: where it sits, what it does, what happens on click, how state changes.
2. Proportional layout: approximate proportions and size relations.
3. Visual mockup: colors, typography, icons, blur, borders, shadows, transparency.
4. Design system: official values for sizes, spacing, radius, typography, colors, states and animation.
5. Prototype: clicks, panel opening, workspace change, Tiling to Window, minimize and restore, Settings navigation.

## 18. Open questions

- Sidebar width, final module order, taskbar indicators, title bar look.
- Window selector format for applications with several windows.
- Multiple monitors: sidebar on every monitor or only on the primary; whether the taskbar shows windows from all monitors; whether workspaces are global or per monitor.
- Behavior on small screens.
- Brightness control on external monitors (hardware support per device) and whether an "all monitors" option exists.
- Whether any edge trigger works on shared edges (for example with a delay or a pressure threshold), or only buttons and shortcuts.
- Delay and pinning rules for hover panels.
- Shell technology (D15) and the way QML reaches the daemon.
- Exact meaning of minimize and maximize/restore, and the protocol extension that carries them.
- Whether Nycti should control window geometry when switching to Window Mode, or leave it to Hyprland.
- How new windows are treated in a Window Mode workspace.
- Atomic mode switch in the daemon.
- ADR for Hyprbars or an equivalent title bar.
- ADR for how the Shell is built from Caelestia components (D9, D10). It must state how it amends ADR 0001.
- GPL "only" or "or later", and the license of each visual asset.
- Whether the Home item stays in Navigation, and what it would open.
- What happens to a window opened while Show Desktop is active: restore everything, or keep the desktop shown.
- Whether the Displays icon is shown when there is a single monitor whose brightness cannot be adjusted.
- Animations, definitive palette, typography and iconography.
