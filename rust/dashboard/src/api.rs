//! The data/actions half of `dashboard.Api` (no window access): state, save, flags, hooks, widget settings.
//! Semantics follow the Python code: `save` is `cfg.update(v)` + the interval floor + autostart, nothing else.

use cu_core::cache::load_cache;
use cu_core::config::{self, Config};
use cu_core::fmt::effective_interval;
use cu_core::hookinstall::{self, HookProfile};
use cu_core::paths::Paths;
use cu_core::pyval::{py_int, truthy};
use cu_core::MIN_INTERVAL;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Core {
    pub paths: Paths,
    pub home: PathBuf,
    /// The tray exe (`claude-usage.exe` next to this one): what hooks and autostart must point at.
    pub tray_exe: PathBuf,
    /// Registry writes allowed (release build and no `CU_NO_AUTOSTART`).
    pub autostart_ok: bool,
}

impl Core {
    pub fn cfg(&self) -> Config {
        config::load_config(&self.paths, &self.home)
    }

    fn save_cfg(&self, cfg: &Config) -> bool {
        config::save_config(&self.paths, cfg).is_ok()
    }

    /// `Api.state()`
    pub fn state(&self) -> Value {
        let cfg = self.cfg();
        let interval = effective_interval(&cfg);
        json!({"cfg": cfg, "cache": load_cache(&self.paths), "version": VERSION, "interval": interval})
    }

    /// `page()`: ui.html with the current state inlined so the numbers show the instant the window appears.
    pub fn page(&self, html: &str) -> String {
        let init = json!({"cfg": self.cfg(), "cache": load_cache(&self.paths)})
            .to_string()
            .replace("</", "<\\/");
        html.replacen("/*__INIT__*/null", &init, 1)
    }

    pub fn flag(&self, path: &std::path::Path) -> Value {
        Value::Bool(fs::write(path, "1").is_ok())
    }

    fn hook_profiles(&self) -> Vec<HookProfile> {
        config::profiles(&self.cfg())
            .iter()
            .map(|p| HookProfile::new(p.name.clone(), &p.path))
            .collect()
    }

    pub fn hook_status(&self) -> Value {
        json!(hookinstall::status(&self.hook_profiles()))
    }

    pub fn hook_install(&self) -> Value {
        json!(hookinstall::install(&self.hook_profiles(), &self.tray_exe))
    }

    pub fn hook_uninstall(&self) -> Value {
        json!(hookinstall::uninstall(&self.hook_profiles()))
    }

    pub fn widget_unlock(&self) -> Value {
        let mut cfg = self.cfg();
        cfg.insert("widget_locked".into(), Value::Bool(false));
        Value::Bool(self.save_cfg(&cfg))
    }

    pub fn widget_reset(&self) -> Value {
        let mut cfg = self.cfg();
        for (k, v) in [
            ("widget_cx", Value::Null),
            ("widget_cy", Value::Null),
            ("widget_angle", json!(0.0)),
            ("widget_size", json!(36)),
            ("widget_layout", json!("auto")),
            ("widget_locked", Value::Bool(true)),
        ] {
            cfg.insert(k.into(), v);
        }
        Value::Bool(self.save_cfg(&cfg))
    }

    /// `Api.save(v)`: false on any failure (like the blanket `except` of the Python code).
    pub fn save(&self, v: &Value) -> Value {
        let Some(patch) = v.as_object() else {
            return Value::Bool(false);
        };
        let mut cfg = self.cfg();
        for (k, val) in patch {
            cfg.insert(k.clone(), val.clone());
        }
        let Some(iv) = cfg.get("interval_sec").and_then(py_int) else {
            return Value::Bool(false);
        };
        cfg.insert("interval_sec".into(), Value::from(iv.max(MIN_INTERVAL)));
        if !self.save_cfg(&cfg) {
            return Value::Bool(false);
        }
        if self.autostart_ok {
            crate::win::set_autostart(cfg.get("autostart").is_some_and(truthy), &self.tray_exe);
        }
        Value::Bool(true)
    }

    pub fn set_dash_rect(&self, r: Option<[i32; 4]>) {
        let mut cfg = self.cfg();
        cfg.insert("dash_rect".into(), r.map_or(Value::Null, |r| json!(r)));
        self.save_cfg(&cfg);
    }

