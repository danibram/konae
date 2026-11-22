use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[cfg(any(test, feature = "testing"))]
use std::path::Path;

use aws_sdk_s3::Client as S3Client;
use tokio::sync::RwLock;

use crate::{
    Deployment, accounts, app_data::LockMap, copies, execution, file_store,
    functions, installation, limits::Guards, runtime,
};

#[derive(Clone)]
pub struct AppState {
    pub execution_mode: execution::ExecutionMode,
    pub deployments: Arc<RwLock<HashMap<String, Deployment>>>,
    pub db_locks: LockMap,
    pub s3: Option<S3Client>,
    pub data_bucket: String,
    pub data_root: PathBuf,
    /// Where file bytes live and how many a Deployment may hold.
    pub files: file_store::Files,
    pub copies: Arc<copies::CopyStatus>,
    pub public_origin: String,
    /// The origins the installation answers on itself besides the public
    /// origin, such as where Konae's MCP answers. The first is where a
    /// client signs in.
    pub own_origins: Vec<String>,
    pub host_suffix: String,
    /// `<scheme>://<domain>` where path addresses answer; `None` without `KONAE_DOMAIN`.
    pub path_origin: Option<String>,
    pub guards: Arc<Guards>,
    pub accounts: accounts::AccountSettings,
    /// The stores at the data root besides the app folders.
    pub installation: Arc<installation::Installation>,
    /// Where an app user's file uploads and deletes are recorded.
    pub audit: Option<runtime::AuditSink>,
    pub functions: Arc<functions::runtime::Runtime>,
}

#[cfg(any(test, feature = "testing"))]
impl AppState {
    /// A Kernel on `root` with no Deployments, local files, no mail and the
    /// `localhost` names. A test changes the fields it needs with `..`.
    pub fn for_test(root: &Path) -> Self {
        Self {
            execution_mode: execution::ExecutionMode::Live,
            deployments: Arc::new(RwLock::new(HashMap::new())),
            db_locks: LockMap::default(),
            s3: None,
            data_bucket: "data".into(),
            files: file_store::Files::local(root),
            data_root: root.into(),
            copies: Default::default(),
            accounts: accounts::AccountSettings::local(),
            public_origin: "http://localhost".into(),
            own_origins: Vec::new(),
            path_origin: None,
            host_suffix: "localhost".into(),
            guards: Arc::new(Guards::new(Vec::new())),
            installation: Arc::new(installation::Installation::for_test()),
            audit: None,
            functions: Arc::new(functions::runtime::Runtime::disabled()),
        }
    }
}
