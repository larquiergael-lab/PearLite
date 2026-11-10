// Injected at document start into the top frame. `__PEAR_LITE_CFG__` is prepended by the host.
(() => {
  const CFG = window.__PEAR_LITE_CFG__ || {};
  delete window.__PEAR_LITE_CFG__;
  if (window.__pearLite || location.hostname !== 'music.youtube.com') return;

  const post = (msg) => {
    try { window.ipc.postMessage(JSON.stringify(msg)); } catch (_) {}
  };

  // ---------------------------------------------------------------- ad block
  // Same idea as uBlock Origin's json-prune: drop ad payloads from player responses
  // before the player sees them. Hooks are Proxies so they still look native.
  if (CFG.adBlock) {
    const AD_KEYS = ['adPlacements', 'adSlots', 'playerAds', 'adBreakHeartbeatParams'];
    const prune = (o) => {
      if (!o || typeof o !== 'object') return;
      for (const k of AD_KEYS) if (k in o) delete o[k];
      if (o.playerResponse && typeof o.playerResponse === 'object') prune(o.playerResponse);
    };
    JSON.parse = new Proxy(JSON.parse, {
      apply(target, self, args) {
        const r = Reflect.apply(target, self, args);
        try { prune(r); } catch (_) {}
        return r;
      },
    });
    Response.prototype.json = new Proxy(Response.prototype.json, {
      apply(target, self, args) {
        return Reflect.apply(target, self, args).then((r) => {
          try { prune(r); } catch (_) {}
          return r;
        });
      },
    });
  }

  // Fallback for ads that still get through: skip, or mute + fast-forward.
  let adSaved = null;
  const checkAd = (v) => {
    const player = document.getElementById('movie_player');
    if (player && player.classList.contains('ad-showing')) {
      const skip = document.querySelector('.ytp-skip-ad-button, .ytp-ad-skip-button, .ytp-ad-skip-button-modern');
      if (skip) skip.click();
      if (!adSaved) adSaved = { rate: v.playbackRate, muted: v.muted };
      v.muted = true;
      if (v.playbackRate !== 16) v.playbackRate = 16;
    } else if (adSaved) {
      v.playbackRate = adSaved.rate;
      v.muted = adSaved.muted;
      adSaved = null;
    }
  };

  // ------------------------------------------------------- media session tap
  // YouTube Music publishes track info and transport handlers via the Media Session
  // API; capture both so the native side (SMTC, tray, Discord) mirrors the page exactly.
  const handlers = {};
  const msProto = window.MediaSession && MediaSession.prototype;
  if (msProto) {
    const origSet = msProto.setActionHandler;
    msProto.setActionHandler = function (action, fn) {
      handlers[action] = fn;
      return origSet.call(this, action, fn);
    };
    const desc = Object.getOwnPropertyDescriptor(msProto, 'metadata');
    if (desc && desc.set) {
      Object.defineProperty(msProto, 'metadata', {
        ...desc,
        set(v) {
          desc.set.call(this, v);
          queueMicrotask(sendMeta);
        },
      });
    }
  }

  const video = () => document.querySelector('video');

  let lastMetaKey = '';
  function sendMeta() {
    const md = navigator.mediaSession && navigator.mediaSession.metadata;
    if (!md || !md.title) return;
    const art = md.artwork && md.artwork.length ? md.artwork[md.artwork.length - 1].src : '';
    const key = [md.title, md.artist, md.album, art].join('\u0000');
    if (key === lastMetaKey) return;
    lastMetaKey = key;
    post({ t: 'meta', title: md.title || '', artist: md.artist || '', album: md.album || '', art });
  }

  let lastStateAt = 0;
  let lastPlaying = null;
  function sendState(force) {
    const v = video();
    if (!v) return;
    const playing = !v.paused && !v.ended;
    const now = Date.now();
    // Periodic updates keep the Windows media flyout's progress bar honest without IPC spam.
    if (!force && playing === lastPlaying && now - lastStateAt < 5000) return;
    lastStateAt = now;
    lastPlaying = playing;
    sendMeta();
    post({
      t: 'state',
      playing,
      pos: v.currentTime || 0,
      dur: Number.isFinite(v.duration) ? v.duration : 0,
    });
  }

  // After a long stretch without input YouTube Music pauses and asks "Are you still there?".
  // Nobody is looking at a player hidden in the tray, so answer it.
  const dismissStillThere = () => {
    const btn = document.querySelector('ytmusic-you-there-renderer button, ytmusic-you-there-renderer yt-button-renderer');
    if (btn) btn.click();
  };

  const onMedia = (ev) => {
    const v = ev.target;
    if (!(v instanceof HTMLMediaElement)) return;
    if (CFG.adBlock) checkAd(v);
    if (ev.type === 'pause') setTimeout(dismissStillThere, 1000);
    sendState(ev.type !== 'timeupdate');
  };
  // Media events don't bubble, but capture-phase listeners on document still see them.
  for (const type of ['play', 'playing', 'pause', 'ended', 'seeked', 'durationchange', 'timeupdate']) {
    document.addEventListener(type, onMedia, true);
  }

  // --------------------------------------------------------- host commands
  const call = (action, details) => {
    const fn = handlers[action];
    if (!fn) return false;
    try { fn({ action, ...details }); return true; } catch (_) { return false; }
  };
  const click = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.click();
    return true;
  };

  window.__pearLite = {
    cmd(name, arg) {
      const v = video();
      switch (name) {
        case 'toggle':
          return this.cmd(v && !v.paused ? 'pause' : 'play');
        case 'play':
          if (v && !v.paused) return;
          return call('play') || click('#play-pause-button') || (v && v.play());
        case 'pause':
          if (!v || v.paused) return;
          return call('pause') || click('#play-pause-button') || v.pause();
        case 'next':
          return call('nexttrack') || click('.next-button');
        case 'prev':
          return call('previoustrack') || click('.previous-button');
        case 'seek':
          return call('seekto', { seekTime: arg }) || (v && (v.currentTime = arg));
      }
    },
  };

  // ------------------------------------------------------------ user styles
  if (CFG.css) {
    const add = () => {
      const s = document.createElement('style');
      s.id = 'pear-lite-custom-css';
      s.textContent = CFG.css;
      document.documentElement.appendChild(s);
    };
    document.documentElement ? add() : document.addEventListener('DOMContentLoaded', add, { once: true });
  }
})();
