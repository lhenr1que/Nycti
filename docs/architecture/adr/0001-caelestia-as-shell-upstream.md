# ADR 0001: Caelestia as the Shell Upstream

- Status: Accepted
- Date: 2026-09-26

## Context

CLEA needs a clear upstream relationship for its shell. Treating several shells
as equivalent sources would make updates, ownership, and architectural direction
ambiguous.

## Decision

Caelestia is the primary upstream for the CLEA shell. Shell work should preserve
a traceable relationship with Caelestia and avoid unnecessary divergence.

This decision establishes upstream direction only. It does not import Caelestia
or select which revision, integration process, or local adaptations CLEA will
use.

## Consequences

- Shell changes are evaluated for compatibility with ongoing Caelestia
  maintenance.
- CLEA-specific system authority remains outside the shell even if upstream code
  follows a different boundary.
- Import and update procedures must be decided and documented separately before
  upstream code is introduced.
