use kaduox_ssh_core::{
    Authentication, ConnectionProgress, JumpAuthFuture, JumpAuthProvider, JumpAuthRequest,
};

#[derive(Debug, Default)]
pub(crate) struct InteractiveJumpAuth;

impl JumpAuthProvider for InteractiveJumpAuth {
    fn authentication<'a>(&'a mut self, request: JumpAuthRequest) -> JumpAuthFuture<'a> {
        Box::pin(async move {
            if request.attempt == 1 {
                return Ok(Some(Authentication::Auto {
                    identity_files: request.jump.identity_files.clone(),
                    passphrase: None,
                }));
            }

            let prompt = format!(
                "Jump {}/{} {} ({}@{}:{}) password, attempt {}: ",
                request.index + 1,
                request.total,
                request.jump.alias,
                request.jump.username,
                request.jump.host,
                request.jump.port,
                request.attempt
            );
            let password = tokio::task::spawn_blocking(move || rpassword::prompt_password(prompt))
                .await??;
            if password.is_empty() {
                return Ok(None);
            }
            Ok(Some(Authentication::Password(password)))
        })
    }
}

pub(crate) async fn print_progress(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<ConnectionProgress>,
) {
    while let Some(event) = receiver.recv().await {
        match event {
            ConnectionProgress::JumpConnecting {
                index,
                total,
                alias,
                host,
                port,
            } => eprintln!(
                "[jump {}/{}] connecting {} -> {}:{}",
                index + 1,
                total,
                terminal_safe(&alias),
                terminal_safe(&host),
                port
            ),
            ConnectionProgress::JumpConnected { index, total, alias } => eprintln!(
                "[jump {}/{}] transport ready: {}",
                index + 1,
                total,
                terminal_safe(&alias)
            ),
            ConnectionProgress::JumpAuthenticating {
                index,
                total,
                alias,
                attempt,
                method,
            } => eprintln!(
                "[jump {}/{}] authenticating {} via {} (attempt {})",
                index + 1,
                total,
                terminal_safe(&alias),
                method.as_str(),
                attempt
            ),
            ConnectionProgress::JumpAuthenticationFailed {
                index,
                total,
                alias,
                attempt,
            } => eprintln!(
                "[jump {}/{}] authentication failed for {} (attempt {})",
                index + 1,
                total,
                terminal_safe(&alias),
                attempt
            ),
            ConnectionProgress::JumpAuthenticated {
                index,
                total,
                alias,
                method,
            } => eprintln!(
                "[jump {}/{}] authenticated {} via {}",
                index + 1,
                total,
                terminal_safe(&alias),
                method.as_str()
            ),
            ConnectionProgress::FinalConnecting { alias, host, port } => eprintln!(
                "[target] connecting {} -> {}:{}",
                terminal_safe(&alias),
                terminal_safe(&host),
                port
            ),
            ConnectionProgress::FinalAuthenticating {
                alias,
                user,
                method,
            } => eprintln!(
                "[target] authenticating {} as {} via {}",
                terminal_safe(&alias),
                terminal_safe(&user),
                method.as_str()
            ),
            ConnectionProgress::Connected { alias } => {
                eprintln!("[target] connected {}", terminal_safe(&alias));
            }
        }
    }
}

fn terminal_safe(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}
