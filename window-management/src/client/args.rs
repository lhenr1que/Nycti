//! Command-line grammar of `nycti`, parsed with the standard library only.
//!
//! Commands are one per protocol method, in two groups. The `query` group can
//! only build read methods: it has its own type, separate from the `change`
//! group, so a query can never become a request that changes state. Workspace
//! tokens are passed through exactly as typed; the daemon decides whether they
//! are valid.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::time::Duration;

use crate::protocol::{ProtocolMode, RequestMethod};

/// The text printed by `--help`.
pub const USAGE: &str = "\
Usage:
  nycti [OPTIONS] wm query <COMMAND>
  nycti [OPTIONS] wm change <COMMAND>

Query commands (read only):
  status                          identify the daemon and the protocol version
  get-default-mode                show the default workspace mode
  list-workspaces                 list workspaces with their mode resolution
  get-workspace-mode WORKSPACE    show one workspace's mode resolution
  list-windows                    list the current windows

Change commands:
  set-default-mode MODE           change the daemon's default mode (memory only)
  set-workspace-mode WORKSPACE MODE
                                  set a workspace's explicit mode (memory only)
  clear-workspace-mode WORKSPACE  remove a workspace's explicit mode
  apply-workspace-mode WORKSPACE --yes
                                  apply the effective mode to the real windows
                                  of the workspace; --yes is required

MODE is `tiling` or `windows`. WORKSPACE is a token exactly as printed by
`list-workspaces` (for example `w:2`); it is opaque and only the daemon
decides whether it is valid.

Options (before or after the command):
  --json             print the protocol result as one JSON line
  --timeout SECONDS  how long to wait for the response (default 10)
  --dry-run          print the request that would be sent and do not connect
  --yes              confirm apply-workspace-mode
  -h, --help         print this help

Exit codes: 0 success, 1 internal failure, 2 incorrect usage, 3 daemon
unavailable, 4 the daemon answered with an error, 5 communication failure.
The output and the exit codes are provisional.
";

/// A command of the read-only `query` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryCommand {
    Status,
    GetDefaultMode,
    ListWorkspaces,
    GetWorkspaceMode { workspace_id: String },
    ListWindows,
}

impl QueryCommand {
    /// Returns the protocol method; always one of the five read methods.
    pub fn method(&self) -> RequestMethod {
        match self {
            Self::Status => RequestMethod::Status,
            Self::GetDefaultMode => RequestMethod::GetDefaultMode,
            Self::ListWorkspaces => RequestMethod::ListWorkspaces,
            Self::GetWorkspaceMode { workspace_id } => RequestMethod::GetWorkspaceMode {
                workspace_id: workspace_id.clone(),
            },
            Self::ListWindows => RequestMethod::ListWindows,
        }
    }
}

/// A command of the `change` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeCommand {
    SetDefaultMode {
        mode: ProtocolMode,
    },
    SetWorkspaceMode {
        workspace_id: String,
        mode: ProtocolMode,
    },
    ClearWorkspaceMode {
        workspace_id: String,
    },
    /// Changes real windows. Parsing succeeds only when `--yes` was given.
    ApplyWorkspaceMode {
        workspace_id: String,
    },
}

impl ChangeCommand {
    /// Returns the protocol method.
    pub fn method(&self) -> RequestMethod {
        match self {
            Self::SetDefaultMode { mode } => RequestMethod::SetDefaultMode { mode: *mode },
            Self::SetWorkspaceMode { workspace_id, mode } => RequestMethod::SetWorkspaceMode {
                workspace_id: workspace_id.clone(),
                mode: *mode,
            },
            Self::ClearWorkspaceMode { workspace_id } => RequestMethod::ClearWorkspaceMode {
                workspace_id: workspace_id.clone(),
            },
            Self::ApplyWorkspaceMode { workspace_id } => RequestMethod::ApplyWorkspaceMode {
                workspace_id: workspace_id.clone(),
            },
        }
    }
}

/// Any command of the CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Query(QueryCommand),
    Change(ChangeCommand),
}

