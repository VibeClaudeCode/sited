// Aurora content script — injected into every page before it runs.
// Talks to the browser only through window.ipc.postMessage with a tiny, whitelisted protocol.
(() => {
  if (window.__aurora || window.top !== window) return;
  const SHIELD = __SHIELD__;
  const DNT = __DNT__;
  const post = (m) => { try { window.ipc.postMessage(JSON.stringify(m)); } catch (_) {} };

  // ---------- keyboard shortcuts (forwarded to the browser UI) ----------
  const KEYS = new Set([
    'Ctrl+T', 'Ctrl+Shift+T', 'Ctrl+W', 'Ctrl+L', 'Ctrl+K', 'Ctrl+R', 'Ctrl+Shift+R', 'F5',
    'Ctrl+F', 'Ctrl+D', 'Ctrl+Tab', 'Ctrl+Shift+Tab', 'Ctrl+=', 'Ctrl++', 'Ctrl+-', 'Ctrl+0',
    'Ctrl+Shift+N', 'Ctrl+Shift+S', 'Ctrl+H', 'Ctrl+J', 'Ctrl+Shift+B', 'Ctrl+,', 'Alt+ArrowLeft',
    'Alt+ArrowRight', 'Alt+D', 'Alt+R', 'F11', 'F12', 'Ctrl+Shift+I', 'Ctrl+Shift+P', 'Ctrl+G', 'Ctrl+Shift+G',
    'Ctrl+1', 'Ctrl+2', 'Ctrl+3', 'Ctrl+4', 'Ctrl+5', 'Ctrl+6', 'Ctrl+7', 'Ctrl+8', 'Ctrl+9', 'Escape'
  ]);
  // layout-independent: letters/digits come from e.code, so shortcuts work on Russian layout too
  const combo = (e) => {
    let k = e.key.length === 1 ? e.key.toUpperCase() : e.key;
    if (e.code && /^Key[A-Z]$/.test(e.code)) k = e.code.slice(3);
    if (e.code && /^Digit\d$/.test(e.code)) k = e.code.slice(5);
    const shift = e.shiftKey && (k.length > 1 || /^[A-Z0-9]$/.test(k));
    return (e.ctrlKey || e.metaKey ? 'Ctrl+' : '') + (e.altKey ? 'Alt+' : '') + (shift ? 'Shift+' : '') + k;
  };
  window.addEventListener('keydown', (e) => {
    const c = combo(e);
    if (!KEYS.has(c)) return;
    if (c === 'Escape') { if (finder.active) { finder.clear(); post({ t: 'key', k: c }); } return; }
    e.preventDefault(); e.stopPropagation();
    post({ t: 'key', k: c });
  }, true);
  // mouse back / forward buttons
  window.addEventListener('mouseup', (e) => {
    if (e.button === 3) { e.preventDefault(); history.back(); }
    if (e.button === 4) { e.preventDefault(); history.forward(); }
  }, true);

  // ---------- SPA url changes ----------
  const notifyUrl = () => post({ t: 'url', url: location.href });
  for (const fn of ['pushState', 'replaceState']) {
    const orig = history[fn];
    history[fn] = function () { const r = orig.apply(this, arguments); setTimeout(notifyUrl, 0); return r; };
  }
  window.addEventListener('popstate', notifyUrl);
  window.addEventListener('hashchange', notifyUrl);

  // ---------- page metadata: favicon + theme colour ----------
  const meta = () => {
    const icon = document.querySelector('link[rel~="icon"][href], link[rel="apple-touch-icon"][href]');
    let favicon = icon ? icon.href : (location.protocol.startsWith('http') ? location.origin + '/favicon.ico' : '');
    const tc = document.querySelector('meta[name="theme-color"]');
    let theme = tc ? tc.content : '';
    if (!theme && document.body) {
      const bg = getComputedStyle(document.body).backgroundColor;
      if (bg && bg !== 'rgba(0, 0, 0, 0)' && bg !== 'transparent') theme = bg;
    }
    post({ t: 'meta', favicon, theme });
  };
  window.addEventListener('DOMContentLoaded', meta);
  window.addEventListener('load', () => setTimeout(meta, 300));

  // ---------- privacy signals ----------
  if (DNT) {
    try { Object.defineProperty(navigator, 'globalPrivacyControl', { get: () => true }); } catch (_) {}
    try { Object.defineProperty(navigator, 'doNotTrack', { get: () => '1' }); } catch (_) {}
  }

  // ---------- Shield: tracker & ad blocking (script-level + cosmetic) ----------
  let blocked = 0;
  if (SHIELD) {
    const HOSTS = ['doubleclick.net', 'googlesyndication.com', 'googleadservices.com', 'google-analytics.com',
      'googletagmanager.com', 'googletagservices.com', 'adservice.google.com', 'facebook.net', 'connect.facebook.net',
      'scorecardresearch.com', 'quantserve.com', 'adnxs.com', 'criteo.com', 'criteo.net', 'taboola.com', 'outbrain.com',
      'hotjar.com', 'mixpanel.com', 'segment.io', 'segment.com', 'amplitude.com', 'adsrvr.org', 'rubiconproject.com',
      'pubmatic.com', 'openx.net', 'moatads.com', 'yandex.ru/metrika', 'mc.yandex.ru', 'an.yandex.ru', 'top-fwz1.mail.ru',
      'tiktok.com/i18n/pixel', 'analytics.tiktok.com', 'bat.bing.com', 'clarity.ms', 'ads-twitter.com', 'static.ads-twitter.com',
      'amazon-adsystem.com', 'adform.net', 'smartadserver.com', 'teads.tv', 'zedo.com', 'popads.net', 'propellerads.com'];
    const isBad = (u) => {
      try {
        const url = new URL(u, location.href);
        const s = url.hostname + url.pathname;
        if (url.hostname === location.hostname) return false;
        return HOSTS.some((h) => s === h || s.includes('.' + h) || s.startsWith(h));
      } catch (_) { return false; }
    };
    const hit = () => { blocked++; clearTimeout(hit.t); hit.t = setTimeout(() => post({ t: 'blocked', n: blocked }), 250); };
    const ofetch = window.fetch;
    window.fetch = function (input, init) {
      const u = typeof input === 'string' ? input : (input && input.url);
      if (u && isBad(u)) { hit(); return Promise.reject(new TypeError('Blocked by Aurora Shield')); }
      return ofetch.apply(this, arguments);
    };
    const oopen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (m, u) {
      if (u && isBad(String(u))) { hit(); u = 'data:,'; arguments[1] = u; }
      return oopen.apply(this, arguments);
    };
    if (navigator.sendBeacon) {
      const ob = navigator.sendBeacon.bind(navigator);
      navigator.sendBeacon = (u, d) => (isBad(u) ? (hit(), true) : ob(u, d));
    }
    // intercept src assignment for dynamically injected scripts/iframes/pixels
    for (const [proto, attr] of [[HTMLScriptElement.prototype, 'src'], [HTMLIFrameElement.prototype, 'src'], [HTMLImageElement.prototype, 'src']]) {
      const d = Object.getOwnPropertyDescriptor(proto, attr);
      if (!d || !d.set) continue;
      Object.defineProperty(proto, attr, {
        configurable: true, enumerable: d.enumerable, get: d.get,
        set(v) { if (isBad(String(v))) { hit(); return d.set.call(this, 'about:blank'); } return d.set.call(this, v); }
      });
    }
    const oset = Element.prototype.setAttribute;
    Element.prototype.setAttribute = function (n, v) {
      if ((n === 'src' || n === 'href') && /^(SCRIPT|IFRAME|IMG|LINK)$/.test(this.tagName) && isBad(String(v))) { hit(); v = 'about:blank'; }
      return oset.call(this, n, v);
    };
    // parser-inserted elements
    new MutationObserver((muts) => {
      for (const m of muts) for (const n of m.addedNodes) {
        if (n.nodeType !== 1) continue;
        const src = n.getAttribute && (n.getAttribute('src') || '');
        if (src && /^(SCRIPT|IFRAME|IMG)$/.test(n.tagName) && isBad(src)) { hit(); n.remove(); }
      }
    }).observe(document.documentElement, { childList: true, subtree: true });
    // cosmetic filters
    const css = `ins.adsbygoogle,.adsbygoogle,[id^="google_ads_"],[id^="div-gpt-ad"],iframe[src*="doubleclick"],
      .ad-banner,.ad-container,.advertisement,[data-ad-slot],[data-ad-unit],.sponsored-content,.taboola,#taboola,.OUTBRAIN,
      .yandex-rtb,[id^="yandex_rtb"],[id^="adfox_"]{display:none!important}`;
    const style = () => { const s = document.createElement('style'); s.textContent = css; (document.head || document.documentElement).appendChild(s); };
    if (document.documentElement) style(); else document.addEventListener('DOMContentLoaded', style);
  }

  // ---------- find in page ----------
  const finder = {
    active: false, q: '',
    count(q) {
      if (!q || !document.body) return 0;
      const text = document.body.innerText.toLowerCase();
      let n = 0, i = 0; q = q.toLowerCase();
      while ((i = text.indexOf(q, i)) !== -1) { n++; i += q.length; }
      return n;
    },
    find(q, back) {
      this.active = !!q;
      if (q !== this.q) { this.q = q; window.getSelection().removeAllRanges(); }
      const found = q ? window.find(q, false, !!back, true, false, false, false) : false;
      post({ t: 'find', total: this.count(q), found });
    },
    clear() { this.active = false; this.q = ''; window.getSelection().removeAllRanges(); }
  };

  // ---------- force dark ----------
  const dark = (on) => {
    let s = document.getElementById('__aurora_dark');
    if (!on) { if (s) s.remove(); return; }
    if (!s) { s = document.createElement('style'); s.id = '__aurora_dark'; (document.head || document.documentElement).appendChild(s); }
    s.textContent = `html{filter:invert(.92) hue-rotate(180deg)!important;background:#fff!important}
      img,video,picture,canvas,svg image,iframe,embed,object,[style*="background-image"]{filter:invert(1) hue-rotate(180deg)!important}`;
  };

  // ---------- reader mode ----------
  const reader = () => {
    if (document.getElementById('__aurora_reader')) { location.reload(); return; }
    let best = null, bestScore = 0;
    for (const el of document.querySelectorAll('article, main, [role="main"], .post, .article, .entry-content, .content, div, section')) {
      const ps = el.querySelectorAll(':scope > p, :scope > div > p');
      let score = 0;
      for (const p of ps) score += Math.min(p.innerText.length, 600);
      if (el.tagName === 'ARTICLE') score *= 1.5;
      if (score > bestScore) { bestScore = score; best = el; }
    }
    if (!best || bestScore < 400) { post({ t: 'toast', msg: 'Не нашёл статью на этой странице' }); return; }
    const clone = best.cloneNode(true);
    const title = (document.querySelector('h1') || {}).innerText || document.title;
    const firstH = clone.querySelector('h1');
    if (firstH && firstH.innerText.trim() === String(title).trim()) firstH.remove();
    clone.querySelectorAll('script,style,iframe,nav,aside,form,button,[class*="share"],[class*="comment"],[class*="related"]').forEach((n) => n.remove());
    document.documentElement.innerHTML = `<head><meta charset="utf-8"><title>${document.title}</title><style>
      :root{color-scheme:light dark}body{margin:0;background:#faf8f3;color:#222;font:19px/1.7 Georgia,'Times New Roman',serif}
      @media (prefers-color-scheme:dark){body{background:#17161c;color:#ddd}a{color:#a89bff}}
      #__aurora_reader{max-width:720px;margin:0 auto;padding:64px 24px 120px;animation:in .6s cubic-bezier(.2,.8,.2,1)}
      @keyframes in{from{opacity:0;transform:translateY(24px)}}
      h1{font:700 2.1em/1.2 system-ui,sans-serif;letter-spacing:-.02em;margin:0 0 .2em}
      .src{font:14px system-ui,sans-serif;opacity:.6;margin-bottom:2.5em}
      img,video,figure{max-width:100%;height:auto;border-radius:12px}pre{overflow:auto;font-size:14px}
    </style></head><body><div id="__aurora_reader"><h1></h1><div class="src"></div></div></body>`;
    const root = document.getElementById('__aurora_reader');
    root.querySelector('h1').textContent = title;
    root.querySelector('.src').textContent = location.hostname + ' · ' + Math.max(1, Math.round(clone.innerText.split(/\s+/).length / 200)) + ' мин чтения';
    root.appendChild(clone);
    window.scrollTo(0, 0);
  };

  Object.defineProperty(window, '__aurora', { value: Object.freeze({ find: (q, b) => finder.find(q, b), clearFind: () => finder.clear(), dark, reader }) });
})();
