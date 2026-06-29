// pattern: Imperative Shell

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eyre::WrapErr;

use crate::filter::FilterSet;
use crate::osc::{ByteAction, TitleObserver};
use crate::runtime::{ActiveSessionGuard, ProxyRuntime};
use crate::terminal;

const BUF_SIZE: usize = 8192;
/// Upper bound on a buffered candidate OSC sequence. If a sequence never
/// terminates (malformed stream), the relay flushes whatever it has held and
/// resets the parser rather than buffering without limit.
const MAX_PENDING: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl TryFrom<Vec<String>> for CommandSpec {
    type Error = eyre::Report;

    fn try_from(command: Vec<String>) -> Result<Self, Self::Error> {
        let mut parts = command.into_iter();
        let Some(program) = parts.next() else {
            eyre::bail!("no command specified");
        };
        Ok(Self {
            program,
            args: parts.collect(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TitleState {
    inner: Arc<Mutex<Option<String>>>,
}

impl TitleState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set(&self, title: String) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some(title);
        }
    }

    pub fn latest(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|guard| guard.clone())
    }
}

impl Default for TitleState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayEnd {
    Completed(u64),
    NormalPtyClosed,
    IgnoredStdinError,
    Panicked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayJoinReport {
    pub stdin_to_pty: RelayEnd,
    pub pty_to_stdout: RelayEnd,
}

#[derive(Debug)]
pub struct SessionExit {
    pub command: CommandSpec,
    pub exit_code: i32,
    pub raw_status: std::process::ExitStatus,
    pub last_title: Option<String>,
    pub relay_report: RelayJoinReport,
}

impl SessionExit {
    pub fn from_status(
        command: CommandSpec,
        raw_status: std::process::ExitStatus,
        last_title: Option<String>,
        relay_report: RelayJoinReport,
    ) -> Self {
        Self {
            command,
            exit_code: exit_code_from_status(raw_status),
            raw_status,
            last_title,
            relay_report,
        }
    }
}

pub fn exit_code_from_status(exit_status: std::process::ExitStatus) -> i32 {
    match exit_status.code() {
        Some(code) => code,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                128 + exit_status.signal().unwrap_or(0)
            }
            #[cfg(not(unix))]
            {
                1
            }
        }
    }
}

#[allow(dead_code)]
pub fn copy_retrying_eintr<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buf = [0u8; BUF_SIZE];
    let mut total: u64 = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..n])?;
        writer.flush()?;
        total += n as u64;
    }
}

pub fn copy_pty_to_stdout_with_titles<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    title_state: TitleState,
    filters: Arc<FilterSet>,
) -> io::Result<u64> {
    let mut observer = TitleObserver::new();
    let mut buf = [0u8; BUF_SIZE];
    // Bytes that may belong to an in-progress OSC sequence; flushed on
    // completion (unless the title is stripped) or when the parser aborts.
    let mut pending: Vec<u8> = Vec::new();
    let mut out: Vec<u8> = Vec::with_capacity(BUF_SIZE);
    let mut total: u64 = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.clear();
        for &byte in &buf[..n] {
            let step = observer.advance(byte);
            if step.flush_pending {
                out.extend_from_slice(&pending);
                pending.clear();
            }
            match step.byte {
                ByteAction::Emit => out.push(byte),
                ByteAction::Hold => pending.push(byte),
            }
            if let Some(completion) = step.completed {
                // The terminator byte was held above, so `pending` now holds
                // the entire OSC sequence.
                match completion.title {
                    Some(title) => {
                        let decision = filters.decide(&title);
                        if !decision.strip {
                            out.extend_from_slice(&pending);
                        }
                        pending.clear();
                        if !decision.ignore {
                            title_state.set(title);
                        }
                    }
                    None => {
                        // Non-title OSC or overflowed — always pass through.
                        out.extend_from_slice(&pending);
                        pending.clear();
                    }
                }
            }
            if pending.len() > MAX_PENDING {
                out.extend_from_slice(&pending);
                pending.clear();
                observer.force_reset();
            }
        }
        writer.write_all(&out)?;
        writer.flush()?;
        total += n as u64;
    }
}

