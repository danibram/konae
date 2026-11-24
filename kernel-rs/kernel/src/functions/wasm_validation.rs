use serde::Deserialize;
use sha2::{Digest, Sha256};
use wasmparser::{
    Encoding, Parser, Payload, PrimitiveValType, Validator,
    component_types::{ComponentDefinedType, ComponentEntityType, ComponentValType},
    types::TypesRef,
};

use crate::refusal::{Refusal, refuse};

const ABI: &str = "konae-functions-wasm/1";
const ALLOWED_COMPONENT_IMPORTS: [&str; 15] = [
    "konae:worker/host",
    "wasi:cli/stderr@0.2.10",
    "wasi:cli/stderr@0.2.12",
    "wasi:clocks/monotonic-clock@0.2.10",
    "wasi:clocks/monotonic-clock@0.2.12",
    "wasi:clocks/wall-clock@0.2.10",
    "wasi:clocks/wall-clock@0.2.12",
    "wasi:io/error@0.2.10",
    "wasi:io/error@0.2.12",
    "wasi:io/poll@0.2.10",
    "wasi:io/poll@0.2.12",
    "wasi:io/streams@0.2.10",
    "wasi:io/streams@0.2.12",
    "wasi:random/random@0.2.10",
    "wasi:random/random@0.2.12",
];

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
    let types = if let Ok(types) = validator.validate_all(component) {
        types
    } else {
        return Err(refuse_error(
            "function.wasm-invalid",
            "The function component is not valid WebAssembly.",
        ));
    };
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
    let types = types.as_ref();
    let supported_imports = imports.iter().all(|(name, kind)| {
        ALLOWED_COMPONENT_IMPORTS.contains(&name.as_str())
            && *kind == wasmparser::ComponentExternalKind::Instance
    });
    let has_host = imports
        .iter()
        .filter(|(name, _)| name == "konae:worker/host")
        .count()
        == 1;
    let has_request_web_imports = [
        "wasi:clocks/monotonic-clock@0.2.10",
        "wasi:clocks/monotonic-clock@0.2.12",
    ]
    .iter()
    .any(|name| imports.iter().any(|(actual, _)| actual == name))
        && ["wasi:io/poll@0.2.10", "wasi:io/poll@0.2.12"]
            .iter()
            .any(|name| imports.iter().any(|(actual, _)| actual == name))
        && ["wasi:random/random@0.2.10", "wasi:random/random@0.2.12"]
            .iter()
            .any(|name| imports.iter().any(|(actual, _)| actual == name));
    if !supported_imports || !has_host || !has_request_web_imports || !host_interface_matches(types)
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
        || !runner_exports_match(types)
    {
        return Err(refuse_error(
            "function.wasm-export",
            "The function component must export the required runner interface with matching types.",
        ));
    }
    Ok(())
}

fn host_interface_matches(types: TypesRef<'_>) -> bool {
    let Some(ComponentEntityType::Instance(instance)) =
        types.component_entity_type_of_import("konae:worker/host")
    else {
        return false;
    };
    let exports = &types[instance].exports;
    exports.len() == 6
        && function_matches(
            types,
            exports.get("op-worker-call"),
            &[(&["args"], "string")],
            Some("string"),
        )
        && function_matches(
            types,
            exports.get("op-worker-body-read"),
            &[],
            Some("bytes"),
        )
        && function_matches(
            types,
            exports.get("op-worker-response-chunk"),
            &[(&["bytes"], "bytes")],
            Some("bool"),
        )
        && function_matches(
            types,
            exports.get("op-worker-failure"),
            &[(&["code"], "string"), (&["detail"], "option-string")],
            None,
        )
        && function_matches(types, exports.get("now-ms"), &[], Some("f64"))
        && function_matches(types, exports.get("random-seed"), &[], Some("u64"))
}

fn runner_exports_match(types: TypesRef<'_>) -> bool {
    function_matches(
        types,
        types.component_entity_type_of_export("handle").as_ref(),
        &[(&["input"], "string"), (&["limit"], "u32")],
        Some("string"),
    ) && function_matches(
        types,
        types
            .component_entity_type_of_export("has-shutdown")
            .as_ref(),
        &[],
        Some("bool"),
    ) && function_matches(
        types,
        types.component_entity_type_of_export("shutdown").as_ref(),
        &[],
        None,
    ) && function_matches(
        types,
        types.component_entity_type_of_export("spin").as_ref(),
        &[],
        None,
    )
}

fn function_matches(
    types: TypesRef<'_>,
    entity: Option<&ComponentEntityType>,
    params: &[(&[&str], &str)],
    result: Option<&str>,
) -> bool {
    let Some(ComponentEntityType::Func(id)) = entity else {
        return false;
    };
    let function = &types[*id];
    if function.async_
        || function.params.len() != params.len()
        || function.result.is_some() != result.is_some()
    {
        return false;
    }
    function
        .params
        .iter()
        .zip(params)
        .all(|((name, ty), (expected_name, expected_ty))| {
            name.as_str() == expected_name[0] && value_matches(types, *ty, expected_ty)
        })
        && function
            .result
            .zip(result)
            .is_none_or(|(actual, expected)| value_matches(types, actual, expected))
}

