//! Secure ownership of the per-user Unix protocol socket.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, Metadata, Permissions};
use std::io::{self, BufReader};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::backend::WindowBackend;
use crate::service::WindowManagementService;
use crate::transport::{
    ServeError, TransportError, serve_connection, serve_connection_with_handler,
};

const RUNTIME_ENVIRONMENT_VARIABLE: &str = "XDG_RUNTIME_DIR";
const NYCTI_RUNTIME_DIRECTORY: &str = "nycti";
const WINDOW_MANAGEMENT_SOCKET: &str = "window-management.sock";
const RUNTIME_DIRECTORY_MODE: u32 = 0o700;
const SOCKET_MODE: u32 = 0o600;
const PERMISSION_BITS: u32 = 0o777;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedRuntimeRoot(PathBuf);

impl ValidatedRuntimeRoot {
    fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A startup or lifecycle failure in the Unix runtime adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnixRuntimeError {
    RuntimeDirUnavailable,
    InvalidRuntimeDir,
    RuntimeDirectorySetupFailed,
    UnsafeRuntimeDirectory,
    UnsafeSocketPath,
    AlreadyRunning,
    ExistingSocketCheckFailed,
    StaleSocketCleanupFailed,
    BindFailed,
    SocketPermissionFailed,
    AcceptFailed,
    ConnectionPreparationFailed,
    SocketCleanupFailed,
}

impl fmt::Display for UnixRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::RuntimeDirUnavailable => "runtime directory is unavailable",
            Self::InvalidRuntimeDir => "runtime directory is invalid",
            Self::RuntimeDirectorySetupFailed => "Nycti runtime directory setup failed",
            Self::UnsafeRuntimeDirectory => "Nycti runtime directory path is unsafe",
            Self::UnsafeSocketPath => "window management socket path is unsafe",
            Self::AlreadyRunning => "window management listener is already running",
            Self::ExistingSocketCheckFailed => "existing socket could not be classified safely",
            Self::StaleSocketCleanupFailed => "stale socket cleanup failed",
            Self::BindFailed => "window management socket bind failed",
            Self::SocketPermissionFailed => "window management socket permissions failed",
            Self::AcceptFailed => "accepting a Unix connection failed",
            Self::ConnectionPreparationFailed => "preparing a Unix connection failed",
            Self::SocketCleanupFailed => "owned socket cleanup failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for UnixRuntimeError {}

/// Preserves the distinction between Unix preparation and framed transport errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnixConnectionError {
    Runtime(UnixRuntimeError),
    Transport(TransportError),
}

impl fmt::Display for UnixConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for UnixConnectionError {}

/// Preserves the distinction between Unix preparation, framed transport, and
/// request-handler failures for a connection served through a handler.
///
/// `Handler` carries the caller's own error type. It is deliberately not part
/// of [`UnixRuntimeError`], which never contains handler, protocol, or backend
/// errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnixServeError<E> {
    Runtime(UnixRuntimeError),
    Transport(TransportError),
    Handler(E),
}

impl<E: fmt::Display> fmt::Display for UnixServeError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
            Self::Handler(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for UnixServeError<E> {}

fn resolve_runtime_root_from_env() -> Result<ValidatedRuntimeRoot, UnixRuntimeError> {
    resolve_runtime_root_value(env::var_os(RUNTIME_ENVIRONMENT_VARIABLE))
}

fn resolve_runtime_root_value(
    value: Option<OsString>,
) -> Result<ValidatedRuntimeRoot, UnixRuntimeError> {
    let value = value.ok_or(UnixRuntimeError::RuntimeDirUnavailable)?;
    if value.is_empty() {
        return Err(UnixRuntimeError::RuntimeDirUnavailable);
    }

    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(UnixRuntimeError::InvalidRuntimeDir);
    }

    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(UnixRuntimeError::InvalidRuntimeDir);
        }
        Err(_) => return Err(UnixRuntimeError::RuntimeDirUnavailable),
    };

    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || permission_mode(&metadata) != RUNTIME_DIRECTORY_MODE
    {
        return Err(UnixRuntimeError::InvalidRuntimeDir);
    }

    Ok(ValidatedRuntimeRoot(path))
}

