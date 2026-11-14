'use strict';

// ---------- утилиты ----------
const $ = (sel, el = document) => el.querySelector(sel);
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* приватный режим */ } },
};
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const escRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

const ICONS = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  hash: '<path d="M5 9h14M5 15h14M10 3 8 21M16 3l-2 18"/>',
  users: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5"/><path d="M16 4.6a3.5 3.5 0 0 1 0 6.8M18 14.8c1.9.7 3.1 2.4 3.5 5.2"/>',
  logout: '<path d="M14 4h5v16h-5M10 8l-4 4 4 4M6 12h11"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
  send: '<path d="M5 12h13M12 5l7 7-7 7"/>',
  edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13 7 4 4"/>',
  trash: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>',
  menu: '<path d="M4 7h16M4 12h16M4 17h10"/>',
  compass: '<circle cx="12" cy="12" r="9"/><path d="m15.5 8.5-2 5-5 2 2-5z"/>',
};
const icon = (name, size = 20) =>
  `<svg class="ic" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">${ICONS[name]}</svg>`;

const SERVER_COLORS = ['#5eead4', '#93c5fd', '#c4b5fd', '#f9a8d4', '#fdba74', '#fde047', '#86efac', '#fca5a5'];
const serverColor = (id) => SERVER_COLORS[id % SERVER_COLORS.length];
const initials = (name) => name.trim().split(/\s+/).slice(0, 2).map((w) => [...w][0] || '').join('').toUpperCase() || '?';
const firstChar = (s) => ([...s][0] || '?').toUpperCase();

const avatar = (u, size, status = false) =>
  `<div class="av" style="--c:${esc(u.color)};width:${size}px;height:${size}px">${esc(firstChar(u.username))}${
    status ? `<i class="dot ${u.online ? 'on' : ''}"></i>` : ''}</div>`;

const timeFmt = new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' });
const dateFmt = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', year: 'numeric' });
const sameDay = (a, b) => a.toDateString() === b.toDateString();
function fmtDay(ts) {
  const d = new Date(ts), now = new Date(), y = new Date(now - 864e5);
  if (sameDay(d, now)) return 'Сегодня';
  if (sameDay(d, y)) return 'Вчера';
  return dateFmt.format(d);
}
const fmtTime = (ts) => timeFmt.format(new Date(ts));
const fmtFull = (ts) => `${fmtDay(ts)} в ${fmtTime(ts)}`;

