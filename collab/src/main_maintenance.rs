use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsRawFd;

pub(crate) fn user_terminal() -> anyhow::Result<File> {
    if [
        "CODEX_SESSION_ID",
        "CODEX_THREAD_ID",
        "DSH_SESSION_ID",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "AGY_SESSION_ID",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        anyhow::bail!("USER_MAINTENANCE_REQUIRED: daemon lifecycle is user-managed");
    }
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| {
            anyhow::anyhow!("USER_MAINTENANCE_REQUIRED: daemon lifecycle is user-managed")
        })?;
    if !terminal.is_terminal() {
        anyhow::bail!("USER_MAINTENANCE_REQUIRED: daemon lifecycle is user-managed");
    }
    Ok(terminal)
}

struct RestoreTerminal {
    fd: std::os::fd::RawFd,
    previous: libc::termios,
}

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.previous);
        }
    }
}

pub(crate) fn read_password() -> anyhow::Result<crate::proto::MaintenancePassword> {
    let mut terminal = user_terminal()?;
    let fd = terminal.as_raw_fd();
    let mut previous = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut previous) } != 0 {
        anyhow::bail!("USER_MAINTENANCE_REQUIRED: terminal is unavailable");
    }
    let mut hidden = previous;
    hidden.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
    hidden.c_cc[libc::VMIN] = 1;
    hidden.c_cc[libc::VTIME] = 0;
    let _restore = RestoreTerminal { fd, previous };
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
        anyhow::bail!("USER_MAINTENANCE_REQUIRED: terminal is unavailable");
    }
    terminal.write_all(b"Password: ")?;
    terminal.flush()?;
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        if terminal.read(&mut byte)? == 0 || matches!(byte[0], 3 | 4) {
            terminal.write_all(b"\n")?;
            anyhow::bail!("USER_MAINTENANCE_CANCELLED");
        }
        match byte[0] {
            b'\n' | b'\r' => break,
            8 | 127 => {
                bytes.pop();
            }
            value => bytes.push(value),
        }
    }
    terminal.write_all(b"\n")?;
    let password = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("MAINTENANCE_AUTHENTICATION_FAILED"))?;
    Ok(crate::proto::MaintenancePassword::new(password))
}
