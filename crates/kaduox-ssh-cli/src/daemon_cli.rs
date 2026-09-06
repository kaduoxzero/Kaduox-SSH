use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{Authentication, RemoteCommandSpec};
use kaduox_ssh_daemon::{
    AuthRequest, DaemonClient, DaemonExecOutcome, DaemonJumpAuthChallenge,
    DaemonJumpAuthFuture, DaemonJumpAuthProvider, DaemonShellOutcome,
};
use tokio::sync::watch;

use crate::{Cli, Command, RawModeGuard, parse_remote_environment, resolve_authentication};

pub async fn try_run(cli: &Cli, command: &Command) -> Result<bool> {
    if !eligible(cli, command) {
        return Ok(false);
    }

    match command {
        Command::Exec {
            as_user,
            cwd,
            environment,
            command,
        } => {
            let (program, arguments) = command
                .split_first()
                .context("remote command requires a program")?;
            let environment = environment
                .iter()
                .map(|value| parse_remote_environment(value))
                .collect::<Result<Vec<_>>>()?;
            let rendered = RemoteCommandSpec {
                program: program.clone(),
                arguments: arguments.to_vec(),
                environment,
                working_directory: cwd.clone(),
            }
            .render_posix()?;
            run_exec(cli, &rendered, as_user.as_deref()).await
        }
        Command::Shell { as_user } => run_shell(cli, as_user.as_deref()).await,
        _ => Ok(false),
    }
}

fn eligible(cli: &Cli, command: &Command) -> bool {
    matches!(command, Command::Exec { .. } | Command::Shell { .. })
        && cli.port.is_none()
        && cli.user.is_none()
        && cli.jump.is_none()
        && cli.proxy_command.is_none()
        && cli.host_key.is_none()
        && !cli.forward_agent
        && cli.local_forward.is_empty()
        && cli.remote_forward.is_empty()
        && cli.dynamic_forward.is_empty()
        && !cli.host.contains('@')
}

struct InteractiveDaemonJumpAuth;

impl DaemonJumpAuthProvider for InteractiveDaemonJumpAuth {
    fn authentication<'a>(
        &'a mut self,
        challenge: DaemonJumpAuthChallenge,
    ) -> DaemonJumpAuthFuture<'a> {
        Box::pin(async move {
            let raw_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
            if raw_enabled {
                crossterm::terminal::disable_raw_mode()?;
            }
            let prompt = format!(
                "Jump {}/{} {} ({}:{}) password, attempt {}: ",
                challenge.index + 1,
                challenge.total,
                challenge.alias,
                challenge.host,
                challenge.port,
                challenge.attempt,
            );
            let secret = rpassword::prompt_password(prompt);
            if raw_enabled {
                crossterm::terminal::enable_raw_mode()?;
            }
            let secret = secret?;
            if secret.is_empty() {
                return Ok(None);
            }
            Ok(Some(AuthRequest::Password(secret)))
        })
    }
}

async fn run_exec(cli: &Cli, command: &str, as_user: Option<&str>) -> Result<bool> {
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let first = DaemonClient::exec(
        &cli.host,
        None,
        command,
        as_user,
        &mut stdout,
        &mut stderr,
    )
    .await;
    let first = match first {
        Ok(outcome) => outcome,
        Err(error) if DaemonClient::is_unavailable(&error) => return Ok(false),
        Err(error) => return Err(error.context("daemon exec request failed")),
    };

    let outcome = match first {
        DaemonExecOutcome::Completed { .. } => first,
        DaemonExecOutcome::AuthRequired => {
            let config = kaduox_ssh_hosts::HostStore::open_default()
                .and_then(|store| {
                    kaduox_ssh_hosts::resolve_host(store.database(), &cli.host, None, None)
                })?
                .config;
            let authentication = resolve_authentication(cli, &config)?;
            let mut jump_auth = InteractiveDaemonJumpAuth;
            DaemonClient::exec_with_jump_auth(
                &cli.host,
                Some(auth_request(authentication)),
                command,
                as_user,
                &mut stdout,
                &mut stderr,
                Some(&mut jump_auth),
            )
            .await?
        }
    };

    match outcome {
        DaemonExecOutcome::AuthRequired => {
            bail!("daemon requested authentication twice for {:?}", cli.host)
        }
        DaemonExecOutcome::Completed {
            exit_status,
            reused,
        } => {
            if reused {
                eprintln!("reused daemon SSH transport for {}", cli.host);
            }
            match exit_status {
                Some(0) => Ok(true),
                Some(status) => bail!("remote command exited with status {status}"),
                None => bail!("remote command closed without an SSH exit status"),
            }
        }
    }
}

