//! Sited — минималистичный мессенджер с серверами, каналами и чатом в реальном времени.

mod api;
mod db;
mod ws;

use std::{
    collections::HashMap,
    env,
    sync::{Arc, Mutex},
};

use axum::{routing::get, Router};
use rusqlite::Connection;
use tokio::sync::broadcast;
use tower_http::services::ServeDir;

/// Событие для рассылки по WebSocket.
#[derive(Clone)]
pub struct Event {
    /// Получают только участники этого сервера.
    pub server_id: Option<i64>,
    /// Если server_id нет: получают те, у кого есть общий сервер с этим пользователем.
    pub about_user: Option<i64>,
    pub json: Arc<str>,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub tx: broadcast::Sender<Event>,
    /// user_id -> число открытых соединений.
    pub online: Arc<Mutex<HashMap<i64, usize>>>,
}

impl AppState {
    pub fn emit(&self, server_id: Option<i64>, about_user: Option<i64>, value: serde_json::Value) {
        let _ = self.tx.send(Event { server_id, about_user, json: value.to_string().into() });
    }

    pub fn is_online(&self, user_id: i64) -> bool {
        self.online.lock().unwrap().contains_key(&user_id)
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(false).init();

    let db_path = env::var("SITED_DB").unwrap_or_else(|_| "sited.db".into());
    let static_dir =
        env::var("SITED_STATIC").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/static").into());
    let addr = env::var("SITED_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());

    let conn = db::open(&db_path).expect("не удалось открыть базу данных");
    let (tx, _) = broadcast::channel(1024);
    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        tx,
        online: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .nest("/api", api::router())
        .route("/ws", get(ws::handler))
        .fallback_service(ServeDir::new(static_dir))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("не удалось занять адрес");
    tracing::info!("Sited запущен: http://{addr}");
    axum::serve(listener, app).await.expect("ошибка сервера");
}

