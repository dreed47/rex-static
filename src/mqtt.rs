//! Announcements from Home Assistant over MQTT.
//!
//! Rex connects out to the broker (nothing listens on this machine) and
//! shows up in Home Assistant through MQTT discovery as `notify.rex_static`
//! plus a connectivity sensor. A message is JSON with the text and the time
//! it was sent: `{"text": "Coffee is ready", "ts": 1731500000}`. Messages
//! older than [`MAX_AGE`] are dropped, so a pile that waited at the broker
//! while this machine slept isn't read out on wake.

use rumqttc::{Client, Event, LastWill, MqttOptions, Packet, QoS};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Broker settings from the config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MqttConf {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
}

pub const SAY_TOPIC: &str = "rex-static/say";
pub const STATUS_TOPIC: &str = "rex-static/status";
const CLIENT_ID: &str = "rex-static";
/// Oldest message still spoken, and how far ahead a sender's clock may be.
pub const MAX_AGE: u64 = 120;
/// Longest announcement spoken, in characters.
const TEXT_MAX: usize = 500;
/// Largest packet accepted from the broker.
const PACKET_MAX: usize = 4096;

/// Password from the keyring (`service rex-static key mqtt`).
fn password() -> Option<String> {
    let out = Command::new("/usr/bin/secret-tool")
        .args(["lookup", "service", "rex-static", "key", "mqtt"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8(out.stdout).ok()?.trim_end_matches('\n').to_string();
    (!p.is_empty()).then_some(p)
}

/// The text of a message, if it's well formed and fresh. `now` is Unix
/// seconds.
pub fn parse_say(payload: &[u8], now: u64) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_slice(payload).map_err(|_| "not JSON".to_string())?;
    let text = v.get("text").and_then(|t| t.as_str()).ok_or("no \"text\" string")?;
    let ts = v.get("ts").and_then(|t| t.as_f64()).ok_or("no \"ts\" number")?;
    if !ts.is_finite() || ts < 0.0 {
        return Err("bad \"ts\"".into());
    }
    let ts = ts as u64;
    if now.saturating_sub(ts) > MAX_AGE {
        return Err(format!("{}s old, skipped", now - ts));
    }
    if ts.saturating_sub(now) > MAX_AGE {
        return Err("\"ts\" is in the future".into());
    }
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(TEXT_MAX)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if clean.is_empty() {
        return Err("empty text".into());
    }
    Ok(clean)
}

/// Retained discovery messages: (topic, JSON).
pub fn discovery() -> Vec<(String, String)> {
    let device = serde_json::json!({
        "identifiers": ["rex_static"],
        "name": "Rex Static",
        "model": "Rex Static",
        "sw_version": env!("CARGO_PKG_VERSION"),
    });
    let notify = serde_json::json!({
        "name": null,
        "unique_id": "rex_static_notify",
        "object_id": "rex_static",
        "command_topic": SAY_TOPIC,
        "command_template": "{\"text\": {{ value | tojson }}, \"ts\": {{ now().timestamp() | int }}}",
        "qos": 1,
        "availability_topic": STATUS_TOPIC,
        "device": device,
    });
    let status = serde_json::json!({
        "name": "Status",
        "unique_id": "rex_static_status",
        "object_id": "rex_static_status",
        "state_topic": STATUS_TOPIC,
        "payload_on": "online",
        "payload_off": "offline",
        "device_class": "connectivity",
        "entity_category": "diagnostic",
        "device": device,
    });
    vec![
        ("homeassistant/notify/rex_static/config".into(), notify.to_string()),
        ("homeassistant/binary_sensor/rex_static/config".into(), status.to_string()),
    ]
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Connect and pass fresh announcements to `out` until it closes. Reconnects
/// on its own, waiting longer after each failure (up to a minute).
pub fn run(conf: MqttConf, out: Sender<String>) {
    let mut opts = MqttOptions::new(CLIENT_ID, conf.host.as_str(), conf.port);
    opts.set_keep_alive(Duration::from_secs(30))
        // Keep the session, so messages sent during a short drop wait at
        // the broker. The age check throws out stale ones.
        .set_clean_session(false)
        .set_max_packet_size(PACKET_MAX, PACKET_MAX)
        .set_last_will(LastWill::new(STATUS_TOPIC, "offline", QoS::AtLeastOnce, true));
    if let Some(user) = &conf.user {
        match password() {
            Some(p) => {
                opts.set_credentials(user.as_str(), p);
            }
            None => eprintln!("rex-static: mqtt: no password in the keyring; trying without one"),
        }
    }
    let (client, mut conn) = Client::new(opts, 16);
    let mut wait = Duration::from_secs(2);
    let mut down = false;
    loop {
        match conn.recv() {
            Err(_) => return,
            Ok(Ok(Event::Incoming(Packet::ConnAck(_)))) => {
                if down {
                    eprintln!("rex-static: mqtt: connected again");
                }
                down = false;
                wait = Duration::from_secs(2);
                let _ = client.try_subscribe(SAY_TOPIC, QoS::AtLeastOnce);
                for (topic, body) in discovery() {
                    let _ = client.try_publish(topic, QoS::AtLeastOnce, true, body);
                }
                let _ = client.try_publish(STATUS_TOPIC, QoS::AtLeastOnce, true, "online");
            }
            Ok(Ok(Event::Incoming(Packet::Publish(p)))) if p.topic == SAY_TOPIC => {
                if p.retain {
                    continue;
                }
                match parse_say(&p.payload, now()) {
                    Ok(text) => {
                        if out.send(text).is_err() {
                            return;
                        }
                    }
                    Err(e) => eprintln!("rex-static: mqtt: message ignored: {e}"),
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                if !down {
                    eprintln!("rex-static: mqtt: {e}; retrying");
                }
                down = true;
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(60));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn msg(text: &str, ts: u64) -> Vec<u8> {
        serde_json::json!({"text": text, "ts": ts}).to_string().into_bytes()
    }

    #[test]
    fn fresh_message_is_spoken() {
        assert_eq!(parse_say(&msg("Coffee is ready", NOW - 5), NOW).unwrap(), "Coffee is ready");
        assert!(parse_say(&msg("x", NOW - MAX_AGE), NOW).is_ok());
        assert!(parse_say(br#"{"text":"x","ts":1800000000.7}"#, NOW).is_ok());
    }

    #[test]
    fn stale_or_future_is_dropped() {
        assert!(parse_say(&msg("old", NOW - MAX_AGE - 1), NOW).is_err());
        assert!(parse_say(&msg("later", NOW + MAX_AGE + 1), NOW).is_err());
    }

    #[test]
    fn bad_payloads_are_dropped() {
        for bad in [
            &b"Coffee is ready"[..],
            br#"{"text":"x"}"#,
            br#"{"ts":1800000000}"#,
            br#"{"text":5,"ts":1800000000}"#,
            br#"{"text":"x","ts":"now"}"#,
            br#"{"text":"x","ts":-1}"#,
            br#"{"text":"  \n ","ts":1800000000}"#,
        ] {
            assert!(parse_say(bad, NOW).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn text_is_cleaned_and_capped() {
        assert_eq!(parse_say(&msg("a\u{7}b\n\nc", NOW), NOW).unwrap(), "a b c");
        let long = "a".repeat(2000);
        assert_eq!(parse_say(&msg(&long, NOW), NOW).unwrap().chars().count(), TEXT_MAX);
    }

    #[test]
    fn discovery_is_valid_json() {
        let d = discovery();
        assert_eq!(d.len(), 2);
        for (topic, body) in d {
            assert!(topic.starts_with("homeassistant/") && topic.ends_with("/config"));
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["device"]["identifiers"][0], "rex_static");
        }
    }
}
