//! Current conditions and today's forecast from a Tempest weather station,
//! for the morning update. The token and station come from the
//! `TEMPEST_TOKEN` and `TEMPEST_STATION_ID` environment variables, or else
//! from the Tempest weather bar plugin's settings in
//! `~/.config/omarchy/shell.json`. The token never goes into a message.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const API: &str = "https://swd.weatherflow.com/swd/rest";
const PLUGIN_ID: &str = "io.github.dreed47.tempest-weather";
/// Most bytes read from shell.json.
const SHELL_MAX: u64 = 1024 * 1024;
/// Most bytes read from one API response.
const RESPONSE_MAX: u64 = 2 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct Station {
    token: String,
    /// None: look it up from the token.
    station: Option<String>,
    metric: bool,
}

fn shell_json() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("omarchy/shell.json")
}

fn is_token(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn is_station(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.chars().all(|c| c.is_ascii_digit())
}

/// Finds the plugin's settings object anywhere in the shell config.
fn find_plugin(v: &Value, depth: u32) -> Option<&Value> {
    if depth > 12 {
        return None;
    }
    match v {
        Value::Object(m) => {
            if m.get("id").and_then(Value::as_str) == Some(PLUGIN_ID) {
                return Some(v);
            }
            m.values().find_map(|x| find_plugin(x, depth + 1))
        }
        Value::Array(a) => a.iter().find_map(|x| find_plugin(x, depth + 1)),
        _ => None,
    }
}

/// Station settings from the plugin entry in shell.json text.
fn from_shell(text: &str) -> Option<Station> {
    let v: Value = serde_json::from_str(text).ok()?;
    let p = find_plugin(&v, 0)?;
    let token = p["token"].as_str().filter(|t| is_token(t))?.to_string();
    let station = match &p["stationId"] {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
    .filter(|s| is_station(s));
    Some(Station { token, station, metric: p["units"].as_str() == Some("metric") })
}

fn read_capped(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let mut s = String::new();
    f.take(SHELL_MAX).read_to_string(&mut s).ok()?;
    Some(s)
}

impl Station {
    /// The station settings, or None when Tempest isn't set up.
    pub fn load() -> Option<Self> {
        if cfg!(test) {
            return None;
        }
        if let Some(token) = std::env::var("TEMPEST_TOKEN").ok().filter(|t| is_token(t)) {
            let station = std::env::var("TEMPEST_STATION_ID").ok().filter(|s| is_station(s));
            return Some(Self { token, station, metric: false });
        }
        from_shell(&read_capped(&shell_json())?)
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .https_only(true)
            .build()
            .into();
        let mut req = agent.get(format!("{API}/{path}")).query("token", &self.token);
        for (k, v) in query {
            req = req.query(k, v);
        }
        // ureq's error text can hold the URL, and with it the token.
        let mut resp = req.call().map_err(|_| "Tempest: network error".to_string())?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(format!("Tempest said HTTP {status}"));
        }
        let text = resp
            .body_mut()
            .with_config()
            .limit(RESPONSE_MAX)
            .read_to_string()
            .map_err(|_| "Tempest: bad response".to_string())?;
        serde_json::from_str(&text).map_err(|_| "Tempest: bad response".to_string())
    }

    fn station_id(&self) -> Result<String, String> {
        if let Some(s) = &self.station {
            return Ok(s.clone());
        }
        let v = self.get("stations", &[])?;
        v["stations"][0]["station_id"]
            .as_u64()
            .map(|n| n.to_string())
            .ok_or_else(|| "Tempest: no station on this account".to_string())
    }

    /// A few short lines: now, then today.
    pub fn summary(&self) -> Result<String, String> {
        let units: &[(&str, &str)] = if self.metric {
            &[("units_temp", "c"), ("units_wind", "kph"), ("units_pressure", "mb"), ("units_precip", "mm"), ("units_distance", "km")]
        } else {
            &[("units_temp", "f"), ("units_wind", "mph"), ("units_pressure", "inhg"), ("units_precip", "in"), ("units_distance", "mi")]
        };
        let id = self.station_id()?;
        let mut q = vec![("station_id", id.as_str())];
        q.extend_from_slice(units);
        let v = self.get("better_forecast", &q)?;
        summarize(&v, self.metric).ok_or_else(|| "Tempest: no current conditions".to_string())
    }
}

fn text(v: &Value, max: usize) -> Option<String> {
    let s: String = v.as_str()?.chars().filter(|c| !c.is_control()).take(max).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn num(v: &Value) -> Option<i64> {
    v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64)
}

/// Spoken-style weather lines from a better_forecast response.
fn summarize(v: &Value, metric: bool) -> Option<String> {
    let (deg, speed) = if metric { ("°C", "km/h") } else { ("°F", "mph") };
    let c = &v["current_conditions"];
    let temp = num(&c["air_temperature"])?;
    let mut now = format!("Now: {temp}{deg}");
    if let Some(f) = num(&c["feels_like"]).filter(|&f| (f - temp).abs() >= 3) {
        now.push_str(&format!(", feels like {f}{deg}"));
    }
    if let Some(s) = text(&c["conditions"], 40) {
        now.push_str(&format!(", {s}"));
    }
    if let Some(h) = num(&c["relative_humidity"]) {
        now.push_str(&format!(", humidity {h}%"));
    }
    if let Some(w) = num(&c["wind_avg"]) {
        let dir = text(&c["wind_direction_cardinal"], 4).map(|d| format!(" from the {d}")).unwrap_or_default();
        now.push_str(&format!(", wind {w} {speed}{dir}"));
        if let Some(g) = num(&c["wind_gust"]).filter(|&g| g >= w + 5) {
            now.push_str(&format!(", gusts {g}"));
        }
    }
    let mut out = vec![now];
    if let Some(d) = v["forecast"]["daily"].get(0) {
        let mut today = String::from("Today");
        if let Some(s) = text(&d["conditions"], 40) {
            today.push_str(&format!(": {s}"));
        }
        if let (Some(hi), Some(lo)) = (num(&d["air_temp_high"]), num(&d["air_temp_low"])) {
            today.push_str(&format!(", high {hi}{deg}, low {lo}{deg}"));
        }
        if let Some(p) = num(&d["precip_probability"]).filter(|&p| p > 0) {
            today.push_str(&format!(", {p}% chance of rain"));
        }
        out.push(today);
    }
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_plugin_settings() {
        let text = json!({"bar": {"layout": {"centerEnd": [
            {"id": "omarchy.clock"},
            {"id": PLUGIN_ID, "token": "abc-123", "stationId": "93748", "units": "metric"}
        ]}}})
        .to_string();
        let s = from_shell(&text).unwrap();
        assert_eq!(s, Station { token: "abc-123".into(), station: Some("93748".into()), metric: true });
    }

    #[test]
    fn rejects_bad_values() {
        let t = |token: &str, station: Value| {
            from_shell(&json!({"x": [{"id": PLUGIN_ID, "token": token, "stationId": station}]}).to_string())
        };
        assert!(t("a&b=c", json!("1")).is_none(), "token with query characters");
        assert!(t("", json!("1")).is_none());
        assert!(t(&"a".repeat(65), json!("1")).is_none());
        assert_eq!(t("abc", json!("12&x=1")).unwrap().station, None, "bad station is looked up instead");
        assert_eq!(t("abc", json!(42)).unwrap().station.as_deref(), Some("42"));
        assert!(from_shell("{\"id\": \"other\", \"token\": \"abc\"}").is_none());
        assert!(from_shell("not json").is_none());
    }

    #[test]
    fn summary_lines() {
        let v = json!({
            "current_conditions": {"air_temperature": 41.6, "feels_like": 36.2, "conditions": "Clear",
                "relative_humidity": 70, "wind_avg": 6.2, "wind_direction_cardinal": "NW", "wind_gust": 14},
            "forecast": {"daily": [{"conditions": "Partly Cloudy", "air_temp_high": 55, "air_temp_low": 33,
                "precip_probability": 10}]}
        });
        assert_eq!(
            summarize(&v, false).unwrap(),
            "Now: 42°F, feels like 36°F, Clear, humidity 70%, wind 6 mph from the NW, gusts 14\n\
             Today: Partly Cloudy, high 55°F, low 33°F, 10% chance of rain"
        );
        assert!(summarize(&json!({}), false).is_none());
    }
}
