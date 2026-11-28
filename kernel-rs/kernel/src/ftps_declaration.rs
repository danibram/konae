use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DECLARATION_FILE: &str = "ftps.json";
const MAX_DECLARATION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Declaration {
    pub version: u64,
    pub destinations: std::collections::BTreeMap<String, Destination>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Destination {
    pub kind: DestinationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DestinationKind {
    Member,
    Device,
}

pub fn validate_declaration(raw: &str) -> Result<Declaration> {
    ensure!(
        raw.len() <= MAX_DECLARATION_BYTES,
        "app/ftps.json exceeds 65536 bytes."
    );
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| anyhow::anyhow!("app/ftps.json is not valid JSON."))?;
    ensure!(value.is_object(), "app/ftps.json must be an object.");
    ensure!(
        value.as_object().is_some_and(|object| object
            .keys()
            .all(|key| key == "version" || key == "destinations")),
        "app/ftps.json has an unknown field."
    );
    ensure!(
        value["version"].as_f64() == Some(1.0),
        "app/ftps.json version must be 1."
    );
    let destinations = value["destinations"]
        .as_object()
        .context("app/ftps.json must declare between 1 and 32 destinations.")?;
    ensure!(
        !destinations.is_empty() && destinations.len() <= 32,
        "app/ftps.json must declare between 1 and 32 destinations."
    );
    for (id, destination) in destinations {
        ensure!(
            valid_id(id),
            "FTPS destination ID must use 1 to 64 ASCII letters, digits, '_' or '-'."
        );
        ensure!(
            destination.is_object(),
            "FTPS destination {id} must be an object."
        );
        ensure!(
            destination
                .as_object()
                .is_some_and(|object| object.keys().all(|key| key == "kind" || key == "folder")),
            "FTPS destination {id} has an unknown field."
        );
        ensure!(
            destination["kind"] == "member" || destination["kind"] == "device",
            "FTPS destination {id} kind must be \"member\" or \"device\"."
        );
        if destination.get("folder").is_some() {
            let folder = destination["folder"].as_str().ok_or_else(|| anyhow::anyhow!("FTPS destination {id} folder must be a normalized relative path of at most 1024 bytes."))?;
            normalize_folder(id, folder)?;
        }
    }
    Ok(Declaration {
        version: 1,
        destinations: serde_json::from_value(value["destinations"].clone())?,
    })
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn normalize_folder(id: &str, folder: &str) -> Result<()> {
    ensure!(
        !folder.is_empty()
            && folder.len() <= 1024
            && !folder.starts_with('/')
            && !folder.contains('\\')
            && !folder.bytes().any(|byte| byte.is_ascii_control()),
        "FTPS destination {id} folder must be a normalized relative path of at most 1024 bytes."
    );
    ensure!(
        folder
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        "FTPS destination {id} folder must not contain empty, '.' or '..' components."
    );
    Ok(())
}

pub fn pin_declaration(manifest: &mut Value, source: Option<&str>) -> Result<()> {
    if let Some(source) = source {
        let declaration = validate_declaration(source)?;
        manifest["ftps"] = serde_json::to_value(declaration)?;
    } else if let Some(object) = manifest.as_object_mut() {
        object.remove("ftps");
    }
    Ok(())
}

pub fn published_destination(
    deployment: &crate::Deployment,
    destination: &str,
) -> Option<Destination> {
    serde_json::from_value::<Declaration>(deployment.manifest.get("ftps")?.clone())
        .ok()?
        .destinations
        .get(destination)
        .cloned()
}

pub(crate) fn manifest_uses_member(manifest: &Value) -> bool {
    serde_json::from_value::<Declaration>(manifest.get("ftps").cloned().unwrap_or(Value::Null))
        .is_ok_and(|declaration| {
            declaration
                .destinations
                .values()
                .any(|destination| destination.kind == DestinationKind::Member)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shared_declaration_cases_match() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("ftps-cases.json")).unwrap();
        for case in cases {
            let result = validate_declaration(&case["value"].to_string());
            assert_eq!(
                result.is_ok(),
                case["valid"].as_bool().unwrap(),
                "{}: {result:?}",
                case["case"]
            );
            if let Some(expected) = case["error"].as_str() {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    expected,
                    "{}",
                    case["case"]
                );
            }
        }
    }

    #[test]
    fn pinning_keeps_only_the_published_declaration() {
        let mut manifest =
            json!({"visibility":"public", "ftps":{"destinations":{"old":{"kind":"device"}}}});
        pin_declaration(
            &mut manifest,
            Some(
                r#"{"version":1,"destinations":{"meters":{"kind":"device","folder":"readings"}}}"#,
            ),
        )
        .unwrap();
        assert_eq!(
            manifest["ftps"]["destinations"]["meters"]["folder"],
            "readings"
        );
        assert!(manifest["ftps"]["destinations"]["old"].is_null());
        pin_declaration(&mut manifest, None).unwrap();
        assert!(manifest.get("ftps").is_none());
    }

    #[test]
    fn only_a_pinned_member_destination_uses_member_modules() {
        assert!(manifest_uses_member(
            &json!({"ftps":{"version":1,"destinations":{"photos":{"kind":"member"}}}})
        ));
        assert!(!manifest_uses_member(
            &json!({"ftps":{"version":1,"destinations":{"meters":{"kind":"device"}}}})
        ));
        assert!(!manifest_uses_member(
            &json!({"ftps":{"version":1,"destinations":{"meters":{"kind":"device"}}},"draft":{"destinations":{"photos":{"kind":"member"}}}})
        ));
    }
}
