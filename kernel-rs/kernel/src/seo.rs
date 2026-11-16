use crate::{AppState, ArtifactCache, Deployment};
use axum::{http::{HeaderMap, HeaderValue}, response::Response};
use sha2::Digest;
use std::path::Path;

pub struct Address {
    pub url: String,
    pub reason: String,
}

pub fn address(state: &AppState, deployment: &Deployment) -> Address {
    let options = &deployment.modules["seo"];
    let selected = options["canonical"].as_str();
    let default = || {
        let url = crate::domain::app_url(state, deployment);
        let reason = if crate::domain::read_record(state, &deployment.id)
            .is_some_and(|record| record.verified)
        {
            "Verified custom domain is the default."
        } else {
            "Generated hostname is the default."
        };
        Address {
            url,
            reason: reason.into(),
        }
    };
    match selected {
        None => default(),
        Some("domain") => {
            if crate::domain::read_record(state, &deployment.id).is_some_and(|record| record.verified) {
                Address { url: crate::domain::app_url(state, deployment), reason: "Verified custom domain was chosen.".into() }
            } else {
                Address { reason: "Custom domain is not verified, so the generated hostname is in use.".into(), ..default() }
            }
        }
        Some("hostname") => Address {
            url: deployment.url.clone(),
            reason: "Generated hostname was chosen.".into(),
        },
        Some("path") => {
            let Some(origin) = &state.path_origin else {
                return Address {
                    reason: "Path address is unavailable, so the default address is in use.".into(),
                    ..default()
                };
            };
            let Some(record) =
                crate::path::read_record(state, &deployment.id).filter(|record| record.enabled)
            else {
                return Address {
                    reason: "Path address is not enabled, so the default address is in use.".into(),
                    ..default()
                };
            };
            if record.name.as_deref() == Some(crate::path::HOME_NAME) {
                Address {
                    url: format!("{}/", origin.trim_end_matches('/')),
                    reason: "Home path address was chosen.".into(),
                }
            } else {
                let segment = record.name.unwrap_or_else(|| deployment.id.clone());
                Address {
                    url: format!("{}/p/{segment}/", origin.trim_end_matches('/')),
                    reason: "Path address was chosen.".into(),
                }
            }
        }
        Some(_) => default(),
    }
}

pub fn enrich_html(
    html: &[u8],
    canonical: &str,
    title: &str,
    description: &str,
    image: Option<&str>,
) -> Vec<u8> {
    enrich_html_with(html, canonical, title, description, image, false)
}

fn enrich_html_with(
    html: &[u8],
    canonical: &str,
    title: &str,
    description: &str,
    image: Option<&str>,
    escape_entities: bool,
) -> Vec<u8> {
    let Ok(mut html) = String::from_utf8(html.to_vec()) else {
        return html.to_vec();
    };
    let head_end = head_end(&html);
    let head = html[..head_end].to_ascii_lowercase();
    let escaped = |value: &str| if escape_entities { escape_all(value) } else { escape(value) };
    let mut tags = String::new();
    if !has_value(&head, "link", "rel", "canonical") {
        tags.push_str(&format!(
            "<link rel=\"canonical\" href=\"{}\">",
            escaped(canonical)
        ));
    }
    for (key, value, tag) in [
        (
            "property",
            "og:url",
            format!(
                "<meta property=\"og:url\" content=\"{}\">",
                escaped(canonical)
            ),
        ),
        (
            "property",
            "og:title",
            format!(
                "<meta property=\"og:title\" content=\"{}\">",
                escaped(title)
            ),
        ),
        (
            "property",
            "og:description",
            format!(
                "<meta property=\"og:description\" content=\"{}\">",
                escaped(description)
            ),
        ),
        (
            "property",
            "og:type",
            "<meta property=\"og:type\" content=\"website\">".into(),
        ),
    ] {
        if !has_value(&head, "meta", key, value) {
            tags.push_str(&tag);
        }
    }
    if let Some(image) = image {
        if !has_value(&head, "meta", "property", "og:image") {
            let image = if image.starts_with("http://") || image.starts_with("https://") {
                image.to_owned()
            } else {
                format!(
                    "{}/{}",
                    canonical.trim_end_matches('/'),
                    image.trim_start_matches('/')
                )
            };
            tags.push_str(&format!(
                "<meta property=\"og:image\" content=\"{}\">",
                escaped(&image)
            ));
        }
    }
    if !has_value(&head, "meta", "name", "twitter:card") {
        tags.push_str(&format!(
            "<meta name=\"twitter:card\" content=\"{}\">",
            if image.is_some() {
                "summary_large_image"
            } else {
                "summary"
            }
        ));
    }
    html.insert_str(head_end, &tags);
    html.into_bytes()
}

