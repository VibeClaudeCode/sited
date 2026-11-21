//! Aurora — a motion-designed browser written in Rust.
//!
//! Architecture
//! ------------
//! * One native window (tao).
//! * A "chrome" webview at the top: tab strip, toolbar, command bar — plain HTML/CSS/JS
//!   with all the motion design (src/ui/chrome.html).
//! * One child webview per tab (wry). On Windows this is Chromium via Microsoft Edge WebView2,
//!   on macOS WKWebView, on Linux WebKitGTK.
//! * Everything talks over JSON IPC; Rust owns the state and pushes snapshots to the UI.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod pages;
mod store;
mod urls;

use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use store::{Download, SessionTab, Store};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Fullscreen, Window, WindowBuilder},
};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};

const CHROME_H: f64 = 88.0;
const SPLIT_GAP: f64 = 6.0;
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(20);
const CHROME_HTML: &str = include_str!("ui/chrome.html");
const CONTENT_JS: &str = include_str!("ui/content.js");

#[derive(Debug)]
enum UserEvent {
    Chrome(String),
    Content { tab: u32, internal: bool, body: String },
    Load { tab: u32, started: bool, url: String },
    Title { tab: u32, title: String },
    OpenTab { url: String },
    DownloadStarted { url: String, path: PathBuf },
    DownloadDone { url: String, path: Option<PathBuf>, ok: bool },
}

struct Tab {
    id: u32,
    webview: Option<WebView>,
    /// Real URL (internal pages are http://aurora.localhost/... on Windows)
    url: String,
    title: String,
    loading: bool,
    favicon: String,
    theme: String,
    incognito: bool,
    pinned: bool,
    zoom: f64,
    blocked: u32,
    dark: bool,
    last_active: Instant,
}

struct App {
    window: Window,
    chrome: WebView,
    context: WebContext,
    #[cfg(target_os = "linux")]
    protocol_registered: bool,
    proxy: EventLoopProxy<UserEvent>,
    store: Arc<Mutex<Store>>,
    tabs: Vec<Tab>,
    active: u32,
    split: Option<u32>,
    overlay: bool,
    fullscreen: bool,
    next_id: u32,
    closed: Vec<String>,
    chrome_ready: bool,
}