/// Owns one securely bound Window Management Unix listener and its cleanup evidence.
pub struct UnixRuntimeListener {
    listener: UnixListener,
    socket_path: PathBuf,
    socket_identity: SocketIdentity,
    cleanup_active: bool,
}

impl UnixRuntimeListener {
    /// Resolves `XDG_RUNTIME_DIR` and binds the normative socket path.
    pub fn bind_from_env() -> Result<Self, UnixRuntimeError> {
        let root = resolve_runtime_root_from_env()?;
        Self::bind(&root)
    }

    /// Binds at an explicit runtime root, for tests only. The root is validated
    /// like the production root; production always resolves `XDG_RUNTIME_DIR`.
    #[cfg(test)]
    pub(crate) fn bind_at(root: &Path) -> Result<Self, UnixRuntimeError> {
        let root = resolve_runtime_root_value(Some(root.as_os_str().to_os_string()))?;
        Self::bind(&root)
    }

    fn bind(root: &ValidatedRuntimeRoot) -> Result<Self, UnixRuntimeError> {
        let runtime_directory = prepare_nycti_directory(root)?;
        let socket_path = runtime_directory.join(WINDOW_MANAGEMENT_SOCKET);

        prepare_existing_socket_path(&socket_path)?;
        let listener = bind_socket_once(&socket_path)?;

        let metadata = fs::symlink_metadata(&socket_path)
            .map_err(|_| UnixRuntimeError::SocketPermissionFailed)?;
        if !metadata.file_type().is_socket() {
            return Err(UnixRuntimeError::SocketPermissionFailed);
        }
        let socket_identity = SocketIdentity::from_metadata(&metadata);

        if finish_socket_permissions(&socket_path, socket_identity).is_err() {
            let _ = cleanup_owned_socket(&socket_path, socket_identity);
            return Err(UnixRuntimeError::SocketPermissionFailed);
        }

        Ok(Self {
            listener,
            socket_path,
            socket_identity,
            cleanup_active: true,
        })
    }

    /// Returns the bound socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Accepts one client stream without selecting a daemon scheduling model.
    pub fn accept(&self) -> Result<UnixStream, UnixRuntimeError> {
        self.listener
            .accept()
            .map(|(stream, _address)| stream)
            .map_err(|_| UnixRuntimeError::AcceptFailed)
    }

    /// Removes this owner's socket if the path still has the recorded identity.
    pub fn cleanup(mut self) -> Result<(), UnixRuntimeError> {
        if !self.cleanup_active {
            return Ok(());
        }

        cleanup_owned_socket(&self.socket_path, self.socket_identity)?;
        self.cleanup_active = false;
        Ok(())
    }
}

impl Drop for UnixRuntimeListener {
    fn drop(&mut self) {
        if self.cleanup_active {
            let _ = cleanup_owned_socket(&self.socket_path, self.socket_identity);
            self.cleanup_active = false;
        }
    }
}

/// Prepares an accepted stream as the `BufRead` plus `Write` pair the generic
/// framing layer requires. This is the only place a stream is cloned and
/// buffered for serving.
fn prepare_stream(
    stream: UnixStream,
) -> Result<(BufReader<UnixStream>, UnixStream), UnixRuntimeError> {
    let reader_stream = stream
        .try_clone()
        .map_err(|_| UnixRuntimeError::ConnectionPreparationFailed)?;
    Ok((BufReader::new(reader_stream), stream))
}

/// Serves one accepted Unix stream through the existing generic framing layer.
pub fn serve_unix_connection<B: WindowBackend>(
    stream: UnixStream,
    service: &mut WindowManagementService<B>,
) -> Result<(), UnixConnectionError> {
    let (reader, writer) = prepare_stream(stream).map_err(UnixConnectionError::Runtime)?;
    serve_connection(reader, writer, service).map_err(UnixConnectionError::Transport)
}