/** Простое форматирование: ```блок```, `код`, **жирный**, *курсив*, ~~зачёркнутый~~, ссылки, @упоминания. */
function format(text) {
  const keep = [];
  const hold = (html) => `\u0000${keep.push(html) - 1}\u0000`;
  let s = esc(text);
  s = s.replace(/```(?:\w+\n)?([\s\S]+?)```/g, (_, c) => hold(`<pre><code>${c.replace(/^\n|\n$/g, '')}</code></pre>`));
  s = s.replace(/`([^`\n]+)`/g, (_, c) => hold(`<code>${c}</code>`));
  s = s.replace(/\bhttps?:\/\/[^\s<]+[^\s<.,;:!?)\]'"]/g, (u) => hold(`<a href="${u}" target="_blank" rel="noopener noreferrer">${u}</a>`));
  s = s.replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<i>$2</i>')
    .replace(/~~([^~\n]+)~~/g, '<s>$1</s>')
    .replace(/(^|\s)@([\p{L}\p{N}_.]{2,32})/gu, '$1<span class="ment">@$2</span>');
  return s.replace(/\u0000(\d+)\u0000/g, (_, i) => keep[+i]);
}

let toastTimer;
function toast(msg) {
  const t = $('#toast');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), 2600);
}

function autosize(el) {
  el.style.height = 'auto';
  el.style.height = Math.min(el.scrollHeight, window.innerHeight * 0.4) + 'px';
}

// ---------- состояние ----------
const S = {
  token: store.get('token'),
  me: null,
  servers: [],
  channels: {},      // server_id -> [channel]
  members: {},       // server_id -> [member]
  messages: {},      // channel_id -> [message]
  hasMore: {},       // channel_id -> bool
  unread: new Map(), // channel_id -> server_id
  sid: null,
  cid: null,
  lastChannel: (() => { try { return JSON.parse(store.get('lastChannel') || '{}'); } catch { return {}; } })(),
  typing: {},        // channel_id -> { user_id: { user, until } }
  ws: null,
  wsDelay: 1000,
  wasConnected: false,
  editing: null,
  loadingOlder: false,
  lastTypingSent: 0,
};
const curServer = () => S.servers.find((s) => s.id === S.sid);
const curChannel = () => (S.channels[S.sid] || []).find((c) => c.id === S.cid);
const isWide = () => window.matchMedia('(min-width: 1101px)').matches;

// ---------- API ----------
async function api(method, path, body) {
  const headers = { 'Content-Type': 'application/json' };
  if (S.token) headers.Authorization = 'Bearer ' + S.token;
  let res;
  try {
    res = await fetch('/api' + path, { method, headers, body: body ? JSON.stringify(body) : undefined });
  } catch {
    throw new Error('Нет связи с сервером');
  }
  const data = res.status === 204 ? null : await res.json().catch(() => null);
  if (!res.ok) {
    if (res.status === 401 && S.me) logout(false);
    throw new Error((data && data.error) || 'Что-то пошло не так');
  }
  return data;
}

// ---------- вход ----------
function showAuth(mode = 'login') {
  $('#app').classList.add('hidden');
  const root = $('#auth');
  root.classList.remove('hidden');
  const isLogin = mode === 'login';
  root.innerHTML = `
    <div class="auth-card">
      <div class="brand">
        <img class="logo" src="/favicon.svg" alt="">
        <div><h1>Sited</h1><p>${isLogin ? 'С возвращением!' : 'Создайте аккаунт за пару секунд'}</p></div>
      </div>
      <form id="auth-form">
        <label class="field"><span>Имя пользователя</span>
          <input name="username" autocomplete="username" required minlength="2" maxlength="32"></label>
        <label class="field"><span>Пароль</span>
          <input name="password" type="password" autocomplete="${isLogin ? 'current-password' : 'new-password'}" required minlength="6"></label>
        <div class="error" id="auth-error"></div>
        <button class="btn primary wide" type="submit">${isLogin ? 'Войти' : 'Зарегистрироваться'}</button>
      </form>
      <p class="switch">${isLogin
        ? 'Нет аккаунта? <a href="#" data-mode="register">Зарегистрироваться</a>'
        : 'Уже есть аккаунт? <a href="#" data-mode="login">Войти</a>'}</p>
    </div>`;
  $('[data-mode]', root).onclick = (e) => { e.preventDefault(); showAuth(e.target.dataset.mode); };
  $('#auth-form').onsubmit = async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    const btn = $('button[type=submit]', e.target);
    btn.disabled = true;
    try {
      const r = await api('POST', '/' + mode, { username: f.get('username'), password: f.get('password') });
      S.token = r.token;
      S.me = r.user;
      store.set('token', r.token);
      await start();
    } catch (err) {
      $('#auth-error').textContent = err.message;
      btn.disabled = false;
    }
  };
  $('input', root).focus();
}

async function logout(callServer = true) {
  if (callServer) await api('POST', '/logout').catch(() => {});
  store.set('token', null);
  const ws = S.ws;
  Object.assign(S, { token: null, me: null, ws: null, servers: [], channels: {}, members: {}, messages: {}, sid: null, cid: null });
  S.unread.clear();
  if (ws) ws.close();
  showAuth();
}

// ---------- запуск ----------
async function start() {
  $('#auth').classList.add('hidden');
  $('#app').classList.remove('hidden');
  if (store.get('hideMembers') === '1') $('#app').classList.add('no-members');
  renderMe();
  S.servers = await api('GET', '/servers');
  connect();
  renderRail();
  const last = Number(store.get('lastServer'));
  const target = (S.servers.find((s) => s.id === last) || S.servers[0]);
  if (target) await selectServer(target.id);
  else renderEmpty();
}

// ---------- отрисовка ----------
function renderRail() {
  const unreadServers = new Set(S.unread.values());
  $('#rail').innerHTML =
    `<img class="rail-logo" src="/favicon.svg" alt="Sited" title="Sited"><div class="rail-sep"></div>` +
    S.servers.map((s) => `<button class="srv ${s.id === S.sid ? 'active' : ''} ${unreadServers.has(s.id) && s.id !== S.sid ? 'unread' : ''}"
        data-sid="${s.id}" title="${esc(s.name)}" style="--c:${serverColor(s.id)}">${esc(initials(s.name))}</button>`).join('') +
    `<button class="srv add" data-action="create-server" title="Создать сервер">${icon('plus', 22)}</button>
     <button class="srv add" data-action="join-server" title="Войти по приглашению">${icon('compass', 22)}</button>`;
}

function renderMe() {
  $('#me').innerHTML = `${avatar({ ...S.me, online: true }, 36, true)}
    <div class="who"><b>${esc(S.me.username)}</b><small>В сети</small></div>
    <button class="icon-btn" data-action="logout" title="Выйти">${icon('logout', 18)}</button>`;
}

function renderChannels() {
  const srv = curServer();
  if (!srv) return;
  $('#server-name').textContent = srv.name;
  const owner = srv.owner_id === S.me.id;
  $('#channels').innerHTML =
    `<div class="cat"><span>Текстовые каналы</span>${owner ? `<button class="icon-btn sm" data-action="create-channel" title="Создать канал">${icon('plus', 16)}</button>` : ''}</div>` +
    (S.channels[srv.id] || []).map((c) => `<button class="ch ${c.id === S.cid ? 'active' : ''} ${S.unread.has(c.id) ? 'unread' : ''}" data-cid="${c.id}">
        ${icon('hash', 18)}<span>${esc(c.name)}</span></button>`).join('');
}

function renderMembers() {
  const list = (S.members[S.sid] || []).slice().sort((a, b) => a.username.localeCompare(b.username, 'ru'));
  const section = (title, arr) => arr.length ? `<div class="cat"><span>${title} — ${arr.length}</span></div>` +
    arr.map((m) => `<div class="mem ${m.online ? '' : 'off'}">${avatar(m, 32, true)}
      <span class="name" style="color:${esc(m.color)}">${esc(m.username)}</span>
      ${m.owner ? '<span class="crown" title="Владелец сервера">★</span>' : ''}</div>`).join('') : '';
  $('#members').innerHTML = section('В сети', list.filter((m) => m.online)) + section('Не в сети', list.filter((m) => !m.online));
}

function renderEmpty() {
  S.sid = null; S.cid = null;
  $('#server-name').textContent = 'Sited';
  $('#channels').innerHTML = '';
  $('#members').innerHTML = '';
  $('#channel-name').textContent = '';
  $('#channel-topic').textContent = '';
  $('#chat-hash').innerHTML = '';
  $('#invite-btn').classList.add('hidden');
  $('#composer-wrap').classList.add('hidden');
  $('#messages').innerHTML = `<div class="empty">
      <img class="logo" src="/favicon.svg" alt="">
      <h2>Добро пожаловать в Sited</h2>
      <p>Создайте свой сервер для друзей или присоединитесь к существующему по коду приглашения.</p>
      <div class="row">
        <button class="btn primary" data-action="create-server">Создать сервер</button>
        <button class="btn outline" data-action="join-server">У меня есть приглашение</button>
      </div></div>`;
  $('#messages').style.display = 'flex';
  renderRail();
}

function renderMessages(forceBottom = false) {
  const box = $('#messages');
  const ch = curChannel();
  if (!ch) return;
  const atBottom = forceBottom || box.scrollHeight - box.scrollTop - box.clientHeight < 120;
  const msgs = S.messages[S.cid] || [];
  const srv = curServer();
  const mentionRe = new RegExp(`(^|\\s)@${escRe(S.me.username)}(?![\\p{L}\\p{N}_.])`, 'iu');

  let html = S.hasMore[S.cid]
    ? '<div class="older">Прокрутите вверх, чтобы загрузить более ранние сообщения</div>'
    : `<div class="welcome"><div class="welcome-ic">${icon('hash', 34)}</div>
        <h2>Добро пожаловать в #${esc(ch.name)}</h2><p>Это самое начало канала #${esc(ch.name)}.</p></div>`;

  let prev = null;
  for (const m of msgs) {
    const d = new Date(m.created_at);
    if (!prev || !sameDay(new Date(prev.created_at), d)) {
      html += `<div class="day"><span>${fmtDay(m.created_at)}</span></div>`;
      prev = null;
    }
    const grouped = prev && prev.author.id === m.author.id && m.created_at - prev.created_at < 5 * 60e3;
    const mine = m.author.id === S.me.id;
    const canDelete = mine || (srv && srv.owner_id === S.me.id);
    const editing = S.editing === m.id;
    html += `<div class="msg ${grouped ? 'grouped' : ''} ${!mine && mentionRe.test(m.content) ? 'mention' : ''}" data-mid="${m.id}">
      ${grouped ? `<span class="ts-side">${fmtTime(m.created_at)}</span>` : avatar(m.author, 40)}
      <div class="body">
        ${grouped ? '' : `<div class="meta"><span class="author" style="color:${esc(m.author.color)}">${esc(m.author.username)}</span>
          <span class="ts">${fmtFull(m.created_at)}</span></div>`}
        ${editing
          ? `<textarea class="edit-box" rows="1">${esc(m.content)}</textarea><div class="edit-hint">Esc — отмена · Enter — сохранить</div>`
          : `<div class="content">${format(m.content)}${m.edited_at ? '<span class="edited">(изменено)</span>' : ''}</div>`}
      </div>
      ${canDelete && !editing ? `<div class="actions">
        ${mine ? `<button data-act="edit" title="Изменить">${icon('edit', 16)}</button>` : ''}
        <button data-act="delete" class="danger" title="Удалить">${icon('trash', 16)}</button></div>` : ''}
    </div>`;
    prev = m;
  }
  box.innerHTML = html;
  if (atBottom) box.scrollTop = box.scrollHeight;

  const eb = $('.edit-box', box);
  if (eb) {
    autosize(eb);
    eb.focus();
    eb.setSelectionRange(eb.value.length, eb.value.length);
  }
}

