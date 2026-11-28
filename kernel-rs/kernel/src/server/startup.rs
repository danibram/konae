use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use crate::limits::{Guards, trusted_proxies};
use crate::{
    AppState, LockMap, accounts, copies, copy_bucket_from_env, domain, env, execution,
    file_store, functions, heap, hosts, installation, load_deployments, mail, path, runtime,
    schedule, security,
};

/// What a Kernel process reads from its environment before it serves.
pub struct Startup {
    pub port: u16,
    pub bind: String,
    pub hosts: hosts::Hosts,
    pub data_root: PathBuf,
    pub execution_mode: execution::ExecutionMode,
}

impl Startup {
    pub fn from_env() -> Result<Self> {
        let bind = env("KONAE_BIND", "127.0.0.1");
        let execution_mode = execution::ExecutionMode::parse(std::env::var("KONAE_EXECUTION_MODE").ok().as_deref(), &bind)?;
        Ok(Self {
            port: env("KONAE_KERNEL_PORT", "8787").parse::<u16>()?,
            bind,
            hosts: hosts::from_env()?,
            data_root: PathBuf::from(env("KONAE_DATA_DIR", "/data")),
            execution_mode,
        })
    }
}

/// A started Kernel: its state, and the lock that makes `files-migrate` and
/// `files-reconcile` refuse while it serves.
pub struct Running {
    pub state: AppState,
    pub _lock: std::fs::File,
    copy_config: Option<copies::CopyConfig>,
    _ftps: Option<crate::ftps::Runtime>,
}

/// Loads every app folder, builds the state and starts the workers: copies,
/// schedules, email and calls. `mail_key` seals queued links when
/// `KONAE_MAIL_SECRET` is unset. `own_origins` and `audit` are what the
/// starter adds: the origins it answers on and where app users' uploads and
/// deletes are recorded.
pub async fn start(
    startup: &Startup,
    installation: installation::Installation,
    mail_key: &str,
    own_origins: Vec<String>,
    audit: Option<runtime::AuditSink>,
) -> Result<Running> {
    let lock = file_store::hold_kernel_lock(&startup.data_root)?;
    let mut running = start_locked(startup, lock, installation, mail_key, own_origins, audit).await?;
    running.start_copies();
    Ok(running)
}

impl Running {
    pub fn start_copies(&mut self) {
        if let Some(config) = self.copy_config.take() {
            copies::spawn_schedule(self.state.clone(), config);
        }
    }
}

/// The binaries acquire the volume before opening accounts or reading the local mail key.
pub async fn start_locked(
    startup: &Startup,
    lock: std::fs::File,
    installation: installation::Installation,
    mail_key: &str,
    own_origins: Vec<String>,
    audit: Option<runtime::AuditSink>,
) -> Result<Running> {
    tokio::fs::create_dir_all(crate::layout::apps(&startup.data_root)).await?;
    let schedule_clock = schedule::Clock::from_env(&startup.bind)?;
    let (state, copy_config) = state_from_env(startup, installation, mail_key, own_origins, audit).await?;
    if state.deployments.read().await.values().any(|deployment| crate::modules::is_on(&deployment.modules, "documents")) {
        crate::documents::require_renderer().await?;
    }
    let deployments = state.deployments.read().await.values()
        .filter(|deployment| deployment.server.as_ref().is_some_and(|server| !server.manifest.functions.is_empty()))
        .cloned().collect::<Vec<_>>();
    for deployment in deployments {
        let id = &deployment.id;
        let prepared = async {
            let artifact = functions::read_artifact(&crate::layout::server(&state.data_root, id))?
                .context("Published Server Artifact is missing")?;
            functions::prepare_artifact(&state, Some(&artifact)).await
        }.await;
        match prepared {
            Ok(pin) => state.functions.retain_prepared(id, pin),
            Err(error) if error.is::<functions::ConfigurationError>() => return Err(error),
            Err(error) => {
                state.functions.block_preparation(id);
                eprintln!("Not serving functions for apps/{id}: {error:#}");
            }
        }
    }
    path::load(&state);
    let deployed_apps: Vec<_> = state.deployments.read().await.values().cloned().collect();
    for deployment in deployed_apps {
        if let Err(error) = crate::files::reconcile_ftps_uploads(&state, &deployment).await {
            eprintln!("Could not recover FTPS uploads for apps/{}: {error:#}", deployment.id);
        }
    }
    let own_hosts = domain::Rules::of(&state).installation_hosts;
    for deployment in state.deployments.read().await.values().filter(|deployment| own_hosts.contains(&deployment.hostname)) {
        eprintln!("Not serving apps/{}: {} is this installation's own host. Move the app with hostname_set.", deployment.id, deployment.hostname);
    }

    schedule::spawn(state.clone(), schedule_clock);
    if let Some(queue) = state.accounts.queue.clone() {


        if let Some(mailbox) = queue.local_mailbox().filter(|_| security::https(&state)) {
            eprintln!(
                "WARNING: {} serves over https with KONAE_MAIL_TRANSPORT=development. No email leaves this server. Invitations, confirmation and recovery links and notices all go to {} instead. Set KONAE_MAIL_TRANSPORT to resend or smtp.",
                state.public_origin,
                mailbox.display()
            );
        }
        mail::spawn_worker(queue, state.copies.clone(), state.clone());
    }
    crate::modules::start_durable_work(&state);
    let ftps = crate::ftps::start(&state).await?;
    Ok(Running { state, _lock: lock, copy_config, _ftps: ftps })
}

