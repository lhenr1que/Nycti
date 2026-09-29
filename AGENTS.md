# Agent Working Rules

These repository rules apply to Codex, Claude Code, OpenCode, and other agents.

## Before working

- Read [CURRENT_STAGE.md](CURRENT_STAGE.md) before starting work. Confirm the
  current branch and working tree; the recorded baseline is not a substitute
  for inspecting current files.
- Consult [the architecture overview](docs/architecture/overview.md),
  [principles](docs/architecture/principles.md), relevant
  [ADRs](docs/architecture/adr/), and component contracts before changing
  architecture or behavior. Distinguish accepted decisions from implemented work.
- Do not create commits without explicit user authorization.
- Prefer small, verifiable changes separated by responsibility. Preserve
  unrelated work and, when several agents share a checkout, coordinate file
  ownership before editing overlapping files.

## Architectural boundaries

- Preserve the separation of policy, protocol, and backend. `WindowManager`
  owns policy; the service and protocol expose typed client operations; backends
  observe compositor state and apply requested outcomes.
- Keep Hyprland-specific behavior, commands, sockets, selectors, native
  identities, and wire formats inside the Hyprland backend. Shell, Settings,
  and other clients must not own window-management policy.
- Do not change public contracts implicitly. Document proposed changes,
  compatibility implications, and any required protocol version or ADR update
  explicitly as part of the work.
- Do not introduce dependencies without explaining why they are needed and
  why existing dependencies or the standard library are insufficient.

## Verification and handoff

- Run relevant tests after implementation changes. For Rust changes, use the
  package commands recorded in CURRENT_STAGE.md and check formatting and Clippy
  without automatic fixes unless fixes are part of the authorized task.
- Live Hyprland tests are opt-in and may mutate desktop state. Do not enable
  ignored live tests without explicit authorization for that validation.
- For documentation-only changes, check the diff and links; Rust tests are not
  required when no implementation or build configuration changed.
- Report changes, validation results, and remaining limitations. Keep
  CURRENT_STAGE.md accurate when the implemented stage or validated baseline
  changes; do not present planned functionality as implemented.
