// pattern: Imperative Shell

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use nix::sys::signal::Signal;
use signal_hook::iterator::{Handle as SignalHandle, Signals};

use crate::session::SessionExit;
use crate::signals::{self, ActiveSignalAction, SignalPolicy};
use crate::terminal::{self, RawTerminalGuard};

#[derive(Debug)]
pub enum ProxyEvent {
    ChildExited(SessionExit),
    SignalReceived(Signal),
    PromptKey(u8),
    ShutdownRequested(ShutdownReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReason {
    pub signal: Signal,
}

impl ShutdownReason {
    pub fn exit_code(self) -> i32 {
        128 + self.signal as i32
    }
}

struct SignalState {
    active_child_pid: AtomicI32,
    shutdown_signal: AtomicI32,
}

impl SignalState {
    fn new() -> Self {
        Self {
            active_child_pid: AtomicI32::new(0),
            shutdown_signal: AtomicI32::new(0),
        }
    }

    fn request_shutdown(&self, signal: Signal) -> ShutdownReason {
        let value = signal as i32;
        let _ = self
            .shutdown_signal
            .compare_exchange(0, value, Ordering::SeqCst, Ordering::SeqCst);
        ShutdownReason { signal }
    }

    fn shutdown_requested(&self) -> Option<ShutdownReason> {
        let value = self.shutdown_signal.load(Ordering::SeqCst);
        if value == 0 {
            return None;
        }
        Signal::try_from(value)
            .ok()
            .map(|signal| ShutdownReason { signal })
    }
}

pub struct ActiveSessionGuard {
    state: Arc<SignalState>,
}

impl Drop for ActiveSessionGuard {
    fn drop(&mut self) {
        self.state.active_child_pid.store(0, Ordering::SeqCst);
    }
}

struct PanicRestoreHook {
    installed: bool,
}

impl PanicRestoreHook {
    fn install(terminal: &RawTerminalGuard) -> Self {
        let Some((fd, original)) = terminal.saved_original() else {
            return Self { installed: false };
        };
        let original = Arc::new(Mutex::new(original));
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Ok(original) = original.lock() {
                terminal::restore_termios(fd, &original);
            }
            previous(info);
        }));
        Self { installed: true }
    }
}

impl Drop for PanicRestoreHook {
    fn drop(&mut self) {
        if self.installed {
            let _ = std::panic::take_hook();
        }
    }
}

pub struct ProxyRuntime {
    _terminal: RawTerminalGuard,
    signal_state: Arc<SignalState>,
    signal_handle: Option<SignalHandle>,
    signal_thread: Option<JoinHandle<()>>,
    event_tx: Sender<ProxyEvent>,
    event_rx: Receiver<ProxyEvent>,
    _panic_restore: PanicRestoreHook,
}

impl ProxyRuntime {
    pub fn enter(policy: &SignalPolicy) -> eyre::Result<Self> {
        let terminal = RawTerminalGuard::enable(&std::io::stdin())?;
        let panic_restore = PanicRestoreHook::install(&terminal);
        let signal_state = Arc::new(SignalState::new());
        let (event_tx, event_rx) = mpsc::channel();
        let observed = policy.observed_signals();
        let mut signal_handle = None;
        let mut signal_thread = None;

        if !observed.is_empty() {
            let mut signals = Signals::new(&observed)?;
            let handle = signals.handle();
            let thread_state = signal_state.clone();
            let thread_tx = event_tx.clone();
            let thread_policy = policy.clone();
            signal_thread = Some(thread::spawn(move || {
                for raw_signal in signals.forever() {
                    let Ok(signal) = Signal::try_from(raw_signal) else {
                        continue;
                    };
                    let _ = thread_tx.send(ProxyEvent::SignalReceived(signal));
                    let child_pid = thread_state.active_child_pid.load(Ordering::SeqCst);
                    if child_pid > 0 {
                        match thread_policy.action_for_active_child(signal) {
                            ActiveSignalAction::ForwardOnly => {
                                let _ = signals::forward_to_process_group(child_pid as u32, signal);
                            }
                            ActiveSignalAction::ForwardThenShutdown => {
                                let _ = signals::forward_to_process_group(child_pid as u32, signal);
                                let reason = thread_state.request_shutdown(signal);
                                let _ = thread_tx.send(ProxyEvent::ShutdownRequested(reason));
                            }
                            ActiveSignalAction::Ignore => {}
                        }
                    } else if thread_policy.should_shutdown_without_child(signal) {
                        let reason = thread_state.request_shutdown(signal);
                        let _ = thread_tx.send(ProxyEvent::ShutdownRequested(reason));
                    }
                }
            }));
            signal_handle = Some(handle);
        }

        Ok(Self {
            _terminal: terminal,
            signal_state,
            signal_handle,
            signal_thread,
            event_tx,
            event_rx,
            _panic_restore: panic_restore,
        })
    }

    pub fn activate_session(&self, child_pid: u32) -> ActiveSessionGuard {
        self.signal_state
            .active_child_pid
            .store(child_pid as i32, Ordering::SeqCst);
        ActiveSessionGuard {
            state: self.signal_state.clone(),
        }
    }

    pub fn shutdown_requested(&self) -> Option<ShutdownReason> {
        self.signal_state.shutdown_requested()
    }

    pub fn record_event(&self, event: ProxyEvent) {
        let _ = self.event_tx.send(event);
    }

    pub fn drain_events(&self) -> Vec<ProxyEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Drop for ProxyRuntime {
    fn drop(&mut self) {
        if let Some(handle) = self.signal_handle.take() {
            handle.close();
        }
        if let Some(thread) = self.signal_thread.take() {
            let _ = thread.join();
        }
    }
}
