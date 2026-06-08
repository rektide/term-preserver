// Imperative shell: PTY relay helpers and child process orchestration.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::Config;
use crate::osc::TitleObserver;
use crate::prompt;
use crate::signals;
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

fn exit_code_from_status(exit_status: std::process::ExitStatus) -> i32 {
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

fn run_child_session(
    program: &str,
    args: &[String],
    config: &Config,
) -> eyre::Result<(i32, Arc<Mutex<Option<String>>>)> {
    let (pty, pts) = pty_process::blocking::open()?;

    let size = terminal::terminal_size(&io::stdout())
        .unwrap_or_else(|_| pty_process::Size::new(24, 80));
    pty.resize(size)?;

    let mut child = pty_process::blocking::Command::new(program)
        .args(args)
        .spawn(pts)?;

    let child_alive = Arc::new(AtomicBool::new(true));
    let title_state: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stdin_fd = io::stdin().as_raw_fd();

    let pty_write_fd = unsafe { dup_fd(pty.as_raw_fd())? };
    let pty_read_fd = unsafe { dup_fd(pty.as_raw_fd())? };
    let pty_resize_fd = unsafe { dup_fd(pty.as_raw_fd())? };
    drop(pty);

    let child_pid = child.id();

    let forward_sigs = signals::parse_forward_signals(&config.forward_signals)?;
    let sig_nums: Vec<i32> = forward_sigs.iter().map(|s| *s as i32).collect();
    let mut sig_handle = if !sig_nums.is_empty() {
        Some(signal_hook::iterator::Signals::new(&sig_nums)?)
    } else {
        None
    };

    let winch_handle =
        signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH])?;

    let alive_for_signal = child_alive.clone();
    let signal_thread = if let Some(sigs) = sig_handle.take() {
        std::thread::spawn(move || {
            let mut sigs = sigs;
            loop {
                if !alive_for_signal.load(Ordering::Relaxed) {
                    break;
                }
                for sig in sigs.pending() {
                    if !alive_for_signal.load(Ordering::Relaxed) {
                        break;
                    }
                    let nix_sig = match nix::sys::signal::Signal::try_from(sig) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let _ = signals::forward_to_process_group(child_pid, nix_sig);
                }
                thread::sleep(Duration::from_millis(50));
            }
        })
    } else {
        std::thread::spawn(|| {})
    };

    let alive_for_resize = child_alive.clone();
    let resize_thread = std::thread::spawn(move || {
        let pty_file = unsafe { std::fs::File::from_raw_fd(pty_resize_fd) };
        let mut winch = winch_handle;
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

    let _ = signal_thread.join();
    let _ = resize_thread.join();

    match stdout_thread.join() {
        Ok(Ok(_)) => {}
        Ok(Err(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => {}
        Ok(Err(ref e)) if e.raw_os_error() == Some(5) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(eyre::eyre!("PTY→stdout thread panicked")),
    }

    match stdin_thread.join() {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {}
        Err(_) => {}
    }

    Ok((exit_code_from_status(exit_status), title_state))
}

pub fn run(config: Config) -> eyre::Result<i32> {
    if config.command.is_empty() {
        eyre::bail!("no command specified");
    }

    let _guard = terminal::RawTerminalGuard::enable(&io::stdin())?;

    let program = config.command[0].clone();
    let args: Vec<String> = config.command[1..].to_vec();

    loop {
        let (exit_code, title_state) =
            run_child_session(&program, &args, &config)?;

        if config.reemit_title {
            if let Ok(guard) = title_state.lock() {
                if let Some(ref title) = *guard {
                    let seq = prompt::title_reemit_sequence(title);
                    let _ = io::stdout().write_all(&seq);
                    let _ = io::stdout().flush();
                }
            }
        }

        let prompt_text = prompt::format_status_prompt(exit_code);
        let _ = io::stdout().write_all(prompt_text.as_bytes());
        let _ = io::stdout().flush();

        loop {
            let mut buf = [0u8; 1];
            let stdin_borrowed =
                unsafe { std::os::fd::BorrowedFd::borrow_raw(io::stdin().as_raw_fd()) };
            match nix::unistd::read(stdin_borrowed, &mut buf) {
                Ok(0) => return Ok(exit_code),
                Ok(_) => {}
                Err(_) => return Ok(exit_code),
            }
            match buf[0] {
                b' ' => break,
                b'q' => return Ok(0),
                0x03 => return Ok(130),
                _ => continue,
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
