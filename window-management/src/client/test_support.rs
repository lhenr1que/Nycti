//! A scriptable fake daemon for the client tests.
//!
//! It listens on a real Unix socket in a private test directory, serves one
//! connection, and records the request line it read. Nothing here sleeps or
//! polls: the test and the server synchronize through channels.

use std::fs::{self, File, Permissions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::daemon::test_support::{GUARD, TestDirectory};

/// Computes the bytes the fake daemon writes for one request line.
pub(crate) type Handler = Box<dyn FnMut(&[u8]) -> Vec<u8> + Send>;

/// What the fake daemon does after it has read the request line.
pub(crate) enum Behavior {
    /// Writes exactly the bytes the handler returns for the request line
    /// (which was read without its LF), then closes the connection.
    Respond(Handler),
    /// Closes the connection without writing anything.
    Close,
    /// Keeps the connection open and silent until the server is dropped.
    Mute,
}

impl Behavior {
    /// Always writes the same bytes, with no LF added.
    pub(crate) fn raw(bytes: impl Into<Vec<u8>>) -> Self {
        let bytes = bytes.into();
        Self::Respond(Box::new(move |_| bytes.clone()))
    }

    /// Always writes the same line, adding its LF.
    pub(crate) fn line(text: &str) -> Self {
        Self::raw(format!("{text}\n"))
    }
}

pub(crate) struct FakeServer {
    path: PathBuf,
    lines: Receiver<Vec<u8>>,
    release: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub(crate) fn start(directory: &TestDirectory, behavior: Behavior) -> Self {
        let path = directory.path().join("fake.sock");
        let listener = UnixListener::bind(&path).expect("fake daemon socket should bind");
        let (line_sender, lines) = mpsc::channel();
        let (release, released) = mpsc::channel();

        let thread = thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            serve(stream, behavior, &line_sender, &released);
        });

        Self {
            path,
            lines,
            release: Some(release),
            thread: Some(thread),
        }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.path.clone()
    }

    /// Returns the request line the server read, without its LF.
    pub(crate) fn request_line(&self) -> String {
        let line = self
            .lines
            .recv_timeout(GUARD)
            .expect("the fake daemon should have read a request line");
        String::from_utf8(line).expect("the request should be UTF-8")
    }
}

fn serve(
    stream: UnixStream,
    mut behavior: Behavior,
    lines: &Sender<Vec<u8>>,
    released: &Receiver<()>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let mut line = Vec::new();
    // A wake-up connection from `Drop` ends here with no line.
    if !matches!(reader.read_until(b'\n', &mut line), Ok(count) if count > 0) {
        return;
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    let _ = lines.send(line.clone());

    match &mut behavior {
        Behavior::Respond(handler) => {
            let mut stream = stream;
            let _ = stream.write_all(&handler(&line));
        }
        Behavior::Close => {}
        Behavior::Mute => {
            let _ = released.recv_timeout(GUARD);
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        // Releases a mute server, and wakes a server nobody connected to.
        drop(self.release.take());
        let _ = UnixStream::connect(&self.path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Whether the user can open a file whose mode is 000, as root can. A
/// privileged user also connects to any socket, so the permission-denied case
/// cannot be simulated for it.
pub(crate) fn ignores_file_permissions(directory: &TestDirectory) -> bool {
    let probe = directory.path().join("probe");
    File::create(&probe).expect("probe file should be created");
    fs::set_permissions(&probe, Permissions::from_mode(0o000)).expect("probe mode should be set");
    File::open(&probe).is_ok()
}
