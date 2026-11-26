// Port of tests/e2e_dashboard.py (pywebview, evaluate_js) to the real Rust dashboard. Runs in the page; the test
// (dashboard_legacy.rs) answers the stage() calls by editing the sandbox like the Python driver thread did.
const accs = () => [...document.querySelectorAll('.acc')];
const geo = () => { const m = document.querySelector('main'), d = document.documentElement; return { a: m.scrollHeight, b: m.clientHeight, c: d.scrollHeight, d: innerHeight, hs: d.scrollWidth > innerWidth }; };
const noScroll = g => g.a <= g.b + 1 && g.c <= g.d + 1 && !g.hs;

const readyAccounts = await waitFor(() => accs().length === 4, 8000);
check('startup populated all four accounts', readyAccounts, accs().length);
await stage('ready');                                   // the test measures spawn -> populated page
await sleep(1800);

check('4 accounts rendered', accs().length === 4, accs().length);
const spin = document.querySelectorAll('.acc .spin');
check('spinner shown for the account being fetched', spin.length === 1 && spin[0].parentElement.textContent.includes('claude-da'));
check("footer shows 'updating' state while busy", $('stat').textContent.includes('đang cập nhật'));
check('refresh button stays clickable while the app polls in the background', $('rbtn').disabled === false);
const bt = document.body.innerText;
check('429 account shows cached value + warning + countdown', bt.includes('dữ liệu cũ') && bt.includes('thử lại sau'));
let g = geo();
check('NO scrollbar needed', noScroll(g), JSON.stringify(g));
check('default width = initial_width for 4 profiles (760)', Math.abs(innerWidth - 760) <= 2, innerWidth);

window.__AVAIL = 740; lastFit = 0; fit(); await sleep(1200);          // simulate a 768px-high laptop screen
g = geo(); const need = natural();
check('small screen: compact mode keeps everything visible without scrolling',
  document.body.classList.contains('compact') && need <= 740 - 24 + 4 && g.c <= g.d + 1, JSON.stringify({ need, g }));
// Exercise a specific taller work area, not the hosted runner's physical desktop.
window.__AVAIL = 1080; lastFit = 0; fit(); await sleep(1000);
check('normal screen: compact mode is off again', !document.body.classList.contains('compact'));
window.__AVAIL = 0; lastFit = 0; fit();

await stage('idle1');                                   // fetch finished (test rewrites the cache without busy)
await sleep(2600);
check('spinner gone and button enabled when idle', document.querySelectorAll('.acc .spin').length === 0 && $('rbtn').disabled === false);

// no flicker: polls that bring nothing new must not rebuild the list (that restarts the spinner/pulse animations)
document.querySelector('.acc').__mark = 1; await sleep(4500);
check('account list is not rebuilt when nothing changed (no flicker)', document.querySelector('.acc').__mark === 1);

// refresh feedback
refresh(); await sleep(600);
check("click refresh -> instant 'requesting' feedback", $('rbtn').textContent.includes('đang') && $('rbtn').disabled === true);
await stage('refresh_result');                          // test: refresh.flag exists; cache gets busy + refresh result
await sleep(1500);
check('refresh result toast shown (what started / skipped)', $('toast').textContent.includes('đang refresh 1 account'), $('toast').textContent);
await stage('idle2'); await sleep(2500);

// the (faded) toast sits over refresh/save: it must never swallow clicks
toast('widget đã mở khoá: kéo để di chuyển, ổ khoá để khoá lại', 200); await sleep(600);
const covered = [...document.querySelectorAll('button')].filter(b => b.offsetParent).filter(b => {
  const r = b.getBoundingClientRect();
  return [[.5, .5], [.15, .25], [.85, .25], [.15, .75], [.85, .75]].some(([fx, fy]) => {
    const e = document.elementFromPoint(r.left + r.width * fx, r.top + r.height * fy);
    return !(e === b || b.contains(e)); });
}).map(b => b.textContent.trim());
check('no button covered by toast / overlay (clicks land first time)', covered.length === 0, JSON.stringify(covered));
check('toast has pointer-events:none', getComputedStyle($('toast')).pointerEvents === 'none');

