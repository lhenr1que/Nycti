//! Synchronous JSON Lines framing over generic byte streams.

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

/// Serves sequential protocol requests over one generic readable/writable pair.
///
/// Framing is byte-oriented and removes only the terminating LF byte. A final
/// sequence of bytes without LF is considered incomplete and is discarded at
/// clean EOF without a response. No line-size limit is imposed at this layer.
pub fn serve_connection<R, W, B>(
    mut reader: R,
    mut writer: W,
    service: &mut WindowManagementService<B>,
) -> Result<(), TransportError>
where
    R: BufRead,
    W: Write,
    B: WindowBackend,
{
    let mut request_line = Vec::new();

    loop {
        request_line.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut request_line)
            .map_err(|_| TransportError::ReadFailed)?;

        if bytes_read == 0 {
            return Ok(());
        }

        if request_line.last() != Some(&b'\n') {
            return Ok(());
        }
        request_line.pop();

        let response_line = service.handle_json_bytes(&request_line);
        writer
            .write_all(response_line.as_bytes())
            .map_err(|_| TransportError::WriteFailed)?;
        writer.flush().map_err(|_| TransportError::FlushFailed)?;
    }
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
}