pub fn copy_stdin_to_pty_until_cancelled<W: Write>(
    stdin_fd: std::os::fd::RawFd,
    pty_writer: &mut W,
    child_alive: &Arc<AtomicBool>,
) -> io::Result<u64> {
    let mut buf = [0u8; BUF_SIZE];
    let mut total: u64 = 0;
    loop {
        if !child_alive.load(Ordering::Relaxed) {
            return Ok(total);
        }

        let stdin_borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) };
        let mut fds = [nix::poll::PollFd::new(
            stdin_borrowed,
            nix::poll::PollFlags::POLLIN,
        )];
        match nix::poll::poll(&mut fds, 100u16) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(io::Error::other(e)),
        }

        if !child_alive.load(Ordering::Relaxed) {
            return Ok(total);
        }

        let stdin_borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) };
        let n = match nix::unistd::read(stdin_borrowed, &mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(io::Error::other(e)),
        };

        if !child_alive.load(Ordering::Relaxed) {
            return Ok(total);
        }

        pty_writer.write_all(&buf[..n])?;
        pty_writer.flush()?;
        total += n as u64;
    }
}

unsafe fn dup_fd(fd: std::os::fd::RawFd) -> eyre::Result<std::os::fd::RawFd> {
    let new = nix::libc::dup(fd);
    if new == -1 {
        Err(eyre::eyre!("dup failed"))
    } else {
        Ok(new)
    }
}

pub struct RelayThreads {
    child_alive: Arc<AtomicBool>,
    stdin_to_pty: JoinHandle<io::Result<u64>>,
    pty_to_stdout: JoinHandle<io::Result<u64>>,
}

impl RelayThreads {
    fn start(
        stdin_fd: std::os::fd::RawFd,
        pty_write_fd: std::os::fd::RawFd,
        pty_read_fd: std::os::fd::RawFd,
        child_alive: Arc<AtomicBool>,
        title_state: TitleState,
        filters: Arc<FilterSet>,
    ) -> Self {
        let alive_for_stdin = child_alive.clone();
        let stdin_to_pty = thread::spawn(move || {
            let mut pty_writer = unsafe { std::fs::File::from_raw_fd(pty_write_fd) };
            copy_stdin_to_pty_until_cancelled(stdin_fd, &mut pty_writer, &alive_for_stdin)
        });

        let pty_to_stdout = thread::spawn(move || {
            let mut pty_reader = unsafe { std::fs::File::from_raw_fd(pty_read_fd) };
            let mut stdout = io::stdout();
            copy_pty_to_stdout_with_titles(&mut pty_reader, &mut stdout, title_state, filters)
        });

        Self {
            child_alive,
            stdin_to_pty,
            pty_to_stdout,
        }
    }

