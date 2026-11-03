//! The Kernel alone: it serves every app folder under `KONAE_DATA_DIR/apps`
//! with all its primitives, and answers the operator commands `restore`,
//! `reconcile` and the file store commands.

use kernel::installation::Installation;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let installation = Installation::kernel();
    if kernel::operator_command(&installation).await? {
        return Ok(());
    }
    let startup = kernel::Startup::from_env()?;
    // Without KONAE_MAIL_SECRET, queued links are sealed with this key.
    let mail_key = std::env::var("KONAE_ADMIN_KEY").ok().filter(|key| !key.is_empty()).unwrap_or_else(|| kernel::DEFAULT_ADMIN_KEY.into());
    let running = kernel::start(&startup, installation, &mail_key, Vec::new(), None).await?;
    kernel::listen(&startup, running.state.clone(), axum::Router::new(), "Konae Kernel").await
}
