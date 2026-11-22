// Port of tests/e2e_responsive.py: 14 profiles, resizing from the 8 edge handles with the page's own pointer logic,
// maximize / restore / double-click, remembered size. PHASE ('1' | '2') and N are defined by the test.
const dpr = devicePixelRatio;
const sh = screen.height * dpr;
const info = () => {
  const m = document.querySelector('main'), d = document.documentElement, a = [...document.querySelectorAll('.acc')];
  return { iw: innerWidth, ih: innerHeight, hs: d.scrollWidth > innerWidth + 1, ms: m.scrollHeight, mc: m.clientHeight,
    cols: new Set(a.map(x => Math.round(x.getBoundingClientRect().left))).size,
    setCols: getComputedStyle($('gen')).gridTemplateColumns.split(' ').length,
    user: userSized, maxed: maxed, compact: document.body.classList.contains('compact'), n: a.length };
};
const near = (a, b, t) => a.every((v, i) => Math.abs(v - b[i]) <= t);
// real pointer events on the page's own edge handle (startResize -> api.rect() -> set_rect)
const drag = async (edge, dx, dy) => {
  const h = document.querySelector('.rz.' + edge);
  h.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 1, screenX: 1000, screenY: 700 })); await sleep(500);
  window.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, pointerId: 1, screenX: 1000 + dx / dpr, screenY: 700 + dy / dpr })); await sleep(400);
  window.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, pointerId: 1 })); await sleep(900);
};

await waitFor(() => document.querySelectorAll('.acc').length === N, 8000);
await sleep(1800);

