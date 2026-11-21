// Provides the pywebview bridge the page (ui.html, shared with the Python build) expects:
// window.pywebview.api.<method>(...args) -> Promise, the 'pywebviewready' event, and the title-bar drag region.
(function () {
  const pend = new Map();
  let n = 0;
  window.__cu_resolve = function (id, ok, v) {
    const p = pend.get(id);
    if (!p) return;
    pend.delete(id);
    (ok ? p.res : p.rej)(v);
  };
  const send = (m, a) => window.ipc.postMessage(JSON.stringify({ id: 0, m: m, a: a || [] }));
  window.__cu_send = send;
  const call = (m, a) =>
    new Promise((res, rej) => {
      const id = ++n;
      pend.set(id, { res: res, rej: rej });
      window.ipc.postMessage(JSON.stringify({ id: id, m: m, a: a }));
    });
  window.pywebview = { api: new Proxy({}, { get: (_, m) => (...a) => call(m, a) }) };
  window.addEventListener('error', (e) => send('__log', [String(e.message) + ' @' + e.lineno + ':' + e.colno]));
  window.addEventListener('unhandledrejection', (e) => send('__log', ['unhandled: ' + String(e.reason)]));
  document.addEventListener('DOMContentLoaded', () =>
    setTimeout(() => window.dispatchEvent(new Event('pywebviewready')), 0));
  // Drag the frameless window by .pywebview-drag-region. The native move loop starts only after the pointer has
  // moved a few pixels, so a plain double click still reaches ondblclick (maximize).
  let down = null;
  document.addEventListener('mousedown', (e) => {
    if (e.button === 0 && e.target.closest && e.target.closest('.pywebview-drag-region'))
      down = { x: e.screenX, y: e.screenY };
  });
  document.addEventListener('mousemove', (e) => {
    if (!down) return;
    if (e.buttons !== 1) { down = null; return; }
    if (Math.abs(e.screenX - down.x) + Math.abs(e.screenY - down.y) > 3) { down = null; send('__drag'); }
  });
  document.addEventListener('mouseup', () => { down = null; });
})();