/// The state a Kernel serves with, read from the environment, without its
/// workers. An operator command holds the volume permit beside a running
/// Kernel. Also answers the copy settings the workers need.
pub async fn state_from_env(
    startup: &Startup,
    installation: installation::Installation,
    mail_key: &str,
    own_origins: Vec<String>,
    audit: Option<runtime::AuditSink>,
) -> Result<(AppState, Option<copies::CopyConfig>)> {

    heap::limit_from_env()?;
    let data_root = startup.data_root.clone();
    let public_origin = startup.hosts.public_origin.clone();
    tokio::fs::create_dir_all(crate::layout::apps(&data_root)).await?;
    let account_settings = accounts::AccountSettings::from_env(&data_root, mail_key, &public_origin, &installation, startup.execution_mode)?;
    let (s3, data_bucket, copy_config) = if startup.execution_mode.is_test() {
        (None, String::new(), None)
    } else {
        let (s3, data_bucket) = match copy_bucket_from_env()? {
            Some((client, bucket)) => (Some(client), bucket),
            None => (None, String::new()),
        };
        let copy_config = copies::CopyConfig::from_env(&data_root, s3.is_some())?;
        (s3, data_bucket, copy_config)
    };
    let deployments = load_deployments(&data_root, &public_origin).await?;
    let files = if startup.execution_mode.is_test() {
        file_store::Files::new(Arc::new(file_store::LocalFolder::new(&data_root)), file_store::FileLimits::from_env()?)
    } else {
        file_store::Files::from_env(&data_root).await?
    };
    let copies = copies::CopyStatus::open(copy_config.as_ref(), &copies::status_directory(&data_root),
        &file_store::env_var("KONAE_S3_ENDPOINT").unwrap_or_default(), &data_bucket);
    let otlp = functions::otlp::Exporter::from_env()?;
    let state = AppState {
        execution_mode: startup.execution_mode,
        deployments: Arc::new(RwLock::new(deployments)),
        db_locks: LockMap::default(),
        s3,
        data_bucket,
        data_root,
        files,
        copies: Arc::new(copies),
        public_origin,
        own_origins,
        host_suffix: startup.hosts.host_suffix.clone(),
        path_origin: startup.hosts.path_origin.clone(),
        guards: Arc::new(Guards::new(trusted_proxies(
            &std::env::var("KONAE_TRUSTED_PROXIES").unwrap_or_default(),
        )?)),
        accounts: account_settings,
        functions: Arc::new(functions::runtime::Runtime::from_env()?),
        dispatcher: Arc::new(execution::Dispatcher::default()),
        otlp,
        audit,
        installation: Arc::new(installation),
    };
    Ok((state, copy_config))
}

/// Binds the Kernel's port. A port another process holds is refused with its
/// number and the variable that picks another; any other failure keeps the
/// system's text beside the address.
pub async fn bind(startup: &Startup) -> Result<tokio::net::TcpListener> {
    let (host, port) = (startup.bind.as_str(), startup.port);
    tokio::net::TcpListener::bind((host, port)).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AddrInUse {
            anyhow::anyhow!("Port {port} on {host} is already in use. Another Konae or Kernel may hold it. Set KONAE_KERNEL_PORT to a free port, or pass --port to konae dev.")
        } else {
            anyhow::anyhow!("Could not listen on {host}:{port}: {error}")
        }
    })
}

#[cfg(test)]
mod bind_tests {
    use super::*;

    #[tokio::test]
    async fn a_taken_port_is_refused_with_its_number_and_how_to_pick_another() {
        let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = held.local_addr().unwrap().port();
        let startup = Startup { port, bind: "127.0.0.1".into(), hosts: hosts::resolve(|_| None).unwrap(), data_root: PathBuf::new(), execution_mode: execution::ExecutionMode::Live };
        let message = bind(&startup).await.unwrap_err().to_string();
        assert!(message.contains(&format!("Port {port} on 127.0.0.1 is already in use")), "{message}");
        assert!(message.contains("KONAE_KERNEL_PORT") && message.contains("--port"), "{message}");
    }

