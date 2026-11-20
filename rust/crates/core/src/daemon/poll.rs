//! Scheduler: seed from cache, poll one profile with backoff, manual refresh, config change.

use super::{py_str, Env, Poller};
use crate::config::{profiles, Config, Profile};
use crate::fmt::effective_interval;
use crate::pyval::{fixed, py_num, truthy};
use serde_json::{json, Map, Value};

impl<E: Env> Poller<E> {
    /// `App.seed`: start from the cache so a restart never causes an API burst; no cache -> stagger 4 s.
    /// Deviation: a profile entry whose `fetched_at`/`retry_at` is not a number is treated as no data
    /// (Python would raise).
    pub fn seed(&mut self, cache: Option<&Value>) {
        let now = self.env.now();
        let iv = effective_interval(&self.cfg) as f64;
        for (i, p) in profiles(&self.cfg).iter().enumerate() {
            let old = cache
                .and_then(|c| c.get("profiles"))
                .and_then(|ps| ps.get(&p.name))
                .and_then(Value::as_object);
            let seeded = old.and_then(|o| {
                if !o.contains_key("five") {
                    return None;
                }
                let fa = o.get("fetched_at").filter(|v| truthy(v)).and_then(py_num)?;
                let retry = o
                    .get("retry_at")
                    .filter(|v| truthy(v))
                    .and_then(py_num)
                    .unwrap_or(0.0);
                Some((o, fa, retry))
            });
            match seeded {
                Some((o, fa, retry)) => {
                    let d: Map<String, Value> = o
                        .iter()
                        .filter(|(k, _)| k.as_str() != "path")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    self.data.insert(p.name.clone(), Value::Object(d));
                    self.next_at.insert(p.name.clone(), (fa + iv).max(retry));
                }
                None => {
                    self.next_at.insert(p.name.clone(), now + i as f64 * 4.0);
                }
            }
        }
    }

    /// The config-reload branch of `App.loop`: the new interval applies to the next fetch.
    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
        let iv = effective_interval(&self.cfg) as f64;
        for (n, d) in &self.data {
            let Some(o) = d.as_object() else { continue };
            if let Some(fa) = o.get("fetched_at") {
                let fa = py_num(fa).unwrap_or(0.0);
                let retry = o.get("retry_at").and_then(py_num).unwrap_or(0.0);
                self.next_at.insert(n.clone(), (fa + iv).max(retry));
            }
        }
    }

    /// `(refresh_if_expired(p) if cfg.token_refresh else None) or fetch(p)`.
    fn run_fetch(&mut self, p: &Profile) -> Map<String, Value> {
        let token_refresh = self.cfg.get("token_refresh").is_none_or(truthy);
        if token_refresh {
            if let Some(r) = self.env.refresh_if_expired(p) {
                if !r.is_empty() {
                    return r;
                }
            }
        }
        self.env.fetch(p)
    }

    /// `App.poll_one`: busy is written BEFORE the request; failures keep the last numbers and back off.
    pub fn poll_one(&mut self, p: &Profile) {
        let name = p.name.clone();
        let iv = effective_interval(&self.cfg) as f64;
        self.busy.insert(name.clone());
        self.write();
        self.refresh_icon();
        let r = self.run_fetch(p);
        self.busy.remove(&name);
        let now = self.env.now();
        let prev: Map<String, Value> = self
            .data
            .get(&name)
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "err" | "status" | "retry_at"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if r.contains_key("five") {
            self.fails.insert(name.clone(), 0);
            let mut d = r;
            d.insert("fetched_at".into(), Value::from(now));
            self.data.insert(name.clone(), Value::Object(d));
            let j = 1.0 + self.env.rand() * 0.1;
            self.next_at.insert(name, now + iv * j);
        } else {
            let n = self.fails.get(&name).copied().unwrap_or(0) + 1;
            self.fails.insert(name.clone(), n);
            let err = r.get("err").cloned().unwrap_or(Value::Null);
            let status = r.get("status").cloned().unwrap_or(Value::Null);
            let st = py_num(&status);
            let retry_after = r.get("retry_after").filter(|v| truthy(v)).and_then(py_num);
            let pow2 = 2f64.powi(i32::try_from(n - 1).unwrap_or(i32::MAX));
            let mut wait = if st == Some(429.0) {
                retry_after.unwrap_or_else(|| 900f64.min(120.0 * pow2))
            } else if st == Some(0.0) && retry_after.is_some() {
                retry_after.unwrap_or(0.0) // claude itself is refreshing this token right now
            } else if st == Some(401.0) || py_str(&err).contains("credentials") {
                iv.max(300.0) // token expired / login needed: retry slowly
            } else {
                600f64.min(iv * pow2)
            };
            wait = wait.max(30.0) * (1.0 + self.env.rand() * 0.1);
            let line = format!(
                "fetch {name}: {} (fail #{n}), retry in {}s",
                py_str(&err),
                fixed(wait, 0)
            );
            self.env.log(&line);
            let mut d = prev;
            d.insert("err".into(), err);
            d.insert("status".into(), status);
            d.insert("retry_at".into(), Value::from(now + wait));
            self.data.insert(name.clone(), Value::Object(d));
            self.next_at.insert(name, now + wait);
        }
    }

    /// `App.refresh_requested`: manual refresh, never faster than 20 s per profile and never before a 429
    /// retry window ends; records started/skipped (Vietnamese reasons) for the UI.
    pub fn refresh_requested(&mut self) {
        let now = self.env.now();
        let mut started: Vec<Value> = Vec::new();
        let mut skipped: Vec<Value> = Vec::new();
        for (i, p) in profiles(&self.cfg).iter().enumerate() {
            let d = self.data.get(&p.name).and_then(Value::as_object);
            let num = |k: &str| d.and_then(|o| o.get(k)).and_then(py_num).unwrap_or(0.0);
            let fetched_at = num("fetched_at");
            let retry_at = num("retry_at");
            if now - fetched_at < 20.0 {
                skipped.push(json!({"name": p.name, "why": "vừa cập nhật xong"}));
            } else if now < retry_at {
                let secs = (retry_at - now).trunc() as i64;
                skipped.push(json!({
                    "name": p.name,
                    "why": format!("đang chờ giới hạn API, thử lại sau {secs}s")
                }));
            } else {
                let cur = self.next_at.get(&p.name).copied().unwrap_or(now);
                self.next_at
                    .insert(p.name.clone(), cur.min(now + i as f64 * 2.0));
                self.busy.insert(p.name.clone());
                started.push(Value::String(p.name.clone()));
            }
        }
        self.refresh_info = Some(json!({"ts": now, "started": started, "skipped": skipped}));
        self.write();
        self.refresh_icon();
    }
}
