use crate::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    /// The app's ID. It names the app's folder, and the project in Konae.
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub slug: String,
    pub url: String,
    pub created_at: u64,


    #[serde(default)]
    pub released_at: u64,



    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    pub manifest: Value,
    pub hostname: String,
    pub actions: String,
    /// The release whose files `dist/` holds. It tags them for browser caches.
    pub artifact_version: String,
    /// Who may create an account.
    pub signup_policy: SignupPolicy,
    /// How people sign in. Password always; Google when the owner turned it on.
    pub sign_in_methods: Vec<SignInMethod>,
    #[serde(default = "modules::defaults")]
    pub modules: std::collections::BTreeMap<String, Value>,


    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_email: Option<String>,
    #[serde(skip, default)]
    pub released_schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<functions::Published>,
}



#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub visibility: String,
    pub signup_policy: SignupPolicy,
    pub sign_in_methods: Vec<SignInMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_email: Option<String>,
    /// The version of the Kernel that wrote the file.
    pub kernel: String,
    /// The Capsule manifest without its visibility: routing, connects and the declared actions.
    pub manifest: Value,
    #[serde(default = "modules::defaults")]
    pub modules: std::collections::BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directories: Vec<String>,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<ReleasedSettings>,
}



#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub hostname: String,
    pub created_at: u64,


    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    /// The `actions.json` of the release `dist/` holds.
    pub actions: String,
    pub artifact_version: String,


    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<ReleasedSettings>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasedSettings {
    pub name: String,
    pub slug: String,
    pub visibility: String,
    pub signup_policy: SignupPolicy,
    pub sign_in_methods: Vec<SignInMethod>,
    pub contact_email: Option<String>,
    pub manifest: Value,
    #[serde(default)]
    pub actions: String,
    #[serde(default)]
    pub schema_sql: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_transforms: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<functions::Published>,
    #[serde(default)]
    pub source_digest: Option<String>,
    #[serde(default)]
    pub artifact_digest: Option<String>,
    #[serde(default)]
    pub released_at: Option<u64>,
}

pub(crate) fn released_settings(deployment: &Deployment) -> ReleasedSettings {
    let mut manifest = deployment.manifest.clone();
    if let Some(object) = manifest.as_object_mut() { object.remove("visibility"); object.remove("runtime"); }
    ReleasedSettings {
        name: deployment.project_name.clone(), slug: deployment.slug.clone(),
        visibility: deployment.manifest["visibility"].as_str().unwrap_or("private").into(),
        signup_policy: deployment.signup_policy.clone(), sign_in_methods: deployment.sign_in_methods.clone(),
        contact_email: deployment.contact_email.clone(), manifest,
        actions: deployment.actions.clone(), schema_sql: deployment.released_schema.clone(),
        schema_transforms: None,
        server: deployment.server.clone(),
        source_digest: deployment.source_digest.clone(), released_at: Some(deployment.released_at),
        artifact_digest: None,
    }
}

pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Settings {
    pub fn new(id: &str, name: &str, slug: &str, manifest: &Value) -> Self {
        let visibility = manifest["visibility"].as_str().unwrap_or("private").to_owned();
        let mut manifest = manifest.clone();
        if let Some(object) = manifest.as_object_mut() {
            object.remove("visibility");
            object.remove("runtime");
            object.remove("fileAccess");
        }
        let mut settings = Self {
            id: id.into(),
            name: name.into(),
            slug: slug.into(),
            signup_policy: SignupPolicy::initial(&visibility),
            visibility,
            sign_in_methods: default_sign_in_methods(),
            contact_email: None,
            kernel: KERNEL_VERSION.into(),
            manifest,
            modules: {
                let mut modules = modules::defaults();
                modules.remove("actions");
                modules.remove("http");
                modules.remove("calls");
                modules
            },
            directories: Vec::new(),
            updated_at: 0,
            released: None,
        };
        settings.sync_auth_options();
        settings
    }

    pub fn of(deployment: &Deployment) -> Self {
        let mut settings = Self {
            signup_policy: deployment.signup_policy.clone(),
            sign_in_methods: deployment.sign_in_methods.clone(),
            contact_email: deployment.contact_email.clone(),
            modules: deployment.modules.clone(),
            ..Self::new(&deployment.id, &deployment.project_name, &deployment.slug, &deployment.manifest)
        };
        settings.released = Some(released_settings(deployment));
        settings.sync_auth_options();
        settings
    }

    fn sync_auth_options(&mut self) {
        let auth = self.modules.get_mut("auth").expect("auth is switchable");
        auth["signupPolicy"] = serde_json::to_value(&self.signup_policy).expect("Signup Policy serializes");
        auth["signInMethods"] = serde_json::to_value(&self.sign_in_methods).expect("sign-in methods serialize");
    }

    pub fn read(root: &Path, id: &str) -> Result<Self> {
        let bytes = std::fs::read(layout::settings(root, id))?;
        let raw: Value = serde_json::from_slice(&bytes)?;
        let mut settings: Self = serde_json::from_slice(&bytes)?;
        if let Some(raw_modules) = raw["modules"].as_object() {
            let raw_modules = raw_modules.iter().map(|(name, state)| (name.clone(), state.clone())).collect();
            if let Err(refusal) = modules::validate_state(&raw_modules) {
                eprintln!("konae: ignoring invalid module state in apps/{id}/konae.json: {}", refusal.message);
                anyhow::bail!("{}", refusal.message);
            }
        }
        modules::complete(&mut settings.modules);
        let nested_policy = settings.modules["auth"]["signupPolicy"].clone();
        let nested_methods = settings.modules["auth"]["signInMethods"].clone();
        settings.signup_policy = match raw.get("signupPolicy").filter(|value| !value.is_null()) {
            Some(value) => serde_json::from_value(value.clone())?,
            None if !nested_policy.is_null() => serde_json::from_value(nested_policy)?,
            None => SignupPolicy::initial(&settings.visibility),
        };
        settings.sign_in_methods = match raw.get("signInMethods").filter(|value| value.is_array()) {
            Some(value) => serde_json::from_value(value.clone())?,
            None if nested_methods.is_array() => serde_json::from_value(nested_methods)?,
            None => default_sign_in_methods(),
        };
        settings.sync_auth_options();
        if let Err(refusal) = modules::validate_state(&settings.modules) {
            eprintln!("konae: ignoring invalid module state in apps/{id}/konae.json: {}", refusal.message);
            anyhow::bail!("{}", refusal.message);
        }
        let old_file_access = raw["manifest"]["fileAccess"] == "app-users" || raw["released"]["manifest"]["fileAccess"] == "app-users";
        if raw["modules"]["files"].is_null() && old_file_access {
            let files = settings.modules.get_mut("files").expect("files is switchable");
            files["on"] = json!(true);
            files["fileAccess"] = json!("app-users");
        }
        if let Some(manifest) = settings.manifest.as_object_mut() { manifest.remove("fileAccess"); }
        if let Some(manifest) = settings.released.as_mut().and_then(|released| released.manifest.as_object_mut()) { manifest.remove("fileAccess"); }
        Ok(settings)
    }

    pub async fn write(&self, root: &Path) -> Result<()> {
        write_atomic(&layout::settings(root, &self.id), &self.folder_bytes()?).await?;
        Ok(())
    }

    pub(super) fn folder_bytes(&self) -> Result<Vec<u8>> {
        let mut settings = self.clone();
        settings.sync_auth_options();
        let mut value = serde_json::to_value(settings)?;
        if let Some(auth) = value.get_mut("modules").and_then(|modules| modules.get_mut("auth")).and_then(Value::as_object_mut) {
            auth.remove("signupPolicy");
            auth.remove("signInMethods");
        }
        Ok(serde_json::to_vec_pretty(&value)?)
    }

    pub fn deployment(self, installed: Installed, public_origin: &str) -> Deployment {
        let released = installed.released.clone().or_else(|| self.released.clone());
        let mut manifest = released.as_ref().map(|r| r.manifest.clone()).unwrap_or(self.manifest);
        let name = released.as_ref().map(|r| r.name.clone()).unwrap_or(self.name);
        let slug = released.as_ref().map(|r| r.slug.clone()).unwrap_or(self.slug);
        let visibility = released.as_ref().map(|r| r.visibility.clone()).unwrap_or(self.visibility);
        let signup_policy = released.as_ref().map(|r| r.signup_policy.clone()).unwrap_or(self.signup_policy);
        let sign_in_methods = released.as_ref().map(|r| r.sign_in_methods.clone()).unwrap_or(self.sign_in_methods);
        let contact_email = released.as_ref().and_then(|r| r.contact_email.clone()).or(self.contact_email);
        if let Some(object) = manifest.as_object_mut() {
            object.insert("visibility".into(), Value::String(visibility));
        }
        Deployment {
            project_id: self.id.clone(),
            id: self.id,
            project_name: name,
            slug,
            url: deployment_url(public_origin, &installed.hostname),
            created_at: installed.created_at,
            released_at: installed.released_at.or_else(|| released.as_ref().and_then(|r| r.released_at)).unwrap_or(installed.created_at),
            source_digest: installed.source_digest.or_else(|| released.as_ref().and_then(|r| r.source_digest.clone())),
            manifest,
            hostname: installed.hostname,
            actions: installed.actions,
            artifact_version: installed.artifact_version,
            signup_policy,
            sign_in_methods,
            contact_email,
            modules: self.modules,
            released_schema: released.as_ref().map(|r| r.schema_sql.clone()).unwrap_or_default(),
            server: released.as_ref().and_then(|r| r.server.clone()),
        }
    }
}

/// A Deployment is public when its manifest says "public".
pub fn is_private(deployment: &Deployment) -> bool {
    deployment
        .manifest
        .get("visibility")
        .and_then(Value::as_str)
        != Some("public")
}
