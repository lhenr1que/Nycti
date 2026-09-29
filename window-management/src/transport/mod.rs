//! Synchronous JSON Lines framing over generic byte streams.

use std::convert::Infallible;
use std::fmt;
use std::io::{BufRead, Write};

use crate::backend::WindowBackend;
use crate::service::WindowManagementService;

/// An operational failure while serving one framed connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    ReadFailed,
    WriteFailed,
    FlushFailed,
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ReadFailed => "failed to read protocol connection",
            Self::WriteFailed => "failed to write protocol response",
            Self::FlushFailed => "failed to flush protocol response",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TransportError {}

/// A failure while serving one framed connection through a request handler.
///
/// The two domains stay separate: `Transport` is a framing or stream failure of
/// this connection, and `Handler` is the handler's own failure, which is never
/// converted into a protocol response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServeError<E> {
    Transport(TransportError),
    Handler(E),
}

impl<E: fmt::Display> fmt::Display for ServeError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::Handler(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for ServeError<E> {}

/// Serves sequential requests over one generic readable/writable pair by
/// invoking `handler` once per complete request line.
///
/// This is the only JSON Lines framing implementation. Framing is
/// byte-oriented and removes only the terminating LF byte. A final sequence of
/// bytes without LF is considered incomplete and is discarded at clean EOF
/// without calling the handler or writing a response. No line-size limit is
/// imposed at this layer.
///
/// The handler receives the exact request bytes without the LF and does not
/// interpret framing. On success it returns exactly one complete response line
/// terminated by LF. The transport writes those bytes unchanged and does not
/// add, remove, or validate the terminator.
///
/// If the handler fails, no response is written, serving stops, and
/// `ServeError::Handler` is returned. Read, write, and flush failures are
/// reported as `ServeError::Transport`.
pub fn serve_connection_with_handler<R, W, H, E>(
    mut reader: R,
    mut writer: W,
    mut handler: H,
) -> Result<(), ServeError<E>>
where
    R: BufRead,
    W: Write,
    H: FnMut(&[u8]) -> Result<String, E>,
{
    let mut request_line = Vec::new();

    loop {
        request_line.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut request_line)
            .map_err(|_| ServeError::Transport(TransportError::ReadFailed))?;

        if bytes_read == 0 {
            return Ok(());
        }

        if request_line.last() != Some(&b'\n') {
            return Ok(());
        }
        request_line.pop();

        let response_line = handler(&request_line).map_err(ServeError::Handler)?;
        writer
            .write_all(response_line.as_bytes())
            .map_err(|_| ServeError::Transport(TransportError::WriteFailed))?;
        writer
            .flush()
            .map_err(|_| ServeError::Transport(TransportError::FlushFailed))?;
    }
}

