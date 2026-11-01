(function () {
  'use strict';
  var COLS = 24, ROWS = 16, CELL = 40, W = 960, H = 640, TICK = 125;
  var HOSTS = ['host-a', 'host-b', 'host-c'];
  var HOST_RGB = ['255,122,89', '91,140,255', '168,219,76'];
  var token = (document.querySelector('meta[name="csrf-token"]') || {}).content || '';
  var canvas = document.getElementById('sky'), ctx = canvas.getContext('2d');
  var showOwners = document.getElementById('show-owners');
  var reduced = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
  var frame = null, prev = {}, curr = {}, arrived = 0, state = null, busy = false;
  var stars = [];

  // A fixed scatter of stars, so a hole always shows the same night.
  (function () {
    var s = 7;
    for (var i = 0; i < 900; i++) {
      s = (s * 1103515245 + 12345) & 0x7fffffff; var x = s % W;
      s = (s * 1103515245 + 12345) & 0x7fffffff; var y = s % H;
      s = (s * 1103515245 + 12345) & 0x7fffffff; stars.push([x, y, 0.3 + (s % 100) / 140]);
    }
  })();

  function el(tag, attrs, kids) {
    var e = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) {
      if (k === 'text') e.textContent = attrs[k];
      else if (k === 'style') e.setAttribute('style', attrs[k]);
      else e.setAttribute(k, attrs[k]);
    });
    (kids || []).forEach(function (c) { if (c) e.appendChild(c); });
    return e;
  }
  function secs(ms) { return (ms / 1000).toFixed(1) + 's'; }
  function n(x) { return (x || 0).toLocaleString('en-US'); }

  function size() {
    var dpr = window.devicePixelRatio || 1, r = canvas.getBoundingClientRect();
    canvas.width = Math.round(r.width * dpr);
    canvas.height = Math.round(r.width * dpr * H / W);
  }

  function wrapLerp(a, b, t, len) {
    var d = b - a;
    if (d > len / 2) d -= len; else if (d < -len / 2) d += len;
    var v = a + d * t;
    return v < 0 ? v + len : (v >= len ? v - len : v);
  }

  function drawSky(k) {
    var g = ctx.createLinearGradient(0, 0, 0, H);
    g.addColorStop(0, '#f6d3ad'); g.addColorStop(0.45, '#eaa38f'); g.addColorStop(0.8, '#a98aa6'); g.addColorStop(1, '#6f6f97');
    ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
    if (!frame || !frame.cells.length) return;
    var holes = [];
    for (var i = 0; i < frame.cells.length; i++) {
      var o = frame.cells[i], cx = (i % COLS) * CELL, cy = Math.floor(i / COLS) * CELL;
      if (o < 0) { holes.push(i); continue; }
      if (showOwners.checked) {
        ctx.fillStyle = 'rgba(' + HOST_RGB[o] + ',0.22)';
        ctx.fillRect(cx, cy, CELL, CELL);
        ctx.strokeStyle = 'rgba(' + HOST_RGB[o] + ',0.55)';
        ctx.lineWidth = 0.6; ctx.strokeRect(cx + 0.5, cy + 0.5, CELL - 1, CELL - 1);
      }
    }
    if (!holes.length) return;
    // The holes: night showing through.
    ctx.save();
    ctx.beginPath();
    holes.forEach(function (i) { ctx.rect((i % COLS) * CELL, Math.floor(i / COLS) * CELL, CELL, CELL); });
    ctx.clip();
    ctx.fillStyle = '#0d0f17'; ctx.fillRect(0, 0, W, H);
    var tw = reduced ? 1 : 0.75 + 0.25 * Math.sin(k / 600);
    stars.forEach(function (s) {
      ctx.fillStyle = 'rgba(236,231,223,' + (s[2] * tw).toFixed(2) + ')';
      ctx.fillRect(s[0], s[1], 1.2, 1.2);
    });
    ctx.restore();
    ctx.strokeStyle = 'rgba(13,15,23,0.55)'; ctx.lineWidth = 1;
    holes.forEach(function (i) { ctx.strokeRect((i % COLS) * CELL + 0.5, Math.floor(i / COLS) * CELL + 0.5, CELL - 1, CELL - 1); });
  }

  function bird(x, y, vx, vy, fill, stroke) {
    var a = Math.atan2(vy, vx);
    ctx.save(); ctx.translate(x, y); ctx.rotate(a);
    ctx.beginPath();
    ctx.moveTo(5.5, 0); ctx.lineTo(-4, 3.6); ctx.lineTo(-2, 0); ctx.lineTo(-4, -3.6); ctx.closePath();
    if (fill) { ctx.fillStyle = fill; ctx.fill(); }
    if (stroke) { ctx.strokeStyle = stroke; ctx.lineWidth = 1; ctx.stroke(); }
    ctx.restore();
  }

  function draw(now) {
    var dpr = canvas.width / W;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    drawSky(now);
    if (!frame) { requestAnimationFrame(draw); return; }
    var t = reduced ? 1 : Math.min(1, (now - arrived) / TICK);
    frame.boids.forEach(function (b) {
      var id = b[0], st = b[5], p = prev[id];
      var x = b[1], y = b[2];
      if (p && st !== 1 && st !== 2) { x = wrapLerp(p[0], b[1], t, W); y = wrapLerp(p[1], b[2], t, H); }
      if (st === 1) bird(x, y, b[3], b[4], null, 'rgba(236,231,223,0.7)');
      else if (st === 3) bird(x, y, b[3], b[4], '#fff1c9', '#1b1a22');
      else bird(x, y, b[3], b[4], '#1b1a22', null);
    });
    var at = state ? state.now_ms : 0;
    frame.marks.forEach(function (m) {
      var age = Math.max(0, Math.min(1, (at - m.at_ms) / 4000));
      var r = 6 + age * 16, a = (1 - age).toFixed(2);
      ctx.lineWidth = 1.6;
      ctx.strokeStyle = m.kind === 'lost' ? 'rgba(220,40,40,' + a + ')' : 'rgba(255,190,60,' + a + ')';
      ctx.beginPath(); ctx.arc(m.x, m.y, r, 0, Math.PI * 2); ctx.stroke();
      if (m.kind === 'lost') {
        ctx.beginPath(); ctx.moveTo(m.x - 4, m.y - 4); ctx.lineTo(m.x + 4, m.y + 4);
        ctx.moveTo(m.x + 4, m.y - 4); ctx.lineTo(m.x - 4, m.y + 4); ctx.stroke();
      }
    });
    requestAnimationFrame(draw);
  }

  function readout() {
    var r = document.getElementById('readout');
    if (!frame || !frame.tick) { r.textContent = 'waiting for the hosts to claim their cells…'; return; }
    r.textContent = 'tick ' + n(frame.tick) + ' · ' + n(frame.flying) + ' flying · ' + n(frame.frozen) +
      ' frozen · ' + frame.holes + ' holes · ' + frame.tick_ms + ' ms/tick';
  }

  function hosts() {
    var root = document.getElementById('hosts');
    root.textContent = '';
    var counts = [0, 0, 0];
    if (frame) frame.cells.forEach(function (o) { if (o >= 0) counts[o]++; });
    state.hosts.forEach(function (h, idx) {
      var held = (h.view.held || []).map(function (r) {
        return el('div', { class: 'held' }, [document.createTextNode('holds '), el('b', { text: r.range }),
          document.createTextNode(' at epoch ' + r.epoch + (r.serving ? '' : ' — not serving'))]);
      });
      if (!held.length) held = [el('div', { class: 'held', text: h.view.reachable ? 'holds nothing' : 'not answering' })];
      (h.view.waiting || []).forEach(function (w) { held.push(el('div', { class: 'held', text: 'waiting to claim ' + w })); });
      var b = function (label, action, cls, enabled) {
        var x = el('button', { class: cls || '', 'data-post': '/hosts/' + h.name + '/' + action, text: label });
        if (!enabled) x.setAttribute('disabled', '');
        return x;
      };
      var p = h.process;
      root.appendChild(el('div', { class: 'host', style: '--lane:var(--' + h.name + ')' }, [
        el('div', {}, [el('span', { class: 'name', text: h.name + ' ' }), el('span', { class: 'pill ' + p, text: p })]),
        el('div', { class: 'role', text: h.role + ' · claims ' + h.slots + ' · :' + h.port + (h.pid ? ' · pid ' + h.pid : '') }),
        el('div', { class: 'cells' }, [el('b', { text: String(counts[idx]) }), document.createTextNode(' of 384 cells served last tick')]),
        h.view.lag_s == null ? null : el('div', { class: 'cells' + (h.view.lag_s > 6 ? ' behind' : '') }, [
          document.createTextNode('replica '), el('b', { text: h.view.lag_s.toFixed(0) + ' s' }),
          document.createTextNode(' behind · ' + (h.view.replicating || 0) + ' files shipping')])
      ].concat(held).concat([el('div', { class: 'buttons' }, [
        b('Start', 'start', 'go', p === 'stopped'),
        b('kill -9', 'kill', 'danger', p !== 'stopped'),
        b('Stop cleanly', 'term', '', p === 'running'),
        b('Freeze', 'freeze', '', p === 'running'),
        b('Thaw', 'thaw', '', p === 'frozen')
      ])])));
    });
    var fly = document.getElementById('fly');
    fly.textContent = state.flying ? 'Pause' : 'Fly';
    fly.setAttribute('data-post', state.flying ? '/flock/stop' : '/flock/start');
  }

  function stat(label, value, cls) {
    return el('div', { class: 'stat ' + (cls || '') }, [el('b', { text: value }), el('span', { text: label })]);
  }

  function lossesByFailure() {
    // Each lost boid against the first failure after its last acknowledged write.
    var out = {};
    (frame.lost || []).forEach(function (l) {
      var f = (state.failures || []).find(function (f) { return f.at_ms >= l.acked_ms; });
      var key = f ? f.host + ' ' + f.what + ' at ' + secs(f.at_ms) : 'no failure after it';
      var gap = f ? f.at_ms - l.acked_ms : null;
      var g = out[key] || (out[key] = { count: 0, min: Infinity, max: 0 });
      g.count++;
      if (gap !== null) { g.min = Math.min(g.min, gap); g.max = Math.max(g.max, gap); }
    });
    return out;
  }

  function rewindsByFailure() {
    // Each rewind against the last failure before it was seen.
    var out = {};
    (frame.rewinds || []).forEach(function (r) {
      var f = null;
      (state.failures || []).forEach(function (x) { if (x.at_ms <= r.seen_ms) f = x; });
      var key = f ? f.host + ' ' + f.what + ' at ' + secs(f.at_ms) : 'no failure before it';
      var g = out[key] || (out[key] = { count: 0, max: 0 });
      g.count++; g.max = Math.max(g.max, r.by_ms);
    });
    return out;
  }

  function headcount() {
    var root = document.getElementById('headcount');
    root.textContent = '';
    root.appendChild(el('h3', { text: 'Headcount' }));
    if (!frame || !frame.tick) { root.appendChild(el('div', { class: 'hint', text: 'The flock is released once both primaries serve every cell.' })); return; }
    var c = frame.counts, pop = frame.flying + frame.frozen;
    root.appendChild(el('div', { class: 'verdict ' + (c.lost ? 'bad' : 'good'),
      text: c.lost ? n(c.lost) + ' birds lost' : 'No bird lost' }));
    root.appendChild(el('div', { class: 'stats' }, [
      stat('in the sky', n(pop)), stat('flying', n(frame.flying), 'ok'), stat('frozen in holes', n(frame.frozen), frame.frozen ? 'warn' : ''),
      stat('born', n(c.births)), stat('came back older', n(c.rewound), c.rewound ? 'warn' : ''), stat('ghost copies cleared', n(c.ghosts)),
      stat('lost', n(c.lost), c.lost ? 'alert' : ''), stat('stale epochs refused', n(c.stale), c.stale ? 'alert' : ''),
      stat('cells served twice', n(c.double), c.double ? 'alert' : '')
    ]));
    var rw = rewindsByFailure(), rk = Object.keys(rw);
    if (rk.length) {
      root.appendChild(el('div', { class: 'losses' }, rk.map(function (k) {
        var g = rw[k];
        return el('div', { text: g.count + ' came back older after ' + k + ': up to ' + secs(g.max) + ' back in time' });
      })));
    }
    var groups = lossesByFailure(), keys = Object.keys(groups);
    if (keys.length) {
      root.appendChild(el('div', { class: 'losses' }, keys.map(function (k) {
        var g = groups[k];
        return el('div', { text: g.count + ' lost after ' + k + ': birds that flew into its cells after its last write reached the replica' });
      })));
    } else {
      root.appendChild(el('div', { class: 'hint', text: 'A bird is lost only if it is in no cell and the sky has no holes left.' }));
    }
  }

  function timeline() {
    var root = document.getElementById('timeline');
    root.textContent = '';
    state.timeline.slice(0, 120).forEach(function (e) {
      root.appendChild(el('li', {}, [el('span', { class: 't', text: secs(e.at_ms) }), el('span', { class: 'k ' + e.kind, text: e.kind }), el('span', { text: e.text })]));
    });
  }

  function renderPanels() { if (!state) return; hosts(); headcount(); timeline(); }

  function loadFrame() {
    fetch('/_frame.json', { credentials: 'same-origin' }).then(function (r) { return r.json(); }).then(function (f) {
      if (frame && f.tick === frame.tick) return;
      prev = curr; curr = {};
      f.boids.forEach(function (b) { curr[b[0]] = [b[1], b[2]]; });
      frame = f; arrived = performance.now();
      readout();
    }).catch(function () {});
  }
  function loadState() {
    if (busy) return;
    fetch('/_state.json', { credentials: 'same-origin' }).then(function (r) { return r.json(); }).then(function (s) {
      state = s; renderPanels();
    }).catch(function () {});
  }

  document.addEventListener('click', function (e) {
    var b = e.target.closest('[data-post]');
    if (!b || b.hasAttribute('disabled')) return;
    var c = b.getAttribute('data-confirm');
    if (c && !window.confirm(c)) return;
    b.setAttribute('disabled', ''); busy = true;
    fetch(b.getAttribute('data-post'), { method: 'POST', headers: { 'x-csrf-token': token }, credentials: 'same-origin' })
      .then(function (r) { return r.json().then(function (body) { if (!r.ok) throw new Error(body.detail || body.title || ('HTTP ' + r.status)); return body; }); })
      .then(function (s) { state = s; renderPanels(); })
      .catch(function (err) { var t = el('div', { class: 'err', text: err.message }); b.parentNode.appendChild(t); setTimeout(function () { t.remove(); }, 5000); })
      .finally(function () { busy = false; b.removeAttribute('disabled'); loadState(); });
  });

  window.addEventListener('resize', size);
  size();
  setInterval(loadFrame, TICK);
  setInterval(loadState, 500);
  loadFrame(); loadState();
  requestAnimationFrame(draw);
})();
