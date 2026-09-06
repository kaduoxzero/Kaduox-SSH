use anyhow::Result;
use kaduox_ssh_daemon::{DaemonState, default_endpoint_path, serve_default};

#[tokio::main]
async fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--help" | "-h") => {
            println!(
                "kssh-daemon\n\nRun the per-user Kaduox SSH connection-reuse daemon in the foreground."
            );
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("kssh-daemon {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(argument) => anyhow::bail!("unknown kssh-daemon argument {argument:?}"),
        None => {}
    }

    let endpoint = default_endpoint_path()?;
    eprintln!("kssh-daemon listening on {}", endpoint.display());
    serve_default(DaemonState::default()).await
}