async fn run_shell(cli: &Cli, as_user: Option<&str>) -> Result<bool> {
    let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_owned());

    let probe = shell_attempt(
        cli,
        None,
        &term,
        columns.into(),
        rows.into(),
        as_user,
        false,
    )
    .await;
    let probe = match probe {
        Ok(outcome) => outcome,
        Err(error) if DaemonClient::is_unavailable(&error) => return Ok(false),
        Err(error) => return Err(error.context("daemon shell request failed")),
    };

    if let Some(completed) = probe {
        return finish_shell(completed);
    }

    let config = kaduox_ssh_hosts::HostStore::open_default()
        .and_then(|store| kaduox_ssh_hosts::resolve_host(store.database(), &cli.host, None, None))?
        .config;
    let authentication = resolve_authentication(cli, &config)?;
    let outcome = shell_attempt(
        cli,
        Some(auth_request(authentication)),
        &term,
        columns.into(),
        rows.into(),
        as_user,
        true,
    )
    .await?
    .context("daemon requested authentication twice for shell")?;
    finish_shell(outcome)
}

#[allow(clippy::too_many_arguments)]
async fn shell_attempt(
    cli: &Cli,
    auth: Option<AuthRequest>,
    term: &str,
    columns: u32,
    rows: u32,
    as_user: Option<&str>,
    interactive: bool,
) -> Result<Option<DaemonShellOutcome>> {
    if !interactive {
        // Check endpoint availability before raw mode so an absent daemon falls
        // back to direct SSH without touching terminal state.
        DaemonClient::ping().await?;
    }

    let (resize_tx, resize_rx) = watch::channel((columns, rows));
    let resize_task = tokio::spawn(track_terminal_size(resize_tx));
    let _raw_mode = RawModeGuard::enable()?;
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let outcome = if interactive {
        let mut jump_auth = InteractiveDaemonJumpAuth;
        DaemonClient::shell_with_jump_auth(
            &cli.host,
            auth,
            term,
            columns,
            rows,
            as_user,
            &mut stdin,
            &mut stdout,
            Some(resize_rx),
            Some(&mut jump_auth),
        )
        .await
    } else {
        DaemonClient::shell(
            &cli.host,
            auth,
            term,
            columns,
            rows,
            as_user,
            &mut stdin,
            &mut stdout,
            Some(resize_rx),
        )
        .await
    };
    resize_task.abort();
    match outcome? {
        DaemonShellOutcome::AuthRequired => Ok(None),
        completed @ DaemonShellOutcome::Completed { .. } => Ok(Some(completed)),
    }
}

fn finish_shell(outcome: DaemonShellOutcome) -> Result<bool> {
    match outcome {
        DaemonShellOutcome::AuthRequired => bail!("unexpected daemon shell authentication state"),
        DaemonShellOutcome::Completed {
            exit_status,
            reused,
        } => {
            if reused {
                eprintln!("reused daemon SSH transport");
            }
            if let Some(status) = exit_status {
                if status != 0 {
                    bail!("remote shell exited with status {status}");
                }
            }
            Ok(true)
        }
    }
}

fn auth_request(authentication: Authentication) -> AuthRequest {
    match authentication {
        Authentication::Password(secret) => AuthRequest::Password(secret),
        Authentication::KeyboardInteractive(secret) => AuthRequest::KeyboardInteractive(secret),
        Authentication::PrivateKey { path, passphrase } => {
            AuthRequest::PrivateKey { path, passphrase }
        }
        Authentication::Agent => AuthRequest::Agent,
        Authentication::Auto { .. } => AuthRequest::Auto,
    }
}

async fn track_terminal_size(sender: watch::Sender<(u32, u32)>) {
    let mut last = crossterm::terminal::size().unwrap_or((80, 24));
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let Ok(current) = crossterm::terminal::size() else {
            continue;
        };
        if current != last {
            last = current;
            let _ = sender.send((current.0.into(), current.1.into()));
        }
    }
}
