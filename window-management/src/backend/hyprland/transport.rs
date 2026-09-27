//! Synchronous read-only transport for Hyprland's request socket.

use std::env;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};

use super::super::BackendError;

const WORKSPACES_REQUEST: &[u8] = b"j/workspaces";
const CLIENTS_REQUEST: &[u8] = b"j/clients";
const ACTIVE_WINDOW_REQUEST: &[u8] = b"j/activewindow";

pub(super) struct HyprlandTransport {
    socket_path: PathBuf,
}

impl HyprlandTransport {
    pub(super) fn from_env() -> Result<Self, BackendError> {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR");
        let instance_signature = env::var_os("HYPRLAND_INSTANCE_SIGNATURE");
        let socket_path =
            socket_path_from_values(runtime_dir.as_deref(), instance_signature.as_deref())?;

        Ok(Self { socket_path })
    }

    #[cfg(test)]
    pub(super) fn with_socket_path(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub(super) fn request_workspaces(&self) -> Result<String, BackendError> {
        self.request(WORKSPACES_REQUEST)
    }

    pub(super) fn request_clients(&self) -> Result<String, BackendError> {
        self.request(CLIENTS_REQUEST)
    }

    pub(super) fn request_active_window(&self) -> Result<String, BackendError> {
        self.request(ACTIVE_WINDOW_REQUEST)
    }

    fn request(&self, request: &[u8]) -> Result<String, BackendError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|_| BackendError::CompositorUnavailable)?;
        stream
            .write_all(request)
            .map_err(|_| BackendError::CompositorUnavailable)?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|_| BackendError::CompositorUnavailable)?;

        String::from_utf8(response).map_err(|_| BackendError::ObservedStateUnavailable)
    }
}

fn socket_path_from_values(
    runtime_dir: Option<&OsStr>,
    instance_signature: Option<&OsStr>,
) -> Result<PathBuf, BackendError> {
    let runtime_dir = runtime_dir.ok_or(BackendError::CompositorUnavailable)?;
    let runtime_dir = Path::new(runtime_dir);
    if runtime_dir.as_os_str().is_empty()
        || runtime_dir.as_os_str().as_bytes().contains(&0)
        || !runtime_dir.is_absolute()
    {
        return Err(BackendError::CompositorUnavailable);
    }

    let instance_signature = instance_signature.ok_or(BackendError::CompositorUnavailable)?;
    if instance_signature.as_bytes().contains(&0) {
        return Err(BackendError::CompositorUnavailable);
    }
    let mut components = Path::new(instance_signature).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(BackendError::CompositorUnavailable);
    }

    Ok(runtime_dir
        .join("hypr")
        .join(instance_signature)
        .join(".socket.sock"))
}

#[cfg(test)]
pub(super) mod test_support {
    use std::fs;
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::{self, JoinHandle};

    static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

