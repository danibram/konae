//! The Kernel alone: it opens every app folder under `KONAE_DATA_DIR/apps`
//! and serves it with all its primitives. With a command it does that one
//! thing instead: the operator commands on an app, `restore`, `reconcile`
//! and the file store commands.

use kernel::installation::Installation;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let installation = Installation::kernel();
    if kernel::operator_command(&installation).await? || kernel::operator::command(&installation).await? {
        return Ok(());
    }
    let startup = kernel::Startup::from_env()?;
    // Without KONAE_MAIL_SECRET, queued links are sealed with the key the first start wrote.
    let mail_key = kernel::operator::mail_key(&startup.data_root)?;
    let running = kernel::start(&startup, installation, &mail_key, Vec::new(), None).await?;
    let confirmation = std::env::var(kernel::folder::CONFIRM_SCHEMA).ok();
    for (id, opened) in kernel::folder::open_all(&running.state, confirmation.as_deref()).await? {
        match opened {
            Ok(deployment) => println!("Serving {} at {}", deployment.project_name, deployment.url),
            Err(error_value) => eprintln!("Not serving apps/{id}: {error_value:#}"),
        }
    }
    kernel::folder::follow_settings(running.state.clone());
    kernel::listen(&startup, running.state.clone(), axum::Router::new(), "Kernel").await
}
