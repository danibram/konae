use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use crate::{ReleasedSettings, SignInMethod, SignupPolicy, functions, package, sources};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployInput {
    pub project: ProjectInput,
    pub visibility: String,
    pub artifact: BuildArtifact,
    #[serde(default)]
    pub server_artifact: Option<functions::ServerArtifact>,
    #[serde(default)]
    pub initial_released: Option<ReleasedSettings>,
    #[serde(default)]
    pub initial_signup_policy: Option<SignupPolicy>,
    #[serde(default)]
    pub initial_sign_in_methods: Option<Vec<SignInMethod>>,
    #[serde(default)]
    pub initial_modules: Option<std::collections::BTreeMap<String, Value>>,

    #[serde(default)]
    pub initial_package: Option<package::InitialPackage>,

    #[serde(default)]
    pub contact_email: Option<String>,


    #[serde(default)]
    pub schema_confirmation: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub id: String,
    pub name: String,
    pub slug: String,



    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub encodings: HashMap<String, String>,
    #[serde(default)]
    pub directories: Vec<String>,
    pub manifest: Value,
}

impl ProjectInput {
    /// The bytes of each file of `app/` the release carries.
    pub fn sources(&self) -> Result<sources::Files, String> {
        sources::decode(&self.files, &self.encodings)
    }
}

#[derive(Clone, Deserialize)]
pub struct BuildArtifact {
    pub version: u8,
    pub entrypoint: String,
    pub files: HashMap<String, String>,

    #[serde(default)]
    pub encodings: HashMap<String, String>,
}
