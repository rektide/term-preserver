// pattern: Imperative Shell

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

    pub fn saved_original(&self) -> Option<(std::os::fd::RawFd, Termios)> {
        self.original.clone().map(|original| (self.fd, original))
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

pub fn restore_termios(fd: std::os::fd::RawFd, original: &Termios) {
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let _ = tcsetattr(borrowed, SetArg::TCSANOW, original);
}

pub fn terminal_size<F: AsFd>(stdout: &F) -> eyre::Result<Size> {
    let ws = query_winsize(stdout)?;
    Ok(Size::new(ws.ws_row, ws.ws_col))
}

pub fn query_winsize<F: AsFd>(fd: &F) -> eyre::Result<nix::libc::winsize> {
    let raw_fd = fd.as_fd().as_raw_fd();
    let ws: nix::libc::winsize = unsafe {
        let mut ws = std::mem::zeroed();
        let ret = nix::libc::ioctl(raw_fd, nix::libc::TIOCGWINSZ as _, &mut ws);
        if ret == -1 {
            return Err(eyre::eyre!("TIOCGWINSZ ioctl failed"));
        }
        ws
    };
    Ok(ws)
}

pub fn set_pty_size(fd: std::os::fd::RawFd, ws: &nix::libc::winsize) -> eyre::Result<()> {
    let ret = unsafe { nix::libc::ioctl(fd, nix::libc::TIOCSWINSZ as _, ws) };
    if ret == -1 {
        Err(eyre::eyre!("TIOCSWINSZ ioctl failed"))
    } else {
        Ok(())
    }
}