// notification settings persist
$('nen').checked = false; $('nwarn').checked = false; $('nblock').checked = true; await save(); await sleep(400);
let c = await cfgNow();
check('notification toggles saved (master off, warn off, block on)', c.notify_enabled === false && c.notify_warn === false && c.notify_block === true);
$('nen').checked = true; $('nwarn').checked = true; await save(); await sleep(400);
c = await cfgNow(); check('notification toggles saved (back on)', c.notify_enabled && c.notify_warn);
await stage('pre_notify');                              // test removes notify_test.flag
await testNotify(); await sleep(400);
await stage('notify_flag');                             // test: flag exists again
$('wstyle').value = 'cards'; await save(); await sleep(400);
check('widget style selector saved', (await cfgNow()).widget_style === 'cards');
$('wstyle').value = 'rings'; await save(); await sleep(400);

// appearance tab: hidden by default, auto-applies, per-account names + visibility
check('appearance tab is not shown by default (general tab is)', $('look').hidden === true && $('gen').hidden === false);
check('exactly 2 tabs in the settings box (general + widget look)', document.querySelectorAll('.tabs .tab').length === 2);
const hClosed = document.documentElement.scrollHeight;
tab('look'); await sleep(1200);
g = geo(); const rows = document.querySelectorAll('#acclabels .al').length;
check('appearance tab has one row per account and needs NO scrollbar', rows === 4 && noScroll(g), JSON.stringify({ rows, g }));
setv('wlabel', 'short'); setv('wfont', '125'); setv('wopac', '80'); setv('wtheme', 'light'); tick($('wplate'), true);
const names = [...document.querySelectorAll('#acclabels input[data-n]')]; const firstName = names[0].dataset.n;
names[0].value = 'Chính'; names[0].dispatchEvent(new Event('input', { bubbles: true }));
const shows = [...document.querySelectorAll('#acclabels input[data-show]')]; const lastName = shows[3].dataset.show; tick(shows[3], false);
await sleep(150);
c = await cfgNow();
check('auto-save is debounced: nothing written 150 ms after the edit', c.widget_font !== 125, c.widget_font);
await sleep(1400);                                      // no save button pressed: must auto-apply
c = await cfgNow();
check('changes auto-apply without pressing save', c.widget_label === 'short' && c.widget_font === 125 && c.widget_opacity === 80 && c.widget_theme === 'light' && c.widget_plate === true,
  JSON.stringify({ l: c.widget_label, f: c.widget_font, o: c.widget_opacity, t: c.widget_theme, p: c.widget_plate }));
check('custom account name saved, blank ones stay automatic', JSON.stringify(c.aliases) === JSON.stringify({ [firstName]: 'Chính' }), JSON.stringify(c.aliases));
check('unticked account is hidden from the widget', JSON.stringify(c.widget_hidden) === JSON.stringify([lastName]), JSON.stringify(c.widget_hidden));
setv('wsort', 'usage', 'change'); setv('wmax', '7', 'change'); await sleep(1500);
c = await cfgNow();
check('widget sort order and max-accounts cap are saved', c.widget_sort === 'usage' && c.widget_max_cells === 7, `${c.widget_sort} / ${c.widget_max_cells}`);
setv('wsort', 'config', 'change'); setv('wmax', '0', 'change'); await sleep(1200);

// what-to-show checkboxes
const flags = ['widget_show_label', 'widget_show_5h', 'widget_show_7d', 'widget_show_reset5', 'widget_show_reset7', 'widget_tooltip'];
c = await cfgNow();
check("all 'what to show' boxes start ticked and are saved as True", flags.every(k => c[k] === true), JSON.stringify(flags.map(k => c[k])));
tick($('sl'), false); tick($('s7'), false); tick($('r5'), false); tick($('wtip'), false); await sleep(1500);
c = await cfgNow();
check('unticking hides exactly those items', JSON.stringify(flags.map(k => c[k])) === JSON.stringify([false, true, false, false, true, false]), JSON.stringify(flags.map(k => c[k])));
check('reset master stays on while one reset line is still shown', c.widget_reset === true);
tick($('r7'), false); await sleep(1200);
check('both reset lines off -> master reset flag off', (await cfgNow()).widget_reset === false);
['sl', 's7', 'r5', 'r7', 'wtip'].forEach(i => tick($(i), true)); await sleep(1500);
c = await cfgNow(); check('everything can be ticked back on', flags.every(k => c[k] === true) && c.widget_reset === true);
const desk = accs().find(e => e.dataset.n === 'default (desktop)');
const tt = desk ? desk.title : '';
check('hovering an account row shows full details (5h, 7d, per-model, credit)',
  ['5 giờ', '7 ngày', 'Fable', 'Credit dùng thêm: $0.98 / $25.00'].every(s => tt.includes(s)), tt.replace(/\n/g, ' | ').slice(0, 160));