    pub fn join_after_child_exit(self) -> eyre::Result<RelayJoinReport> {
        self.child_alive.store(false, Ordering::Relaxed);
        let pty_to_stdout = match self.pty_to_stdout.join() {
            Ok(Ok(bytes)) => RelayEnd::Completed(bytes),
            Ok(Err(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => RelayEnd::NormalPtyClosed,
            Ok(Err(ref e)) if e.raw_os_error() == Some(5) => RelayEnd::NormalPtyClosed,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(eyre::eyre!("PTY→stdout thread panicked")),
        };

        let stdin_to_pty = match self.stdin_to_pty.join() {
            Ok(Ok(bytes)) => RelayEnd::Completed(bytes),
            Ok(Err(_)) => RelayEnd::IgnoredStdinError,
            Err(_) => RelayEnd::Panicked,
        };

        Ok(RelayJoinReport {
            stdin_to_pty,
            pty_to_stdout,
        })
    }
}

pub struct SignalForwarder {
    _guard: ActiveSessionGuard,
}

impl SignalForwarder {
    fn start(runtime: &ProxyRuntime, child_pid: u32) -> Self {
        Self {
            _guard: runtime.activate_session(child_pid),
        }
    }
}

pub struct ResizeForwarder {
    child_alive: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl ResizeForwarder {
    fn start(
        pty_resize_fd: std::os::fd::RawFd,
        child_alive: Arc<AtomicBool>,
    ) -> eyre::Result<Self> {
        let mut winch = signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH])
            .wrap_err("registering SIGWINCH handler")?;
        let alive_for_resize = child_alive.clone();
        let handle = thread::spawn(move || {
            let pty_file = unsafe { std::fs::File::from_raw_fd(pty_resize_fd) };
            loop {
                if !alive_for_resize.load(Ordering::Relaxed) {
                    break;
                }
                for _sig in winch.pending() {
                    if !alive_for_resize.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(ws) = terminal::query_winsize(&io::stdout()) {
                        let _ = terminal::set_pty_size(pty_file.as_raw_fd(), &ws);
                    }
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        Ok(Self {
            child_alive,
            handle,
        })
    }

    fn stop(self) {
        self.child_alive.store(false, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

pub struct SessionForwarders {
    _signal: SignalForwarder,
    resize: ResizeForwarder,
}

impl SessionForwarders {
    fn stop(self) {
        self.resize.stop();
    }
}

pub struct ChildSession {
    command: CommandSpec,
    child: std::process::Child,
    title_state: TitleState,
    child_alive: Arc<AtomicBool>,
    relays: RelayThreads,
    forwarders: SessionForwarders,
}

impl ChildSession {
    pub fn spawn(
        command: CommandSpec,
        runtime: &ProxyRuntime,
        filters: Arc<FilterSet>,
    ) -> eyre::Result<Self> {
        let (pty, pts) = pty_process::blocking::open().wrap_err("opening PTY")?;

        let size = terminal::terminal_size(&io::stdout())
            .unwrap_or_else(|_| pty_process::Size::new(24, 80));
        pty.resize(size).wrap_err("resizing PTY")?;

        let child = pty_process::blocking::Command::new(&command.program)
            .args(&command.args)
            .spawn(pts)
            .wrap_err_with(|| format!("spawning child command: {}", command.program))?;

        let child_alive = Arc::new(AtomicBool::new(true));
        let title_state = TitleState::new();
        let stdin_fd = io::stdin().as_raw_fd();

        let pty_write_fd = unsafe { dup_fd(pty.as_raw_fd())? };
        let pty_read_fd = unsafe { dup_fd(pty.as_raw_fd())? };
        let pty_resize_fd = unsafe { dup_fd(pty.as_raw_fd())? };
        drop(pty);

        let child_pid = child.id();
        let signal = SignalForwarder::start(runtime, child_pid);
        let resize = ResizeForwarder::start(pty_resize_fd, child_alive.clone())?;
        let relays = RelayThreads::start(
            stdin_fd,
            pty_write_fd,
            pty_read_fd,
            child_alive.clone(),
            title_state.clone(),
            filters,
        );

        Ok(Self {
            command,
            child,
            title_state,
            child_alive,
            relays,
            forwarders: SessionForwarders {
                _signal: signal,
                resize,
            },
        })
    }

    pub fn wait(mut self) -> eyre::Result<SessionExit> {
        let raw_status = self.child.wait()?;
        self.child_alive.store(false, Ordering::Relaxed);
        self.forwarders.stop();
        let relay_report = self.relays.join_after_child_exit()?;
        Ok(SessionExit::from_status(
            self.command,
            raw_status,
            self.title_state.latest(),
            relay_report,
        ))
    }

    pub fn run(
        command: CommandSpec,
        runtime: &ProxyRuntime,
        filters: Arc<FilterSet>,
    ) -> eyre::Result<SessionExit> {
        Self::spawn(command, runtime, filters)?.wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct InterruptThenData {
        interrupts: usize,
        data: Cursor<Vec<u8>>,
    }

    impl InterruptThenData {
        fn new(data: Vec<u8>, interrupts: usize) -> Self {
            Self {
                interrupts,
                data: Cursor::new(data),
            }
        }
    }

    impl Read for InterruptThenData {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.interrupts > 0 {
                self.interrupts -= 1;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "EINTR"));
            }
            self.data.read(buf)
        }
    }

    #[test]
    fn copy_retrying_eintr_retries_on_interrupt() {
        let mut reader = InterruptThenData::new(b"hello".to_vec(), 3);
        let mut writer = Vec::new();
        let copied = copy_retrying_eintr(&mut reader, &mut writer).unwrap();
        assert_eq!(copied, 5);
        assert_eq!(&writer, b"hello");
    }

    #[test]
    fn command_spec_rejects_empty_command() {
        assert!(CommandSpec::try_from(Vec::<String>::new()).is_err());
    }

    #[test]
    fn command_spec_splits_program_and_args() {
        let command = CommandSpec::try_from(vec!["ssh".into(), "host".into()]).unwrap();
        assert_eq!(command.program, "ssh");
        assert_eq!(command.args, vec!["host"]);
    }

    #[test]
    fn title_state_snapshots_latest_title() {
        let state = TitleState::new();
        state.set("first".into());
        state.set("second".into());
        assert_eq!(state.latest(), Some("second".into()));
    }

    // --- title filtering at the relay -------------------------------------

    use crate::filter::{FilterSet, TitleFilterRule};

    fn run_relay(input: &[u8], filters: &FilterSet) -> (Vec<u8>, Option<String>) {
        let mut reader = Cursor::new(input.to_vec());
        let mut writer = Vec::new();
        let state = TitleState::new();
        let _ = copy_pty_to_stdout_with_titles(
            &mut reader,
            &mut writer,
            state.clone(),
            Arc::new(filters.clone()),
        )
        .unwrap();
        (writer, state.latest())
    }

    fn rule(pattern: &str, strip: bool, ignore: bool) -> TitleFilterRule {
        TitleFilterRule {
            pattern: pattern.into(),
            strip,
            ignore,
        }
    }

    #[test]
    fn no_filters_passes_through_and_captures() {
        let filters = FilterSet::empty();
        let (out, title) = run_relay(b"\x1b]2;rektide@host\x07tail", &filters);
        assert_eq!(&out, b"\x1b]2;rektide@host\x07tail");
        assert_eq!(title.as_deref(), Some("rektide@host"));
    }

    #[test]
    fn strip_removes_bytes_but_records_state_unless_ignored() {
        // strip and ignore are independent: strip=true, ignore=false drops the
        // OSC bytes from the stream but STILL records the title internally.
        let filters = FilterSet::compile(&[rule("^rektide@", true, false)]).unwrap();
        let (out, title) = run_relay(b"\x1b]2;rektide@host\x07tail", &filters);
        assert_eq!(&out, b"tail", "OSC bytes must be stripped from stream");
        assert_eq!(
            title.as_deref(),
            Some("rektide@host"),
            "ignore=false must still record the title"
        );
    }

    #[test]
    fn strip_with_ignore_drops_bytes_and_state() {
        let filters = FilterSet::compile(&[rule("^rektide@", true, true)]).unwrap();
        let (out, title) = run_relay(b"\x1b]2;rektide@host\x07tail", &filters);
        assert_eq!(&out, b"tail");
        assert!(title.is_none());
    }

    #[test]
    fn ignore_keeps_sequence_but_skips_state() {
        let filters = FilterSet::compile(&[rule("^rektide@", false, true)]).unwrap();
        let (out, title) = run_relay(b"\x1b]2;rektide@host\x07tail", &filters);
        assert_eq!(
            &out,
            b"\x1b]2;rektide@host\x07tail",
            "OSC bytes must pass through under ignore-only"
        );
        assert!(title.is_none(), "title state must not record ignored title");
    }

    #[test]
    fn non_matching_title_passes_through() {
        let filters = FilterSet::compile(&[rule("^rektide@", true, true)]).unwrap();
        let (out, title) = run_relay(b"\x1b]2;other@host\x07tail", &filters);
        assert_eq!(&out, b"\x1b]2;other@host\x07tail");
        assert_eq!(title.as_deref(), Some("other@host"));
    }

    #[test]
    fn ignore_keeps_previous_title_intact() {
        // A non-ignored title sets state; a later ignored title leaves it.
        let input = b"\x1b]2;keep\x07\x1b]2;rektide@host\x07";
        let filters = FilterSet::compile(&[rule("^rektide@", false, true)]).unwrap();
        let (out, title) = run_relay(input, &filters);
        assert_eq!(&out, input);
        assert_eq!(title.as_deref(), Some("keep"));
    }

    #[test]
    fn strip_and_ignore_combine_across_rules() {
        let filters = FilterSet::compile(&[
            rule("^rektide@", true, false),
            rule("@host$", false, true),
        ])
        .unwrap();
        let (out, title) = run_relay(b"\x1b]2;rektide@host\x07", &filters);
        assert!(out.is_empty(), "combined strip must drop the sequence");
        assert!(title.is_none(), "combined ignore must skip the state");
    }

    #[test]
    fn non_title_osc_passes_through_under_strip_filter() {
        // OSC 4 (color) is not a title — must never be stripped.
        let filters = FilterSet::compile(&[rule(".*", true, true)]).unwrap();
        let (out, title) = run_relay(b"\x1b]4;color\x07", &filters);
        assert_eq!(&out, b"\x1b]4;color\x07");
        assert!(title.is_none());
    }
}