function renderTyping() {
  const now = Date.now();
  const map = S.typing[S.cid] || {};
  const names = Object.values(map).filter((t) => t.until > now).map((t) => t.user.username);
  const el = $('#typing');
  if (!names.length) { el.innerHTML = ''; return; }
  const who = names.length === 1 ? `<b>${esc(names[0])}</b> печатает`
    : names.length <= 3 ? `${names.slice(0, -1).map((n) => `<b>${esc(n)}</b>`).join(', ')} и <b>${esc(names.at(-1))}</b> печатают`
      : 'Несколько человек печатают';
  el.innerHTML = `<span class="dots"><i></i><i></i><i></i></span><span>${who}…</span>`;
}

function renderMembersToggle() {
  const app = $('#app');
  const shown = isWide() ? !app.classList.contains('no-members') : app.classList.contains('show-members');
  $('#members-btn').classList.toggle('on', shown);
}

// ---------- навигация ----------
async function selectServer(sid) {
  S.sid = sid;
  store.set('lastServer', String(sid));
  $('#invite-btn').classList.remove('hidden');
  $('#composer-wrap').classList.remove('hidden');
  $('#messages').style.display = '';
  $('#chat-hash').innerHTML = icon('hash', 22);
  renderRail();
  if (!S.channels[sid]) {
    try {
      const [chs, mems] = await Promise.all([api('GET', `/servers/${sid}/channels`), api('GET', `/servers/${sid}/members`)]);
      S.channels[sid] = chs;
      S.members[sid] = mems;
    } catch (e) { toast(e.message); return; }
  }
  if (S.sid !== sid) return;
  renderMembers();
  const chs = S.channels[sid];
  const ch = chs.find((c) => c.id === S.lastChannel[sid]) || chs[0];
  if (ch) await selectChannel(ch.id);
  else renderChannels();
}

