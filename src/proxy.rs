// pattern: Imperative Shell

use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::sync::Arc;

use eyre::WrapErr;

use crate::config::Config;
use crate::filter::FilterSet;
use crate::prompt::{self, PromptDecision, PromptPolicy};
use crate::runtime::{ProxyEvent, ProxyRuntime};
use crate::session::{ChildSession, CommandSpec, SessionExit};
use crate::signals::SignalPolicy;
enum PromptInput {
    Key(u8),
    Timeout,
    Closed,
}

pub struct RetryLoop {
    command: CommandSpec,
    prompt_policy: PromptPolicy,
    reemit_title: bool,
    print_gone_title: bool,
    filters: Arc<FilterSet>,
}

impl RetryLoop {
    pub fn new(
        command: CommandSpec,
        reemit_title: bool,
        print_gone_title: bool,
        filters: Arc<FilterSet>,
    ) -> Self {
        Self {
            command,
            prompt_policy: PromptPolicy,
            reemit_title,
            print_gone_title,
            filters,
        }
    }

    pub fn run(&mut self, runtime: &mut ProxyRuntime) -> eyre::Result<i32> {
        loop {
            if let Some(reason) = runtime.shutdown_requested() {
                return Ok(reason.exit_code());
            }

            let exit = ChildSession::run(self.command.clone(), runtime, self.filters.clone())?;
            runtime.record_event(ProxyEvent::ChildExited(Box::new(SessionExit {
                command: exit.command.clone(),
                exit_code: exit.exit_code,
                raw_status: exit.raw_status,
                child_pid: exit.child_pid,
                observed: exit.observed.clone(),
                relay_report: exit.relay_report.clone(),
            })));

            if let Some(reason) = runtime.shutdown_requested() {
                return Ok(reason.exit_code());
            }

            self.write_post_exit_output(&exit)?;

            loop {
                if let Some(reason) = runtime.shutdown_requested() {
                    return Ok(reason.exit_code());
                }

                match read_prompt_input(runtime)? {
                    PromptInput::Timeout => continue,
                    PromptInput::Closed => return Ok(exit.exit_code),
                    PromptInput::Key(byte) => {
                        runtime.record_event(ProxyEvent::PromptKey(byte));
                        match self.prompt_policy.decide_key(byte, &exit) {
                            PromptDecision::Retry => break,
                            PromptDecision::Exit(code) => return Ok(code),
                            PromptDecision::Ignore => continue,
                        }
                    }
                }
            }
        }
    }

    fn write_post_exit_output(&self, exit: &SessionExit) -> eyre::Result<()> {
        // Invisible restore: re-emit captured OSC state (title/icon/color).
        if self.reemit_title {
            let seq = prompt::reemit_sequences(&exit.observed);
            if !seq.is_empty() {
                io::stdout().write_all(&seq)?;
                io::stdout().flush()?;
            }
        }

        // Visible record: print "gone <title> (pid N)" so the user can see what
        // the exiting process owned. Orthogonal to the invisible re-emit above.
        if self.print_gone_title {
            if let Some(line) = prompt::format_gone_line(&exit.observed, exit.child_pid) {
                io::stdout().write_all(line.as_bytes())?;
                io::stdout().flush()?;
            }
        }

        let prompt_text = self.prompt_policy.format_prompt(exit);
        io::stdout().write_all(prompt_text.as_bytes())?;
        io::stdout().flush()?;
        Ok(())
    }
}

fn read_prompt_input(runtime: &mut ProxyRuntime) -> eyre::Result<PromptInput> {
    let _ = runtime.drain_events();
    let stdin_borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(io::stdin().as_raw_fd()) };
    let mut fds = [nix::poll::PollFd::new(
        stdin_borrowed,
        nix::poll::PollFlags::POLLIN,
    )];
    match nix::poll::poll(&mut fds, 100u16) {
        Ok(0) => return Ok(PromptInput::Timeout),
        Ok(_) => {}
        Err(nix::errno::Errno::EINTR) => return Ok(PromptInput::Timeout),
        Err(e) => return Err(e).wrap_err("polling prompt input"),
    }

    let mut buf = [0u8; 1];
    let stdin_borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(io::stdin().as_raw_fd()) };
    match nix::unistd::read(stdin_borrowed, &mut buf) {
        Ok(0) => Ok(PromptInput::Closed),
        Ok(_) => Ok(PromptInput::Key(buf[0])),
        Err(nix::errno::Errno::EINTR) => Ok(PromptInput::Timeout),
        Err(e) => Err(e).wrap_err("reading prompt input"),
    }
}

pub fn run(config: Config) -> eyre::Result<i32> {
    let command = if config.launch.is_active() {
        let env_shell = std::env::var("SHELL").ok();
        config
            .launch
            .resolve(&config.command, env_shell.as_deref())?
    } else {
        CommandSpec::try_from(config.command.clone())?
    };
    let signal_policy = SignalPolicy::from_config(&config).wrap_err("parsing signal policy")?;
    let filters = Arc::new(
        FilterSet::compile(&config.title_filters).wrap_err("compiling title filters")?,
    );
    let mut runtime = ProxyRuntime::enter(&signal_policy)?;
    let mut retry_loop = RetryLoop::new(command, config.reemit_title, config.print_gone_title, filters);
    retry_loop.run(&mut runtime)
}
