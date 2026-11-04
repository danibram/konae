//! The Kernel alone: it opens every app folder under `KONAE_DATA_DIR/apps`
//! and serves it with all its primitives, and answers the operator commands
//! `restore`, `reconcile` and the file store commands.

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
    let confirmation = std::env::var(kernel::folder::CONFIRM_SCHEMA).ok();
    for (id, opened) in kernel::folder::open_all(&running.state, confirmation.as_deref()).await? {
        match opened {
            Ok(deployment) => println!("Serving {} at {}", deployment.project_name, deployment.url),
            Err(error_value) => eprintln!("Not serving apps/{id}: {error_value:#}"),
        }
    }
    kernel::folder::follow_settings(running.state.clone());
    kernel::listen(&startup, running.state.clone(), axum::Router::new(), "Konae Kernel").await
}