impl Command {
    /// Returns the protocol method this command sends.
    pub fn method(&self) -> RequestMethod {
        match self {
            Self::Query(command) => command.method(),
            Self::Change(command) => command.method(),
        }
    }

    /// Returns whether the command belongs to the `change` group.
    pub const fn is_change(&self) -> bool {
        matches!(self, Self::Change(_))
    }
}

/// Options that apply to every command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub json: bool,
    pub dry_run: bool,
    /// How long to wait for the response; `None` keeps the default.
    pub timeout: Option<Duration>,
}

/// A complete, validated invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub options: Options,
}

/// The result of parsing the arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Run(Invocation),
}

/// Incorrect usage of the command line. The process exit code is 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageError {
    NotUtf8,
    /// The command is incomplete (nothing, `wm`, or a group without a command).
    MissingCommand,
    UnknownCommand(String),
    UnknownOption(String),
    MissingArgument(&'static str),
    UnexpectedArgument(String),
    InvalidMode(String),
    MissingOptionValue(&'static str),
    InvalidTimeout(String),
    DuplicateOption(&'static str),
    /// `--yes` was given to a command other than `apply-workspace-mode`.
    YesNotApplicable,
    /// `apply-workspace-mode` changes real windows and needs `--yes`.
    ConfirmationRequired,
}

impl UsageError {
    /// Returns the machine-readable code used in the `--json` error line.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ConfirmationRequired => "confirmation_required",
            _ => "usage_error",
        }
    }
}

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => formatter.write_str("an argument is not valid UTF-8"),
            Self::MissingCommand => formatter.write_str("the command is incomplete"),
            Self::UnknownCommand(word) => write!(formatter, "unknown command `{word}`"),
            Self::UnknownOption(option) => write!(formatter, "unknown option `{option}`"),
            Self::MissingArgument(name) => write!(formatter, "missing argument {name}"),
            Self::UnexpectedArgument(argument) => {
                write!(formatter, "unexpected argument `{argument}`")
            }
            Self::InvalidMode(mode) => write!(
                formatter,
                "invalid mode `{mode}`; it must be `tiling` or `windows`"
            ),
            Self::MissingOptionValue(option) => {
                write!(formatter, "option {option} needs a value")
            }
            Self::InvalidTimeout(value) => write!(
                formatter,
                "invalid timeout `{value}`; it must be a positive number of seconds"
            ),
            Self::DuplicateOption(option) => {
                write!(formatter, "option {option} was given more than once")
            }
            Self::YesNotApplicable => {
                formatter.write_str("--yes applies only to apply-workspace-mode")
            }
            Self::ConfirmationRequired => formatter.write_str(
                "apply-workspace-mode changes the placement of real windows; \
                 repeat it with --yes to confirm. Nothing was sent",
            ),
        }
    }
}

impl std::error::Error for UsageError {}

