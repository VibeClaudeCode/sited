//! REST API: авторизация, серверы, каналы, сообщения.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    async_trait,
    extract::{FromRequestParts, Path, Query, State},
    http::{header::AUTHORIZATION, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    db::{self, Channel, Message, Server, User},
    AppState,
};

const USER_COLORS: [&str; 8] =
    ["#f87171", "#fb923c", "#facc15", "#4ade80", "#2dd4bf", "#60a5fa", "#a78bfa", "#f472b6"];
const PAGE: i64 = 50;
const MAX_MESSAGE: usize = 2000;

// ---------- ошибки ----------

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Нужно войти в аккаунт".into()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Недостаточно прав".into()),
            AppError::NotFound => (StatusCode::NOT_FOUND, "Не найдено".into()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Internal(m) => {
                tracing::error!("внутренняя ошибка: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Внутренняя ошибка сервера".into())
            }
        };
        (code, Json(json!({ "error": msg }))).into_response()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

type ApiResult<T> = Result<T, AppError>;

fn bad(msg: &str) -> AppError {
    AppError::BadRequest(msg.into())
}

// ---------- авторизация ----------

/// Текущий пользователь из заголовка `Authorization: Bearer <token>`.
pub struct Auth(pub User, pub String);

#[async_trait]
impl FromRequestParts<AppState> for Auth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> ApiResult<Self> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?
            .to_string();
        let db = st.db.lock().unwrap();
        let user = db::user_by_token(&db, &token)?.ok_or(AppError::Unauthorized)?;
        Ok(Auth(user, token))
    }
}

fn random_token() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_invite() -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).take(8).map(char::from).collect()
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AuthOut {
    token: String,
    user: User,
}

fn valid_username(name: &str) -> bool {
    let len = name.chars().count();
    (2..=32).contains(&len) && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
}

async fn register(State(st): State<AppState>, Json(b): Json<Credentials>) -> ApiResult<Json<AuthOut>> {
    let username = b.username.trim().to_string();
    if !valid_username(&username) {
        return Err(bad("Имя: 2–32 символа, буквы, цифры, «_» и «.»"));
    }
    if b.password.chars().count() < 6 {
        return Err(bad("Пароль должен быть не короче 6 символов"));
    }
    let password = b.password;
    let hash = tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string())
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let color = USER_COLORS[rand::thread_rng().gen_range(0..USER_COLORS.len())];
    let token = random_token();
    let db = st.db.lock().unwrap();
    if db::username_taken(&db, &username)? {
        return Err(AppError::Conflict("Это имя уже занято".into()));
    }
    let user = db::create_user(&db, &username, &hash, color)?;
    db::create_session(&db, &token, user.id)?;
    Ok(Json(AuthOut { token, user }))
}

async fn login(State(st): State<AppState>, Json(b): Json<Credentials>) -> ApiResult<Json<AuthOut>> {
    let found = {
        let db = st.db.lock().unwrap();
        db::user_with_hash(&db, b.username.trim())?
    };
    let wrong = || bad("Неверное имя пользователя или пароль");
    let (user, hash) = found.ok_or_else(wrong)?;
    let password = b.password;
    let ok = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|p| Argon2::default().verify_password(password.as_bytes(), &p).is_ok())
            .unwrap_or(false)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    if !ok {
        return Err(wrong());
    }
    let token = random_token();
    db::create_session(&st.db.lock().unwrap(), &token, user.id)?;
    Ok(Json(AuthOut { token, user }))
}

