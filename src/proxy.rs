// Imperative shell: PTY relay helpers and child process orchestration.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::osc::TitleObserver;
use crate::terminal;

const BUF_SIZE: usize = 8192;

#[allow(dead_code)]
pub fn copy_retrying_eintr<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> io::Result<u64> {
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
    title_state: Arc<Mutex<Option<String>>>,
) -> io::Result<u64> {
    let mut observer = TitleObserver::new();
    let mut buf = [0u8; BUF_SIZE];
    let mut total: u64 = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        for &byte in &buf[..n] {
            if let Some(title) = observer.advance(byte) {
                if let Ok(mut guard) = title_state.lock() {
                    *guard = Some(title);
                }
            }
        }
        writer.write_all(&buf[..n])?;
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

        let stdin_borrowed =
            unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) };
        let mut fds =
            [nix::poll::PollFd::new(stdin_borrowed, nix::poll::PollFlags::POLLIN)];
        match nix::poll::poll(&mut fds, 100u16) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(io::Error::other(e)),
        }

        if !child_alive.load(Ordering::Relaxed) {
            return Ok(total);
        }

        let stdin_borrowed =
            unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) };
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

pub fn run(config: Config) -> eyre::Result<(i32, Arc<Mutex<Option<String>>>)> {
    if config.command.is_empty() {
        eyre::bail!("no command specified");
    }

    let program = &config.command[0];
    let args = &config.command[1..];

    let (pty, pts) = pty_process::blocking::open()?;

    let size = terminal::terminal_size(&io::stdout())
        .unwrap_or_else(|_| pty_process::Size::new(24, 80));
    pty.resize(size)?;

    let mut child = pty_process::blocking::Command::new(program)
        .args(args)
        .spawn(pts)?;

    let _guard = terminal::RawTerminalGuard::enable(&io::stdin())?;

    let child_alive = Arc::new(AtomicBool::new(true));
    let title_state: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stdin_fd = io::stdin().as_raw_fd();

    let pty_write_fd = unsafe { dup_fd(pty.as_raw_fd())? };
    let pty_read_fd = unsafe { dup_fd(pty.as_raw_fd())? };

    drop(pty);

    let alive_for_stdin = child_alive.clone();
    let stdin_thread = std::thread::spawn(move || {
        let mut pty_writer = unsafe { std::fs::File::from_raw_fd(pty_write_fd) };
        copy_stdin_to_pty_until_cancelled(stdin_fd, &mut pty_writer, &alive_for_stdin)
    });

    let title_state_for_thread = title_state.clone();
    let stdout_thread = std::thread::spawn(move || {
        let mut pty_reader = unsafe { std::fs::File::from_raw_fd(pty_read_fd) };
        let mut stdout = io::stdout();
        copy_pty_to_stdout_with_titles(&mut pty_reader, &mut stdout, title_state_for_thread)
    });

    let exit_status = child.wait()?;
    child_alive.store(false, Ordering::Relaxed);

    match stdout_thread.join() {
        Ok(Ok(_)) => {}
        Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(eyre::eyre!("PTY→stdout thread panicked")),
    }

    match stdin_thread.join() {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {}
        Err(_) => {}
    }

    match exit_status.code() {
        Some(code) => Ok((code, title_state)),
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                Ok((128 + exit_status.signal().unwrap_or(0), title_state))
            }
            #[cfg(not(unix))]
            {
                Ok((1, title_state))
            }
        }
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
}
