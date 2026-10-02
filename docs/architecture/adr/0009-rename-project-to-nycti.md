# ADR 0009: Rename the Project to Nycti

- Status: Accepted
- Date: 2026-10-01

## Context

The maintainer decided to rename the project from CLEA to Nycti. The name CLEA
appears in the Rust package, the binary, the library, the runtime socket
directory, the `status.service` value, the live-test environment variables,
thread names, stderr prefixes, the specifications, and the repository URL.

Some of these names are part of a contract: the socket directory and the
`status.service` value are observable by clients, and the environment variables
are observable by whoever runs the ignored live tests. Changing them is
therefore a contract change and is recorded here instead of being treated as a
mechanical edit.

ADR 0005 left the name of a future CLI open. This ADR does not close it.

## Decision

The project is named Nycti. The names below replace the old ones from the
commits that follow this ADR.

| Item | Old name | New name |
| --- | --- | --- |
| Cargo package and daemon binary | `clea-windowd` | `nycti-windowd` |
| Library crate | `clea_windowd` | `nycti_windowd` |
| Runtime socket directory | `$XDG_RUNTIME_DIR/clea/` | `$XDG_RUNTIME_DIR/nycti/` |
| Socket path | `$XDG_RUNTIME_DIR/clea/window-management.sock` | `$XDG_RUNTIME_DIR/nycti/window-management.sock` |
| `status.service` value | `"clea-windowd"` | `"nycti-windowd"` |
| Live action test variable | `CLEA_LIVE_ACTION_TEST` | `NYCTI_LIVE_ACTION_TEST` |
| Live manager test variable | `CLEA_LIVE_MANAGER_TEST` | `NYCTI_LIVE_MANAGER_TEST` |
| Stderr line prefix (provisional) | `clea-windowd:` | `nycti-windowd:` |
| Repository | `lhenr1que/clea-desktop` | `https://github.com/lhenr1que/nycti` |
| Local folder | `/home/lhen/clea-desktop` | unchanged |

The local folder keeps its name. Renaming it is outside this decision.

### Contract changes

- The runtime directory is `$XDG_RUNTIME_DIR/nycti/`. Its safety rules (owner,
  mode `0700`, no symlink, no regular file in its place) are unchanged.
- `status.service` is `"nycti-windowd"`. The protocol version stays `1`; the
  value of `service` identifies the daemon and is not a protocol feature.
- The live tests are enabled only by `NYCTI_LIVE_ACTION_TEST=1` and
  `NYCTI_LIVE_MANAGER_TEST=1`. The old variables no longer enable any test.

### What stays historical

- ADRs 0001 to 0008 keep their text and cite the old name. Their meaning is
  read through the table above.
- Earlier commit subjects and history are not rewritten.

### Transition rule

- Any daemon started under the old name must be stopped before the new daemon
  is started.
- There is no compatibility layer: no alias for the old socket directory, no
  acceptance of the old environment variables, no old `status.service` value.
- The new daemon does not remove the old `$XDG_RUNTIME_DIR/clea/` directory.
  Removing it is a manual step. The directory lives in a tmpfs and disappears
  at the end of the user session.

### Name notice

A project named Nycti already exists (a Discord bot). No trademark search was
carried out. This ADR makes no claim about the availability of the name.

### CLI name

ADR 0005 left the name of a future CLI open. The name is not set here; ADR
0010 will close it.

## Consequences

- Clients that connect to the old socket path or expect the old
  `status.service` value stop working until updated. There are no known
  clients outside this repository.
- Scripts that export the old live-test variables silently run nothing, because
  the tests stay ignored; they must be updated.
- The specification and the code change together, so they do not diverge at
  any commit.
- Setting `publish = false` in the package manifest prevents an accidental
  publication while the name is not cleared.

## Debts and limitations

- No code protects against two daemons running at the same time, one under the
  old name and one under the new. They use different socket directories and
  both can run against the same Hyprland session; avoiding this is the
  operator's responsibility.
- The stderr prefix remains provisional, as in ADR 0008.
- The brand search for the name Nycti is pending.