pub fn page_html(html: &[u8], params: &serde_json::Value, title: &str, description: &str, image: Option<&str>, canonical: Option<&str>) -> Vec<u8> {
    let Ok(mut html) = String::from_utf8(html.to_vec()) else { return html.to_vec() };
    let escaped_title = escape_all(title);
    let lower = html.to_ascii_lowercase();
    let title_start = lower.find("<title").and_then(|start| lower[start..].find('>').map(|end| start + end + 1));
    let title_end = title_start.and_then(|start| lower[start..].find("</title").map(|end| start + end));
    if let (Some(start), Some(end)) = (title_start, title_end) {
        html.replace_range(start..end, &escaped_title);
    } else {
        let at = head_end(&html);
        html.insert_str(at, &format!("<title>{escaped_title}</title>"));
    }
    let encoded = serde_json::to_string(params).unwrap_or_else(|_| "{}".into());
    let meta = format!("<meta name=\"konae-page-params\" content=\"{}\">", escape_all(&encoded));
    html.insert_str(head_end(&html), &meta);
    if let Some(canonical) = canonical {
        for (element, key, value) in [
            ("link", "rel", "canonical"),
            ("meta", "property", "og:url"),
            ("meta", "property", "og:title"),
            ("meta", "property", "og:description"),
            ("meta", "property", "og:type"),
            ("meta", "property", "og:image"),
            ("meta", "name", "twitter:card"),
        ] {
            remove_head_tag(&mut html, element, key, value);
        }
        enrich_html_with(html.as_bytes(), canonical, title, description, image, true)
    } else {
        html.into_bytes()
    }
}

fn remove_head_tag(html: &mut String, element: &str, key: &str, value: &str) {
    let boundary = head_end(html);
    let head = html[..boundary].to_ascii_lowercase();
    let marker = format!("<{element}");
    let mut cursor = 0;
    while let Some(offset) = head[cursor..].find(&marker) {
        let start = cursor + offset;
        let Some(end) = head[start..].find('>').map(|end| start + end) else { return };
        let matches = attribute(&html[start..=end], key).is_some_and(|actual| actual.eq_ignore_ascii_case(value));
        cursor = end + 1;
        if matches {
            html.replace_range(start..cursor, "");
            return;
        }
    }
}