/// Parses the arguments that follow the program name.
///
/// `--help` anywhere wins over every other check. Options may appear before or
/// after the command words.
pub fn parse(args: &[OsString]) -> Result<Parsed, UsageError> {
    if args
        .iter()
        .any(|arg| arg == OsStr::new("--help") || arg == OsStr::new("-h"))
    {
        return Ok(Parsed::Help);
    }

    let mut options = Options::default();
    let mut yes = false;
    let mut words = Vec::new();
    let mut remaining = args.iter();

    while let Some(arg) = remaining.next() {
        let arg = arg.to_str().ok_or(UsageError::NotUtf8)?;
        match arg {
            "--json" => options.json = true,
            "--dry-run" => options.dry_run = true,
            "--yes" => yes = true,
            "--timeout" => {
                if options.timeout.is_some() {
                    return Err(UsageError::DuplicateOption("--timeout"));
                }
                let value = remaining
                    .next()
                    .ok_or(UsageError::MissingOptionValue("--timeout"))?;
                let value = value.to_str().ok_or(UsageError::NotUtf8)?;
                options.timeout = Some(parse_timeout(value)?);
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return Err(UsageError::UnknownOption(arg.to_owned()));
            }
            _ => words.push(arg),
        }
    }

    let mut words = words.into_iter();
    match words.next() {
        None => return Err(UsageError::MissingCommand),
        Some("wm") => {}
        Some(other) => return Err(UsageError::UnknownCommand(other.to_owned())),
    }
    let group = words.next().ok_or(UsageError::MissingCommand)?;
    let name = match group {
        "query" | "change" => words.next().ok_or(UsageError::MissingCommand)?,
        other => return Err(UsageError::UnknownCommand(other.to_owned())),
    };

    let command = if group == "query" {
        Command::Query(parse_query(name, &mut words)?)
    } else {
        Command::Change(parse_change(name, &mut words)?)
    };
    if let Some(extra) = words.next() {
        return Err(UsageError::UnexpectedArgument(extra.to_owned()));
    }

    let applies = matches!(
        command,
        Command::Change(ChangeCommand::ApplyWorkspaceMode { .. })
    );
    match (applies, yes) {
        (true, false) => Err(UsageError::ConfirmationRequired),
        (false, true) => Err(UsageError::YesNotApplicable),
        _ => Ok(Parsed::Run(Invocation { command, options })),
    }
}

fn parse_query<'a>(
    name: &str,
    words: &mut impl Iterator<Item = &'a str>,
) -> Result<QueryCommand, UsageError> {
    match name {
        "status" => Ok(QueryCommand::Status),
        "get-default-mode" => Ok(QueryCommand::GetDefaultMode),
        "list-workspaces" => Ok(QueryCommand::ListWorkspaces),
        "get-workspace-mode" => Ok(QueryCommand::GetWorkspaceMode {
            workspace_id: workspace(words)?,
        }),
        "list-windows" => Ok(QueryCommand::ListWindows),
        other => Err(UsageError::UnknownCommand(other.to_owned())),
    }
}

fn parse_change<'a>(
    name: &str,
    words: &mut impl Iterator<Item = &'a str>,
) -> Result<ChangeCommand, UsageError> {
    match name {
        "set-default-mode" => Ok(ChangeCommand::SetDefaultMode { mode: mode(words)? }),
        "set-workspace-mode" => {
            let workspace_id = workspace(words)?;
            Ok(ChangeCommand::SetWorkspaceMode {
                workspace_id,
                mode: mode(words)?,
            })
        }
        "clear-workspace-mode" => Ok(ChangeCommand::ClearWorkspaceMode {
            workspace_id: workspace(words)?,
        }),
        "apply-workspace-mode" => Ok(ChangeCommand::ApplyWorkspaceMode {
            workspace_id: workspace(words)?,
        }),
        other => Err(UsageError::UnknownCommand(other.to_owned())),
    }
}

/// The token is passed through as typed, even when it is empty.
fn workspace<'a>(words: &mut impl Iterator<Item = &'a str>) -> Result<String, UsageError> {
    words
        .next()
        .map(str::to_owned)
        .ok_or(UsageError::MissingArgument("WORKSPACE"))
}

fn mode<'a>(words: &mut impl Iterator<Item = &'a str>) -> Result<ProtocolMode, UsageError> {
    match words.next() {
        None => Err(UsageError::MissingArgument("MODE")),
        Some("tiling") => Ok(ProtocolMode::Tiling),
        Some("windows") => Ok(ProtocolMode::Windows),
        Some(other) => Err(UsageError::InvalidMode(other.to_owned())),
    }
}