    pub fn saved_rect(&self) -> Value {
        self.cfg().get("dash_rect").cloned().unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(dir: &std::path::Path) -> Core {
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();
        Core {
            paths: Paths::with_root(dir.join("data")),
            home,
            tray_exe: dir.join("claude-usage.exe"),
            autostart_ok: false,
        }
    }

    fn read(c: &Core) -> Value {
        serde_json::from_str(&fs::read_to_string(&c.paths.config).unwrap()).unwrap()
    }

    #[test]
    fn save_updates_merges_and_floors_interval() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        fs::write(&c.paths.config, r#"{"warn_pct": 70, "custom_key": 1}"#).unwrap();
        assert_eq!(
            c.save(&json!({"interval_sec": 10, "block_pct": 99, "new": "x"})),
            true
        );
        let f = read(&c);
        assert_eq!(f["interval_sec"], 90); // floor
        assert_eq!(f["block_pct"], 99);
        assert_eq!(f["warn_pct"], 70); // untouched keys survive
        assert_eq!(f["custom_key"], 1); // unknown keys survive
        assert_eq!(f["new"], "x"); // no whitelist, like Python
        assert_eq!(f["profiles"], json!([])); // the merged config is written whole
    }

    #[test]
    fn save_failures_return_false() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        assert_eq!(c.save(&json!("not a dict")), false);
        assert_eq!(c.save(&json!({"interval_sec": null})), false);
        assert_eq!(c.save(&json!({"interval_sec": "abc"})), false);
        assert_eq!(c.save(&json!({"interval_sec": "200"})), true); // int("200") works
        assert_eq!(read(&c)["interval_sec"], 200);
    }

    #[test]
    fn widget_actions_edit_config() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        c.save(&json!({"widget_locked": true, "widget_cx": 5, "widget_size": 50, "widget_angle": 30.0, "widget_layout": "h"}));
        c.widget_unlock();
        assert_eq!(read(&c)["widget_locked"], false);
        c.widget_reset();
        let f = read(&c);
        assert_eq!(f["widget_cx"], Value::Null);
        assert_eq!(f["widget_size"], 36);
        assert_eq!(f["widget_angle"], 0.0);
        assert_eq!(f["widget_layout"], "auto");
        assert_eq!(f["widget_locked"], true);
    }

    #[test]
    fn page_inlines_state_and_escapes_script_end() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        c.save(&json!({"aliases": {"a": "</script>"}}));
        let p = c.page("x=/*__INIT__*/null; y=/*__INIT__*/null;");
        assert!(p.starts_with("x={\"cfg\":"), "{p}");
        assert!(!p.contains("</script>") && p.contains("<\\/script>"));
        assert!(p.ends_with("y=/*__INIT__*/null;")); // only the first marker
    }

    #[test]
    fn state_has_cache_and_interval() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        let s = c.state();
        assert_eq!(s["cache"], Value::Null);
        assert_eq!(s["interval"], 180);
        assert_eq!(s["version"], VERSION);
        fs::write(&c.paths.cache, r#"{"profiles": {}}"#).unwrap();
        assert_eq!(c.state()["cache"], json!({"profiles": {}}));
    }

    #[test]
    fn hooks_install_status_uninstall_in_sandbox() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        let prof = t.path().join("prof");
        fs::create_dir_all(&prof).unwrap();
        fs::write(prof.join("settings.json"), "{}").unwrap();
        c.save(&json!({"profiles": [{"name": "p1", "path": prof.to_string_lossy()}]}));
        assert_eq!(c.hook_status(), json!([["p1", false]]));
        assert_eq!(c.hook_install(), json!([["p1", "đã cài"]]));
        assert_eq!(c.hook_status(), json!([["p1", true]]));
        let s = fs::read_to_string(prof.join("settings.json")).unwrap();
        assert!(s.contains("claude-usage.exe") && s.contains(" hook"));
        assert_eq!(c.hook_uninstall(), json!([["p1", "đã gỡ"]]));
        assert_eq!(c.hook_status(), json!([["p1", false]]));
    }
}
