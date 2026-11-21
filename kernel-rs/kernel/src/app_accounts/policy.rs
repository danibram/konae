use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignInMethod {
    Password,
    Google,
}

pub fn default_sign_in_methods() -> Vec<SignInMethod> {
    vec![SignInMethod::Password]
}

pub const PASSWORD_REQUIRED: &str = "Password sign-in cannot be turned off, so every account keeps a way back in.";

/// Builds the methods from owner input: password first, each method once.
/// Password cannot be turned off in this version.
pub fn sign_in_methods_from_input(methods: &[SignInMethod]) -> std::result::Result<Vec<SignInMethod>, String> {
    if !methods.contains(&SignInMethod::Password) {
        return Err(PASSWORD_REQUIRED.into());
    }
    let mut result = vec![SignInMethod::Password];
    if methods.contains(&SignInMethod::Google) {
        result.push(SignInMethod::Google);
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignupMode {
    #[default]
    Open,

    Invitation,
    Allowlist,
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignupPolicy {
    pub mode: SignupMode,

    #[serde(default)]
    pub allowlist: Vec<String>,
}

pub const MAX_ALLOWLIST_ENTRIES: usize = 100;
pub const SIGNUP_REFUSED: &str = "This app is not accepting new accounts for this email address.";

impl SignupPolicy {
    /// Public apps start open. Private apps start closed until the owner
    /// chooses who may join.
    /// Open only for a Deployment that says it is public; any other value is closed.
    pub fn initial(visibility: &str) -> Self {
        Self {
            mode: if visibility == "public" {
                SignupMode::Open
            } else {
                SignupMode::Closed
            },
            allowlist: Vec::new(),
        }
    }

    /// The policy a redeploy keeps. Turning a public app private closes an
    /// open signup, since strangers who could join the public app should not
    /// get into the private one. Any other choice the owner made carries over.
    pub fn for_republish(&self, was_private: bool, visibility: &str) -> Self {
        if !was_private && visibility == "private" && self.mode == SignupMode::Open {
            Self {
                mode: SignupMode::Closed,
                allowlist: self.allowlist.clone(),
            }
        } else {
            self.clone()
        }
    }

    /// Builds a policy from owner input, normalizing and deduplicating entries.
    pub fn from_input(mode: SignupMode, entries: &[String]) -> std::result::Result<Self, String> {
        if entries.len() > MAX_ALLOWLIST_ENTRIES {
            return Err(format!(
                "The allowlist holds at most {MAX_ALLOWLIST_ENTRIES} entries."
            ));
        }
        let mut allowlist: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.trim().to_lowercase();
            if entry.is_empty() {
                continue;
            }
            let valid = match entry.strip_prefix('@') {
                Some(domain) => {
                    domain.len() <= 253
                        && domain.contains('.')
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                        && domain
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
                }
                None => auth::normalize_email(&entry).is_ok(),
            };
            if !valid {
                return Err(format!(
                    "{entry} is not an email address or an @domain entry."
                ));
            }
            if !allowlist.contains(&entry) {
                allowlist.push(entry);
            }
        }
        Ok(Self { mode, allowlist })
    }

    /// Decides whether this email may create an account. Domain entries match
    /// the exact domain after the `@`, never a subdomain.
    pub fn allows(&self, email: &str) -> bool {
        match self.mode {
            SignupMode::Open => true,
            SignupMode::Invitation | SignupMode::Closed => false,
            SignupMode::Allowlist => {
                let Ok(email) = auth::normalize_email(email) else {
                    return false;
                };
                let domain = email.rsplit_once('@').map(|(_, domain)| domain);
                self.allowlist.iter().any(|entry| match entry.strip_prefix('@') {
                    Some(allowed) => domain == Some(allowed),
                    None => *entry == email,
                })
            }
        }
    }
}

/// The Signup Policy as `project_inspect.deployment.signupPolicy` returns it.
pub fn signup_policy_value(deployment: &Deployment) -> Value {
    json!({"deploymentId":deployment.id,"visibility":if is_private(deployment) { "private" } else { "public" },"mode":deployment.signup_policy.mode,"allowlist":deployment.signup_policy.allowlist})
}

/// Sets who may create an account in the app `id`, as Konae's
/// `module_set` on auth and the `kernel` binary's `signup-policy` both do.
/// The entries stay as they are when `entries` is `None`. The change lands
/// in `konae.json`, which a running `kernel` binary follows, and the answer
/// is `signup_policy_value` with `previousMode`.
pub async fn set_signup_policy(state: &AppState, id: &str, mode: SignupMode, entries: Option<Vec<String>>) -> Result<Value> {
    let served = |deployments: &HashMap<String, Deployment>| {
        find_project_deployment(deployments, id).ok_or_else(|| anyhow::anyhow!("The app {id} is not published."))
    };
    let current = served(&*state.deployments.read().await)?;
    let entries = entries.unwrap_or_else(|| current.signup_policy.allowlist.clone());
    let policy = SignupPolicy::from_input(mode, &entries).map_err(anyhow::Error::msg)?;


    let _guard = database_lock(state, &format!("project:{}", current.project_id)).await;
    let mut updated = served(&*state.deployments.read().await)?;
    let previous = updated.signup_policy.mode;
    updated.signup_policy = policy;
    persist_deployment(state, &updated).await?;
    state.deployments.write().await.insert(updated.hostname.clone(), updated.clone());
    let mut value = signup_policy_value(&updated);
    value["previousMode"] = json!(previous);
    Ok(value)
}