/// Serves one accepted Unix stream by calling `handler` once per complete
/// request line through the existing generic framing layer.
///
/// The handler receives the exact request bytes without the terminating LF and
/// returns exactly one complete response line terminated by LF, which is
/// written unchanged; the LF is a documented convention that is not validated.
///
/// If the handler fails, no response is written for that request, serving
/// stops, and `UnixServeError::Handler` is returned. Dropping the stream closes
/// the connection, so the client observes only EOF and no protocol response.
/// The handler failure is never converted into a `UnixRuntimeError` or a
/// protocol response. If the stream cannot be prepared, the handler is not
/// called.
pub fn serve_unix_connection_with_handler<H, E>(
    stream: UnixStream,
    handler: H,
) -> Result<(), UnixServeError<E>>
where
    H: FnMut(&[u8]) -> Result<String, E>,
{
    let (reader, writer) = prepare_stream(stream).map_err(UnixServeError::Runtime)?;
    serve_connection_with_handler(reader, writer, handler).map_err(|error| match error {
        ServeError::Transport(error) => UnixServeError::Transport(error),
        ServeError::Handler(error) => UnixServeError::Handler(error),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

impl SocketIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

fn permission_mode(metadata: &Metadata) -> u32 {
    metadata.permissions().mode() & PERMISSION_BITS
}

fn prepare_nycti_directory(root: &ValidatedRuntimeRoot) -> Result<PathBuf, UnixRuntimeError> {
    let path = root.as_path().join(NYCTI_RUNTIME_DIRECTORY);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match create_nycti_directory(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(UnixRuntimeError::RuntimeDirectorySetupFailed),
            }
            inspect_nycti_directory(&path)?
        }
        Err(_) => return Err(UnixRuntimeError::RuntimeDirectorySetupFailed),
    };

    normalize_nycti_directory(&path, &metadata)?;
    Ok(path)
}

fn create_nycti_directory(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(RUNTIME_DIRECTORY_MODE);
    builder.create(path)
}

fn inspect_nycti_directory(path: &Path) -> Result<Metadata, UnixRuntimeError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| UnixRuntimeError::RuntimeDirectorySetupFailed)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(UnixRuntimeError::UnsafeRuntimeDirectory);
    }
    Ok(metadata)
}

fn normalize_nycti_directory(
    path: &Path,
    initial_metadata: &Metadata,
) -> Result<(), UnixRuntimeError> {
    if initial_metadata.file_type().is_symlink() || !initial_metadata.file_type().is_dir() {
        return Err(UnixRuntimeError::UnsafeRuntimeDirectory);
    }
    let initial_identity = (initial_metadata.dev(), initial_metadata.ino());

    fs::set_permissions(path, Permissions::from_mode(RUNTIME_DIRECTORY_MODE))
        .map_err(|_| UnixRuntimeError::RuntimeDirectorySetupFailed)?;

    let final_metadata = inspect_nycti_directory(path)?;
    if (final_metadata.dev(), final_metadata.ino()) != initial_identity {
        return Err(UnixRuntimeError::UnsafeRuntimeDirectory);
    }
    if permission_mode(&final_metadata) != RUNTIME_DIRECTORY_MODE {
        return Err(UnixRuntimeError::RuntimeDirectorySetupFailed);
    }
    Ok(())
}

fn prepare_existing_socket_path(path: &Path) -> Result<(), UnixRuntimeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(UnixRuntimeError::ExistingSocketCheckFailed),
    };

    if !metadata.file_type().is_socket() {
        return Err(UnixRuntimeError::UnsafeSocketPath);
    }
    let initial_identity = SocketIdentity::from_metadata(&metadata);

    match UnixStream::connect(path) {
        Ok(stream) => {
            drop(stream);
            Err(UnixRuntimeError::AlreadyRunning)
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            remove_confirmed_stale_socket(path, initial_identity)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(UnixRuntimeError::ExistingSocketCheckFailed),
    }
}

