use serde::Deserialize;
use sha2::{Digest, Sha256};
use wasmparser::{Encoding, Parser, Payload, Validator};

use crate::refusal::{Refusal, refuse};

const ABI: &str = "konae-functions-wasm/1";

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
    let (code, message) = match status {
        PrepareStatus::BundleContract => (
            "function.bundle-contract",
            format!(
                "Function {function} bundle must be a self-contained ESM module exporting handle."
            ),
        ),
        PrepareStatus::UnsupportedApi => (
            "function.wasm-api",
            "A function uses an API that StarlingMonkey does not support.".into(),
        ),
        PrepareStatus::InitPending => (
            "function.wasm-init-pending",
            "A function must finish its top-level initialization before it can run.".into(),
        ),
        PrepareStatus::InitEffect => (
            "function.wasm-init-effect",
            "A function cannot start timers or fetch during top-level initialization.".into(),
        ),
        PrepareStatus::InitFailed => (
            "function.wasm-init",
            "A function failed during top-level initialization.".into(),
        ),
        PrepareStatus::BuildLimit => (
            "function.wasm-build-limit",
            "Function component preparation exceeded its build limits.".into(),
        ),
    };
    anyhow::Error::new(Refusal { code, message })
}

pub(crate) fn verify_component(
    manifest: &WasmManifest,
    input_sha256: &str,
    component: &[u8],
) -> anyhow::Result<()> {
    let refuse_error = |code, message: &'static str| anyhow::Error::new(refuse(code, message));
    if manifest.abi != ABI {
        return Err(refuse_error(
            "function.wasm-abi",
            "The function component requires ABI konae-functions-wasm/1.",
        ));
    }
    let valid_digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    if !valid_digest(input_sha256)
        || !valid_digest(&manifest.input_sha256)
        || input_sha256 != manifest.input_sha256
        || !valid_digest(&manifest.component_sha256)
        || format!("{:x}", Sha256::digest(component)) != manifest.component_sha256
    {
        return Err(refuse_error(
            "function.wasm-digest",
            "The function component SHA-256 does not match its manifest.",
        ));
    }

    let mut validator = Validator::new();
    if validator.validate_all(component).is_err() {
        return Err(refuse_error(
            "function.wasm-invalid",
            "The function component is not valid WebAssembly.",
        ));
    }
    if Parser::new(0)
        .parse_all(component)
        .next()
        .is_none_or(|payload| {
            !matches!(
                payload,
                Ok(Payload::Version {
                    encoding: Encoding::Component,
                    ..
                })
            )
        })
    {
        return Err(refuse_error(
            "function.wasm-invalid",
            "The function component is not valid WebAssembly.",
        ));
    }

    let mut imports = Vec::new();
    let mut exports = Vec::new();
    for payload in Parser::new(0).parse_all(component) {
        match payload.map_err(|_| {
            refuse_error(
                "function.wasm-invalid",
                "The function component is not valid WebAssembly.",
            )
        })? {
            Payload::ComponentImportSection(section) => {
                for import in section {
                    let import = import.map_err(|_| {
                        refuse_error(
                            "function.wasm-invalid",
                            "The function component is not valid WebAssembly.",
                        )
                    })?;
                    imports.push((import.name.0.to_owned(), import.ty.kind()));
                }
            }
            Payload::ComponentExportSection(section) => {
                for export in section {
                    let export = export.map_err(|_| {
                        refuse_error(
                            "function.wasm-invalid",
                            "The function component is not valid WebAssembly.",
                        )
                    })?;
                    exports.push((export.name.0.to_owned(), export.kind));
                }
            }
            _ => {}
        }
    }
    let allowed_imports = ["konae:worker/host"];
    if imports.len() != 1
        || !allowed_imports.contains(&imports[0].0.as_str())
        || imports[0].1 != wasmparser::ComponentExternalKind::Instance
    {
        return Err(refuse_error(
            "function.wasm-import",
            "The function component has an import outside the allowed host interface.",
        ));
    }
    let expected_exports = ["handle", "has-shutdown", "shutdown", "spin"];
    if exports.len() != expected_exports.len()
        || expected_exports.iter().any(|name| {
            !exports.iter().any(|(actual, kind)| {
                actual == name && *kind == wasmparser::ComponentExternalKind::Func
            })
        })
    {
        return Err(refuse_error(
            "function.wasm-export",
            "The function component must export the required runner interface with matching types.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn helper_statuses_match_shared_refusal_cases() {
        let cases: Value =
            serde_json::from_str(include_str!("../functions-wasm-cases.json")).unwrap();
        for case in cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["status"].is_string())
        {
            let status: PrepareStatus = serde_json::from_value(case["status"].clone()).unwrap();
            let error = helper_refusal(status, "orders");
            let refused = error.downcast_ref::<Refusal>().unwrap();
            assert_eq!(refused.code, case["code"], "{}", case["case"]);
            assert_eq!(refused.message, case["error"], "{}", case["case"]);
            assert_eq!(refused.hint(), case["hint"], "{}", case["case"]);
        }
    }

    #[test]
    fn verifier_refusals_match_shared_cases() {
        let cases: Value =
            serde_json::from_str(include_str!("../functions-wasm-cases.json")).unwrap();
        let cases = cases["cases"].as_array().unwrap();
        let expected = |case: &str| cases.iter().find(|item| item["case"] == case).unwrap();
        let mut manifest = WasmManifest {
            abi: "old".into(),
            input_sha256: "00".repeat(32),
            component_sha256: "00".repeat(32),
        };
        let error = verify_component(&manifest, &manifest.input_sha256, b"bad").unwrap_err();
        assert_eq!(
            error.downcast_ref::<Refusal>().unwrap().code,
            expected("wrong component ABI")["code"]
        );

        manifest.abi = ABI.into();
        manifest.component_sha256 = format!("{:x}", Sha256::digest(b"bad"));
        let error = verify_component(&manifest, &manifest.input_sha256, b"bad").unwrap_err();
        let refused = error.downcast_ref::<Refusal>().unwrap();
        assert_eq!(refused.code, expected("malformed component bytes")["code"]);
        assert_eq!(
            refused.message,
            expected("malformed component bytes")["error"]
        );
        assert_eq!(
            refused.hint(),
            expected("malformed component bytes")["hint"]
        );

        manifest.component_sha256 = format!("{:x}", Sha256::digest(b"bad"));
        manifest.input_sha256 = "not-a-digest".into();
        let error = verify_component(&manifest, "not-a-digest", b"bad").unwrap_err();
        let refused = error.downcast_ref::<Refusal>().unwrap();
        assert_eq!(refused.code, expected("component digest mismatch")["code"]);
        assert_eq!(
            refused.message,
            expected("component digest mismatch")["error"]
        );
        assert_eq!(refused.hint(), expected("component digest mismatch")["hint"]);
    }
}