async function selectChannel(cid) {
  S.cid = cid;
  S.editing = null;
  S.lastChannel[S.sid] = cid;
  store.set('lastChannel', JSON.stringify(S.lastChannel));
  S.unread.delete(cid);
  closeDrawers();
  renderChannels();
  renderRail();
  const ch = curChannel();
  $('#channel-name').textContent = ch.name;
  $('#channel-topic').textContent = ch.topic || '';
  $('#input').placeholder = `Написать в #${ch.name}`;
  renderTyping();
  if (!S.messages[cid]) {
    $('#messages').innerHTML = '<div class="loading">Загрузка…</div>';
    try {
      const msgs = await api('GET', `/channels/${cid}/messages`);
      S.messages[cid] = msgs;
      S.hasMore[cid] = msgs.length === 50;
    } catch (e) { toast(e.message); return; }
  }
  if (S.cid !== cid) return;
  renderMessages(true);
  if (isWide()) $('#input').focus();
}

async function loadOlder() {
  const cid = S.cid;
  const msgs = S.messages[cid];
  if (S.loadingOlder || !msgs || !S.hasMore[cid] || !msgs.length) return;
  S.loadingOlder = true;
  try {
    const older = await api('GET', `/channels/${cid}/messages?before=${msgs[0].id}`);
    S.hasMore[cid] = older.length === 50;
    S.messages[cid] = older.concat(msgs);
    if (S.cid === cid) {
      const box = $('#messages');
      const fromBottom = box.scrollHeight - box.scrollTop;
      renderMessages();
      box.scrollTop = box.scrollHeight - fromBottom;
    }
  } catch (e) { toast(e.message); }
  S.loadingOlder = false;
}