fn remove_confirmed_stale_socket(
    path: &Path,
    initial_identity: SocketIdentity,
) -> Result<(), UnixRuntimeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(UnixRuntimeError::StaleSocketCleanupFailed),
    };

    if !metadata.file_type().is_socket()
        || SocketIdentity::from_metadata(&metadata) != initial_identity
    {
        return Err(UnixRuntimeError::UnsafeSocketPath);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(UnixRuntimeError::StaleSocketCleanupFailed),
    }
}

fn bind_socket_once(path: &Path) -> Result<UnixListener, UnixRuntimeError> {
    UnixListener::bind(path).map_err(|_| UnixRuntimeError::BindFailed)
}

fn finish_socket_permissions(
    path: &Path,
    identity: SocketIdentity,
) -> Result<(), UnixRuntimeError> {
    fs::set_permissions(path, Permissions::from_mode(SOCKET_MODE))
        .map_err(|_| UnixRuntimeError::SocketPermissionFailed)?;

    let metadata =
        fs::symlink_metadata(path).map_err(|_| UnixRuntimeError::SocketPermissionFailed)?;
    if !metadata.file_type().is_socket()
        || SocketIdentity::from_metadata(&metadata) != identity
        || permission_mode(&metadata) != SOCKET_MODE
    {
        return Err(UnixRuntimeError::SocketPermissionFailed);
    }
    Ok(())
}

