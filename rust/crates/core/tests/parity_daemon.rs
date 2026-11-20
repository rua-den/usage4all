//! Replays tests/fixtures/daemon.json (made by running the real Python `app.App`, tools/gen_fixtures_daemon.py)
//! against `daemon::Poller` with a scripted `Env`: every step must give the same ordered side effects and state.

use cu_core::cache::build_cache;
use cu_core::config::{Config, Profile};
use cu_core::daemon::{Env, FetchResult, Poller, TickInput, Toast, TrayView};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Default)]
struct Shared {
    t: f64,
    rand: Vec<f64>,
    used: usize,
    fq: VecDeque<Value>,
    rq: VecDeque<Value>,
    default_fetch: Value,
    ev: Vec<Value>,
}

type S = Rc<RefCell<Shared>>;

struct FakeEnv(S);

fn pop(s: &mut Shared, is_fetch: bool, name: &str) -> Option<FetchResult> {
    s.ev.push(json!([if is_fetch { "fetch" } else { "refresh" }, name]));
    let q = if is_fetch { &mut s.fq } else { &mut s.rq };
    if q.front().and_then(|e| e["name"].as_str()) == Some(name) {
        let e = q.pop_front().unwrap();
        s.t += e.get("advance").and_then(Value::as_f64).unwrap_or(0.0);
        return e["res"].as_object().cloned();
    }
    if is_fetch {
        let d = s.default_fetch.as_object().cloned();
        assert!(d.is_some(), "unscripted fetch for {name}");
        return d;
    }
    None
}

impl Env for FakeEnv {
    fn now(&self) -> f64 {
        self.0.borrow().t
    }
    fn rand(&mut self) -> f64 {
        let mut s = self.0.borrow_mut();
        let v = s.rand[s.used % s.rand.len()];
        s.used += 1;
        v
    }
    fn refresh_if_expired(&mut self, p: &Profile) -> Option<FetchResult> {
        pop(&mut self.0.borrow_mut(), false, &p.name)
    }
    fn fetch(&mut self, p: &Profile) -> FetchResult {
        pop(&mut self.0.borrow_mut(), true, &p.name).expect("fetch result must be an object")
    }
    fn write_cache(
        &mut self,
        cfg: &Config,
        data: &Map<String, Value>,
        busy: &[String],
        refresh: Option<Value>,
    ) {
        let mut m = build_cache(cfg, data, busy, refresh, 0.0);
        m.remove("ts");
        self.0.borrow_mut().ev.push(json!(["write", m]));
    }
    fn refresh_icon(&mut self, v: &TrayView) {
        self.0.borrow_mut().ev.push(json!([
            "icon", v.tooltip, v.pct, v.color, v.text, v.bar_right, v.busy
        ]));
    }
    fn log(&mut self, msg: &str) {
        self.0.borrow_mut().ev.push(json!(["log", msg]));
    }
    fn save_alert_state(&mut self, st: &Map<String, Value>) {
        self.0.borrow_mut().ev.push(json!(["alerts_save", st]));
    }
    fn send_toast(&mut self, t: &Toast) -> Result<(), String> {
        self.0.borrow_mut().ev.push(json!(["toast", t.title, t.body]));
        Ok(())
    }
}

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-6_f64.max(1e-9 * a.abs().max(b.abs()))
}

/// JSON equality: numbers within tolerance, object key order ignored.
fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => close(x.as_f64().unwrap(), y.as_f64().unwrap()),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, w)))
        }
        _ => a == b,
    }
}

fn map_of<V: Clone + Into<Value>>(it: impl Iterator<Item = (String, V)>) -> Value {
    Value::Object(it.map(|(k, v)| (k, v.into())).collect())
}

fn observe(p: &Poller<FakeEnv>, s: &S) -> Value {
    let sh = s.borrow();
    json!({
        "now": sh.t,
        "rand_used": sh.used,
        "data": p.data,
        "next_at": map_of(p.next_at.iter().map(|(k, v)| (k.clone(), *v))),
        "fails": map_of(p.fails.iter().map(|(k, v)| (k.clone(), *v))),
        "busy": p.busy.iter().collect::<Vec<_>>(),
        "refresh_info": p.refresh_info,
        "pending": p.pending.iter().map(|x| json!({"t": x.t, "lvl": x.lvl, "name": x.name, "label": x.label, "v": x.v})).collect::<Vec<_>>(),
        "last_toast": p.last_toast,
        "baseline": p.baseline,
        "alert_state": p.alert_state,
    })
}

#[test]
fn daemon_scenarios_match_python() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/daemon.json");
    let scenarios: Vec<Value> = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let mut bad = Vec::new();
    for sc in &scenarios {
        let name = sc["name"].as_str().unwrap();
        let s: S = Rc::new(RefCell::new(Shared {
            t: sc["t0"].as_f64().unwrap(),
            rand: sc["rand"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect(),
            default_fetch: sc["default_fetch"].clone(),
            ..Default::default()
        }));
        let cfg: Config = sc["cfg"].as_object().unwrap().clone();
        let alerts = sc["alerts"].as_object().cloned();
        let mut p = Poller::new(FakeEnv(s.clone()), cfg, alerts);
        'steps: for (i, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
            {
                let mut sh = s.borrow_mut();
                sh.t += st["dt"].as_f64().unwrap_or(0.0);
                sh.ev.clear();
                sh.fq.extend(st["fetch"].as_array().cloned().unwrap_or_default());
                sh.rq.extend(st["refresh"].as_array().cloned().unwrap_or_default());
            }
            match st["op"].as_str().unwrap() {
                "start" => p.start(st["cache"].as_object().map(|_| &st["cache"])),
                "tick" => p.tick(TickInput {
                    config: st["config"].as_object().cloned(),
                    test_toast: st["flag_test"].as_bool().unwrap_or(false),
                    refresh: st["flag_refresh"].as_bool().unwrap_or(false),
                }),
                "refresh" => p.refresh_requested(),
                "test_toast" => p.test_toast(),
                "flush" => p.flush_toasts(),
                "publish" => p.publish(),
                "set_data" => p.data = st["data"].as_object().unwrap().clone(),
                o => panic!("unknown op {o}"),
            }
            let ev = Value::Array(s.borrow().ev.clone());
            let state = observe(&p, &s);
            if !eq(&ev, &st["events"]) {
                bad.push(format!("{name} step {i} events:\n got  {ev}\n want {}", st["events"]));
                break 'steps;
            }
            if !eq(&state, &st["state"]) {
                bad.push(format!("{name} step {i} state:\n got  {state}\n want {}", st["state"]));
                break 'steps;
            }
        }
    }
    assert!(bad.is_empty(), "{} scenario(s) differ:\n{}", bad.len(), bad.iter().take(3).cloned().collect::<Vec<_>>().join("\n\n"));
}
