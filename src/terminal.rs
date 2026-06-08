// Imperative shell: terminal raw-mode management and size queries.

use std::os::fd::{AsFd, AsRawFd};

use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg, Termios};
use pty_process::Size;

pub struct RawTerminalGuard {
    original: Option<Termios>,
    fd: std::os::fd::RawFd,
}

impl RawTerminalGuard {
    pub fn enable<F: AsFd>(stdin: &F) -> eyre::Result<Self> {
        let fd = stdin.as_fd().as_raw_fd();
        let original = match tcgetattr(stdin) {
            Ok(t) => {
                let mut raw = t.clone();
                cfmakeraw(&mut raw);
                tcsetattr(stdin, SetArg::TCSANOW, &raw)?;
                Some(t)
            }
            Err(_) => None,
        };
        Ok(Self { original, fd })
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        if let Some(ref original) = self.original {
            let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(self.fd) };
            let _ = tcsetattr(borrowed, SetArg::TCSANOW, original);
        }
    }
}

pub fn terminal_size<F: AsFd>(stdout: &F) -> eyre::Result<Size> {
    let fd = stdout.as_fd().as_raw_fd();
    let ws: nix::libc::winsize = unsafe {
        let mut ws = std::mem::zeroed();
        let ret = nix::libc::ioctl(fd, nix::libc::TIOCGWINSZ as _, &mut ws);
        if ret == -1 {
            return Err(eyre::eyre!("TIOCGWINSZ ioctl failed"));
        }
        ws
    };
    Ok(Size::new(ws.ws_row, ws.ws_col))
}
