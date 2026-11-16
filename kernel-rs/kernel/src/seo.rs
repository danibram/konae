use crate::{AppState, Deployment};
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
            "verified custom domain is the default"
        } else {
            "generated hostname is the default"
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
                Address { url: crate::domain::app_url(state, deployment), reason: "verified custom domain was chosen".into() }
            } else {
                Address { reason: "custom domain is not verified; using the generated hostname".into(), ..default() }
            }
        }
        Some("hostname") => Address {
            url: deployment.url.clone(),
            reason: "hostname was chosen".into(),
        },
        Some("path") => {
            let Some(origin) = &state.path_origin else {
                return Address {
                    reason: "path address is unavailable; using the default address".into(),
                    ..default()
                };
            };
            let Some(record) =
                crate::path::read_record(state, &deployment.id).filter(|record| record.enabled)
            else {
                return Address {
                    reason: "path address is not enabled; using the default address".into(),
                    ..default()
                };
            };
            if record.name.as_deref() == Some(crate::path::HOME_NAME) {
                Address {
                    url: format!("{}/", origin.trim_end_matches('/')),
                    reason: "home path address was chosen".into(),
                }
            } else {
                let segment = record.name.unwrap_or_else(|| deployment.id.clone());
                Address {
                    url: format!("{}/p/{segment}/", origin.trim_end_matches('/')),
                    reason: "path address was chosen".into(),
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
    let Ok(mut html) = String::from_utf8(html.to_vec()) else {
        return html.to_vec();
    };
    let lower_html = html.to_ascii_lowercase();
    let head_end = lower_html
        .find("</head>")
        .unwrap_or_else(|| lower_html.find("<body").unwrap_or(html.len()));
    let head = html[..head_end].to_ascii_lowercase();
    let escaped = |value: &str| escape(value);
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

pub fn page_canonical(base: &str, file: &str) -> String {
    let mut path = file
        .trim_end_matches(".html")
        .trim_end_matches(".htm")
        .to_owned();
    if path == "index" {
        path.clear();
    } else if let Some(directory) = path.strip_suffix("/index") {
        path = directory.to_owned();
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

pub fn title(html: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(html);
    let lower = text.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = text[start..].find('>')? + start + 1;
    let end = lower[open..].find("</title")? + open;
    let value = text[open..end].trim();
    (!value.is_empty()).then(|| decode_entities(value))
}

pub fn description(html: &[u8]) -> String {
    let text = String::from_utf8_lossy(html);
    let lower = text.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<meta") {
        let start = cursor + offset;
        let Some(end) = lower[start..].find('>').map(|end| start + end) else {
            break;
        };
        let tag = &text[start..=end];
        if attribute(tag, "name").is_some_and(|value| value.eq_ignore_ascii_case("description")) {
            return attribute(tag, "content")
                .map(|value| decode_entities(&value))
                .unwrap_or_default();
        }
        cursor = end + 1;
    }
    String::new()
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
                    escape(&page_canonical(base, &file))
                ));
            }
        }
    }
    xml.push_str("</urlset>");
    xml.into_bytes()
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
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
}
fn decode_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
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
            page_canonical("https://app.example/", "index.html"),
            "https://app.example/"
        );
        assert_eq!(
            page_canonical("https://app.example/", "about.html"),
            "https://app.example/about"
        );
        assert_eq!(
            page_canonical("https://app.example/", "guide/index.html"),
            "https://app.example/guide"
        );
    }

    #[test]
    fn description_skips_other_metadata_and_reads_an_escaped_value() {
        let html = br#"<head><meta charset="utf-8"><meta content="Tea &amp; toast" name="description"></head>"#;
        assert_eq!(description(html), "Tea & toast");
    }
}