function closeDrawers() { $('#app').classList.remove('drawer', 'show-members'); }

// ---------- сообщения ----------
function upsertMessage(m) {
  const list = S.messages[m.channel_id];
  if (!list) return false;
  const i = list.findIndex((x) => x.id === m.id);
  if (i >= 0) list[i] = m; else list.push(m);
  return true;
}

async function sendMessage() {
  const input = $('#input');
  const content = input.value.trim();
  if (!content || !S.cid) return;
  input.value = '';
  autosize(input);
  updateSendBtn();
  const cid = S.cid;
  try {
    const m = await api('POST', `/channels/${cid}/messages`, { content });
    if (upsertMessage(m) && S.cid === cid) renderMessages(true);
  } catch (e) {
    toast(e.message);
    if (!input.value) { input.value = content; autosize(input); updateSendBtn(); }
  }
}

async function saveEdit(mid, content) {
  content = content.trim();
  const msg = (S.messages[S.cid] || []).find((m) => m.id === mid);
  S.editing = null;
  if (!msg || !content || content === msg.content) { renderMessages(); return; }
  try {
    const m = await api('PATCH', `/messages/${mid}`, { content });
    upsertMessage(m);
  } catch (e) { toast(e.message); }
  renderMessages();
}

async function deleteMessage(mid) {
  if (!confirm('Удалить это сообщение?')) return;
  try {
    await api('DELETE', `/messages/${mid}`);
    S.messages[S.cid] = (S.messages[S.cid] || []).filter((m) => m.id !== mid);
    renderMessages();
  } catch (e) { toast(e.message); }
}

function sendTyping() {
  const now = Date.now();
  if (now - S.lastTypingSent < 3000 || !S.ws || S.ws.readyState !== 1 || !S.cid) return;
  S.lastTypingSent = now;
  S.ws.send(JSON.stringify({ type: 'typing', channel_id: S.cid }));
}

function updateSendBtn() { $('#send-btn').disabled = !$('#input').value.trim(); }

