
use std::sync::LazyLock;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use sha2::{Digest, Sha256};

use super::state::Konae;

pub(super) const CSS: &str = include_str!("owner-page/owner.css");

pub(super) static STYLE_HASH: LazyLock<String> = LazyLock::new(|| {
    use base64::Engine;
    format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(CSS.as_bytes())))
});

pub(super) const ICON: &str = r#"<link rel=icon href="data:image/svg+xml,%3Csvg xmlns=%27http://www.w3.org/2000/svg%27 viewBox=%270 0 32 32%27%3E%3Crect x=%273%27 y=%273%27 width=%2726%27 height=%2726%27 fill=%27%23c8f34a%27 stroke=%27%2320231c%27 stroke-width=%273%27/%3E%3C/svg%3E">"#;

pub(super) const HEADER: &str = "<header class=header><a class=brand href=/_konae/owner/login><span class=brand-mark aria-hidden=true><svg width=16 height=16 viewBox=\"0 0 24 24\" fill=none stroke=currentColor stroke-width=2 stroke-linecap=round stroke-linejoin=round><path d=\"M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z\"/><path d=\"m3.3 7 8.7 5 8.7-5\"/><path d=\"M12 22V12\"/></svg></span>konae</a></header>";

pub(super) static LOGIN: LazyLock<String> = LazyLock::new(|| {
    include_str!("owner-login.html")
        .replace("/*OWNER_CSS*/", CSS)
        .replace("<!--OWNER_HEADER-->", HEADER)
        .replace("<!--OWNER_ICON-->", ICON)
});

pub fn routes() -> Router<Konae> {
    Router::new().route("/_konae/owner/fonts/{name}", get(font))
}

async fn font(State(state): State<Konae>, headers: HeaderMap, Path(name): Path<String>) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    match kernel::own_page::font_bytes(&name) {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "font/woff2"),
                (header::CACHE_CONTROL, "public, max-age=604800"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_hash_covers_the_stylesheet() {
        use base64::Engine;
        let digest = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(CSS.as_bytes()));
        assert_eq!(*STYLE_HASH, format!("'sha256-{digest}'"));
        for (file, _) in kernel::own_page::FONTS {
            assert!(CSS.contains(&format!("/_konae/owner/fonts/{file}")), "{file} is served but unused");
        }
    }

    #[test]
    fn the_token_form_offers_one_day_a_week_and_a_month_and_sends_the_one_chosen() {
        let page = LOGIN.as_str();
        let select = page.split(r#"<select class="select" id="ttl">"#).nth(1).and_then(|rest| rest.split("</select>").next()).expect("the token form has a duration select");
        let options: Vec<&str> = select.split("<option").skip(1).collect();
        assert_eq!(options.len(), 3, "{select}");
        for (option, (seconds, label)) in options.iter().zip([("86400", "1 day"), ("604800", "7 days"), ("2592000", "30 days")]) {
            assert!(option.contains(&format!(r#"value="{seconds}""#)) && option.contains(&format!(">{label}</option>")), "{option}");
        }
        assert!(options[0].contains("selected"), "1 day is the default");
        assert!(page.contains("ttlSeconds:Number($('#ttl').value)"), "the page sends the chosen duration");
        assert!(!page.contains("ttlSeconds:86400"), "the duration is no longer fixed");
        assert!(page.contains("selectedOptions[0].textContent"), "the summary names the duration");
    }
}