for (const avail of [1080, 900, 768]) {                 // laptop screens with the appearance section open
  window.__AVAIL = avail; lastFit = 0; fit(); await sleep(1000);
  const nd = natural(), doc = document.documentElement.scrollHeight, inner = innerHeight;
  check(`screen ${avail}px high + appearance open: window fits the screen`, nd <= avail - 24 + 4 && doc <= inner + 1, JSON.stringify({ nd, doc, inner, compact: document.body.classList.contains('compact') }));
}
window.__AVAIL = 0; lastFit = 0; fit(); await sleep(800);
tab('gen'); await sleep(1000);
const hNow = document.documentElement.scrollHeight;
check('switching back to the general tab restores its height', Math.abs(hNow - hClosed) < 15, `before=${hClosed} after=${hNow} inner=${innerHeight}`);
setv('wlabel', 'alias'); setv('wfont', '100'); setv('wopac', '100'); setv('wtheme', 'auto'); tick($('wplate'), false);
document.querySelectorAll('#acclabels input[data-n]')[0].value = ''; tick(document.querySelectorAll('#acclabels input[data-show]')[3], true); await sleep(1500);
c = await cfgNow();
check('everything can be set back to defaults', c.widget_label === 'alias' && Object.keys(c.aliases).length === 0 && c.widget_hidden.length === 0 && c.widget_plate === false, JSON.stringify({ a: c.aliases, h: c.widget_hidden }));

// interval floor
$('interval').value = 10; await save(); await sleep(500);
check('interval below 90 is clamped to 90', (await cfgNow()).interval_sec === 90);

// keyboard shortcut r: ignored while typing, refreshes otherwise
await stage('pre_r');                                   // test removes refresh.flag
$('interval').dispatchEvent(new KeyboardEvent('keydown', { key: 'r', bubbles: true })); await sleep(400);
check("'r' typed in an input does not refresh", $('rbtn').disabled === false);
await stage('r_in_input');                              // test: no refresh.flag
document.dispatchEvent(new KeyboardEvent('keydown', { key: 'r', bubbles: true })); await sleep(500);
check("'r' key refreshes (button shows the request)", $('rbtn').disabled === true && $('rbtn').textContent.includes('đang'));
await stage('r_key');                                   // test: refresh.flag exists

// hook buttons (sandbox profiles only)
await hook('install'); await sleep(1400);
check('hook status text shows 4/4', $('hookst').textContent.includes('4/4'), $('hookst').textContent);
await stage('hook_installed');
await hook('uninstall'); await sleep(1400);
check('hook status text shows 0/4 after uninstall', $('hookst').textContent.includes('0/4'), $('hookst').textContent);
await stage('hook_uninstalled');

// widget buttons
document.querySelectorAll('button.b')[1].click(); await sleep(600);
check('unlock widget button', (await cfgNow()).widget_locked === false);
document.querySelectorAll('button.b')[2].click(); await sleep(600);
c = await cfgNow(); check('reset widget button', c.widget_locked === true && c.widget_cx === null && c.widget_size === 36);

// title bar: a mousemove WITHOUT the primary button pressed must not start a window drag (the real drag is a native
// modal move loop, which synthetic events cannot drive); the window must stay where it is
const r0 = await api.rect(); const dr = document.querySelector('.drag'), br = dr.getBoundingClientRect();
dr.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, clientX: br.left + 60, clientY: br.top + 10, screenX: 500, screenY: 400, button: 0 }));
for (let i = 1; i <= 4; i++) { document.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, screenX: 500 + i * 30, screenY: 400 + i * 20, buttons: 0 })); await sleep(100); }
document.dispatchEvent(new MouseEvent('mouseup', { bubbles: true })); await sleep(300);
const r1 = await api.rect();
check('mouse move without button pressed does not drag the window', JSON.stringify(r0) === JSON.stringify(r1), `${r0} -> ${r1}`);
check('window stays responsive after the simulated drag', (await api.win_state()) !== null);

// minimize button (the test checks the window is iconic, then hides it again)
document.querySelector('.wb').click(); await sleep(800);
await stage('minimized');
// closing right after a change still saves it: the pending auto-save is flushed first, then the process ends
await stage('before_close');
setv('wfont', '135');
document.querySelector('.wb.x').click();                // closeDash(): flush save, hide(), TerminateProcess
await sleep(8000);
return { survived: true };
