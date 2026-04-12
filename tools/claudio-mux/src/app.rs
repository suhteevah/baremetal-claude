use anyhow::Result;
use tokio::sync::mpsc;
use terminal_core::{RouterOutcome, DashboardCommand, KeyEvent};
use terminal_ansi::AnsiRenderer;
use crate::config::Config;
use crate::host::Host;
use crate::session::{Session, PtyEvent};
use crate::render;

pub async fn run(config: Config, session_name: String) -> Result<()> {
    let mut host = Host::new()?;
    let (cols, rows) = Host::size()?;

    let (pty_tx, mut pty_rx) = mpsc::channel::<PtyEvent>(256);
    let (key_tx, mut key_rx) = mpsc::channel::<KeyEvent>(64);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(8);

    Host::spawn_input_reader(key_tx, resize_tx);

    let mut session = Session::new(cols, rows, config, session_name, &pty_tx)?;
    let mut renderer = AnsiRenderer::new(cols, rows);

    render::flush(&session, &mut renderer, &mut host)?;

    loop {
        tokio::select! {
            biased;

            _ = tokio::signal::ctrl_c() => {
                tracing::info!("ctrl-c received, exiting");
                break;
            }

            Some(key) = key_rx.recv() => {
                match session.router.handle_key(key) {
                    RouterOutcome::Command(DashboardCommand::Quit) => {
                        tracing::info!("quit command");
                        break;
                    }
                    RouterOutcome::Command(cmd) => {
                        session.apply_command(cmd, &pty_tx).await?;
                    }
                    RouterOutcome::ForwardToPane => {
                        session.forward_to_focused(key).await?;
                    }
                    RouterOutcome::Swallow => {}
                }
            }

            Some(evt) = pty_rx.recv() => {
                match evt {
                    PtyEvent::Output { pane_id, bytes } => {
                        session.feed_pane(pane_id, &bytes);
                    }
                    PtyEvent::Exited { pane_id } => {
                        session.mark_pane_exited(pane_id);
                        if session.pane_count() == 0 {
                            break;
                        }
                    }
                }
            }

            Some((cols, rows)) = resize_rx.recv() => {
                session.resize(cols, rows)?;
                renderer.resize(cols, rows);
            }
        }

        render::flush(&session, &mut renderer, &mut host)?;
    }

    drop(session);
    drop(host);
    Ok(())
}