fn cleanup_owned_socket(path: &Path, identity: SocketIdentity) -> Result<(), UnixRuntimeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(UnixRuntimeError::SocketCleanupFailed),
    };

    if !metadata.file_type().is_socket() || SocketIdentity::from_metadata(&metadata) != identity {
        return Ok(());
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(UnixRuntimeError::SocketCleanupFailed),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{Value, json};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::core::{WindowManager, WorkspaceMode};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "nycti-windowd-runtime-test-{}-{sequence}",
                std::process::id()
            ));
            let mut builder = DirBuilder::new();
            builder.mode(RUNTIME_DIRECTORY_MODE);
            builder
                .create(&path)
                .expect("unique test directory should be created");
            fs::set_permissions(&path, Permissions::from_mode(RUNTIME_DIRECTORY_MODE))
                .expect("test directory mode should be set");
            Self { path }
        }

        fn child(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }

        fn create_valid_root(&self, name: &str) -> PathBuf {
            let path = self.child(name);
            create_directory_with_mode(&path, RUNTIME_DIRECTORY_MODE);
            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn create_directory_with_mode(path: &Path, mode: u32) {
        let mut builder = DirBuilder::new();
        builder.mode(mode);
        builder
            .create(path)
            .expect("test directory should be created");
        fs::set_permissions(path, Permissions::from_mode(mode))
            .expect("test directory mode should be set");
    }

    fn validate_path(path: &Path) -> Result<ValidatedRuntimeRoot, UnixRuntimeError> {
        resolve_runtime_root_value(Some(path.as_os_str().to_os_string()))
    }

    fn valid_root(area: &TestDirectory, name: &str) -> ValidatedRuntimeRoot {
        let path = area.create_valid_root(name);
        validate_path(&path).expect("test runtime root should be valid")
    }

    fn fake_service() -> WindowManagementService<FakeBackend> {
        WindowManagementService::new(WindowManager::new(
            FakeBackend::new(),
            WorkspaceMode::Tiling,
        ))
    }

    fn socket_identity(path: &Path) -> SocketIdentity {
        SocketIdentity::from_metadata(
            &fs::symlink_metadata(path).expect("socket metadata should be available"),
        )
    }

    fn request(id: &str, method: &str) -> String {
        format!(
            "{}\n",
            json!({"version": 1, "id": id, "method": method, "params": {}})
        )
    }

    fn exchange(
        listener: &UnixRuntimeListener,
        service: &mut WindowManagementService<FakeBackend>,
        input: &str,
    ) -> String {
        let mut client =
            UnixStream::connect(listener.socket_path()).expect("test client should connect");
        client
            .write_all(input.as_bytes())
            .expect("test request should be written");
        client
            .shutdown(Shutdown::Write)
            .expect("test client write half should close");

        let server = listener.accept().expect("server should accept client");
        serve_unix_connection(server, service).expect("connection should be served");

        let mut output = String::new();
        client
            .read_to_string(&mut output)
            .expect("test response should be read");
        output
    }

    #[test]
    fn missing_runtime_environment_value_is_unavailable() {
        assert_eq!(
            resolve_runtime_root_value(None),
            Err(UnixRuntimeError::RuntimeDirUnavailable)
        );
    }

    #[test]
    fn empty_runtime_environment_value_is_unavailable() {
        assert_eq!(
            resolve_runtime_root_value(Some(OsString::new())),
            Err(UnixRuntimeError::RuntimeDirUnavailable)
        );
    }

    #[test]
    fn relative_runtime_root_is_invalid() {
        assert_eq!(
            resolve_runtime_root_value(Some(OsString::from("relative/runtime"))),
            Err(UnixRuntimeError::InvalidRuntimeDir)
        );
    }

    #[test]
    fn nonexistent_runtime_root_is_invalid() {
        let area = TestDirectory::new();
        assert_eq!(
            validate_path(&area.child("missing")),
            Err(UnixRuntimeError::InvalidRuntimeDir)
        );
    }

    #[test]
    fn operational_runtime_metadata_failure_is_unavailable() {
        let area = TestDirectory::new();
        let blocker = area.child("blocker");
        fs::write(&blocker, b"not a directory").expect("test blocker should be created");

        assert_eq!(
            validate_path(&blocker.join("runtime")),
            Err(UnixRuntimeError::RuntimeDirUnavailable)
        );
    }

    #[test]
    fn runtime_root_symlink_is_invalid() {
        let area = TestDirectory::new();
        let target = area.create_valid_root("target");
        let link = area.child("runtime-link");
        symlink(&target, &link).expect("runtime symlink should be created");

        assert_eq!(
            validate_path(&link),
            Err(UnixRuntimeError::InvalidRuntimeDir)
        );
    }

    #[test]
    fn runtime_root_regular_file_is_invalid() {
        let area = TestDirectory::new();
        let path = area.child("runtime-file");
        fs::write(&path, b"not a directory").expect("test file should be created");

        assert_eq!(
            validate_path(&path),
            Err(UnixRuntimeError::InvalidRuntimeDir)
        );
    }

    #[test]
    fn wrong_runtime_root_mode_is_rejected_without_chmod() {
        let area = TestDirectory::new();
        let path = area.child("runtime");
        create_directory_with_mode(&path, 0o755);

        assert_eq!(
            validate_path(&path),
            Err(UnixRuntimeError::InvalidRuntimeDir)
        );
        assert_eq!(
            permission_mode(&fs::symlink_metadata(&path).expect("metadata should remain")),
            0o755
        );
    }

    #[test]
    fn mode_0700_runtime_root_is_valid() {
        let area = TestDirectory::new();
        let path = area.create_valid_root("runtime");

        let root = validate_path(&path).expect("runtime root should validate");

        assert_eq!(root.as_path(), path);
    }

    #[test]
    fn absent_nycti_directory_is_created() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");

        let path = prepare_nycti_directory(&root).expect("Nycti directory should be prepared");

        assert!(path.is_dir());
    }

    #[test]
    fn newly_created_nycti_directory_has_final_mode_0700() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");

        let path = prepare_nycti_directory(&root).expect("Nycti directory should be prepared");

        assert_eq!(
            permission_mode(&fs::symlink_metadata(path).expect("metadata should exist")),
            RUNTIME_DIRECTORY_MODE
        );
    }

    #[test]
    fn nycti_directory_creation_is_not_broader_than_0700() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let path = root.as_path().join(NYCTI_RUNTIME_DIRECTORY);

        create_nycti_directory(&path).expect("Nycti directory should be created");
        let mode = permission_mode(&fs::symlink_metadata(path).expect("metadata should exist"));

        assert_eq!(mode & !RUNTIME_DIRECTORY_MODE, 0);
    }

    #[test]
    fn existing_real_nycti_directory_is_normalized_to_0700() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let path = root.as_path().join(NYCTI_RUNTIME_DIRECTORY);
        create_directory_with_mode(&path, 0o755);

        prepare_nycti_directory(&root).expect("existing directory should be prepared");

        assert_eq!(
            permission_mode(&fs::symlink_metadata(path).expect("metadata should exist")),
            RUNTIME_DIRECTORY_MODE
        );
    }

    #[test]
    fn nycti_symlink_is_rejected_and_preserved() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let target = area.create_valid_root("target");
        let path = root.as_path().join(NYCTI_RUNTIME_DIRECTORY);
        symlink(target, &path).expect("Nycti symlink should be created");

        assert_eq!(
            prepare_nycti_directory(&root),
            Err(UnixRuntimeError::UnsafeRuntimeDirectory)
        );
        assert!(
            fs::symlink_metadata(path)
                .expect("symlink should remain")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn nycti_regular_file_is_rejected_and_preserved() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let path = root.as_path().join(NYCTI_RUNTIME_DIRECTORY);
        fs::write(&path, b"preserve me").expect("test file should be created");

        assert_eq!(
            prepare_nycti_directory(&root),
            Err(UnixRuntimeError::UnsafeRuntimeDirectory)
        );
        assert_eq!(fs::read(path).expect("file should remain"), b"preserve me");
    }

    #[test]
    fn absent_socket_path_binds_listener() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");

        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");

        assert!(
            fs::symlink_metadata(listener.socket_path())
                .expect("socket should exist")
                .file_type()
                .is_socket()
        );
    }

    #[test]
    fn bound_socket_has_mode_0600() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");

        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");

        assert_eq!(
            permission_mode(
                &fs::symlink_metadata(listener.socket_path()).expect("socket should exist")
            ),
            SOCKET_MODE
        );
    }

    #[test]
    fn active_listener_is_detected_and_preserved() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("first listener should bind");
        let identity = socket_identity(listener.socket_path());

        assert!(matches!(
            UnixRuntimeListener::bind(&root),
            Err(UnixRuntimeError::AlreadyRunning)
        ));
        assert_eq!(socket_identity(listener.socket_path()), identity);
    }

    #[test]
    fn refused_real_socket_is_removed_and_rebound() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let runtime_directory = prepare_nycti_directory(&root).expect("directory should exist");
        let path = runtime_directory.join(WINDOW_MANAGEMENT_SOCKET);
        let stale_listener = UnixListener::bind(&path).expect("stale fixture should bind");
        drop(stale_listener);
        assert!(path.exists());
        assert_eq!(
            UnixStream::connect(&path)
                .expect_err("dropped listener should refuse")
                .kind(),
            io::ErrorKind::ConnectionRefused
        );
        let stale_identity = socket_identity(&path);

        let listener = UnixRuntimeListener::bind(&root).expect("stale socket should recover");

        assert_ne!(socket_identity(listener.socket_path()), stale_identity);
    }

    #[test]
    fn regular_file_at_socket_path_is_rejected_and_preserved() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let runtime_directory = prepare_nycti_directory(&root).expect("directory should exist");
        let path = runtime_directory.join(WINDOW_MANAGEMENT_SOCKET);
        fs::write(&path, b"preserve me").expect("test file should be created");

        assert!(matches!(
            UnixRuntimeListener::bind(&root),
            Err(UnixRuntimeError::UnsafeSocketPath)
        ));
        assert_eq!(fs::read(path).expect("file should remain"), b"preserve me");
    }

    #[test]
    fn symlink_at_socket_path_is_rejected_and_preserved() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let runtime_directory = prepare_nycti_directory(&root).expect("directory should exist");
        let target = runtime_directory.join("target");
        fs::write(&target, b"target").expect("target should be created");
        let path = runtime_directory.join(WINDOW_MANAGEMENT_SOCKET);
        symlink(&target, &path).expect("socket-path symlink should be created");

        assert!(matches!(
            UnixRuntimeListener::bind(&root),
            Err(UnixRuntimeError::UnsafeSocketPath)
        ));
        assert!(
            fs::symlink_metadata(path)
                .expect("symlink should remain")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn bind_failure_does_not_remove_existing_path() {
        let area = TestDirectory::new();
        let path = area.child("occupied");
        fs::write(&path, b"preserve me").expect("test file should be created");

        assert!(matches!(
            bind_socket_once(&path),
            Err(UnixRuntimeError::BindFailed)
        ));
        assert_eq!(fs::read(path).expect("file should remain"), b"preserve me");
    }

    #[test]
    fn explicit_cleanup_removes_owned_socket() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let path = listener.socket_path().to_owned();

        listener.cleanup().expect("cleanup should succeed");

        assert!(!path.exists());
    }

    #[test]
    fn drop_removes_owned_socket_as_a_best_effort_safety_net() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let path = listener.socket_path().to_owned();

        drop(listener);

        assert!(!path.exists());
    }

    #[test]
    fn cleanup_is_noop_when_path_is_already_absent() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        fs::remove_file(listener.socket_path()).expect("owned path should be removed for test");

        listener
            .cleanup()
            .expect("missing path cleanup should succeed");
    }

    #[test]
    fn cleanup_preserves_replacement_regular_file() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let path = listener.socket_path().to_owned();
        fs::remove_file(&path).expect("owned path should be removed for replacement");
        fs::write(&path, b"replacement").expect("replacement should be created");

        listener
            .cleanup()
            .expect("replacement cleanup should be safe");

        assert_eq!(
            fs::read(path).expect("replacement should remain"),
            b"replacement"
        );
    }

    #[test]
    fn cleanup_preserves_replacement_socket_with_different_identity() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let path = listener.socket_path().to_owned();
        let original_identity = listener.socket_identity;
        fs::remove_file(&path).expect("owned path should be removed for replacement");
        let replacement = UnixListener::bind(&path).expect("replacement socket should bind");
        let replacement_identity = socket_identity(&path);
        assert_ne!(original_identity, replacement_identity);

        listener
            .cleanup()
            .expect("replacement cleanup should be safe");

        assert_eq!(socket_identity(&path), replacement_identity);
        drop(replacement);
    }

    #[test]
    fn explicit_cleanup_allows_rebind_without_delayed_owner_removal() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let path = listener.socket_path().to_owned();

        listener.cleanup().expect("explicit cleanup should succeed");
        let replacement = UnixListener::bind(&path).expect("replacement listener should bind");

        assert!(
            fs::symlink_metadata(&path)
                .expect("replacement socket should remain")
                .file_type()
                .is_socket()
        );
        drop(replacement);
    }

    #[test]
    fn accepts_real_unix_stream() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let client = UnixStream::connect(listener.socket_path()).expect("client should connect");

        let server = listener.accept().expect("server should accept client");

        drop(server);
        drop(client);
    }

    #[test]
    fn accepted_stream_can_use_generic_serve_connection() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut client =
            UnixStream::connect(listener.socket_path()).expect("client should connect");
        client
            .write_all(request("generic", "status").as_bytes())
            .expect("request should be written");
        client
            .shutdown(Shutdown::Write)
            .expect("write half should close");
        let server = listener.accept().expect("server should accept client");
        let reader = BufReader::new(server.try_clone().expect("stream should clone"));
        let mut service = fake_service();

        serve_connection(reader, server, &mut service).expect("generic transport should serve");
        let mut output = String::new();
        client
            .read_to_string(&mut output)
            .expect("response should be read");

        let response: Value = serde_json::from_str(&output).expect("response should be JSON");
        assert_eq!(response["id"], "generic");
    }

    #[test]
    fn status_works_end_to_end_through_unix_helper() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut service = fake_service();

        let output = exchange(&listener, &mut service, &request("status", "status"));

        let response: Value = serde_json::from_str(&output).expect("response should be JSON");
        assert_eq!(response["id"], "status");
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["service"], "nycti-windowd");
    }

    #[test]
    fn multiple_requests_work_on_one_real_unix_connection() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut service = fake_service();
        let input = format!(
            "{}{}",
            request("first", "status"),
            request("second", "get_default_mode")
        );

        let output = exchange(&listener, &mut service, &input);
        let responses = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("response should be JSON"))
            .collect::<Vec<_>>();

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], "first");
        assert_eq!(responses[1]["id"], "second");
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct HandlerFailure;

    fn length_response(line: &[u8]) -> Result<String, HandlerFailure> {
        Ok(format!("{}\n", line.len()))
    }

    /// Serves one real connection through the handler variant and returns the
    /// serve result with every byte the client received until EOF.
    fn handler_exchange<H>(
        listener: &UnixRuntimeListener,
        input: &[u8],
        handler: H,
    ) -> (Result<(), UnixServeError<HandlerFailure>>, Vec<u8>)
    where
        H: FnMut(&[u8]) -> Result<String, HandlerFailure>,
    {
        let mut client =
            UnixStream::connect(listener.socket_path()).expect("test client should connect");
        client
            .write_all(input)
            .expect("test request should be written");
        client
            .shutdown(Shutdown::Write)
            .expect("test client write half should close");

        let server = listener.accept().expect("server should accept client");
        // The stream is moved in and dropped on return, closing the connection.
        let result = serve_unix_connection_with_handler(server, handler);

        let mut output = Vec::new();
        client
            .read_to_end(&mut output)
            .expect("test response should be read");
        (result, output)
    }

    #[test]
    fn handler_receives_exact_bytes_without_lf_over_real_socket() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut received: Vec<Vec<u8>> = Vec::new();

        let (result, output) = handler_exchange(&listener, b"first\n\n\xff\r\nlast\n", |line| {
            received.push(line.to_vec());
            length_response(line)
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
    fn handler_is_not_called_for_clean_eof_or_final_line_without_lf() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut calls = 0;

        let (result, output) = handler_exchange(&listener, b"", |line| {
            calls += 1;
            length_response(line)
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 0);
        assert!(output.is_empty());

        let (result, output) = handler_exchange(&listener, b"incomplete", |line| {
            calls += 1;
            length_response(line)
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 0);
        assert!(output.is_empty());
    }

    #[test]
    fn handler_error_writes_no_response_and_client_observes_only_eof() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut calls = 0;

        let (result, output) = handler_exchange(&listener, b"one\ntwo\n", |_| {
            calls += 1;
            Err(HandlerFailure)
        });

        assert_eq!(result, Err(UnixServeError::Handler(HandlerFailure)));
        assert_eq!(calls, 1);
        assert!(output.is_empty());
    }

    #[test]
    fn handler_error_keeps_earlier_responses_and_stops_serving() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");
        let mut calls = 0;

        let (result, output) = handler_exchange(&listener, b"ok\nfail\nnever\n", |line| {
            calls += 1;
            if line == b"fail" {
                Err(HandlerFailure)
            } else {
                length_response(line)
            }
        });

        assert_eq!(result, Err(UnixServeError::Handler(HandlerFailure)));
        assert_eq!(calls, 2);
        assert_eq!(output, b"2\n");
    }

    #[test]
    fn handler_error_is_distinct_from_runtime_and_transport_errors() {
        let area = TestDirectory::new();
        let root = valid_root(&area, "runtime");
        let listener = UnixRuntimeListener::bind(&root).expect("listener should bind");

        let (result, _) = handler_exchange(&listener, b"x\n", |_| Err(HandlerFailure));

        assert!(matches!(
            result,
            Err(UnixServeError::Handler(HandlerFailure))
        ));
        assert_ne!(
            result,
            Err(UnixServeError::Runtime(
                UnixRuntimeError::ConnectionPreparationFailed
            ))
        );
        for transport_error in [
            TransportError::ReadFailed,
            TransportError::WriteFailed,
            TransportError::FlushFailed,
        ] {
            assert_ne!(result, Err(UnixServeError::Transport(transport_error)));
        }
    }
}
