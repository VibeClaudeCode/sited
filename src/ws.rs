//! WebSocket: рассылка событий в реальном времени, статусы «в сети» и «печатает…».

use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::{api::AppError, db, AppState, Event};

#[derive(Deserialize)]
pub struct WsQuery {
    token: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Typing { channel_id: i64 },
    Ping,
}

pub async fn handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(st): State<AppState>,
) -> Result<Response, AppError> {
    let user = {
        let db = st.db.lock().unwrap();
        db::user_by_token(&db, &q.token)?
    }
    .ok_or(AppError::Unauthorized)?;
    Ok(ws.on_upgrade(move |socket| session(socket, st, user)))
}

/// Должен ли пользователь `uid` получить событие.
fn allowed(st: &AppState, uid: i64, ev: &Event) -> bool {
    let db = st.db.lock().unwrap();
    match (ev.server_id, ev.about_user) {
        (Some(sid), _) => db::is_member(&db, sid, uid).unwrap_or(false),
        (None, Some(other)) => other == uid || db::shares_server(&db, uid, other).unwrap_or(false),
        (None, None) => false,
    }
}

fn set_presence(st: &AppState, uid: i64, online: bool) {
    st.emit(None, Some(uid), json!({ "type": "presence", "user_id": uid, "online": online }));
}

async fn session(socket: WebSocket, st: AppState, user: db::User) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = st.tx.subscribe();
    let uid = user.id;

    let first = {
        let mut online = st.online.lock().unwrap();
        let n = online.entry(uid).or_insert(0);
        *n += 1;
        *n == 1
    };
    if first {
        set_presence(&st, uid, true);
    }

    let out_state = st.clone();
    let mut send_task = tokio::spawn(async move {
        if sink.send(WsMessage::Text(json!({ "type": "ready" }).to_string())).await.is_err() {
            return;
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if allowed(&out_state, uid, &ev)
                        && sink.send(WsMessage::Text(ev.json.to_string())).await.is_err()
                    {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });

    let in_state = st.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            let text = match msg {
                WsMessage::Text(t) => t,
                WsMessage::Close(_) => break,
                _ => continue,
            };
            match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Typing { channel_id }) => {
                    let server_id = {
                        let db = in_state.db.lock().unwrap();
                        db::channel(&db, channel_id).ok().flatten().and_then(|ch| {
                            db::is_member(&db, ch.server_id, uid)
                                .unwrap_or(false)
                                .then_some(ch.server_id)
                        })
                    };
                    if let Some(sid) = server_id {
                        in_state.emit(
                            Some(sid),
                            None,
                            json!({ "type": "typing", "channel_id": channel_id, "user": user }),
                        );
                    }
                }
                Ok(ClientMessage::Ping) | Err(_) => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    let last = {
        let mut online = st.online.lock().unwrap();
        match online.get_mut(&uid) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            _ => {
                online.remove(&uid);
                true
            }
        }
    };
    if last {
        set_presence(&st, uid, false);
    }
}
