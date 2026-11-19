//! The look of the pages the Kernel answers on an app's host itself: sign-in,
//! account recovery, invitations, Google sign-in, entry, maintenance and not found.
//! They share one stylesheet on the design system's tokens and its two fonts.

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

/// Every page holds this marker inside its `<style>` and `style` replaces it.
pub const MARKER: &str = "/*KERNEL_PAGE_CSS*/";

pub const CSS: &str = include_str!("own-page/page.css");

pub const FONTS: [(&str, &[u8]); 3] = [
    ("bricolage-latin.woff2", include_bytes!("own-page/bricolage-latin.woff2")),
    ("bricolage-latin-ext.woff2", include_bytes!("own-page/bricolage-latin-ext.woff2")),
    ("jetbrains-mono-latin.woff2", include_bytes!("own-page/jetbrains-mono-latin.woff2")),
];

/// The page with the stylesheet in place of its marker.
pub fn style(page: &str) -> String {
    page.replacen(MARKER, CSS, 1)
}

pub fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

pub fn font_bytes(name: &str) -> Option<&'static [u8]> {
    FONTS.iter().find(|(file, _)| *file == name).map(|(_, bytes)| *bytes)
}

pub async fn font(Path(name): Path<String>) -> Response {
    match font_bytes(&name) {
        Some(bytes) => ([(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "public, max-age=604800")], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_names_every_font_it_serves() {
        for (file, _) in FONTS {
            assert!(CSS.contains(&format!("/_konae/fonts/{file}")), "{file} is served but unused");
        }
        assert!(!CSS.contains("</style"), "the stylesheet would close its own tag");
    }
}