if (PHASE === '1') {
  let i = info();
  check(`${N} profiles are all listed`, i.n === N, i.n);
  check('the 8 resize handles exist', ['l', 'r', 't', 'b', 'tl', 'tr', 'bl', 'br'].every(e => document.querySelector('.rz.' + e)));
  const want = Math.min(1480, screen.width - 80);
  check(`default window width = initial_width (${N} profiles -> ${want})`, Math.abs(i.iw - want) <= 2, i.iw);
  check('default window is wide for many accounts and shows several columns', i.iw >= 1100 && i.cols >= 3, `width=${i.iw} columns=${i.cols}`);
  check('auto-fit: nothing to scroll, window fits the screen', i.ms <= i.mc + 1 && !i.hs && (await api.rect())[3] <= sh, JSON.stringify(i) + ' rect=' + (await api.rect()));
  const r0 = await api.rect();
  await drag('br', -(r0[2] - 760 * dpr), -(r0[3] - 520 * dpr));
  let r1 = await api.rect(); i = info();
  check('dragging the bottom-right corner shrinks the window', Math.abs(r1[2] - 760 * dpr) < 4 && Math.abs(r1[3] - 520 * dpr) < 4 && r1[0] === r0[0] && r1[1] === r0[1], `${r0} -> ${r1}`);
  await sleep(1600);
  check('a user-chosen size is kept (no auto-fit fighting it)', JSON.stringify(await api.rect()) === JSON.stringify(r1) && i.user === true, String(await api.rect()));
  check('narrower window -> fewer columns, no horizontal scrolling', i.cols < 3 && !i.hs, `${i.cols} columns, hs=${i.hs}`);
  check(`short window with ${N} accounts scrolls inside (content taller than the window, page itself doesn't)`, i.ms > i.mc + 20 && !i.hs, `main ${i.ms}/${i.mc}`);
  await drag('l', 300 * dpr, 0); await drag('t', 0, 120 * dpr);
  const r2 = await api.rect();
  check('left and top edges resize too (opposite edge stays put)', r2[0] + r2[2] === r1[0] + r1[2] && r2[1] + r2[3] === r1[1] + r1[3] && r2[2] < r1[2] && r2[3] < r1[3], `${r1} -> ${r2}`);
  // right and bottom edges, top-right and bottom-left corners (the old test only did br, l, t, tl)
  await drag('r', 80 * dpr, 0);
  const r2r = await api.rect();
  check('right edge grows the window to the right (left edge stays put)', r2r[0] === r2[0] && Math.abs(r2r[2] - (r2[2] + 80 * dpr)) <= 2 && r2r[3] === r2[3], `${r2} -> ${r2r}`);
  await drag('b', 0, 60 * dpr);
  const r2b = await api.rect();
  check('bottom edge grows the window downward (top edge stays put)', r2b[1] === r2r[1] && Math.abs(r2b[3] - (r2r[3] + 60 * dpr)) <= 2 && r2b[2] === r2r[2], `${r2r} -> ${r2b}`);
  await drag('tr', 40 * dpr, 30 * dpr);
  const r2t = await api.rect();
  check('top-right corner: width grows, top edge moves down, left+bottom stay put', r2t[0] === r2b[0] && r2t[1] + r2t[3] === r2b[1] + r2b[3] && r2t[2] > r2b[2] && r2t[3] < r2b[3], `${r2b} -> ${r2t}`);
  await drag('bl', -30 * dpr, 40 * dpr);
  const r2l = await api.rect();
  check('bottom-left corner: width grows to the left, height grows, right+top stay put', r2l[0] + r2l[2] === r2t[0] + r2t[2] && r2l[1] === r2t[1] && r2l[2] > r2t[2] && r2l[3] > r2t[3], `${r2t} -> ${r2l}`);
  await drag('tl', 5000 * dpr, 5000 * dpr);
  const r3 = await api.rect(); i = info();
  check('cannot shrink below the minimum size; layout collapses to ONE column and nothing overflows',
    r3[2] >= 420 * dpr - 2 && r3[3] >= 330 * dpr - 2 && i.cols === 1 && !i.hs && i.setCols <= 2, `rect=${r3} ${JSON.stringify(i)}`);
  check('minimum size is exactly 420x330 (CSS px)', Math.abs(r3[2] - 420 * dpr) <= 2 && Math.abs(r3[3] - 330 * dpr) <= 2, String(r3));
  check('one-column layout at the minimum width: breakpoints 640/560/460 active (settings grid has <=2 columns)',
    i.iw <= 460 && getComputedStyle($('accs')).gridTemplateColumns.split(' ').length === 1, JSON.stringify(i));
  // CSS breakpoints 640 / 560 / 460, one case just above and one at the threshold (media queries use the viewport
  // width: width W CSS px = window W*dpr physical px, the frameless window has no border)
  const cnt = el => getComputedStyle(el).gridTemplateColumns.trim().split(/\s+/).length;
  const atWidth = async w => { await api.set_rect(150, 90, Math.round(w * dpr), Math.round(640 * dpr)); await sleep(600); return innerWidth; };
  const bp = [
    ['640', 641, 640, () => cnt($('gen')), 6, 2, 'settings grid columns (6 -> 2)'],
    ['560', 561, 560, () => cnt(document.querySelector('.acc')), 3, 2, 'account row columns (3 -> 2)'],
    ['460', 461, 460, () => parseFloat(getComputedStyle(document.querySelector('.wb')).width), 34, 30, 'window-button width px (34 -> 30)'],
  ];
  for (const [name, above, at, read, vAbove, vAt, what] of bp) {
    const w1 = await atWidth(above), a = read();
    const w2 = await atWidth(at), b = read();
    check(`breakpoint ${name}px: ${what}: ${above}px wide -> ${vAbove}, ${at}px wide -> ${vAt}`,
      Math.abs(w1 - above) <= 1 && Math.abs(w2 - at) <= 1 && a === vAbove && b === vAt, `innerWidth ${w1}/${w2}, got ${a}/${b}`);
  }
  const w3 = await atWidth(430);
  check('below 460px the settings labels span the full row (2 breakpoints stacked) and nothing scrolls sideways',
    Math.abs(w3 - 430) <= 1 && !info().hs && cnt($('gen')) === 2, `innerWidth ${w3} ${JSON.stringify(info())}`);
  await api.set_rect(150, 90, 900 * dpr, 640 * dpr); await sleep(800);

  // maximize / restore
  const before = await api.rect(); await toggleMax(); await sleep(1200);
  const wa = await api.work_area(), mx = await api.rect(); i = info();
  check("maximize fills the monitor's work area (taskbar stays visible)", near(mx, wa, 2) && i.maxed === true && $('maxb').textContent === '❐', `${mx} vs work area ${wa}`);
  const wantCols = wa[2] / dpr >= 1200 ? 3 : 2;
  check('maximized window shows more columns', i.cols >= wantCols && !i.hs, `${i.cols} columns`);
  await toggleMax(); await sleep(1000); const rr = await api.rect();
  check('restore returns to the previous size and position', JSON.stringify(rr) === JSON.stringify(before) && info().maxed === false, `${before} -> ${rr}`);
  const tb = document.querySelector('.drag');
  tb.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); await sleep(1000);
  check('double-clicking the title bar maximizes', near(await api.rect(), wa, 2));
  tb.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); await sleep(1000);
  check('double-clicking again restores', JSON.stringify(await api.rect()) === JSON.stringify(before) && info().maxed === false, String(await api.rect()));
  // maximized: the edge handles must do nothing
  await toggleMax(); await sleep(1000);
  const mxr = await api.rect();
  await drag('br', -300 * dpr, -300 * dpr);
  check('while maximized the edge handles do not resize', JSON.stringify(await api.rect()) === JSON.stringify(mxr) && info().maxed === true, `${mxr} -> ${await api.rect()}`);
  await toggleMax(); await sleep(1000);

  // the host clamps absurd sizes itself (not only the page's edge handles)
  await api.set_rect(150, 90, 100 * dpr, 100 * dpr); await sleep(400);
  const tiny = await api.rect();
  check('set_rect never goes below the minimum size (420x330)', tiny[2] >= 420 - 1 && tiny[3] >= 330 - 1, String(tiny));

  // remember the size
  await api.set_rect(120, 80, 980 * dpr, 600 * dpr); await sleep(1600);
  const saved = (await cfgNow()).dash_rect;
  check('the chosen size/position is written to config', Array.isArray(saved) && Math.abs(saved[2] - 980 * dpr) <= 3 && Math.abs(saved[3] - 600 * dpr) <= 3, JSON.stringify(saved));
  await stage('phase1_done');
  document.querySelector('.wb.x').click(); await sleep(8000);
  return { survived: true };
} else {
  const rect = await api.rect(); const saved = (await cfgNow()).dash_rect; let i = info();
  check('2nd launch reopens at the remembered size', Array.isArray(saved) && near(rect, saved, 3) && i.user === true, `saved=${saved} now=${rect}`);
  check("'tự vừa' button is visible once a size is remembered", $('autofit').style.display !== 'none');
  autoFit(); await sleep(2000);
  i = info(); const c = await cfgNow();
  check("'tự vừa' forgets the size: window follows the content again", c.dash_rect === null && i.user === false && i.ms <= i.mc + 1, `${JSON.stringify(i)} dash_rect=${c.dash_rect}`);
  check("'tự vừa' button hidden again", $('autofit').style.display === 'none');
  await stage('phase2_done');
  document.querySelector('.wb.x').click(); await sleep(8000);
  return { survived: true };
}