fn parse_timeout(value: &str) -> Result<Duration, UsageError> {
    value
        .parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .filter(|timeout| !timeout.is_zero())
        .ok_or_else(|| UsageError::InvalidTimeout(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Vec<OsString> {
        words.split_whitespace().map(OsString::from).collect()
    }

    fn run(command: Command) -> Result<Parsed, UsageError> {
        Ok(Parsed::Run(Invocation {
            command,
            options: Options::default(),
        }))
    }

    fn query(command: QueryCommand) -> Command {
        Command::Query(command)
    }

    fn change(command: ChangeCommand) -> Command {
        Command::Change(command)
    }

    fn token(text: &str) -> String {
        text.to_owned()
    }

    #[test]
    fn valid_command_lines_parse_to_their_command() {
        let cases = [
            ("wm query status", query(QueryCommand::Status)),
            (
                "wm query get-default-mode",
                query(QueryCommand::GetDefaultMode),
            ),
            (
                "wm query list-workspaces",
                query(QueryCommand::ListWorkspaces),
            ),
            (
                "wm query get-workspace-mode w:2",
                query(QueryCommand::GetWorkspaceMode {
                    workspace_id: token("w:2"),
                }),
            ),
            ("wm query list-windows", query(QueryCommand::ListWindows)),
            (
                "wm change set-default-mode windows",
                change(ChangeCommand::SetDefaultMode {
                    mode: ProtocolMode::Windows,
                }),
            ),
            (
                "wm change set-default-mode tiling",
                change(ChangeCommand::SetDefaultMode {
                    mode: ProtocolMode::Tiling,
                }),
            ),
            (
                "wm change set-workspace-mode w:2 windows",
                change(ChangeCommand::SetWorkspaceMode {
                    workspace_id: token("w:2"),
                    mode: ProtocolMode::Windows,
                }),
            ),
            (
                "wm change clear-workspace-mode w:2",
                change(ChangeCommand::ClearWorkspaceMode {
                    workspace_id: token("w:2"),
                }),
            ),
            (
                "wm change apply-workspace-mode w:2 --yes",
                change(ChangeCommand::ApplyWorkspaceMode {
                    workspace_id: token("w:2"),
                }),
            ),
        ];

        for (line, command) in cases {
            assert_eq!(parse(&args(line)), run(command), "line: {line}");
        }
    }

    #[test]
    fn tokens_are_passed_through_exactly_as_typed() {
        for text in ["w:2", "2", "win:7", "anything", "W:2"] {
            let parsed = parse(&args(&format!("wm query get-workspace-mode {text}")));

            assert_eq!(
                parsed,
                run(query(QueryCommand::GetWorkspaceMode {
                    workspace_id: token(text)
                }))
            );
        }
        let empty = [
            OsString::from("wm"),
            OsString::from("query"),
            OsString::from("get-workspace-mode"),
            OsString::from(""),
        ];
        assert_eq!(
            parse(&empty),
            run(query(QueryCommand::GetWorkspaceMode {
                workspace_id: String::new()
            }))
        );
    }

    #[test]
    fn options_are_accepted_before_and_after_the_command() {
        let expected = Ok(Parsed::Run(Invocation {
            command: query(QueryCommand::ListWindows),
            options: Options {
                json: true,
                dry_run: true,
                timeout: Some(Duration::from_secs(3)),
            },
        }));

        for line in [
            "--json --dry-run --timeout 3 wm query list-windows",
            "wm query list-windows --json --dry-run --timeout 3",
            "--json wm --timeout 3 query list-windows --dry-run",
            "wm --dry-run query --json list-windows --timeout 3",
        ] {
            assert_eq!(parse(&args(line)), expected, "line: {line}");
        }
    }

    #[test]
    fn yes_is_accepted_before_and_after_apply() {
        for line in [
            "--yes wm change apply-workspace-mode w:1",
            "wm --yes change apply-workspace-mode w:1",
            "wm change apply-workspace-mode --yes w:1",
            "wm change apply-workspace-mode w:1 --yes",
        ] {
            assert_eq!(
                parse(&args(line)),
                run(change(ChangeCommand::ApplyWorkspaceMode {
                    workspace_id: token("w:1")
                })),
                "line: {line}"
            );
        }
    }

    #[test]
    fn fractional_timeouts_are_accepted() {
        let parsed = parse(&args("wm query status --timeout 0.5"));

        assert_eq!(
            parsed,
            Ok(Parsed::Run(Invocation {
                command: query(QueryCommand::Status),
                options: Options {
                    timeout: Some(Duration::from_millis(500)),
                    ..Options::default()
                },
            }))
        );
    }

    #[test]
    fn invalid_command_lines_are_usage_errors() {
        let cases = [
            ("", UsageError::MissingCommand),
            ("wm", UsageError::MissingCommand),
            ("wm query", UsageError::MissingCommand),
            ("wm change", UsageError::MissingCommand),
            ("status", UsageError::UnknownCommand(token("status"))),
            ("wm toggle", UsageError::UnknownCommand(token("toggle"))),
            (
                "wm query toggle",
                UsageError::UnknownCommand(token("toggle")),
            ),
            (
                "wm query set-default-mode windows",
                UsageError::UnknownCommand(token("set-default-mode")),
            ),
            (
                "wm change status",
                UsageError::UnknownCommand(token("status")),
            ),
            (
                "wm change toggle w:1",
                UsageError::UnknownCommand(token("toggle")),
            ),
            (
                "wm query get-workspace-mode",
                UsageError::MissingArgument("WORKSPACE"),
            ),
            (
                "wm change set-workspace-mode",
                UsageError::MissingArgument("WORKSPACE"),
            ),
            (
                "wm change set-workspace-mode w:1",
                UsageError::MissingArgument("MODE"),
            ),
            (
                "wm change set-default-mode",
                UsageError::MissingArgument("MODE"),
            ),
            (
                "wm change clear-workspace-mode",
                UsageError::MissingArgument("WORKSPACE"),
            ),
            (
                "wm change apply-workspace-mode --yes",
                UsageError::MissingArgument("WORKSPACE"),
            ),
            (
                "wm change set-default-mode floating",
                UsageError::InvalidMode(token("floating")),
            ),
            (
                "wm change set-default-mode Windows",
                UsageError::InvalidMode(token("Windows")),
            ),
            (
                "wm change set-workspace-mode w:1 tiled",
                UsageError::InvalidMode(token("tiled")),
            ),
            (
                "wm query status extra",
                UsageError::UnexpectedArgument(token("extra")),
            ),
            (
                "wm query list-windows w:1",
                UsageError::UnexpectedArgument(token("w:1")),
            ),
            (
                "wm query get-workspace-mode w:1 w:2",
                UsageError::UnexpectedArgument(token("w:2")),
            ),
            (
                "wm change set-workspace-mode w:1 windows now",
                UsageError::UnexpectedArgument(token("now")),
            ),
            (
                "wm change apply-workspace-mode w:1 w:2 --yes",
                UsageError::UnexpectedArgument(token("w:2")),
            ),
            (
                "wm query status --verbose",
                UsageError::UnknownOption(token("--verbose")),
            ),
            ("wm query status -j", UsageError::UnknownOption(token("-j"))),
            (
                "wm query status --timeout",
                UsageError::MissingOptionValue("--timeout"),
            ),
            (
                "wm query status --timeout abc",
                UsageError::InvalidTimeout(token("abc")),
            ),
            (
                "wm query status --timeout 0",
                UsageError::InvalidTimeout(token("0")),
            ),
            (
                "wm query status --timeout -1",
                UsageError::InvalidTimeout(token("-1")),
            ),
            (
                "wm query status --timeout inf",
                UsageError::InvalidTimeout(token("inf")),
            ),
            (
                "wm query status --timeout NaN",
                UsageError::InvalidTimeout(token("NaN")),
            ),
            (
                "wm query status --timeout 1 --timeout 2",
                UsageError::DuplicateOption("--timeout"),
            ),
            ("wm query status --yes", UsageError::YesNotApplicable),
            (
                "wm change set-default-mode windows --yes",
                UsageError::YesNotApplicable,
            ),
        ];

        for (line, expected) in cases {
            assert_eq!(parse(&args(line)), Err(expected), "line: {line}");
        }
    }

    #[test]
    fn apply_without_yes_is_refused_even_with_dry_run() {
        for line in [
            "wm change apply-workspace-mode w:1",
            "wm change apply-workspace-mode w:1 --dry-run",
            "--json wm change apply-workspace-mode w:1",
        ] {
            assert_eq!(
                parse(&args(line)),
                Err(UsageError::ConfirmationRequired),
                "line: {line}"
            );
        }
        assert_eq!(
            UsageError::ConfirmationRequired.code(),
            "confirmation_required"
        );
        assert_eq!(UsageError::MissingCommand.code(), "usage_error");
    }

    #[test]
    fn apply_with_dry_run_and_yes_is_accepted() {
        let parsed = parse(&args("wm change apply-workspace-mode w:1 --yes --dry-run"));

        assert_eq!(
            parsed,
            Ok(Parsed::Run(Invocation {
                command: change(ChangeCommand::ApplyWorkspaceMode {
                    workspace_id: token("w:1")
                }),
                options: Options {
                    dry_run: true,
                    ..Options::default()
                },
            }))
        );
    }

    #[test]
    fn help_wins_over_every_other_check() {
        for line in [
            "--help",
            "-h",
            "wm --help",
            "wm query status --help",
            "wm change apply-workspace-mode w:1 --help",
            "nonsense --bogus --help",
            "--help --timeout",
        ] {
            assert_eq!(parse(&args(line)), Ok(Parsed::Help), "line: {line}");
        }
    }

    #[test]
    fn an_argument_that_is_not_utf8_is_a_usage_error() {
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![0x66, 0xff]);

        assert_eq!(
            parse(&[OsString::from("wm"), invalid.clone()]),
            Err(UsageError::NotUtf8)
        );
        assert_eq!(
            parse(&[OsString::from("--timeout"), invalid]),
            Err(UsageError::NotUtf8)
        );
    }

    #[test]
    fn every_query_command_builds_only_a_read_method() {
        let queries = [
            QueryCommand::Status,
            QueryCommand::GetDefaultMode,
            QueryCommand::ListWorkspaces,
            QueryCommand::GetWorkspaceMode {
                workspace_id: token("w:1"),
            },
            QueryCommand::ListWindows,
        ];

        for command in queries {
            assert!(
                matches!(
                    command.method(),
                    RequestMethod::Status
                        | RequestMethod::GetDefaultMode
                        | RequestMethod::ListWorkspaces
                        | RequestMethod::GetWorkspaceMode { .. }
                        | RequestMethod::ListWindows
                ),
                "{command:?} must build a read method"
            );
        }
    }

    #[test]
    fn every_change_command_builds_its_own_method() {
        let windows = ProtocolMode::Windows;
        let cases = [
            (
                ChangeCommand::SetDefaultMode { mode: windows },
                RequestMethod::SetDefaultMode { mode: windows },
            ),
            (
                ChangeCommand::SetWorkspaceMode {
                    workspace_id: token("w:1"),
                    mode: windows,
                },
                RequestMethod::SetWorkspaceMode {
                    workspace_id: token("w:1"),
                    mode: windows,
                },
            ),
            (
                ChangeCommand::ClearWorkspaceMode {
                    workspace_id: token("w:1"),
                },
                RequestMethod::ClearWorkspaceMode {
                    workspace_id: token("w:1"),
                },
            ),
            (
                ChangeCommand::ApplyWorkspaceMode {
                    workspace_id: token("w:1"),
                },
                RequestMethod::ApplyWorkspaceMode {
                    workspace_id: token("w:1"),
                },
            ),
        ];

        for (command, method) in cases {
            assert!(Command::Change(command.clone()).is_change());
            assert_eq!(command.method(), method);
        }
        assert!(!Command::Query(QueryCommand::Status).is_change());
    }

    #[test]
    fn the_usage_text_lists_every_command() {
        for command in [
            "status",
            "get-default-mode",
            "list-workspaces",
            "get-workspace-mode",
            "list-windows",
            "set-default-mode",
            "set-workspace-mode",
            "clear-workspace-mode",
            "apply-workspace-mode",
            "--json",
            "--timeout",
            "--dry-run",
            "--yes",
        ] {
            assert!(USAGE.contains(command), "usage must mention {command}");
        }
    }
}
