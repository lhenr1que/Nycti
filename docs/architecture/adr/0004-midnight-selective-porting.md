# ADR 0004: Selective Porting from Midnight Shell

- Status: Accepted
- Date: 2026-09-26

## Context

Midnight Shell contains ideas and features that may be useful to CLEA, but using
it as a second upstream or combining it wholesale with Caelestia would blur
ownership, increase integration risk, and undermine CLEA's component boundaries.

## Decision

Midnight Shell is a secondary reference only. CLEA may port individual features
selectively, but it will not merge or copy Midnight Shell wholesale.

Before a non-trivial feature is ported, its desired behavior, architectural fit,
provenance, and maintenance implications must be reviewed. An accepted port must
respect CLEA's client and backend boundaries and include documentation and
tests.

## Consequences

- Midnight-derived work is proposed and evaluated feature by feature.
- Caelestia remains the single primary shell upstream.
- Features that do not fit CLEA's architecture are redesigned or rejected
  rather than imported with incompatible coupling.
- No Midnight code is introduced by this decision.