    #[tokio::test]
    async fn another_bind_failure_keeps_the_system_text_and_the_address() {
        let startup = Startup { port: 0, bind: "256.0.0.1".into(), hosts: hosts::resolve(|_| None).unwrap(), data_root: PathBuf::new(), execution_mode: execution::ExecutionMode::Live };
        let message = bind(&startup).await.unwrap_err().to_string();
        assert!(message.starts_with("Could not listen on 256.0.0.1:0: "), "{message}");
    }
}

#[cfg(test)]
mod function_tests {
    use super::*;
    use crate::functions::tests::{publish, release};
    use std::{fs, process::{Child, Command, Stdio}, time::{Duration, Instant}};

    struct Server(Child);

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[tokio::test]
    #[ignore = "requires the Konae binary, preparer and a real function worker"]
    async fn startup_refuses_one_apps_functions_and_repairs_a_cold_cache_miss() {
        let runner = std::env::var_os("KONAE_TEST_FUNCTION_WORKER").unwrap();
        let helper = std::env::var_os("KONAE_TEST_FUNCTION_PREPARER")
            .or_else(|| std::env::var_os("KONAE_FUNCTIONS_PREPARER")).unwrap();
        let konae = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("konae");
        let root = std::env::temp_dir().join(format!("konae-function-startup-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let cache = root.join("wasm-cache");
        fs::create_dir(&cache).unwrap();
        let state = AppState::for_test(&root);
        let bad = publish(&state, &release("bad", "Math.random(); export function handle() { return new Response('unexpected'); }")).await;
        let good = publish(&state, &release("good", "export function handle() { return new Response('ready'); }")).await;

        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let origin = format!("http://127.0.0.1:{port}");
        let log = root.join("stderr");
        let wasm = std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() != Ok("deno");
        let mut command = Command::new(&konae);
        command.env_clear()
            .env("KONAE_DATA_DIR", &root)
            .env("KONAE_BIND", "127.0.0.1")
            .env("KONAE_KERNEL_PORT", port.to_string())
            .env("KONAE_PUBLIC_ORIGIN", &origin)
            .env("KONAE_EXECUTION_MODE", "test")
            .env("KONAE_MAIL_TRANSPORT", "development")
            .env("KONAE_FUNCTIONS_RUNNER", runner)
            .env("KONAE_FUNCTIONS_PREPARER", helper)
            .env("KONAE_FUNCTIONS_WASM_CACHE", &cache)
            .env("KONAE_FUNCTIONS_DEVELOPMENT", "1")
            .env("KONAE_FUNCTIONS_TIMEOUT_MS", "5000")
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).unwrap());
        if !wasm {
            command.env("KONAE_FUNCTIONS_RUNNER_KIND", "deno");
        }
        let mut server = Server(command.spawn().unwrap());
        let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).no_proxy().build().unwrap();
        let get = |host: &str, path: &str| client.get(format!("{origin}{path}")).header("Host", host);
        let deadline = Instant::now() + Duration::from_secs(285);
        loop {
            if get("localhost", "/_konae/health").send().await.is_ok() {
                break;
            }
            assert!(server.0.try_wait().unwrap().is_none(), "{}", fs::read_to_string(&log).unwrap());
            assert!(Instant::now() < deadline, "{}", fs::read_to_string(&log).unwrap());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let refused = get(&bad.hostname, "/api/orders").send().await.unwrap();
        assert_eq!(refused.status(), 503);
        let page = get(&bad.hostname, "/").send().await.unwrap();
        assert_eq!(page.status(), 200);
        assert!(page.text().await.unwrap().contains("<p>public</p>"));
        assert!(fs::read_to_string(&log).unwrap().contains("Not serving functions for apps/bad"));

        if wasm {
            let alias = fs::read_dir(&cache).unwrap().map(|entry| entry.unwrap())
                .find(|entry| entry.file_name().to_string_lossy().starts_with(".alias-")).unwrap().path();
            fs::remove_file(&alias).unwrap();
            assert_eq!(get(&good.hostname, "/api/orders").send().await.unwrap().status(), 503);
            let deadline = Instant::now() + Duration::from_secs(185);
            while !alias.exists() {
                assert!(Instant::now() < deadline, "{}", fs::read_to_string(&log).unwrap());
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        let reply = get(&good.hostname, "/api/orders").send().await.unwrap();
        let status = reply.status();
        let body = reply.text().await.unwrap();
        println!("GET {} /api/orders -> {status} {body}", good.hostname);
        assert_eq!(status, 200);
        assert_eq!(body, "ready");
        drop(server);
        fs::remove_dir_all(root).unwrap();
    }
}
