use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

use crate::{Authentication, JumpHost};

pub const MAX_JUMP_AUTH_ATTEMPTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationKind {
    Password,
    KeyboardInteractive,
    PrivateKey,
    Agent,
    Auto,
}

impl AuthenticationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::KeyboardInteractive => "keyboard-interactive",
            Self::PrivateKey => "private-key",
            Self::Agent => "agent",
            Self::Auto => "auto",
        }
    }
}

pub fn authentication_kind(authentication: &Authentication) -> AuthenticationKind {
    match authentication {
        Authentication::Password(_) => AuthenticationKind::Password,
        Authentication::KeyboardInteractive(_) => AuthenticationKind::KeyboardInteractive,
        Authentication::PrivateKey { .. } => AuthenticationKind::PrivateKey,
        Authentication::Agent => AuthenticationKind::Agent,
        Authentication::Auto { .. } => AuthenticationKind::Auto,
    }
}

#[derive(Debug, Clone)]
pub struct JumpAuthRequest {
    /// Zero-based position in the resolved jump chain.
    pub index: usize,
    pub total: usize,
    pub attempt: usize,
    pub jump: JumpHost,
    /// True after the previous authentication object returned an explicit
    /// authentication failure on this already-established SSH jump session.
    pub previous_failed: bool,
}

pub type JumpAuthFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Authentication>>> + Send + 'a>>;

/// Supplies credentials for one already-connected jump-host SSH session.
///
/// Returning `None` aborts authentication for that hop. Implementations may
/// prompt interactively, ask an IPC client, or return deterministic credentials.
/// Core caps requests at `MAX_JUMP_AUTH_ATTEMPTS` so a buggy provider cannot
/// create an unbounded retry loop.
pub trait JumpAuthProvider: Send {
    fn authentication<'a>(&'a mut self, request: JumpAuthRequest) -> JumpAuthFuture<'a>;
}

#[derive(Debug, Default)]
pub struct AutoJumpAuthProvider;

impl JumpAuthProvider for AutoJumpAuthProvider {
    fn authentication<'a>(&'a mut self, request: JumpAuthRequest) -> JumpAuthFuture<'a> {
        Box::pin(async move {
            if request.attempt > 1 {
                return Ok(None);
            }
            Ok(Some(Authentication::Auto {
                identity_files: request.jump.identity_files,
                passphrase: None,
            }))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionProgress {
    JumpConnecting {
        index: usize,
        total: usize,
        alias: String,
        host: String,
        port: u16,
    },
    JumpConnected {
        index: usize,
        total: usize,
        alias: String,
    },
    JumpAuthenticating {
        index: usize,
        total: usize,
        alias: String,
        attempt: usize,
        method: AuthenticationKind,
    },
    JumpAuthenticationFailed {
        index: usize,
        total: usize,
        alias: String,
        attempt: usize,
    },
    JumpAuthenticated {
        index: usize,
        total: usize,
        alias: String,
        method: AuthenticationKind,
    },
    FinalConnecting {
        alias: String,
        host: String,
        port: u16,
    },
    FinalAuthenticating {
        alias: String,
        user: String,
        method: AuthenticationKind,
    },
    Connected {
        alias: String,
    },
}

pub(crate) fn emit_progress(
    progress: Option<&tokio::sync::mpsc::UnboundedSender<ConnectionProgress>>,
    event: ConnectionProgress,
) {
    if let Some(progress) = progress {
        // Connection progress is observational. A frontend going away must not
        // turn a valid SSH connection attempt into a transport failure.
        let _ = progress.send(event);
    }
}