fn escape_all(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

pub fn page_canonical(base: &str, file: &str, routing: crate::artifact::Routing) -> String {
    let mut path = file.to_owned();
    if path == "index.html" {
        path.clear();
    } else if routing == crate::artifact::Routing::Pages && path.ends_with("/index.html") {
        path.truncate(path.len() - "/index.html".len());
        path.push('/');
    } else if routing == crate::artifact::Routing::Pages && path.ends_with(".html") {
        let extensionless = path.trim_end_matches(".html");
        if !extensionless.rsplit('/').next().unwrap_or(extensionless).contains('.')
            && crate::artifact::candidates(extensionless, routing).iter().any(|candidate| candidate == file)
        {
            path = extensionless.to_owned();
        }
    }
    format!(
        "{}{}",
        base.trim_end_matches('/'),
        if path.is_empty() {
            "/".into()
        } else {
            format!("/{path}")
        }
    )
}

pub fn page_path_canonical(base: &str, pattern: &str, params: &serde_json::Map<String, serde_json::Value>) -> String {
    let path = pattern.split('/').skip(1).map(|segment| {
        if let Some(key) = segment.strip_prefix(':') {
            params.get(key).map(|value| encode_path_value(&match value {
                serde_json::Value::String(value) => value.clone(),
                _ => value.to_string(),
            })).unwrap_or_default()
        } else { segment.to_owned() }
    }).collect::<Vec<_>>().join("/");
    format!("{}/{}", base.trim_end_matches('/'), path)
}

pub fn html_response(
    deployment: &Deployment,
    candidate: &str,
    bytes: &[u8],
    address: &Address,
    headers: &HeaderMap,
    cache: &ArtifactCache,
    navigation: bool,
) -> Option<Response> {
    if !(candidate.to_ascii_lowercase().ends_with(".html") || candidate.to_ascii_lowercase().ends_with(".htm")) {
        return None;
    }
    let canonical = page_canonical(&address.url, candidate, crate::artifact::routing(&deployment.manifest));
    let Some(title) = title(&bytes) else {
        let mut response = cache.respond(headers, bytes.to_vec(), candidate);
        response.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
        return Some(response);
    };
    let description = description(&bytes);
    let image = deployment.modules["seo"]["image"].as_str().map(|image| format!("{}/{}", address.url.trim_end_matches('/'), image.trim_start_matches('/')));
    let body = enrich_html(bytes, &canonical, &title, &description, image.as_deref());
    let option_state = serde_json::to_string(&deployment.modules["seo"]).unwrap_or_default();
    let digest = sha2::Sha256::digest([body.as_slice(), address.url.as_bytes(), option_state.as_bytes()].concat());
    let tagged = ArtifactCache { version: format!("{}-{}", deployment.artifact_version, hex::encode(digest)), private: cache.private };
    if !navigation && crate::etag_matches(headers, &tagged.version_tag()) { return Some(tagged.not_modified(tagged.version_tag())); }
    Some(tagged.respond(&HeaderMap::new(), body, candidate))
}

pub fn robots_response(
    deployment: &Deployment,
    candidate: &str,
    bytes: Option<Vec<u8>>,
    address: &Address,
    headers: &HeaderMap,
    cache: &ArtifactCache,
    navigation: bool,
) -> Option<Response> {
    if candidate != "robots.txt" { return None; }
    let mut body = bytes.unwrap_or_else(|| b"User-agent: *\nAllow: /\n".to_vec());
    if !String::from_utf8_lossy(&body).lines().any(|line| line.trim_start().to_ascii_lowercase().starts_with("sitemap:")) {
        if !body.is_empty() && !body.ends_with(b"\n") { body.push(b'\n'); }
        body.extend_from_slice(format!("Sitemap: {}/sitemap.xml\n", address.url.trim_end_matches('/')).as_bytes());
    }
    let digest = sha2::Sha256::digest([body.as_slice(), address.url.as_bytes()].concat());
    let tagged = ArtifactCache { version: format!("{}-{}", deployment.artifact_version, hex::encode(digest)), private: cache.private };
    if !navigation && crate::etag_matches(headers, &tagged.version_tag()) { return Some(tagged.not_modified(tagged.version_tag())); }
    Some(tagged.respond(&HeaderMap::new(), body, candidate))
}

pub fn sitemap_response(
    root: &Path,
    deployment: &Deployment,
    candidate: &str,
    bytes: Option<Vec<u8>>,
    address: &Address,
    headers: &HeaderMap,
    cache: &ArtifactCache,
) -> Option<Response> {
    if candidate != "sitemap.xml" { return None; }
    if let Some(bytes) = bytes { return Some(cache.respond(headers, bytes, candidate)); }
    let body = sitemap(root, deployment, &address.url);
    let digest = sha2::Sha256::digest([body.as_slice(), address.url.as_bytes()].concat());
    let tagged = ArtifactCache { version: format!("{}-{}", deployment.artifact_version, hex::encode(digest)), private: cache.private };
    Some(tagged.respond(headers, body, candidate))
}

pub fn title(html: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(html);
    let head = head_content(&text);
    let lower = head.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = head[start..].find('>')? + start + 1;
    let end = lower[open..].find("</title")? + open;
    let value = head[open..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub fn description(html: &[u8]) -> String {
    let text = String::from_utf8_lossy(html);
    let head = head_content(&text);
    let lower = head.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<meta") {
        let start = cursor + offset;
        let Some(end) = lower[start..].find('>').map(|end| start + end) else {
            break;
        };
        let tag = &head[start..=end];
        if attribute(tag, "name").is_some_and(|value| value.eq_ignore_ascii_case("description")) {
            return attribute(tag, "content").unwrap_or_default();
        }
        cursor = end + 1;
    }
    String::new()
}

fn head_content(html: &str) -> &str {
    let start = html.to_ascii_lowercase().find("<head").and_then(|start| {
        html[start..].find('>').map(|end| start + end + 1)
    }).unwrap_or(0);
    let end = head_end(html);
    if start <= end { &html[start..end] } else { "" }
}

fn head_end(html: &str) -> usize {
    let lower = html.to_ascii_lowercase();
    lower.find("</head>").unwrap_or_else(|| lower.find("<body").unwrap_or(html.len()))
}

pub fn sitemap(root: &Path, deployment: &Deployment, base: &str) -> Vec<u8> {
    let mut files = Vec::new();
    if crate::artifact::routing(&deployment.manifest) == crate::artifact::Routing::Spa {
        files.push("index.html".to_owned());
    } else {
        collect_html(
            &crate::layout::dist(root, &deployment.id),
            &crate::layout::dist(root, &deployment.id),
            &mut files,
        );
    }
    files.sort();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
    );
    for file in files {
        if file.eq_ignore_ascii_case("404.html") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(crate::layout::dist(root, &deployment.id).join(&file)) {
            if title(&bytes).is_some() {
                xml.push_str(&format!(
                    "<url><loc>{}</loc></url>",
                    escape(&page_canonical(base, &file, crate::artifact::routing(&deployment.manifest)))
                ));
            }
        }
    }
    if let Ok(program) = crate::action_shape::read(&deployment.actions) {
        for action in program.actions.values() {
            let Some(page) = action.page.as_ref() else { continue };
            let Some(template) = page.get("path").and_then(serde_json::Value::as_str) else { continue };
            let Some(list_name) = page.get("list").and_then(serde_json::Value::as_str) else { continue };
            let Some(list) = program.actions.get(list_name) else { continue };
            let Ok(rows) = crate::run_query(&crate::layout::database(root, &deployment.id), list, Vec::new()) else { continue };
            let segments = template.trim_start_matches('/').split('/').collect::<Vec<_>>();
            for row in rows {
                let Some(row) = row.as_object() else { continue };
                let mut path = String::new();
                let mut complete = true;
                for segment in &segments {
                    path.push('/');
                    if let Some(key) = segment.strip_prefix(':') {
                        let Some(value) = row.get(key) else { complete = false; break };
                        path.push_str(&encode_path_value(&match value {
                            serde_json::Value::String(value) => value.clone(),
                            _ => value.to_string(),
                        }));
                    } else {
                        path.push_str(segment);
                    }
                }
                if complete {
                    xml.push_str(&format!("<url><loc>{}</loc></url>", escape(&format!("{}{}", base.trim_end_matches('/'), path))));
                }
            }
        }
    }
    xml.push_str("</urlset>");
    xml.into_bytes()
}

pub(crate) fn encode_path_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn collect_html(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_html(root, &path, out);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
            })
        {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

fn escape(value: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let rest = &value[cursor..];
        let character = rest.chars().next().unwrap();
        if character == '&' {
            let entity_end = rest.find(';').filter(|end| {
                *end > 1
                    && rest[1..*end]
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'#')
            });
            if let Some(end) = entity_end {
                output.push_str(&rest[..=end]);
                cursor += end + 1;
            } else {
                output.push_str("&amp;");
                cursor += 1;
            }
        } else {
            output.push_str(match character {
                '"' => "&quot;",
                '<' => "&lt;",
                '>' => "&gt;",
                '\'' => "&#39;",
                _ => {
                    output.push(character);
                    cursor += character.len_utf8();
                    continue;
                }
            });
            cursor += character.len_utf8();
        }
    }
    output
}
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let quote = *tag.as_bytes().get(at)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let end = tag[at + 1..].find(quote as char)? + at + 1;
    Some(tag[at + 1..end].to_owned())
}