fn value_matches(types: TypesRef<'_>, actual: ComponentValType, expected: &str) -> bool {
    use ComponentValType::{Primitive as P, Type as T};
    match (actual, expected) {
        (P(PrimitiveValType::String), "string") => true,
        (P(PrimitiveValType::U32), "u32") => true,
        (P(PrimitiveValType::Bool), "bool") => true,
        (P(PrimitiveValType::F64), "f64") => true,
        (P(PrimitiveValType::U64), "u64") => true,
        (T(id), "bytes") => matches!(
            types[id],
            ComponentDefinedType::List(P(PrimitiveValType::U8))
        ),
        (T(id), "option-string") => matches!(
            types[id],
            ComponentDefinedType::Option(P(PrimitiveValType::String))
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use wasm_encoder::{
        Alias, Component, ComponentAliasSection, ComponentExportKind, ComponentExportSection,
        ComponentImportSection, ComponentTypeRef, ComponentTypeSection,
        ComponentValType as EncodedValType, InstanceType, PrimitiveValType as EncodedPrimitive,
    };

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
        assert_eq!(
            refused.hint(),
            expected("component digest mismatch")["hint"]
        );
    }

    #[test]
    fn generated_wit_fixtures_refuse_malformed_and_wrong_signatures() {
        let cases: Value =
            serde_json::from_str(include_str!("../functions-wasm-cases.json")).unwrap();
        for case in cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["fixture"].is_object())
        {
            let fixture = &case["fixture"];
            let bytes = match fixture["kind"].as_str().unwrap() {
                "malformed_wit" => vec![0, 97, 115, 109, 13, 0, 1, 255],
                "wrong_host_signature" => component_fixture(true, false),
                "wrong_runner_signature" => component_fixture(false, true),
                "extra_import" => {
                    let mut imports = ALLOWED_COMPONENT_IMPORTS
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect::<Vec<_>>();
                    imports.push(fixture["interface"].as_str().unwrap().to_owned());
                    component_with_imports(&imports)
                }
                other => panic!("unknown WIT fixture {other}"),
            };
            let manifest = WasmManifest {
                abi: ABI.into(),
                input_sha256: "00".repeat(32),
                component_sha256: format!("{:x}", Sha256::digest(&bytes)),
            };
            let error =
                verify_component(&manifest, &manifest.input_sha256.clone(), &bytes).unwrap_err();
            let refused = error.downcast_ref::<Refusal>().unwrap();
            assert_eq!(refused.code, case["code"], "{}", case["case"]);
            assert_eq!(refused.message, case["error"], "{}", case["case"]);
            assert_eq!(refused.hint(), case["hint"], "{}", case["case"]);
        }
    }

    #[test]
    fn request_web_api_imports_are_accepted() {
        let cases: Value =
            serde_json::from_str(include_str!("../functions-wasm-cases.json")).unwrap();
        let fixture = cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|case| case.get("acceptedFixture"))
            .unwrap();
        let imports = serde_json::from_value::<Vec<String>>(fixture["imports"].clone()).unwrap();
        assert_eq!(
            imports,
            ALLOWED_COMPONENT_IMPORTS
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        );
        let bytes = component_with_imports(&imports);
        let manifest = WasmManifest {
            abi: ABI.into(),
            input_sha256: "00".repeat(32),
            component_sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        verify_component(&manifest, &manifest.input_sha256, &bytes).unwrap();
    }

    fn component_with_imports(import_names: &[String]) -> Vec<u8> {
        let mut host = InstanceType::new();
        host.ty().defined_type().list(EncodedPrimitive::U8);
        host.ty().defined_type().option(EncodedPrimitive::String);
        host.ty()
            .function()
            .params([("args", EncodedPrimitive::String)])
            .result(Some(EncodedPrimitive::String.into()));
        host.export("op-worker-call", ComponentTypeRef::Func(2));
        host.ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedValType::Type(0)));
        host.export("op-worker-body-read", ComponentTypeRef::Func(3));
        host.ty()
            .function()
            .params([("bytes", EncodedValType::Type(0))])
            .result(Some(EncodedPrimitive::Bool.into()));
        host.export("op-worker-response-chunk", ComponentTypeRef::Func(4));
        host.ty()
            .function()
            .params([
                ("code", EncodedValType::Primitive(EncodedPrimitive::String)),
                ("detail", EncodedValType::Type(1)),
            ])
            .result(None);
        host.export("op-worker-failure", ComponentTypeRef::Func(5));
        host.ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedPrimitive::F64.into()));
        host.export("now-ms", ComponentTypeRef::Func(6));
        host.ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedPrimitive::U64.into()));
        host.export("random-seed", ComponentTypeRef::Func(7));

        let mut runner = InstanceType::new();
        runner
            .ty()
            .function()
            .params([
                ("input", EncodedPrimitive::String),
                ("limit", EncodedPrimitive::U32),
            ])
            .result(Some(EncodedPrimitive::String.into()));
        runner.export("handle", ComponentTypeRef::Func(0));
        runner
            .ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedPrimitive::Bool.into()));
        runner.export("has-shutdown", ComponentTypeRef::Func(1));
        runner
            .ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(None);
        runner.export("shutdown", ComponentTypeRef::Func(2));
        runner
            .ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(None);
        runner.export("spin", ComponentTypeRef::Func(3));

        let mut types = ComponentTypeSection::new();
        types.ty().instance(&host);
        types.ty().instance(&runner);
        types.ty().instance(&InstanceType::new());
        let mut component = Component::new();
        component.section(&types);
        let mut imports = ComponentImportSection::new();
        let mut runner_instance = None;
        let mut instance_index = 0;
        for name in import_names {
            let type_index = if name == "konae:worker/host" {
                0
            } else if runner_instance.is_none() {
                runner_instance = Some(instance_index);
                1
            } else {
                2
            };
            imports.import(name, ComponentTypeRef::Instance(type_index));
            instance_index += 1;
        }
        component.section(&imports);
        let mut aliases = ComponentAliasSection::new();
        let instance = runner_instance.unwrap();
        for name in ["handle", "has-shutdown", "shutdown", "spin"] {
            aliases.alias(Alias::InstanceExport {
                instance,
                kind: ComponentExportKind::Func,
                name,
            });
        }
        component.section(&aliases);
        let mut exports = ComponentExportSection::new();
        for (index, name) in ["handle", "has-shutdown", "shutdown", "spin"]
            .into_iter()
            .enumerate()
        {
            exports.export(name, ComponentExportKind::Func, index as u32, None);
        }
        component.section(&exports);
        component.finish()
    }

    fn component_fixture(wrong_host: bool, wrong_exports: bool) -> Vec<u8> {
        let mut host = InstanceType::new();
        host.ty().defined_type().list(EncodedPrimitive::U8);
        host.ty().defined_type().option(EncodedPrimitive::String);
        host.ty()
            .function()
            .params([(
                "args",
                if wrong_host {
                    EncodedPrimitive::U32
                } else {
                    EncodedPrimitive::String
                },
            )])
            .result(Some(EncodedPrimitive::String.into()));
        host.export("op-worker-call", ComponentTypeRef::Func(2));
        host.ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedValType::Type(0)));
        host.export("op-worker-body-read", ComponentTypeRef::Func(3));
        host.ty()
            .function()
            .params([("bytes", EncodedValType::Type(0))])
            .result(Some(EncodedPrimitive::Bool.into()));
        host.export("op-worker-response-chunk", ComponentTypeRef::Func(4));
        host.ty()
            .function()
            .params([
                ("code", EncodedValType::Primitive(EncodedPrimitive::String)),
                ("detail", EncodedValType::Type(1)),
            ])
            .result(None);
        host.export("op-worker-failure", ComponentTypeRef::Func(5));
        host.ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedPrimitive::F64.into()));
        host.export("now-ms", ComponentTypeRef::Func(6));
        host.ty()
            .function()
            .params(std::iter::empty::<(&str, EncodedValType)>())
            .result(Some(EncodedPrimitive::U64.into()));
        host.export("random-seed", ComponentTypeRef::Func(7));

        let mut types = ComponentTypeSection::new();
        types.ty().instance(&host);
        if wrong_exports {
            types.ty().instance(&InstanceType::new());
        }
        let mut component = Component::new();
        component.section(&types);
        let mut imports = ComponentImportSection::new();
        imports.import("konae:worker/host", ComponentTypeRef::Instance(0));
        if wrong_exports {
            for name in ALLOWED_COMPONENT_IMPORTS
                .iter()
                .filter(|name| **name != "konae:worker/host")
            {
                imports.import(*name, ComponentTypeRef::Instance(1));
            }
        }
        component.section(&imports);
        let mut aliases = ComponentAliasSection::new();
        let source_names = if wrong_exports {
            [
                "op-worker-call",
                "now-ms",
                "op-worker-failure",
                "op-worker-failure",
                "op-worker-body-read",
                "random-seed",
            ]
        } else {
            [
                "op-worker-call",
                "op-worker-call",
                "op-worker-failure",
                "op-worker-failure",
                "op-worker-body-read",
                "random-seed",
            ]
        };
        for name in source_names {
            aliases.alias(Alias::InstanceExport {
                instance: 0,
                kind: ComponentExportKind::Func,
                name,
            });
        }
        component.section(&aliases);
        let mut exports = ComponentExportSection::new();
        for (index, name) in ["handle", "has-shutdown", "shutdown", "spin"]
            .into_iter()
            .enumerate()
        {
            exports.export(name, ComponentExportKind::Func, index as u32, None);
        }
        component.section(&exports);
        component.finish()
    }
}
