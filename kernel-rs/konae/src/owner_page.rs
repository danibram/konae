//! The look of the owner door the server serves without the editor: owner
//! sign-in on `/_konae/owner/login` and OAuth consent on `/oauth/authorize`.
//! Both inline one stylesheet from the design system's tokens and load its
//! two typefaces from this binary, so the door looks the same on an
//! installation that serves no editor.

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

/// The stylesheet both pages inline in one `<style>` element.
pub(super) const CSS: &str = include_str!("owner-page/owner.css");

/// The CSP source that allows exactly [`CSS`] as an inline style.
pub(super) static STYLE_HASH: LazyLock<String> = LazyLock::new(|| {
    use base64::Engine;
    format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(CSS.as_bytes())))
});

/// The tab icon: the acid square of the brand mark.
pub(super) const ICON: &str = r#"<link rel=icon href="data:image/svg+xml,%3Csvg xmlns=%27http://www.w3.org/2000/svg%27 viewBox=%270 0 32 32%27%3E%3Crect x=%273%27 y=%273%27 width=%2726%27 height=%2726%27 fill=%27%23c8f34a%27 stroke=%27%2320231c%27 stroke-width=%273%27/%3E%3C/svg%3E">"#;

/// The header every owner page opens with: the brand and a note on which
/// door this is.
pub(super) const HEADER: &str = "<header class=header><a class=brand href=/_konae/owner/login><span class=brand-mark aria-hidden=true><svg width=16 height=16 viewBox=\"0 0 24 24\" fill=none stroke=currentColor stroke-width=2 stroke-linecap=round stroke-linejoin=round><path d=\"M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z\"/><path d=\"m3.3 7 8.7 5 8.7-5\"/><path d=\"M12 22V12\"/></svg></span>konae</a></header>";

/// The owner sign-in page with the stylesheet and header in place.
pub(super) static LOGIN: LazyLock<String> = LazyLock::new(|| {
    include_str!("owner-login.html")
        .replace("/*OWNER_CSS*/", CSS)
        .replace("<!--OWNER_HEADER-->", HEADER)
        .replace("<!--OWNER_ICON-->", ICON)
});

const FONTS: [(&str, &[u8]); 3] = [
    ("bricolage-latin.woff2", include_bytes!("owner-page/bricolage-latin.woff2")),
    ("bricolage-latin-ext.woff2", include_bytes!("owner-page/bricolage-latin-ext.woff2")),
    ("jetbrains-mono-latin.woff2", include_bytes!("owner-page/jetbrains-mono-latin.woff2")),
];

pub fn routes() -> Router<Konae> {
    Router::new().route("/_konae/owner/fonts/{name}", get(font))
}

async fn font(State(state): State<Konae>, headers: HeaderMap, Path(name): Path<String>) -> Response {
    if !super::oauth::valid_owner_host(&state, &headers).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    match FONTS.iter().find(|(file, _)| *file == name) {
        Some((_, bytes)) => (
            [
                (header::CONTENT_TYPE, "font/woff2"),
                (header::CACHE_CONTROL, "public, max-age=604800"),
            ],
            *bytes,
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
        for (file, _) in FONTS {
            assert!(CSS.contains(&format!("/_konae/owner/fonts/{file}")), "{file} is served but unused");
        }
    }
}
