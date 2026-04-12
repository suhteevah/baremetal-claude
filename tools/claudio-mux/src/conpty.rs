use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize, Child};
use std::io::Write;

pub struct PtyHandle {
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub writer: Box<dyn Write + Send>,
}

pub fn spawn_shell(cols: u16, rows: u16, shell: &str, args: &[String]) -> Result<PtyHandle> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = CommandBuilder::new(shell);
    for arg in args {
        cmd.arg(arg);
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let writer = pair.master.take_writer()?;
    Ok(PtyHandle { master: pair.master, child, writer })
}

pub fn spawn_agent(cols: u16, rows: u16, agent: &str, args: &[String]) -> Result<PtyHandle> {
    spawn_shell(cols, rows, agent, args)
}