fn main() -> wry::Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let store = Arc::new(Mutex::new(Store::open()));

    #[allow(unused_mut)]
    let mut wb = WindowBuilder::new()
        .with_title("Aurora")
        .with_inner_size(LogicalSize::new(1320.0, 840.0))
        .with_min_inner_size(LogicalSize::new(560.0, 360.0))
        // dark backdrop: visible in the split-view gap and while tabs are being created
        .with_background_color((14, 13, 20, 255));
    #[cfg(windows)]
    {
        // Frameless window with our own animated title bar; tao keeps the native shadow
        // and resize borders.
        use tao::platform::windows::WindowBuilderExtWindows;
        wb = wb.with_decorations(false).with_undecorated_shadow(true);
    }
    let window = wb.build(&event_loop).expect("window");

    let chrome = {
        let p = proxy.clone();
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        WebViewBuilder::new()
            // the UI is served from its own scheme (with_html has no working IPC origin on some engines)
            .with_custom_protocol("auroraui".into(), |_id, _req| {
                wry::http::Response::builder()
                    .header(wry::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(std::borrow::Cow::Borrowed(CHROME_HTML.as_bytes()))
                    .unwrap()
            })
            .with_url(urls::UI_URL)
            .with_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(size.width, CHROME_H).into(),
            })
            .with_transparent(false)
            .with_background_color((14, 13, 20, 255))
            .with_devtools(cfg!(debug_assertions))
            .with_hotkeys_zoom(false)
            .with_ipc_handler(move |req| {
                let _ = p.send_event(UserEvent::Chrome(req.into_body()));
            })
            .build_as_child(&window)?
    };

    let profile = store.lock().unwrap().profile_dir();
    let mut app = App {
        window,
        chrome,
        context: WebContext::new(Some(profile)),
        #[cfg(target_os = "linux")]
        protocol_registered: false,
        proxy,
        store,
        tabs: Vec::new(),
        active: 0,
        split: None,
        overlay: false,
        fullscreen: false,
        next_id: 1,
        closed: Vec::new(),
        chrome_ready: false,
    };
    app.restore_or_new();
    app.layout();

    // fixed deadline: a constantly busy event queue must not postpone housekeeping forever
    let mut next_housekeeping = Instant::now() + HOUSEKEEPING_EVERY;
    event_loop.run(move |event, _, control_flow| {
        if Instant::now() >= next_housekeeping {
            app.housekeeping();
            next_housekeeping = Instant::now() + HOUSEKEEPING_EVERY;
        }
        *control_flow = ControlFlow::WaitUntil(next_housekeeping);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {}
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                app.layout();
                app.push_state();
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                app.shutdown();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(ev) => {
                if app.handle(ev) {
                    app.shutdown();
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

impl App {
    // ------------------------------------------------------------------ tabs

    fn restore_or_new(&mut self) {
        let (restore, session) = {
            let s = self.store.lock().unwrap();
            (s.settings.restore_session, s.session.clone())
        };
        if restore && !session.tabs.is_empty() {
            let active_idx = session.active.min(session.tabs.len() - 1);
            for (i, t) in session.tabs.iter().enumerate() {
                // restored tabs are lazy: their webview is created on first activation
                let id = self.add_tab(&t.url, false, false);
                if let Some(tab) = self.tab_mut(id) {
                    tab.title = t.title.clone();
                    tab.pinned = t.pinned;
                }
                if i == active_idx {
                    self.active = id;
                }
            }
            let id = self.active;
            self.activate(id);
        } else {
            let url = urls::internal_to_real("aurora://newtab").unwrap();
            let id = self.add_tab(&url, false, false);
            self.activate(id);
        }
    }

    /// Adds a tab record (without creating a webview) and returns its id.
    fn add_tab(&mut self, url: &str, incognito: bool, after_active: bool) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let tab = Tab {
            id,
            webview: None,
            url: url.to_string(),
            title: String::new(),
            loading: false,
            favicon: String::new(),
            theme: String::new(),
            incognito,
            pinned: false,
            zoom: 1.0,
            blocked: 0,
            dark: false,
            last_active: Instant::now(),
        };
        let pos = if after_active {
            self.index_of(self.active).map(|i| i + 1).unwrap_or(self.tabs.len())
        } else {
            self.tabs.len()
        };
        self.tabs.insert(pos, tab);
        id
    }

    fn open_tab(&mut self, url: &str, incognito: bool, activate: bool) -> u32 {
        let id = self.add_tab(url, incognito, true);
        if activate {
            self.activate(id);
        } else {
            self.push_state();
        }
        id
    }

    fn index_of(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }
    fn tab(&self, id: u32) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }
    fn tab_mut(&mut self, id: u32) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }
    fn active_tab(&self) -> Option<&Tab> {
        self.tab(self.active)
    }
    fn active_view(&self) -> Option<&WebView> {
        self.active_tab().and_then(|t| t.webview.as_ref())
    }

    fn content_script(&self) -> String {
        let s = self.store.lock().unwrap();
        CONTENT_JS
            .replace("__SHIELD__", if s.settings.shield { "true" } else { "false" })
            .replace("__DNT__", if s.settings.dnt { "true" } else { "false" })
    }

    fn ensure_webview(&mut self, id: u32) {
        let Some(idx) = self.index_of(id) else { return };
        if self.tabs[idx].webview.is_some() {
            return;
        }
        let url = self.tabs[idx].url.clone();
        let incognito = self.tabs[idx].incognito;
        let zoom = self.tabs[idx].zoom;
        let script = self.content_script();
        let proxy = self.proxy.clone();
        let store = self.store.clone();

        let ipc_proxy = proxy.clone();
        let load_proxy = proxy.clone();
        let title_proxy = proxy.clone();
        let nw_proxy = proxy.clone();
        let dl_proxy = proxy.clone();
        let dl2_proxy = proxy;

        let rect = self.content_rect(false);
        let mut builder = if incognito {
            WebViewBuilder::new().with_incognito(true)
        } else {
            WebViewBuilder::new_with_web_context(&mut self.context)
        };
        #[cfg(target_os = "linux")]
        let register = incognito || !self.protocol_registered;
        #[cfg(not(target_os = "linux"))]
        let register = true;
        if register {
            builder = builder.with_custom_protocol("aurora".into(), move |_id, req| pages::serve(&store, req));
        }
        builder = builder
            .with_url(url)
            .with_visible(false)
            .with_devtools(true)
            .with_clipboard(true)
            .with_autoplay(true)
            .with_hotkeys_zoom(true)
            .with_back_forward_navigation_gestures(true)
            .with_background_color((255, 255, 255, 255))
            .with_initialization_script_for_main_only(script, true)
            .with_ipc_handler(move |req| {
                let internal = urls::is_internal_real(&req.uri().to_string());
                let _ = ipc_proxy.send_event(UserEvent::Content { tab: id, internal, body: req.into_body() });
            })
            .with_on_page_load_handler(move |ev, url| {
                let _ = load_proxy.send_event(UserEvent::Load { tab: id, started: matches!(ev, PageLoadEvent::Started), url });
            })
            .with_document_title_changed_handler(move |title| {
                let _ = title_proxy.send_event(UserEvent::Title { tab: id, title });
            })
            .with_new_window_req_handler(move |url, features| {
                // Real popups (OAuth, payment windows) keep their opener; plain target=_blank -> new tab.
                if features.size.is_some() {
                    NewWindowResponse::Allow
                } else {
                    let _ = nw_proxy.send_event(UserEvent::OpenTab { url });
                    NewWindowResponse::Deny
                }
            })
            .with_download_started_handler(move |url, path| {
                let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .filter(|n| !n.is_empty())
                    .or_else(|| {
                        url::Url::parse(&url).ok().and_then(|u| {
                            u.path_segments().and_then(|s| s.last().map(|x| x.to_string())).filter(|x| !x.is_empty())
                        })
                    })
                    .unwrap_or_else(|| "download".into());
                *path = unique_path(&dir, &name);
                let _ = dl_proxy.send_event(UserEvent::DownloadStarted { url, path: path.clone() });
                true
            })
            .with_download_completed_handler(move |url, path, ok| {
                let _ = dl2_proxy.send_event(UserEvent::DownloadDone { url, path, ok });
            });

        match builder.with_bounds(rect).build_as_child(&self.window) {
            Ok(wv) => {
                #[cfg(target_os = "linux")]
                if !incognito {
                    self.protocol_registered = true;
                }
                if (zoom - 1.0).abs() > f64::EPSILON {
                    let _ = wv.zoom(zoom);
                }
                self.tabs[idx].webview = Some(wv);
                self.tabs[idx].loading = true;
            }
            Err(e) => self.toast(&format!("Не удалось создать вкладку: {e}"), "error"),
        }
    }

    fn activate(&mut self, id: u32) {
        if self.tab(id).is_none() {
            return;
        }
        if self.split == Some(id) {
            // activating the right pane of a split just swaps focus
            self.split = Some(self.active);
        }
        self.active = id;
        self.ensure_webview(id);
        if let Some(s) = self.split {
            self.ensure_webview(s);
        }
        if let Some(t) = self.tab_mut(id) {
            t.last_active = Instant::now();
        }
        self.layout();
        if !self.overlay {
            if let Some(v) = self.active_view() {
                focus_view(v);
            }
        }
        self.update_title();
        self.push_state();
    }

    fn close_tab(&mut self, id: u32) {
        let Some(idx) = self.index_of(id) else { return };
        let tab = self.tabs.remove(idx);
        if !tab.incognito {
            self.closed.push(tab.url.clone());
        }
        drop(tab); // destroys the webview
        if self.split == Some(id) {
            self.split = None;
        }
        if self.tabs.is_empty() {
            let url = urls::internal_to_real("aurora://newtab").unwrap();
            let nid = self.add_tab(&url, false, false);
            self.activate(nid);
            return;
        }
        if self.active == id {
            let next = self.tabs[idx.min(self.tabs.len() - 1)].id;
            if self.split == Some(next) {
                self.split = None;
            }
            self.activate(next);
        } else {
            self.layout();
            self.push_state();
        }
    }

    fn cycle(&mut self, dir: i32) {
        if let Some(i) = self.index_of(self.active) {
            let n = self.tabs.len() as i32;
            let j = ((i as i32 + dir) % n + n) % n;
            let id = self.tabs[j as usize].id;
            self.activate(id);
        }
    }

    fn navigate(&mut self, input: &str, new_tab: bool, force_search: bool) {
        let engine = self.store.lock().unwrap().settings.search.clone();
        let url = if force_search { urls::search_url(&engine, input.trim()) } else { urls::resolve(input, &engine) };
        if new_tab || self.active_tab().is_none() {
            let incog = false;
            self.open_tab(&url, incog, true);
            return;
        }
        let id = self.active;
        if let Some(tab) = self.tab_mut(id) {
            tab.url = url.clone();
            tab.loading = true;
        }
        self.ensure_webview(id);
        if let Some(v) = self.active_view() {
            let _ = v.load_url(&url);
            focus_view(v);
        }
        self.push_state();
    }

    fn eval_active(&self, js: &str) {
        if let Some(v) = self.active_view() {
            let _ = v.evaluate_script(js);
        }
    }

    // ---------------------------------------------------------------- layout

    fn window_size(&self) -> LogicalSize<f64> {
        self.window.inner_size().to_logical::<f64>(self.window.scale_factor())
    }

    fn chrome_h(&self) -> f64 {
        if self.fullscreen { 0.0 } else { CHROME_H }
    }

    /// Rect of the content area; `right` = right half when split.
    fn content_rect(&self, right: bool) -> Rect {
        let size = self.window_size();
        let top = self.chrome_h();
        let h = (size.height - top).max(1.0);
        let (x, w) = match (self.split.is_some(), right) {
            (false, _) => (0.0, size.width),
            (true, false) => (0.0, ((size.width - SPLIT_GAP) / 2.0).floor()),
            (true, true) => {
                let lw = ((size.width - SPLIT_GAP) / 2.0).floor();
                (lw + SPLIT_GAP, size.width - lw - SPLIT_GAP)
            }
        };
        Rect { position: LogicalPosition::new(x, top).into(), size: LogicalSize::new(w.max(1.0), h).into() }
    }

    fn layout(&self) {
        let size = self.window_size();
        let ch = if self.overlay { size.height } else { self.chrome_h().max(1.0) };
        let _ = self.chrome.set_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(size.width, ch).into(),
        });
        let _ = self.chrome.set_visible(!self.fullscreen || self.overlay);
        for t in &self.tabs {
            let Some(v) = &t.webview else { continue };
            let shown = !self.overlay && (t.id == self.active || Some(t.id) == self.split);
            if shown {
                let right = Some(t.id) == self.split;
                let _ = v.set_bounds(self.content_rect(right));
            }
            let _ = v.set_visible(shown);
        }
    }

    // ------------------------------------------------------------ UI bridge

    fn tab_json(&self, t: &Tab, store: &Store) -> Value {
        let display = urls::display_url(&t.url);
        json!({
            "id": t.id,
            "title": if t.title.is_empty() { display.clone() } else { t.title.clone() },
            "url": display,
            "loading": t.loading,
            "favicon": t.favicon,
            "theme": t.theme,
            "incognito": t.incognito,
            "pinned": t.pinned,
            "zoom": t.zoom,
            "blocked": t.blocked,
            "dark": t.dark,
            "sleeping": t.webview.is_none(),
            "bookmarked": store.is_bookmarked(&t.url),
            "secure": t.url.starts_with("https://") || urls::is_internal_real(&t.url),
        })
    }

    fn push_state(&self) {
        if !self.chrome_ready {
            return;
        }
        let store = self.store.lock().unwrap();
        let tabs: Vec<Value> = self.tabs.iter().map(|t| self.tab_json(t, &store)).collect();
        let state = json!({
            "tabs": tabs,
            "active": self.active,
            "split": self.split,
            "maximized": self.window.is_maximized(),
            "fullscreen": self.fullscreen,
            "closedCount": self.closed.len(),
            "settings": store.settings,
            "platform": std::env::consts::OS,
        });
        drop(store);
        let _ = self.chrome.evaluate_script(&format!("window.Aurora&&Aurora.update({state})"));
    }

    fn push_suggest_data(&self) {
        let store = self.store.lock().unwrap();
        let history: Vec<Value> = store
            .history
            .iter()
            .take(600)
            .map(|h| json!({"title": h.title, "url": h.url, "visits": h.visits}))
            .collect();
        let bookmarks: Vec<Value> = store.bookmarks.iter().map(|b| json!({"title": b.title, "url": b.url})).collect();
        let data = json!({"history": history, "bookmarks": bookmarks});
        drop(store);
        let _ = self.chrome.evaluate_script(&format!("window.Aurora&&Aurora.data({data})"));
    }

    fn toast(&self, msg: &str, kind: &str) {
        let _ = self.chrome.evaluate_script(&format!("window.Aurora&&Aurora.toast({},{})", js_str(msg), js_str(kind)));
    }

    fn update_title(&self) {
        let title = self
            .active_tab()
            .map(|t| if t.title.is_empty() { urls::display_url(&t.url) } else { t.title.clone() })
            .unwrap_or_default();
        self.window.set_title(&format!("{title} — Aurora"));
    }

    fn set_overlay(&mut self, on: bool) {
        if self.overlay == on {
            return;
        }
        self.overlay = on;
        self.layout();
        if on {
            focus_view(&self.chrome);
            self.push_suggest_data();
        } else if let Some(v) = self.active_view() {
            focus_view(v);
        }
    }

    // ------------------------------------------------------------- events

    /// Returns true when the app should exit.
    fn handle(&mut self, ev: UserEvent) -> bool {
        if std::env::var_os("AURORA_DEBUG").is_some() {
            eprintln!("[event] {ev:?}");
        }
        match ev {
            UserEvent::Chrome(body) => {
                let Ok(msg) = serde_json::from_str::<Value>(&body) else { return false };
                return self.chrome_cmd(&msg);
            }
            UserEvent::Content { tab, internal, body } => {
                let Ok(msg) = serde_json::from_str::<Value>(&body) else { return false };
                self.content_msg(tab, internal, &msg);
            }
            UserEvent::Load { tab, started, url } => {
                let mut record = None;
                if let Some(t) = self.tab_mut(tab) {
                    t.loading = started;
                    if !url.is_empty() {
                        t.url = url.clone();
                    }
                    if started {
                        t.blocked = 0;
                        t.theme.clear();
                    } else {
                        if !t.incognito {
                            record = Some((t.url.clone(), t.title.clone()));
                        }
                        if t.dark {
                            if let Some(v) = &t.webview {
                                let _ = v.evaluate_script("window.__aurora&&__aurora.dark(true)");
                            }
                        }
                    }
                }
                if let Some((u, title)) = record {
                    self.store.lock().unwrap().record_visit(&u, &title);
                }
                if tab == self.active {
                    self.update_title();
                }
                self.push_state();
            }
            UserEvent::Title { tab, title } => {
                let mut rec = None;
                if let Some(t) = self.tab_mut(tab) {
                    t.title = title.clone();
                    if !t.incognito {
                        rec = Some(t.url.clone());
                    }
                }
                if let Some(u) = rec {
                    self.store.lock().unwrap().update_title(&u, &title);
                }
                if tab == self.active {
                    self.update_title();
                }
                self.push_state();
            }
            UserEvent::OpenTab { url } => {
                let incog = self.active_tab().map(|t| t.incognito).unwrap_or(false);
                self.open_tab(&url, incog, true);
            }
            UserEvent::DownloadStarted { url, path } => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                {
                    let mut s = self.store.lock().unwrap();
                    s.downloads.insert(0, Download { url, path: path.to_string_lossy().into(), ok: None, ts: store::now() });
                    s.downloads.truncate(200);
                    s.save_downloads();
                }
                self.toast(&format!("Загрузка: {name}"), "download");
            }
            UserEvent::DownloadDone { url, path, ok } => {
                let name = {
                    let mut s = self.store.lock().unwrap();
                    let mut name = String::new();
                    if let Some(d) = s.downloads.iter_mut().find(|d| d.url == url && d.ok.is_none()) {
                        d.ok = Some(ok);
                        if let Some(p) = &path {
                            d.path = p.to_string_lossy().into();
                        }
                        name = PathBuf::from(&d.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    }
                    s.save_downloads();
                    name
                };
                if ok {
                    self.toast(&format!("Готово: {name}"), "success");
                } else {
                    self.toast(&format!("Загрузка не удалась: {name}"), "error");
                }
            }
        }
        false
    }

    /// Messages coming from web content. Pages are untrusted: only a small whitelist is accepted,
    /// and privileged commands are accepted only from Aurora's own internal pages.
    fn content_msg(&mut self, tab: u32, internal: bool, msg: &Value) {
        let t = msg["t"].as_str().unwrap_or("");
        match t {
            "key" => {
                if tab == self.active || Some(tab) == self.split {
                    if Some(tab) == self.split {
                        // keyboard in the right pane makes it the active one
                        self.activate(tab);
                    }
                    let k = msg["k"].as_str().unwrap_or("");
                    let _ = self.chrome.evaluate_script(&format!("window.Aurora&&Aurora.shortcut({})", js_str(k)));
                }
            }
            "url" => {
                if let Some(u) = msg["url"].as_str() {
                    if let Some(tb) = self.tab_mut(tab) {
                        tb.url = u.to_string();
                    }
                    self.push_state();
                }
            }
            "meta" => {
                if let Some(tb) = self.tab_mut(tab) {
                    tb.favicon = msg["favicon"].as_str().unwrap_or("").chars().take(2048).collect();
                    tb.theme = msg["theme"].as_str().unwrap_or("").chars().take(64).collect();
                }
                self.push_state();
            }
            "blocked" => {
                if let Some(tb) = self.tab_mut(tab) {
                    tb.blocked = msg["n"].as_u64().unwrap_or(0) as u32;
                }
                self.push_state();
            }
            "find" => {
                let _ = self.chrome.evaluate_script(&format!("window.Aurora&&Aurora.findResult({msg})"));
            }
            "toast" => {
                if let Some(m) = msg["msg"].as_str() {
                    let m: String = m.chars().take(120).collect();
                    self.toast(&m, "info");
                }
            }
            // ---- privileged: internal pages only ----
            _ if !internal => {}
            "navigate" => {
                let url = msg["url"].as_str().unwrap_or("").to_string();
                let new_tab = msg["newTab"].as_bool().unwrap_or(false);
                if tab != self.active {
                    self.activate(tab);
                }
                self.navigate(&url, new_tab, false);
            }
            "setSetting" => {
                let key = msg["key"].as_str().unwrap_or("");
                let val = &msg["value"];
                {
                    let mut s = self.store.lock().unwrap();
                    let mut v = serde_json::to_value(&s.settings).unwrap_or_default();
                    if v.get(key).is_some() {
                        v[key] = val.clone();
                        if let Ok(ns) = serde_json::from_value(v) {
                            s.settings = ns;
                            s.save_settings();
                        }
                    }
                }
                self.push_state();
            }
            "removeBookmark" => {
                let url = msg["url"].as_str().unwrap_or("");
                let mut s = self.store.lock().unwrap();
                s.bookmarks.retain(|b| b.url != url);
                s.save_bookmarks();
            }
            "removeHistory" => {
                let url = msg["url"].as_str().unwrap_or("");
                let mut s = self.store.lock().unwrap();
                s.history.retain(|h| h.url != url);
                s.history_dirty = true;
            }
            "clearHistory" => {
                let mut s = self.store.lock().unwrap();
                s.history.clear();
                s.history_dirty = true;
                drop(s);
                self.toast("История очищена", "success");
            }
            "clearDownloads" => {
                let mut s = self.store.lock().unwrap();
                s.downloads.retain(|d| d.ok.is_none());
                s.save_downloads();
            }
            "clearBrowsingData" => {
                if let Some(v) = self.active_view() {
                    let _ = v.clear_all_browsing_data();
                }
                self.toast("Cookies и кэш очищены", "success");
            }
            "openPath" => {
                if let Some(p) = msg["path"].as_str() {
                    open_in_os(p);
                }
            }
            _ => {}
        }
    }

    fn chrome_cmd(&mut self, msg: &Value) -> bool {
        let cmd = msg["cmd"].as_str().unwrap_or("");
        let id = msg["id"].as_u64().map(|x| x as u32).unwrap_or(self.active);
        match cmd {
            "ready" => {
                eprintln!("[chrome] ready");
                self.chrome_ready = true;
                self.push_state();
            }
            "navigate" => {
                let url = msg["url"].as_str().unwrap_or("").to_string();
                let new_tab = msg["newTab"].as_bool().unwrap_or(false);
                let search = msg["search"].as_bool().unwrap_or(false);
                self.set_overlay(false);
                self.navigate(&url, new_tab, search);
            }
            "newTab" => {
                let url = msg["url"].as_str().map(|s| s.to_string()).unwrap_or_else(|| urls::internal_to_real("aurora://newtab").unwrap());
                let incognito = msg["incognito"].as_bool().unwrap_or(false);
                self.open_tab(&url, incognito, true);
            }
            "openInternal" => {
                let page = msg["page"].as_str().unwrap_or("newtab");
                if let Some(url) = urls::internal_to_real(&format!("aurora://{page}")) {
                    // reuse an existing tab with that page
                    if let Some(t) = self.tabs.iter().find(|t| urls::display_url(&t.url) == format!("aurora://{page}")) {
                        let tid = t.id;
                        self.activate(tid);
                        if let Some(v) = self.active_view() {
                            let _ = v.reload();
                        }
                    } else {
                        self.open_tab(&url, false, true);
                    }
                }
            }
            "closeTab" => self.close_tab(id),
            "closeOthers" => {
                let keep: Vec<u32> = self.tabs.iter().filter(|t| t.id != id && !t.pinned).map(|t| t.id).collect();
                for k in keep {
                    self.close_tab(k);
                }
                self.activate(id);
            }
            "activate" => self.activate(id),
            "activateIndex" => {
                let i = msg["index"].as_i64().unwrap_or(0);
                let idx = if i < 0 { self.tabs.len().saturating_sub(1) } else { (i as usize).min(self.tabs.len().saturating_sub(1)) };
                if let Some(t) = self.tabs.get(idx) {
                    let tid = t.id;
                    self.activate(tid);
                }
            }
            "cycle" => self.cycle(msg["dir"].as_i64().unwrap_or(1) as i32),
            "moveTab" => {
                if let Some(from) = self.index_of(id) {
                    let to = (msg["to"].as_u64().unwrap_or(0) as usize).min(self.tabs.len() - 1);
                    let t = self.tabs.remove(from);
                    self.tabs.insert(to, t);
                    self.push_state();
                }
            }
            "pin" => {
                if let Some(idx) = self.index_of(id) {
                    let pinned = !self.tabs[idx].pinned;
                    let mut t = self.tabs.remove(idx);
                    t.pinned = pinned;
                    let pos = self.tabs.iter().take_while(|x| x.pinned).count();
                    self.tabs.insert(pos, t);
                    self.push_state();
                }
            }
            "duplicate" => {
                if let Some(t) = self.tab(id) {
                    let (u, inc) = (t.url.clone(), t.incognito);
                    self.open_tab(&u, inc, true);
                }
            }
            "reopenClosed" => {
                if let Some(u) = self.closed.pop() {
                    self.open_tab(&u, false, true);
                }
            }
            "back" => self.eval_active("history.back()"),
            "forward" => self.eval_active("history.forward()"),
            "reload" => {
                if let Some(v) = self.active_view() {
                    let _ = v.reload();
                }
            }
            "hardReload" => self.eval_active("location.reload()"),
            "stop" => self.eval_active("window.stop()"),
            "home" => self.navigate("aurora://newtab", false, false),
            "log" => eprintln!("[chrome] {}", msg["msg"].as_str().unwrap_or("")),
            "focusChrome" => {
                focus_view(&self.chrome);
                let _ = self.chrome.evaluate_script("window.Aurora&&Aurora.refocus()");
            }
            "overlay" => self.set_overlay(msg["on"].as_bool().unwrap_or(false)),
            "suggestData" => self.push_suggest_data(),
            "bookmark" => {
                if let Some(t) = self.tab(id) {
                    let (u, title) = (t.url.clone(), t.title.clone());
                    let on = self.store.lock().unwrap().toggle_bookmark(&u, &title);
                    self.toast(if on { "Добавлено в закладки" } else { "Удалено из закладок" }, if on { "bookmark" } else { "info" });
                    self.push_state();
                }
            }
            "zoom" => {
                let delta = msg["delta"].as_f64().unwrap_or(0.0);
                let id = self.active;
                if let Some(t) = self.tab_mut(id) {
                    t.zoom = if delta == 0.0 { 1.0 } else { (t.zoom + delta).clamp(0.25, 5.0) };
                    t.zoom = (t.zoom * 100.0).round() / 100.0;
                    let z = t.zoom;
                    if let Some(v) = &t.webview {
                        let _ = v.zoom(z);
                    }
                }
                self.push_state();
            }
            "find" => {
                let q = msg["q"].as_str().unwrap_or("");
                let back = msg["back"].as_bool().unwrap_or(false);
                self.eval_active(&format!("window.__aurora&&__aurora.find({},{})", js_str(q), back));
            }
            "findClear" => {
                self.eval_active("window.__aurora&&__aurora.clearFind()");
                if let Some(v) = self.active_view() {
                    focus_view(v);
                }
            }
            "dark" => {
                let id = self.active;
                if let Some(t) = self.tab_mut(id) {
                    t.dark = !t.dark;
                    let on = t.dark;
                    if let Some(v) = &t.webview {
                        let _ = v.evaluate_script(&format!("window.__aurora&&__aurora.dark({on})"));
                    }
                }
                self.push_state();
            }
            "reader" => self.eval_active("window.__aurora&&__aurora.reader()"),
            "devtools" => {
                if let Some(v) = self.active_view() {
                    v.open_devtools();
                }
            }
            "print" => {
                if let Some(v) = self.active_view() {
                    let _ = v.print();
                }
            }
            "split" => {
                if self.split.is_some() {
                    self.split = None;
                } else {
                    let other = msg["with"].as_u64().map(|x| x as u32).or_else(|| {
                        let i = self.index_of(self.active)?;
                        self.tabs.get(i + 1).or_else(|| if i > 0 { self.tabs.get(i - 1) } else { None }).map(|t| t.id)
                    });
                    match other {
                        Some(o) if o != self.active => {
                            self.split = Some(o);
                            self.ensure_webview(o);
                        }
                        _ => {
                            let url = urls::internal_to_real("aurora://newtab").unwrap();
                            let o = self.add_tab(&url, false, true);
                            self.split = Some(o);
                            self.ensure_webview(o);
                        }
                    }
                }
                self.layout();
                self.push_state();
            }
            "swapSplit" => {
                if let Some(s) = self.split {
                    self.split = Some(self.active);
                    self.active = s;
                    self.layout();
                    self.push_state();
                }
            }
            "sleepTab" => {
                if id != self.active && Some(id) != self.split {
                    if let Some(t) = self.tab_mut(id) {
                        t.webview = None;
                    }
                    self.push_state();
                }
            }
            "fullscreen" => {
                self.fullscreen = !self.fullscreen;
                self.window.set_fullscreen(if self.fullscreen { Some(Fullscreen::Borderless(None)) } else { None });
                self.layout();
                self.push_state();
            }
            "setSetting" => {
                // the chrome is trusted
                let key = msg["key"].as_str().unwrap_or("").to_string();
                let val = msg["value"].clone();
                let mut s = self.store.lock().unwrap();
                let mut v = serde_json::to_value(&s.settings).unwrap_or_default();
                if v.get(&key).is_some() {
                    v[&key] = val;
                    if let Ok(ns) = serde_json::from_value(v) {
                        s.settings = ns;
                        s.save_settings();
                    }
                }
                drop(s);
                self.push_state();
            }
            "copyUrl" => {
                if let Some(t) = self.active_tab() {
                    let u = urls::display_url(&t.url);
                    let _ = self.chrome.evaluate_script(&format!("navigator.clipboard&&navigator.clipboard.writeText({})", js_str(&u)));
                    self.toast("Ссылка скопирована", "success");
                }
            }
            "winMin" => self.window.set_minimized(true),
            "winMax" => {
                self.window.set_maximized(!self.window.is_maximized());
                self.push_state();
            }
            "drag" => {
                let _ = self.window.drag_window();
            }
            "winClose" | "quit" => return true,
            _ => {}
        }
        false
    }

    // -------------------------------------------------------- housekeeping

    /// Runs every ~30s: puts idle background tabs to sleep and flushes history.
    fn housekeeping(&mut self) {
        let minutes = self.store.lock().unwrap().settings.sleep_minutes;
        if minutes > 0 {
            let limit = Duration::from_secs(minutes * 60);
            let (active, split) = (self.active, self.split);
            let mut changed = false;
            for t in &mut self.tabs {
                if t.id != active && Some(t.id) != split && t.webview.is_some() && !t.pinned && t.last_active.elapsed() > limit {
                    t.webview = None; // frees the renderer process memory
                    changed = true;
                }
            }
            if changed {
                self.push_state();
            }
        }
        // crash-safety: persist the session regularly, not only on a clean exit
        self.save_session();
    }

    fn shutdown(&mut self) {
        self.save_session();
    }

    fn save_session(&mut self) {
        let mut s = self.store.lock().unwrap();
        let tabs: Vec<&Tab> = self.tabs.iter().filter(|t| !t.incognito).collect();
        s.session.active = tabs.iter().position(|t| t.id == self.active).unwrap_or(0);
        s.session.tabs = tabs
            .iter()
            .map(|t| SessionTab { url: t.url.clone(), title: t.title.clone(), pinned: t.pinned })
            .collect();
        s.save_session();
        s.flush_history();
    }
}

/// Give keyboard focus to a webview. On X11 every child webview is its own native window,
/// so the X input focus has to be moved too, not only the GTK focus.
fn focus_view(v: &WebView) {
    #[cfg(target_os = "linux")]
    let _ = v.focus_parent();
    let _ = v.focus();
}

fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let name: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let p = dir.join(&name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };
    (1..1000)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(p)
}

fn open_in_os(path: &str) {
    let p = PathBuf::from(path);
    if !p.exists() {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p.parent().unwrap_or(&p)).spawn();
}
