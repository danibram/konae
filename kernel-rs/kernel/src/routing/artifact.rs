
use std::collections::HashMap;

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode},
    response::Response,
};
use base64::Engine;
use serde_json::Value;

use crate::BuildArtifact;

pub const ROUTING_MESSAGE: &str = "Routing must be \"pages\" or \"spa\".";
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Routing {
    Pages,
    Spa,
}

/// Refuses a `routing` other than "pages" or "spa" in a release's manifest.
/// An absent value is "pages".
pub fn check_routing(manifest: &Value) -> Result<(), String> {
    match manifest.get("routing") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(mode)) if mode == "pages" || mode == "spa" => Ok(()),
        Some(_) => Err(ROUTING_MESSAGE.to_owned()),
    }
}

/// The routing a published Deployment serves with.
pub fn routing(manifest: &Value) -> Routing {
    if manifest.get("routing").and_then(Value::as_str) == Some("spa") {
        Routing::Spa
    } else {
        Routing::Pages
    }
}

/// The artifact's files as the bytes the Kernel stores and serves. A path the
/// artifact names in `encodings` is base64; every other file is UTF-8 text.
pub fn decode(artifact: &BuildArtifact) -> Result<HashMap<String, Vec<u8>>, String> {
    for (path, encoding) in &artifact.encodings {
        if !artifact.files.contains_key(path) {
            return Err(format!("The artifact names an encoding for {path}, which it does not hold."));
        }
        if encoding != "base64" {
            return Err(format!("The artifact encodes {path} as {encoding}. The only encoding is base64."));
        }
    }
    artifact
        .files
        .iter()
        .map(|(path, contents)| {
            let bytes = if artifact.encodings.contains_key(path) {
                base64::engine::general_purpose::STANDARD
                    .decode(contents)
                    .map_err(|_| format!("Artifact file {path} is marked base64 but is not valid base64."))?
            } else {
                contents.clone().into_bytes()
            };
            Ok((path.clone(), bytes))
        })
        .collect()
}

/// True for a path the Kernel keeps. No artifact file answers it.
pub fn kernel_path(path: &str) -> bool {
    ["_konae", "server", "functions", ".konae"].iter().any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

/// The artifact files that may answer `path`, in order. `path` has no leading
/// slash. An exact file always comes first. Under "pages", `/x` with no
/// extension also tries `x.html`, and `/x/` tries `x/index.html`.
pub fn candidates(path: &str, routing: Routing) -> Vec<String> {
    if path.is_empty() {
        return vec!["index.html".into()];
    }
    if routing == Routing::Spa {
        return vec![path.to_owned()];
    }
    if let Some(directory) = path.strip_suffix('/') {
        return if directory.is_empty() { vec!["index.html".into()] } else { vec![format!("{directory}/index.html")] };
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.contains('.') {
        vec![path.to_owned()]
    } else {
        vec![path.to_owned(), format!("{path}.html")]
    }
}

/// The Kernel's own answer for a page the artifact does not hold and has no
/// `404.html` for. It is never the app's home page.
pub fn not_found_page() -> Response {
    let body = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Page not found</title><style>/*KERNEL_PAGE_CSS*/</style><link rel="icon" href="data:,"><main><h1>Page not found</h1><p>This app has no page at this address.</p><p><a href="/">Open the app's home page</a></p></main></html>"#;
    let mut response = Response::new(Body::from(crate::own_page::style(body)));
    *response.status_mut() = StatusCode::NOT_FOUND;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artifact(files: Value, encodings: Value) -> BuildArtifact {
        serde_json::from_value(json!({"version": 1, "entrypoint": "index.html", "files": files, "encodings": encodings})).unwrap()
    }

    #[test]
    fn routing_takes_pages_or_spa_and_refuses_anything_else() {
        assert_eq!(check_routing(&json!({})), Ok(()));
        assert_eq!(check_routing(&json!({"routing": "pages"})), Ok(()));
        assert_eq!(check_routing(&json!({"routing": "spa"})), Ok(()));
        for value in [json!("SPA"), json!("history"), json!(true), json!(["spa"])] {
            assert_eq!(check_routing(&json!({"routing": value})), Err(ROUTING_MESSAGE.to_owned()));
        }
        assert_eq!(routing(&json!({})), Routing::Pages);
        assert_eq!(routing(&json!({"routing": "spa"})), Routing::Spa);
    }

    #[test]
    fn an_artifact_without_encodings_still_parses_as_text() {
        let parsed: BuildArtifact = serde_json::from_value(json!({"version": 1, "entrypoint": "index.html", "files": {"index.html": "<p>hi</p>"}})).unwrap();
        assert_eq!(decode(&parsed).unwrap()["index.html"], b"<p>hi</p>");
    }

    #[test]
    fn base64_files_decode_to_their_bytes() {
        let decoded = decode(&artifact(json!({"index.html": "<p>hi</p>", "logo.png": "iVBORw0KGgo="}), json!({"logo.png": "base64"}))).unwrap();
        assert_eq!(decoded["logo.png"], [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
        assert_eq!(decoded["index.html"], b"<p>hi</p>");
    }

    #[test]
    fn a_bad_encoding_is_refused_with_the_path() {
        assert_eq!(
            decode(&artifact(json!({"index.html": "x"}), json!({"logo.png": "base64"}))).unwrap_err(),
            "The artifact names an encoding for logo.png, which it does not hold."
        );
        assert_eq!(
            decode(&artifact(json!({"index.html": "x", "logo.png": "AAAA"}), json!({"logo.png": "hex"}))).unwrap_err(),
            "The artifact encodes logo.png as hex. The only encoding is base64."
        );
        assert_eq!(
            decode(&artifact(json!({"index.html": "x", "logo.png": "not base64!"}), json!({"logo.png": "base64"}))).unwrap_err(),
            "Artifact file logo.png is marked base64 but is not valid base64."
        );
    }

    #[test]
    fn pages_resolve_an_exact_file_then_the_html_page() {
        assert_eq!(candidates("", Routing::Pages), ["index.html"]);
        assert_eq!(candidates("about", Routing::Pages), ["about", "about.html"]);
        assert_eq!(candidates("about.html", Routing::Pages), ["about.html"]);
        assert_eq!(candidates("guide/install", Routing::Pages), ["guide/install", "guide/install.html"]);
        assert_eq!(candidates("guide/", Routing::Pages), ["guide/index.html"]);
        assert_eq!(candidates("photos/2026/fog.jpg", Routing::Pages), ["photos/2026/fog.jpg"]);
        assert_eq!(candidates("about/", Routing::Spa), ["about/"]);
        assert_eq!(candidates("about", Routing::Spa), ["about"]);
    }

    #[test]
    fn kernel_paths_are_never_app_files() {
        assert!(kernel_path("_konae"));
        assert!(kernel_path("_konae/files/1"));
        assert!(!kernel_path("_konaees.html"));
    }
}