// ---------- WebSocket ----------
function connect() {
  if (!S.token) return;
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws?token=${encodeURIComponent(S.token)}`);
  S.ws = ws;
  ws.onopen = () => {
    S.wsDelay = 1000;
    $('#conn').classList.remove('off');
    if (S.wasConnected) resync();
    S.wasConnected = true;
  };
  ws.onmessage = (e) => { try { onEvent(JSON.parse(e.data)); } catch (err) { console.error(err); } };
  ws.onclose = () => {
    if (S.ws !== ws || !S.token) return;
    $('#conn').classList.add('off');
    setTimeout(connect, S.wsDelay);
    S.wsDelay = Math.min(S.wsDelay * 2, 15000);
  };
}

/** После переподключения перезагружаем данные — пока нас не было, могли прийти сообщения. */
async function resync() {
  S.channels = {}; S.members = {}; S.messages = {}; S.hasMore = {};
  try {
    S.servers = await api('GET', '/servers');
    if (S.sid && S.servers.some((s) => s.id === S.sid)) await selectServer(S.sid);
    else if (S.servers[0]) await selectServer(S.servers[0].id);
    else renderEmpty();
  } catch { /* повторим при следующем подключении */ }
}

function onEvent(ev) {
  switch (ev.type) {
    case 'message_create': {
      const m = ev.message;
      const typing = S.typing[m.channel_id];
      if (typing) delete typing[m.author.id];
      upsertMessage(m);
      if (m.channel_id === S.cid) { renderMessages(m.author.id === S.me.id); renderTyping(); }
      else if (m.author.id !== S.me.id) {
        S.unread.set(m.channel_id, ev.server_id);
        if (ev.server_id === S.sid) renderChannels();
        renderRail();
      }
      break;
    }
    case 'message_update':
      if (upsertMessage(ev.message) && ev.message.channel_id === S.cid && S.editing !== ev.message.id) renderMessages();
      break;
    case 'message_delete': {
      const list = S.messages[ev.channel_id];
      if (list) {
        S.messages[ev.channel_id] = list.filter((m) => m.id !== ev.message_id);
        if (ev.channel_id === S.cid) renderMessages();
      }
      break;
    }
    case 'channel_create': {
      const c = ev.channel;
      const list = S.channels[c.server_id];
      if (list && !list.some((x) => x.id === c.id)) list.push(c);
      if (c.server_id === S.sid) renderChannels();
      break;
    }
    case 'member_join': {
      const list = S.members[ev.server_id];
      if (list && !list.some((m) => m.id === ev.member.id)) list.push(ev.member);
      if (ev.server_id === S.sid) renderMembers();
      break;
    }
    case 'presence':
      for (const list of Object.values(S.members)) {
        const m = list.find((x) => x.id === ev.user_id);
        if (m) m.online = ev.online;
      }
      renderMembers();
      break;
    case 'typing':
      if (ev.user.id === S.me.id) break;
      (S.typing[ev.channel_id] ||= {})[ev.user.id] = { user: ev.user, until: Date.now() + 6000 };
      if (ev.channel_id === S.cid) renderTyping();
      break;
  }
}

// ---------- модальные окна ----------
function modal({ title, sub = '', body, submit, onSubmit }) {
  const root = $('#modal-root');
  root.innerHTML = `<div class="overlay"><form class="modal" autocomplete="off">
      <h2>${title}</h2>${sub ? `<p class="sub">${sub}</p>` : ''}${body}
      <div class="error"></div>
      <div class="modal-actions">
        <button type="button" class="btn ghost" data-close>${onSubmit ? 'Отмена' : 'Закрыть'}</button>
        ${onSubmit ? `<button class="btn primary">${submit}</button>` : ''}
      </div></form></div>`;
  const close = () => { root.innerHTML = ''; };
  $('.overlay', root).addEventListener('mousedown', (e) => { if (e.target.classList.contains('overlay')) close(); });
  $('[data-close]', root).onclick = close;
  const form = $('form', root);
  form.onsubmit = async (e) => {
    e.preventDefault();
    if (!onSubmit) return;
    const btn = $('.btn.primary', form);
    btn.disabled = true;
    try { await onSubmit(new FormData(form)); close(); }
    catch (err) { $('.error', form).textContent = err.message; btn.disabled = false; }
  };
  const first = $('input', form);
  if (first) { first.focus(); first.select(); }
  return form;
}

async function addServer(server) {
  if (!S.servers.some((s) => s.id === server.id)) S.servers.push(server);
  await selectServer(server.id);
}

const actions = {
  'create-server': () => modal({
    title: 'Создайте свой сервер',
    sub: 'Сервер — это место, где вы общаетесь с друзьями. Название можно придумать любое.',
    body: `<label class="field"><span>Название сервера</span><input name="name" maxlength="50" required value="Сервер ${esc(S.me.username)}"></label>`,
    submit: 'Создать',
    onSubmit: async (f) => addServer(await api('POST', '/servers', { name: f.get('name') })),
  }),
  'join-server': () => modal({
    title: 'Присоединиться к серверу',
    sub: 'Введите код приглашения, который вам прислали.',
    body: `<label class="field"><span>Код приглашения</span><input name="invite" required placeholder="например, aB3dE9xZ"></label>`,
    submit: 'Присоединиться',
    onSubmit: async (f) => addServer(await api('POST', '/servers/join', { invite: f.get('invite') })),
  }),
  'create-channel': () => modal({
    title: 'Новый канал',
    body: `<label class="field"><span>Название канала</span><input name="name" maxlength="40" required placeholder="новый-канал"></label>
           <label class="field"><span>Тема (необязательно)</span><input name="topic" maxlength="120" placeholder="О чём этот канал?"></label>`,
    submit: 'Создать канал',
    onSubmit: async (f) => {
      const c = await api('POST', `/servers/${S.sid}/channels`, { name: f.get('name'), topic: f.get('topic') });
      const list = S.channels[c.server_id];
      if (list && !list.some((x) => x.id === c.id)) list.push(c);
      await selectChannel(c.id);
    },
  }),
  invite: () => {
    const srv = curServer();
    if (!srv) return;
    const form = modal({
      title: `Пригласить друзей на «${esc(srv.name)}»`,
      sub: 'Отправьте другу этот код — он введёт его в «Войти по приглашению».',
      body: `<div class="copy-row field"><input class="field-input" readonly value="${esc(srv.invite)}">
             <button type="button" class="btn primary" data-copy>Копировать</button></div>`,
    });
    const input = $('input', form);
    input.style.cssText = 'height:42px;padding:0 12px;background:var(--bg);border:1px solid var(--line);border-radius:var(--r-sm);outline:none';
    $('[data-copy]', form).onclick = async (e) => {
      try { await navigator.clipboard.writeText(srv.invite); }
      catch { input.select(); document.execCommand('copy'); }
      e.target.textContent = 'Скопировано';
    };
  },
  logout: () => logout(),
};

// ---------- события интерфейса ----------
function bindUi() {
  $('#invite-btn').innerHTML = icon('link', 18);
  $('#members-btn').innerHTML = icon('users', 20);
  $('#menu-btn').innerHTML = icon('menu', 22);
  $('#send-btn').innerHTML = icon('send', 18);
  updateSendBtn();

  document.addEventListener('click', (e) => {
    const act = e.target.closest('[data-action]');
    if (act && actions[act.dataset.action]) { actions[act.dataset.action](); return; }
    const srv = e.target.closest('[data-sid]');
    if (srv) { selectServer(Number(srv.dataset.sid)); return; }
    const ch = e.target.closest('[data-cid]');
    if (ch) { selectChannel(Number(ch.dataset.cid)); return; }
    const msgAct = e.target.closest('.actions button');
    if (msgAct) {
      const mid = Number(msgAct.closest('.msg').dataset.mid);
      if (msgAct.dataset.act === 'edit') { S.editing = mid; renderMessages(); }
      else deleteMessage(mid);
    }
  });

  $('#invite-btn').onclick = actions.invite;
  $('#menu-btn').onclick = () => $('#app').classList.toggle('drawer');
  $('#scrim').onclick = closeDrawers;
  $('#members-btn').onclick = () => {
    const app = $('#app');
    if (isWide()) {
      app.classList.toggle('no-members');
      store.set('hideMembers', app.classList.contains('no-members') ? '1' : '0');
    } else app.classList.toggle('show-members');
    renderMembersToggle();
  };
  window.addEventListener('resize', renderMembersToggle);
  renderMembersToggle();

  const input = $('#input');
  $('#composer').onsubmit = (e) => { e.preventDefault(); sendMessage(); };
  input.addEventListener('input', () => { autosize(input); updateSendBtn(); if (input.value.trim()) sendTyping(); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); sendMessage(); }
    if (e.key === 'ArrowUp' && !input.value) {
      const mine = (S.messages[S.cid] || []).filter((m) => m.author.id === S.me.id).at(-1);
      if (mine) { e.preventDefault(); S.editing = mine.id; renderMessages(); }
    }
  });

  const box = $('#messages');
  box.addEventListener('scroll', () => { if (box.scrollTop < 80) loadOlder(); });
  box.addEventListener('keydown', (e) => {
    if (!e.target.classList.contains('edit-box')) return;
    const mid = Number(e.target.closest('.msg').dataset.mid);
    if (e.key === 'Escape') { S.editing = null; renderMessages(); input.focus(); }
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); saveEdit(mid, e.target.value); input.focus(); }
  });
  box.addEventListener('input', (e) => { if (e.target.classList.contains('edit-box')) autosize(e.target); });

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && $('#modal-root').innerHTML) $('#modal-root').innerHTML = '';
  });

  setInterval(renderTyping, 1000);
}

// ---------- старт ----------
bindUi();
(async () => {
  if (!S.token) return showAuth();
  try {
    S.me = await api('GET', '/me');
    await start();
  } catch {
    store.set('token', null);
    S.token = null;
    showAuth();
  }
})();