fn has_value(head: &str, element: &str, name: &str, value: &str) -> bool {
    let marker = format!("<{element}");
    let mut cursor = 0;
    while let Some(offset) = head[cursor..].find(&marker) {
        let start = cursor + offset;
        let Some(end) = head[start..].find('>').map(|end| start + end) else {
            return false;
        };
        if attribute(&head[start..=end], name)
            .is_some_and(|actual| actual.eq_ignore_ascii_case(value))
        {
            return true;
        }
        cursor = end + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_canonical_and_card_tags_with_escaped_app_text() {
        let html = br#"<!doctype html><html><head><title>A "small" <app> & more</title><meta name="description" content="A &amp; B"></head><body><a href="/x">x</a></body></html>"#;
        let output = String::from_utf8(enrich_html(
            html,
            "https://app.example/about",
            "A \"small\" <app> & more",
            "A & B",
            Some("https://app.example/card.png"),
        ))
        .unwrap();
        for expected in [
            r#"<link rel="canonical" href="https://app.example/about">"#,
            r#"<meta property="og:url" content="https://app.example/about">"#,
            r#"<meta property="og:title" content="A &quot;small&quot; &lt;app&gt; &amp; more">"#,
            r#"<meta property="og:description" content="A &amp; B">"#,
            r#"<meta property="og:image" content="https://app.example/card.png">"#,
            r#"<meta property="og:type" content="website">"#,
            r#"<meta name="twitter:card" content="summary_large_image">"#,
        ] {
            assert!(output.contains(expected), "missing {expected} in {output}");
        }
        assert!(output.contains("<a href=\"/x\">x</a>"));
    }

    #[test]
    fn leaves_existing_card_tags_untouched() {
        let html = br#"<html><head><title>Title</title><meta property="og:title" content="Owner title"></head><body></body></html>"#;
        let output =
            String::from_utf8(enrich_html(html, "https://app.example/", "Title", "", None))
                .unwrap();
        assert_eq!(output.matches("og:title").count(), 1);
        assert!(output.contains("content=\"Owner title\""));
    }

    #[test]
    fn canonical_pages_drop_html_suffix_and_index_names() {
        assert_eq!(
            page_canonical("https://app.example/", "index.html", crate::artifact::Routing::Pages),
            "https://app.example/"
        );
        assert_eq!(
            page_canonical("https://app.example/", "about.html", crate::artifact::Routing::Pages),
            "https://app.example/about"
        );
        assert_eq!(
            page_canonical("https://app.example/", "guide/index.html", crate::artifact::Routing::Pages),
            "https://app.example/guide/"
        );
    }

    #[test]
    fn file_canonicals_resolve_to_the_same_artifact_in_each_routing_mode() {
        for routing in [crate::artifact::Routing::Pages, crate::artifact::Routing::Spa] {
            for file in ["v1.2.html", "page.htm", "about.html", "index.html", "guide/index.html"] {
                let canonical = page_canonical("https://app.example", file, routing);
                let path = canonical.strip_prefix("https://app.example/").unwrap_or("");
                let answer = crate::artifact::candidates(path, routing);
                assert!(answer.iter().any(|candidate| candidate == file), "{routing:?} {file} canonicalized to {canonical}, candidates were {answer:?}");
            }
        }
    }

    #[test]
    fn page_path_canonical_uses_the_sitemap_segment_encoder() {
        let params = serde_json::from_value(serde_json::json!({"slug": "c# 100%"})).unwrap();
        assert_eq!(page_path_canonical("https://app.example/", "/posts/:slug", &params), "https://app.example/posts/c%23%20100%25");
    }

    #[test]
    fn description_skips_other_metadata_and_reads_an_escaped_value() {
        let html = br#"<head><meta charset="utf-8"><meta content="Tea &amp; toast" name="description"></head>"#;
        assert_eq!(description(html), "Tea &amp; toast");
    }

    #[test]
    fn card_text_preserves_entities_and_escapes_bare_ampersands_and_quotes() {
        let html = br#"<html><head><title>Notes &mdash; Caf&#233; &nbsp; fish & chips "today"</title><meta name="description" content="Notes &mdash; Caf&#233; &nbsp; fish &amp; chips &quot;today&quot;"></head><body></body></html>"#;
        let title = title(html).unwrap();
        let description = description(html);
        let output = String::from_utf8(enrich_html(html, "https://app.example/", &title, &description, None)).unwrap();
        assert!(output.contains("property=\"og:title\" content=\"Notes &mdash; Caf&#233; &nbsp; fish &amp; chips &quot;today&quot;\""));
        assert!(output.contains("property=\"og:description\" content=\"Notes &mdash; Caf&#233; &nbsp; fish &amp; chips &quot;today&quot;\""));
    }
}
