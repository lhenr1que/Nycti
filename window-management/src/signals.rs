//! The real shutdown source: `SIGTERM` and `SIGINT`, through `signal-hook`.
//!
//! This is the only code that knows the signal dependency. The library takes a
//! `ShutdownSource`, so its tests inject their own source and never install a
//! real signal handler. See ADR 0008.

use std::io;

use nycti_windowd::daemon::ShutdownSource;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};

/// Delivers `SIGTERM` and `SIGINT` as shutdown requests.
///
/// Registering replaces the default action of both signals. Do it before the
/// service socket is bound, so a signal that arrives during startup stays
/// pending instead of ending the process. `SIGHUP` keeps its default action.
pub struct SignalSource {
    signals: Signals,
    handle: Handle,
}

impl SignalSource {
    /// Registers the handlers for `SIGTERM` and `SIGINT`.
    pub fn register() -> io::Result<Self> {
        let signals = Signals::new([SIGTERM, SIGINT])?;
        let handle = signals.handle();
        Ok(Self { signals, handle })
    }
}

impl ShutdownSource for SignalSource {
    fn wait(&mut self) -> Option<()> {
        self.signals.forever().next().map(|_signal| ())
    }

    fn closer(&self) -> Box<dyn FnOnce() + Send + 'static> {
        let handle = self.handle.clone();
        Box::new(move || handle.close())
    }
}
