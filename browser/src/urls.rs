//! Omnibox logic: turning what the user typed into a URL, internal pages, search engines.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

/// Base URL under which the `aurora` custom protocol is served.
/// WebView2 (Windows) maps custom schemes to `http://<scheme>.<host>/`.
#[cfg(windows)]
pub const INTERNAL_BASE: &str = "http://aurora.localhost/";
#[cfg(not(windows))]
pub const INTERNAL_BASE: &str = "aurora://localhost/";

#[cfg(windows)]
pub const UI_URL: &str = "http://auroraui.localhost/";
#[cfg(not(windows))]
pub const UI_URL: &str = "auroraui://localhost/";

pub const INTERNAL_PAGES: &[&str] = &["newtab", "history", "bookmarks", "downloads", "settings", "shortcuts"];

pub fn search_url(engine: &str, q: &str) -> String {
    let q = utf8_percent_encode(q, NON_ALPHANUMERIC).to_string();
    match engine {
        "duckduckgo" => format!("https://duckduckgo.com/?q={q}"),
        "bing" => format!("https://www.bing.com/search?q={q}"),
        "brave" => format!("https://search.brave.com/search?q={q}"),
        "yandex" => format!("https://yandex.ru/search/?text={q}"),
        "startpage" => format!("https://www.startpage.com/do/search?q={q}"),
        _ => format!("https://www.google.com/search?q={q}"),
    }
}

/// `aurora://history` -> real URL served by the custom protocol.
pub fn internal_to_real(url: &str) -> Option<String> {
    let page = url.strip_prefix("aurora://")?.trim_end_matches('/');
    let page = if page.is_empty() { "newtab" } else { page };
    INTERNAL_PAGES
        .contains(&page)
        .then(|| format!("{INTERNAL_BASE}?p={page}"))
}

pub fn is_internal_real(url: &str) -> bool {
    url.starts_with(INTERNAL_BASE)
}

/// Real URL -> what the user sees in the address bar.
pub fn display_url(real: &str) -> String {
    if let Some(rest) = real.strip_prefix(INTERNAL_BASE) {
        let page = rest
            .split(['?', '&'])
            .find_map(|kv| kv.strip_prefix("p="))
            .unwrap_or("newtab");
        return format!("aurora://{page}");
    }
    real.to_string()
}

fn looks_like_host(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let host_no_port = host.rsplit_once(':').map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host }).unwrap_or(host);
    if host_no_port == "localhost" {
        return true;
    }
    if host_no_port.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    let parts: Vec<&str> = host_no_port.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| !p.is_empty())
        && parts
            .last()
            .map(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_alphanumeric() || c == '-'))
            .unwrap_or(false)
}

/// Resolve omnibox input to a URL that can be loaded.
pub fn resolve(input: &str, engine: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return internal_to_real("aurora://newtab").unwrap();
    }
    if let Some(real) = internal_to_real(s) {
        return real;
    }
    let lower = s.to_ascii_lowercase();
    for scheme in ["http://", "https://", "file://", "about:", "data:", "view-source:"] {
        if lower.starts_with(scheme) {
            return s.to_string();
        }
    }
    if looks_like_host(s) {
        let scheme = if lower.starts_with("localhost") || lower.starts_with("127.") || lower.starts_with("192.168.") {
            "http://"
        } else {
            "https://"
        };
        return format!("{scheme}{s}");
    }
    search_url(engine, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves() {
        assert_eq!(resolve("example.com", "google"), "https://example.com");
        assert_eq!(resolve("localhost:3000/x", "google"), "http://localhost:3000/x");
        assert_eq!(resolve("https://a.b/c", "google"), "https://a.b/c");
        assert!(resolve("rust borrow checker", "duckduckgo").starts_with("https://duckduckgo.com/?q=rust%20borrow"));
        assert!(resolve("hello", "google").contains("google.com/search"));
        assert_eq!(display_url(&resolve("aurora://history", "google")), "aurora://history");
        assert_eq!(display_url(&resolve("", "google")), "aurora://newtab");
        assert!(resolve("1.5", "google").contains("search"));
    }
}