/// Serves sequential protocol requests over one generic readable/writable pair.
///
/// Wrapper over [`serve_connection_with_handler`] with a handler that never
/// fails; framing behavior is defined there.
pub fn serve_connection<R, W, B>(
    reader: R,
    writer: W,
    service: &mut WindowManagementService<B>,
) -> Result<(), TransportError>
where
    R: BufRead,
    W: Write,
    B: WindowBackend,
{
    serve_connection_with_handler(reader, writer, |request_line| {
        Ok::<_, Infallible>(service.handle_json_bytes(request_line))
    })
    .map_err(|error| match error {
        ServeError::Transport(error) => error,
        ServeError::Handler(never) => match never {},
    })
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufReader, Cursor, Read};

    use serde_json::{Value, json};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::core::{WindowManager, WindowPlacement, WorkspaceMode};

    fn service_with_backend(backend: FakeBackend) -> WindowManagementService<FakeBackend> {
        WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling))
    }

    fn request(id: &str, method: &str, params: Value) -> String {
        format!(
            "{}\n",
            json!({"version": 1, "id": id, "method": method, "params": params})
        )
    }

    fn run_connection(input: &[u8], service: &mut WindowManagementService<FakeBackend>) -> Vec<u8> {
        let mut output = Vec::new();
        serve_connection(Cursor::new(input), &mut output, service)
            .expect("in-memory connection should succeed");
        output
    }

    fn response_values(output: &[u8]) -> Vec<Value> {
        let output = std::str::from_utf8(output).expect("responses should be UTF-8");
        output
            .split_terminator('\n')
            .map(|line| serde_json::from_str(line).expect("response line should be JSON"))
            .collect()
    }

    #[test]
    fn valid_status_request_produces_one_response() {
        let mut service = service_with_backend(FakeBackend::new());
        let output = run_connection(
            request("status-1", "status", json!({})).as_bytes(),
            &mut service,
        );
        let responses = response_values(&output);

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], "status-1");
        assert_eq!(responses[0]["ok"], true);
        assert_eq!(responses[0]["result"]["service"], "clea-windowd");
    }

    #[test]
    fn multiple_requests_preserve_order_correlation_and_exact_lf_framing() {
        let mut service = service_with_backend(FakeBackend::new());
        let input = format!(
            "{}{}",
            request("first", "status", json!({})),
            request("second", "get_default_mode", json!({}))
        );

        let output = run_connection(input.as_bytes(), &mut service);
        let responses = response_values(&output);

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], "first");
        assert_eq!(responses[1]["id"], "second");
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 2);
        assert!(output.ends_with(b"\n"));
        assert!(!output.windows(2).any(|bytes| bytes == b"\n\n"));
    }

    #[test]
    fn invalid_json_produces_error_and_connection_continues() {
        let mut service = service_with_backend(FakeBackend::new());
        let input = format!("bad-json\n{}", request("valid", "status", json!({})));

        let responses = response_values(&run_connection(input.as_bytes(), &mut service));

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[0]["error"]["code"], "invalid_request");
        assert_eq!(responses[1]["id"], "valid");
        assert_eq!(responses[1]["ok"], true);
    }

    #[test]
    fn empty_line_is_a_complete_invalid_request() {
        let mut service = service_with_backend(FakeBackend::new());

        let responses = response_values(&run_connection(b"\n", &mut service));

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[0]["error"]["code"], "invalid_request");
    }

    #[test]
    fn protocol_error_does_not_end_connection() {
        let mut service = service_with_backend(FakeBackend::new());
        let input = format!(
            "{}{}",
            request("invalid", "set_default_mode", json!({"mode": "floating"})),
            request("valid", "get_default_mode", json!({}))
        );

        let responses = response_values(&run_connection(input.as_bytes(), &mut service));

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], "invalid");
        assert_eq!(responses[0]["error"]["code"], "invalid_params");
        assert_eq!(responses[1]["id"], "valid");
        assert_eq!(responses[1]["ok"], true);
    }

    #[test]
    fn unsupported_version_does_not_end_connection() {
        let mut service = service_with_backend(FakeBackend::new());
        let input = format!(
            "{}\n{}",
            json!({"version": 2, "id": "future", "method": "status", "params": {}}),
            request("valid", "status", json!({}))
        );

        let responses = response_values(&run_connection(input.as_bytes(), &mut service));

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], "future");
        assert_eq!(responses[0]["error"]["code"], "unsupported_version");
        assert_eq!(responses[1]["id"], "valid");
        assert_eq!(responses[1]["ok"], true);
    }

    #[test]
    fn clean_eof_without_requests_succeeds_without_response() {
        let mut service = service_with_backend(FakeBackend::new());

        let output = run_connection(b"", &mut service);

        assert!(output.is_empty());
    }

    #[test]
    fn final_line_without_lf_is_discarded_without_response() {
        let mut service = service_with_backend(FakeBackend::new());
        let incomplete = json!({
            "version": 1,
            "id": "incomplete",
            "method": "status",
            "params": {}
        })
        .to_string();

        let output = run_connection(incomplete.as_bytes(), &mut service);

        assert!(output.is_empty());
    }

    #[test]
    fn complete_request_before_incomplete_final_line_gets_only_one_response() {
        let mut service = service_with_backend(FakeBackend::new());
        let input = format!(
            "{}{}",
            request("complete", "status", json!({})),
            json!({"version": 1, "id": "incomplete", "method": "status", "params": {}})
        );

        let responses = response_values(&run_connection(input.as_bytes(), &mut service));

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], "complete");
    }

    #[test]
    fn sequential_requests_share_service_state() {
        let mut service = service_with_backend(FakeBackend::new());
        let input = format!(
            "{}{}",
            request("set", "set_default_mode", json!({"mode": "windows"})),
            request("get", "get_default_mode", json!({}))
        );

        let responses = response_values(&run_connection(input.as_bytes(), &mut service));

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["result"]["mode"], "windows");
        assert_eq!(responses[1]["id"], "get");
        assert_eq!(responses[1]["result"]["mode"], "windows");
    }

    #[test]
    fn workspace_set_apply_and_window_list_flow_through_framing() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend);

        let listed = response_values(&run_connection(
            request("list-workspaces", "list_workspaces", json!({})).as_bytes(),
            &mut service,
        ));
        let workspace_token = listed[0]["result"]["workspaces"][0]["workspace_id"]
            .as_str()
            .expect("workspace token should be text")
            .to_owned();
        let input = format!(
            "{}{}{}",
            request(
                "set",
                "set_workspace_mode",
                json!({"workspace_id": workspace_token, "mode": "windows"})
            ),
            request(
                "apply",
                "apply_workspace_mode",
                json!({"workspace_id": workspace_token})
            ),
            request("windows", "list_windows", json!({}))
        );

        let responses = response_values(&run_connection(input.as_bytes(), &mut service));

        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], "set");
        assert_eq!(responses[0]["result"]["effective_mode"], "windows");
        assert_eq!(responses[1]["id"], "apply");
        assert_eq!(responses[1]["result"]["effective_mode"], "windows");
        assert_eq!(responses[2]["id"], "windows");
        assert_eq!(
            responses[2]["result"]["windows"][0]["placement"],
            "floating"
        );
    }

    #[test]
    fn invalid_utf8_is_protocol_error_and_connection_continues() {
        let mut service = service_with_backend(FakeBackend::new());
        let mut input = vec![0xff, b'\n'];
        input.extend_from_slice(request("valid", "status", json!({})).as_bytes());

        let responses = response_values(&run_connection(&input, &mut service));

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[0]["error"]["code"], "invalid_request");
        assert_eq!(responses[1]["id"], "valid");
        assert_eq!(responses[1]["ok"], true);
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("intentional read failure"))
        }
    }

    #[test]
    fn read_failure_returns_transport_error() {
        let reader = BufReader::new(FailingReader);
        let mut output = Vec::new();
        let mut service = service_with_backend(FakeBackend::new());

        let result = serve_connection(reader, &mut output, &mut service);

        assert_eq!(result, Err(TransportError::ReadFailed));
        assert!(output.is_empty());
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("intentional write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_failure_returns_transport_error() {
        let reader = Cursor::new(request("status", "status", json!({})).into_bytes());
        let mut service = service_with_backend(FakeBackend::new());

        let result = serve_connection(reader, FailingWriter, &mut service);

        assert_eq!(result, Err(TransportError::WriteFailed));
    }

    #[derive(Default)]
    struct FlushFailingWriter {
        bytes: Vec<u8>,
    }

    impl Write for FlushFailingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("intentional flush failure"))
        }
    }

    #[test]
    fn flush_failure_returns_transport_error() {
        let reader = Cursor::new(request("status", "status", json!({})).into_bytes());
        let mut writer = FlushFailingWriter::default();
        let mut service = service_with_backend(FakeBackend::new());

        let result = serve_connection(reader, &mut writer, &mut service);

        assert_eq!(result, Err(TransportError::FlushFailed));
        assert!(writer.bytes.ends_with(b"\n"));
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct HandlerFailure;

    fn echo_length(line: &[u8]) -> Result<String, HandlerFailure> {
        Ok(format!("{}\n", line.len()))
    }

    #[test]
    fn handler_is_called_once_per_complete_line_with_exact_bytes_without_lf() {
        let input: &[u8] = b"first\n\n\xff\r\nlast\n";
        let mut received: Vec<Vec<u8>> = Vec::new();
        let mut output = Vec::new();

        let result = serve_connection_with_handler(Cursor::new(input), &mut output, |line| {
            received.push(line.to_vec());
            echo_length(line)
        });

        assert_eq!(result, Ok(()));
        assert_eq!(
            received,
            vec![
                b"first".to_vec(),
                b"".to_vec(),
                b"\xff\r".to_vec(),
                b"last".to_vec()
            ]
        );
        assert_eq!(output, b"5\n0\n2\n4\n");
    }

    #[test]
    fn handler_response_bytes_are_written_unchanged() {
        let mut output = Vec::new();

        let result = serve_connection_with_handler(Cursor::new(b"a\nb\n"), &mut output, |line| {
            Ok::<_, HandlerFailure>(if line == b"a" {
                "no-terminator".to_owned()
            } else {
                "done\n".to_owned()
            })
        });

        assert_eq!(result, Ok(()));
        assert_eq!(output, b"no-terminatordone\n");
    }

    #[test]
    fn handler_is_not_called_at_clean_eof() {
        let mut calls = 0;
        let mut output = Vec::new();

        let result = serve_connection_with_handler(Cursor::new(b""), &mut output, |line| {
            calls += 1;
            echo_length(line)
        });

        assert_eq!(result, Ok(()));
        assert_eq!(calls, 0);
        assert!(output.is_empty());
    }

    #[test]
    fn handler_is_not_called_for_final_line_without_lf() {
        let mut received: Vec<Vec<u8>> = Vec::new();
        let mut output = Vec::new();

        let result = serve_connection_with_handler(
            Cursor::new(b"complete\nincomplete"),
            &mut output,
            |line| {
                received.push(line.to_vec());
                echo_length(line)
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(received, vec![b"complete".to_vec()]);
        assert_eq!(output, b"8\n");
    }

    #[test]
    fn handler_error_writes_no_response_and_returns_handler_variant() {
        let mut calls = 0;
        let mut output = Vec::new();

        let result = serve_connection_with_handler(Cursor::new(b"one\ntwo\n"), &mut output, |_| {
            calls += 1;
            Err::<String, _>(HandlerFailure)
        });

        assert_eq!(result, Err(ServeError::Handler(HandlerFailure)));
        assert_eq!(calls, 1);
        assert!(output.is_empty());
    }

    #[test]
    fn handler_error_keeps_earlier_responses_and_stops_serving() {
        let mut calls = 0;
        let mut output = Vec::new();

        let result =
            serve_connection_with_handler(Cursor::new(b"ok\nfail\nnever\n"), &mut output, |line| {
                calls += 1;
                if line == b"fail" {
                    Err(HandlerFailure)
                } else {
                    echo_length(line)
                }
            });

        assert_eq!(result, Err(ServeError::Handler(HandlerFailure)));
        assert_eq!(calls, 2);
        assert_eq!(output, b"2\n");
    }

    #[test]
    fn handler_error_is_distinct_from_transport_errors() {
        let mut output = Vec::new();

        let result = serve_connection_with_handler(Cursor::new(b"x\n"), &mut output, |_| {
            Err::<String, _>(HandlerFailure)
        });

        for transport_error in [
            TransportError::ReadFailed,
            TransportError::WriteFailed,
            TransportError::FlushFailed,
        ] {
            assert_ne!(result, Err(ServeError::Transport(transport_error)));
        }
        assert!(matches!(result, Err(ServeError::Handler(HandlerFailure))));
    }

    #[test]
    fn transport_failures_remain_transport_variants_with_a_handler() {
        let read =
            serve_connection_with_handler(BufReader::new(FailingReader), Vec::new(), echo_length);
        let write = serve_connection_with_handler(Cursor::new(b"x\n"), FailingWriter, echo_length);

        assert_eq!(read, Err(ServeError::Transport(TransportError::ReadFailed)));
        assert_eq!(
            write,
            Err(ServeError::Transport(TransportError::WriteFailed))
        );
    }
}