    pub(super) fn unused_socket_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "cw-missing-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    pub(in crate::backend::hyprland) struct MockExchange {
        expected_request: &'static [u8],
        response: Vec<u8>,
    }

    impl MockExchange {
        pub(in crate::backend::hyprland) fn new(
            expected_request: &'static [u8],
            response: Vec<u8>,
        ) -> Self {
            Self {
                expected_request,
                response,
            }
        }
    }

    pub(in crate::backend::hyprland) struct MockServer {
        path: PathBuf,
        handle: Option<JoinHandle<io::Result<Vec<Vec<u8>>>>>,
    }

    impl MockServer {
        pub(in crate::backend::hyprland) fn start(exchanges: Vec<MockExchange>) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cw-{}-{}.sock",
                std::process::id(),
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let listener = UnixListener::bind(&path).expect("mock socket should bind");
            let handle = thread::spawn(move || {
                let mut requests = Vec::with_capacity(exchanges.len());

                for exchange in exchanges {
                    let (mut stream, _) = listener.accept()?;
                    let mut request = vec![0; exchange.expected_request.len()];
                    stream.read_exact(&mut request)?;
                    if request != exchange.expected_request {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "mock received an unexpected request",
                        ));
                    }
                    stream.write_all(&exchange.response)?;
                    requests.push(request);
                }

                Ok(requests)
            });

            Self {
                path,
                handle: Some(handle),
            }
        }

        pub(in crate::backend::hyprland) fn path(&self) -> &Path {
            &self.path
        }

        pub(in crate::backend::hyprland) fn finish(mut self) -> Vec<Vec<u8>> {
            let requests = self
                .handle
                .take()
                .expect("mock server thread should be present")
                .join()
                .expect("mock server thread should not panic")
                .expect("mock server should complete I/O");
            fs::remove_file(&self.path).expect("mock socket should be removable");
            requests
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::test_support::{MockExchange, MockServer};
    use super::{HyprlandTransport, socket_path_from_values};
    use crate::backend::BackendError;

    type RequestFn = fn(&HyprlandTransport) -> Result<String, BackendError>;

    fn assert_exact_request(request_fn: RequestFn, expected: &'static [u8]) {
        let response = br#"{"ok":true}"#.to_vec();
        let server = MockServer::start(vec![MockExchange::new(expected, response.clone())]);
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());

        let received = request_fn(&transport).expect("mock request should succeed");
        let requests = server.finish();

        assert_eq!(received.as_bytes(), response);
        assert_eq!(requests, vec![expected]);
    }

    #[test]
    fn socket_path_is_derived_from_runtime_values() {
        let path = socket_path_from_values(
            Some(OsStr::new("/tmp/clea-runtime")),
            Some(OsStr::new("fixture-instance")),
        )
        .expect("valid components should produce a path");

        assert_eq!(
            path,
            PathBuf::from("/tmp/clea-runtime/hypr/fixture-instance/.socket.sock")
        );
    }

    #[test]
    fn missing_runtime_dir_is_compositor_unavailable() {
        assert_eq!(
            socket_path_from_values(None, Some(OsStr::new("fixture-instance"))),
            Err(BackendError::CompositorUnavailable)
        );
    }

    #[test]
    fn missing_instance_signature_is_compositor_unavailable() {
        assert_eq!(
            socket_path_from_values(Some(OsStr::new("/tmp/clea-runtime")), None),
            Err(BackendError::CompositorUnavailable)
        );
    }

    #[test]
    fn unusable_path_components_are_compositor_unavailable() {
        for (runtime_dir, signature) in [
            ("", "fixture-instance"),
            ("relative", "fixture-instance"),
            ("/tmp/clea\0runtime", "fixture-instance"),
            ("/tmp/clea-runtime", ""),
            ("/tmp/clea-runtime", "../other-instance"),
            ("/tmp/clea-runtime", "fixture\0instance"),
        ] {
            assert_eq!(
                socket_path_from_values(Some(OsStr::new(runtime_dir)), Some(OsStr::new(signature))),
                Err(BackendError::CompositorUnavailable)
            );
        }
    }

    #[test]
    fn nonexistent_socket_is_compositor_unavailable() {
        let path = super::test_support::unused_socket_path();
        let transport = HyprlandTransport::with_socket_path(path);

        assert_eq!(
            transport.request_workspaces(),
            Err(BackendError::CompositorUnavailable)
        );
    }

    #[test]
    fn named_requests_send_exact_protocol_bytes() {
        for (request_fn, expected) in [
            (
                HyprlandTransport::request_workspaces as RequestFn,
                b"j/workspaces".as_slice(),
            ),
            (
                HyprlandTransport::request_clients as RequestFn,
                b"j/clients".as_slice(),
            ),
            (
                HyprlandTransport::request_active_window as RequestFn,
                b"j/activewindow".as_slice(),
            ),
        ] {
            assert_exact_request(request_fn, expected);
        }
    }

    #[test]
    fn complete_response_is_returned() {
        let response = vec![b'x'; 32 * 1024];
        let server = MockServer::start(vec![MockExchange::new(b"j/clients", response.clone())]);
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());

        let received = transport
            .request_clients()
            .expect("complete UTF-8 response should be returned");
        let requests = server.finish();

        assert_eq!(received.as_bytes(), response);
        assert_eq!(requests, vec![b"j/clients".to_vec()]);
    }

    #[test]
    fn non_utf8_response_is_observed_state_unavailable() {
        let server =
            MockServer::start(vec![MockExchange::new(b"j/activewindow", vec![0xff, 0xfe])]);
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());

        let result = transport.request_active_window();
        let requests = server.finish();

        assert_eq!(result, Err(BackendError::ObservedStateUnavailable));
        assert_eq!(requests, vec![b"j/activewindow".to_vec()]);
    }

    #[test]
    fn each_query_receives_a_response_without_a_write_half_close() {
        let server = MockServer::start(vec![
            MockExchange::new(b"j/workspaces", b"[]".to_vec()),
            MockExchange::new(b"j/clients", b"[]".to_vec()),
            MockExchange::new(b"j/activewindow", b"{}".to_vec()),
        ]);
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());

        assert_eq!(transport.request_workspaces().as_deref(), Ok("[]"));
        assert_eq!(transport.request_clients().as_deref(), Ok("[]"));
        assert_eq!(transport.request_active_window().as_deref(), Ok("{}"));
        let requests = server.finish();

        assert_eq!(
            requests,
            vec![
                b"j/workspaces".to_vec(),
                b"j/clients".to_vec(),
                b"j/activewindow".to_vec(),
            ]
        );
    }
}
