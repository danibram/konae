use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PrepareStatus {
    BundleContract,
    UnsupportedApi,
    InitPending,
    InitEffect,
    InitFailed,
    BuildLimit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WasmManifest {
    #[serde(default)]
    pub abi: String,
    #[serde(default)]
    pub input_sha256: String,
    #[serde(default)]
    pub component_sha256: String,
}

pub(crate) fn helper_refusal(status: PrepareStatus, function: &str) -> anyhow::Error {
    anyhow::anyhow!("Preparation refusal mapping is unavailable for {function} ({status:?}).")
}

pub(crate) fn verify_component(
    _manifest: &WasmManifest,
    _input_sha256: &str,
    _component: &[u8],
) -> anyhow::Result<()> {
    anyhow::bail!("The function component verifier is unavailable.")
}
