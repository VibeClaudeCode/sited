//! Persistent browser data: settings, bookmarks, history, downloads, session.
//! Everything is stored as small JSON files in the platform data directory.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// google | duckduckgo | bing | brave | yandex | startpage
    pub search: String,
    /// Accent colour of the UI (CSS colour).
    pub accent: String,
    /// Tracker / ad shield.
    pub shield: bool,
    /// Restore tabs of the previous session on start.
    pub restore_session: bool,
    /// Put background tabs to sleep after N minutes (0 = never).
    pub sleep_minutes: u64,
    /// Reduce motion (shorter, calmer animations).
    pub reduce_motion: bool,
    /// Tint the toolbar with the site's theme colour.
    pub site_tint: bool,
    /// Send "Do Not Track" + "Global Privacy Control" signals.
    pub dnt: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            search: "google".into(),
            accent: "#7c5cff".into(),
            shield: true,
            restore_session: true,
            sleep_minutes: 30,
            reduce_motion: false,
            site_tint: true,
            dnt: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    pub added: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryItem {
    pub title: String,
    pub url: String,
    pub visits: u32,
    pub last: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Download {
    pub url: String,
    pub path: String,
    /// None = in progress, Some(true) = done, Some(false) = failed
    pub ok: Option<bool>,
    pub ts: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
}

pub struct Store {
    dir: PathBuf,
    pub settings: Settings,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<HistoryItem>,
    pub downloads: Vec<Download>,
    pub session: Session,
    pub history_dirty: bool,
}

const MAX_HISTORY: usize = 5000;

fn load<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save<T: Serialize>(path: &Path, v: &T) {
    if let Ok(json) = serde_json::to_vec_pretty(v) {
        // write atomically: tmp file + rename
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
}

impl Store {
    pub fn open() -> Self {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Aurora");
        let _ = fs::create_dir_all(&dir);
        Self {
            settings: load(&dir.join("settings.json")),
            bookmarks: load(&dir.join("bookmarks.json")),
            history: load(&dir.join("history.json")),
            downloads: load(&dir.join("downloads.json")),
            session: load(&dir.join("session.json")),
            history_dirty: false,
            dir,
        }
    }

    pub fn profile_dir(&self) -> PathBuf {
        self.dir.join("profile")
    }

    pub fn save_settings(&self) {
        save(&self.dir.join("settings.json"), &self.settings);
    }
    pub fn save_bookmarks(&self) {
        save(&self.dir.join("bookmarks.json"), &self.bookmarks);
    }
    pub fn save_downloads(&self) {
        save(&self.dir.join("downloads.json"), &self.downloads);
    }
    pub fn save_session(&self) {
        save(&self.dir.join("session.json"), &self.session);
    }
    pub fn flush_history(&mut self) {
        if self.history_dirty {
            save(&self.dir.join("history.json"), &self.history);
            self.history_dirty = false;
        }
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    /// Toggle a bookmark; returns the new state.
    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        if let Some(i) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(i);
            self.save_bookmarks();
            false
        } else {
            self.bookmarks.insert(
                0,
                Bookmark {
                    title: if title.is_empty() { url.into() } else { title.into() },
                    url: url.into(),
                    added: now(),
                },
            );
            self.save_bookmarks();
            true
        }
    }

    pub fn record_visit(&mut self, url: &str, title: &str) {
        if url.is_empty() || url.starts_with("about:") || url.starts_with("aurora:") {
            return;
        }
        if let Some(i) = self.history.iter().position(|h| h.url == url) {
            let mut item = self.history.remove(i);
            item.visits += 1;
            item.last = now();
            if !title.is_empty() {
                item.title = title.into();
            }
            self.history.insert(0, item);
        } else {
            self.history.insert(
                0,
                HistoryItem {
                    title: title.into(),
                    url: url.into(),
                    visits: 1,
                    last: now(),
                },
            );
            self.history.truncate(MAX_HISTORY);
        }
        self.history_dirty = true;
    }

    pub fn update_title(&mut self, url: &str, title: &str) {
        if let Some(h) = self.history.iter_mut().find(|h| h.url == url) {
            if h.title != title {
                h.title = title.into();
                self.history_dirty = true;
            }
        }
    }

    /// Most visited sites, one entry per host.
    pub fn top_sites(&self, n: usize) -> Vec<&HistoryItem> {
        let mut items: Vec<&HistoryItem> = self.history.iter().collect();
        items.sort_by(|a, b| b.visits.cmp(&a.visits).then(b.last.cmp(&a.last)));
        let mut seen = std::collections::HashSet::new();
        items
            .into_iter()
            .filter(|h| {
                let host = url::Url::parse(&h.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|s| s.to_string()))
                    .unwrap_or_default();
                !host.is_empty() && seen.insert(host)
            })
            .take(n)
            .collect()
    }
}