async fn logout(State(st): State<AppState>, Auth(_, token): Auth) -> ApiResult<StatusCode> {
    db::delete_session(&st.db.lock().unwrap(), &token)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(Auth(user, _): Auth) -> Json<User> {
    Json(user)
}

// ---------- серверы ----------

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

#[derive(Deserialize)]
struct InviteBody {
    invite: String,
}

#[derive(Serialize)]
struct Member {
    #[serde(flatten)]
    user: User,
    online: bool,
    owner: bool,
}

async fn list_servers(State(st): State<AppState>, Auth(me, _): Auth) -> ApiResult<Json<Vec<Server>>> {
    Ok(Json(db::servers_for_user(&st.db.lock().unwrap(), me.id)?))
}

async fn create_server(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Json(b): Json<NameBody>,
) -> ApiResult<Json<Server>> {
    let name = b.name.trim();
    if name.is_empty() || name.chars().count() > 50 {
        return Err(bad("Название сервера: от 1 до 50 символов"));
    }
    let mut db = st.db.lock().unwrap();
    Ok(Json(db::create_server(&mut db, name, me.id, &random_invite())?))
}

async fn join_server(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Json(b): Json<InviteBody>,
) -> ApiResult<Json<Server>> {
    // Принимаем и сам код, и ссылку вида .../invite/CODE
    let code = b.invite.trim().rsplit('/').next().unwrap_or("").to_string();
    let (server, added) = {
        let db = st.db.lock().unwrap();
        let server = db::server_by_invite(&db, &code)?
            .ok_or_else(|| bad("Приглашение недействительно"))?;
        let added = db::add_member(&db, server.id, me.id)?;
        (server, added)
    };
    if added {
        let member = Member { online: st.is_online(me.id), owner: false, user: me };
        st.emit(
            Some(server.id),
            None,
            json!({ "type": "member_join", "server_id": server.id, "member": member }),
        );
    }
    Ok(Json(server))
}

fn require_member(db: &rusqlite::Connection, server_id: i64, user_id: i64) -> ApiResult<Server> {
    let server = db::server(db, server_id)?.ok_or(AppError::NotFound)?;
    if !db::is_member(db, server_id, user_id)? {
        return Err(AppError::NotFound);
    }
    Ok(server)
}

async fn list_members(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(sid): Path<i64>,
) -> ApiResult<Json<Vec<Member>>> {
    let (server, users) = {
        let db = st.db.lock().unwrap();
        let server = require_member(&db, sid, me.id)?;
        (server, db::members(&db, sid)?)
    };
    let online = st.online.lock().unwrap();
    Ok(Json(
        users
            .into_iter()
            .map(|u| Member { online: online.contains_key(&u.id), owner: u.id == server.owner_id, user: u })
            .collect(),
    ))
}

// ---------- каналы ----------

#[derive(Deserialize)]
struct ChannelBody {
    name: String,
    #[serde(default)]
    topic: String,
}

/// «Мой Канал!» -> «мой-канал»
fn normalize_channel(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_alphanumeric() || c == '_' {
            out.push(c);
        } else if (c.is_whitespace() || c == '-') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(32).collect()
}

async fn list_channels(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(sid): Path<i64>,
) -> ApiResult<Json<Vec<Channel>>> {
    let db = st.db.lock().unwrap();
    require_member(&db, sid, me.id)?;
    Ok(Json(db::channels(&db, sid)?))
}

async fn create_channel(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(sid): Path<i64>,
    Json(b): Json<ChannelBody>,
) -> ApiResult<Json<Channel>> {
    let name = normalize_channel(&b.name);
    if name.is_empty() {
        return Err(bad("Введите название канала"));
    }
    let topic: String = b.topic.trim().chars().take(120).collect();
    let channel = {
        let db = st.db.lock().unwrap();
        let server = require_member(&db, sid, me.id)?;
        if server.owner_id != me.id {
            return Err(AppError::Forbidden);
        }
        db::create_channel(&db, sid, &name, &topic)?
    };
    st.emit(Some(sid), None, json!({ "type": "channel_create", "channel": channel }));
    Ok(Json(channel))
}

// ---------- сообщения ----------

#[derive(Deserialize)]
struct Page {
    before: Option<i64>,
}

#[derive(Deserialize)]
struct ContentBody {
    content: String,
}

fn clean_content(raw: &str) -> ApiResult<String> {
    let content = raw.trim();
    if content.is_empty() {
        return Err(bad("Сообщение пустое"));
    }
    if content.chars().count() > MAX_MESSAGE {
        return Err(bad("Сообщение длиннее 2000 символов"));
    }
    Ok(content.to_string())
}

/// Канал, в котором пользователь состоит, и его сервер.
fn member_channel(db: &rusqlite::Connection, cid: i64, uid: i64) -> ApiResult<(Channel, Server)> {
    let channel = db::channel(db, cid)?.ok_or(AppError::NotFound)?;
    let server = require_member(db, channel.server_id, uid)?;
    Ok((channel, server))
}

async fn list_messages(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(cid): Path<i64>,
    Query(p): Query<Page>,
) -> ApiResult<Json<Vec<Message>>> {
    let db = st.db.lock().unwrap();
    member_channel(&db, cid, me.id)?;
    Ok(Json(db::messages(&db, cid, p.before, PAGE)?))
}

async fn send_message(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(cid): Path<i64>,
    Json(b): Json<ContentBody>,
) -> ApiResult<Json<Message>> {
    let content = clean_content(&b.content)?;
    let (msg, sid) = {
        let db = st.db.lock().unwrap();
        let (ch, _) = member_channel(&db, cid, me.id)?;
        (db::create_message(&db, cid, me.id, &content)?, ch.server_id)
    };
    st.emit(Some(sid), None, json!({ "type": "message_create", "server_id": sid, "message": msg }));
    Ok(Json(msg))
}

async fn edit_message(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(mid): Path<i64>,
    Json(b): Json<ContentBody>,
) -> ApiResult<Json<Message>> {
    let content = clean_content(&b.content)?;
    let (msg, sid) = {
        let db = st.db.lock().unwrap();
        let msg = db::message(&db, mid)?.ok_or(AppError::NotFound)?;
        let (ch, _) = member_channel(&db, msg.channel_id, me.id)?;
        if msg.author.id != me.id {
            return Err(AppError::Forbidden);
        }
        db::update_message(&db, mid, &content)?;
        (db::message(&db, mid)?.ok_or(AppError::NotFound)?, ch.server_id)
    };
    st.emit(Some(sid), None, json!({ "type": "message_update", "message": msg }));
    Ok(Json(msg))
}

async fn delete_message(
    State(st): State<AppState>,
    Auth(me, _): Auth,
    Path(mid): Path<i64>,
) -> ApiResult<StatusCode> {
    let (cid, sid) = {
        let db = st.db.lock().unwrap();
        let msg = db::message(&db, mid)?.ok_or(AppError::NotFound)?;
        let (ch, server) = member_channel(&db, msg.channel_id, me.id)?;
        if msg.author.id != me.id && server.owner_id != me.id {
            return Err(AppError::Forbidden);
        }
        db::delete_message(&db, mid)?;
        (ch.id, ch.server_id)
    };
    st.emit(
        Some(sid),
        None,
        json!({ "type": "message_delete", "channel_id": cid, "message_id": mid }),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/servers", get(list_servers).post(create_server))
        .route("/servers/join", post(join_server))
        .route("/servers/:id/channels", get(list_channels).post(create_channel))
        .route("/servers/:id/members", get(list_members))
        .route("/channels/:id/messages", get(list_messages).post(send_message))
        .route("/messages/:id", patch(edit_message).delete(delete_message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_normalized() {
        assert_eq!(normalize_channel("  Мой Канал!  "), "мой-канал");
        assert_eq!(normalize_channel("dev -- chat"), "dev-chat");
        assert_eq!(normalize_channel("!!!"), "");
    }

    #[test]
    fn usernames_are_validated() {
        assert!(valid_username("anna_99"));
        assert!(valid_username("Аня"));
        assert!(!valid_username("a"));
        assert!(!valid_username("bad name"));
    }
}
