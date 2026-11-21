//! `aurora://` internal pages: new tab, history, bookmarks, downloads, settings, shortcuts.
//! One HTML app; the data snapshot is injected into the HTML at request time.

use crate::store::Store;
use serde_json::json;
use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};
use wry::http::{header, Request, Response, StatusCode};

const PAGES_HTML: &str = include_str!("ui/pages.html");

pub fn serve(store: &Arc<Mutex<Store>>, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = req.uri().path();
    if path != "/" && path != "/index.html" {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(&b"not found"[..]))
            .unwrap();
    }
    let page = req
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("p="))
        .unwrap_or("newtab")
        .to_string();

    let data = {
        let s = store.lock().unwrap();
        let top: Vec<_> = s
            .top_sites(8)
            .into_iter()
            .map(|h| json!({"title": h.title, "url": h.url}))
            .collect();
        json!({
            "page": page,
            "settings": s.settings,
            "bookmarks": s.bookmarks,
            "history": s.history.iter().take(1500).collect::<Vec<_>>(),
            "downloads": s.downloads,
            "topSites": top,
        })
    };
    // `</` must never appear inside the inline <script>
    let data = data.to_string().replace("</", "<\\/");
    let html = PAGES_HTML.replace("/*__DATA__*/null", &data);

    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        // internal pages can trigger privileged actions: never allow framing (clickjacking)
        .header("X-Frame-Options", "DENY")
        .header(
            "Content-Security-Policy",
            "frame-ancestors 'none'; default-src 'self' 'unsafe-inline' data: https: http:",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Owned(html.into_bytes()))
        .unwrap()
}
