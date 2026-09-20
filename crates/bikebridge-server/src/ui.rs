//! Compiled Svelte products are embedded so the CLI needs no web runtime.
use axum::{http::header, response::IntoResponse};

fn asset(content: &'static str, mime: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'",
            ),
        ],
        content,
    )
}

pub(crate) async fn index() -> impl IntoResponse {
    asset(
        include_str!("../../../products/dashboard/dist/index.html"),
        "text/html; charset=utf-8",
    )
}
pub(crate) async fn javascript() -> impl IntoResponse {
    asset(
        include_str!("../../../products/dashboard/dist/app.js"),
        "text/javascript; charset=utf-8",
    )
}
pub(crate) async fn stylesheet() -> impl IntoResponse {
    asset(
        include_str!("../../../products/dashboard/dist/app.css"),
        "text/css; charset=utf-8",
    )
}

pub(crate) async fn overlay_index() -> impl IntoResponse {
    asset(
        include_str!("../../../products/stream-overlay/dist/index.html"),
        "text/html; charset=utf-8",
    )
}
pub(crate) async fn overlay_javascript() -> impl IntoResponse {
    asset(
        include_str!("../../../products/stream-overlay/dist/app.js"),
        "text/javascript; charset=utf-8",
    )
}
pub(crate) async fn overlay_stylesheet() -> impl IntoResponse {
    asset(
        include_str!("../../../products/stream-overlay/dist/app.css"),
        "text/css; charset=utf-8",
    )
}
