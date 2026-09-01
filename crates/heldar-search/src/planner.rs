//! Natural-language → structured [`QueryPlan`]. The planner NEVER answers the question; it only
//! decides *how to query*. Two implementations produce the same plan type — `parse_rules` (a
//! transparent, offline keyword parser, the always-available default) and `plan_llm` (an optional
//! OpenAI-compatible call returning a plan JSON, used only when an endpoint is configured; on any
//! failure the caller falls back to the rules). Either way the plan is executed deterministically by
//! [`crate::query::execute`] and shown to the user.

use chrono::{TimeDelta, Utc};
use serde_json::json;

use crate::config::SearchConfig;
use crate::query::QueryPlan;

const COLORS: &[&str] = &[
    "white", "black", "gray", "grey", "silver", "red", "blue", "green", "yellow", "orange",
    "brown", "purple",
];
const VEHICLE_TYPES: &[&str] = &[
    "car",
    "truck",
    "bus",
    "motorcycle",
    "van",
    "suv",
    "bicycle",
    "motorbike",
];

pub fn norm_plate(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}
pub fn plausible_plate(p: &str) -> bool {
    (4..=10).contains(&p.len())
        && p.bytes().any(|b| b.is_ascii_alphabetic())
        && p.bytes().any(|b| b.is_ascii_digit())
}

/// Whole-word (boundary-aware) substring match — avoids "red" matching inside "covered" or a short
/// camera id matching mid-token. `needle` is already lowercase; `hay` is the lowercased query.
fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = hay.as_bytes();
    let mut start = 0;
    while let Some(pos) = hay[start..].find(needle) {
        let i = start + pos;
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let end = i + needle.len();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = i + 1;
    }
    false
}

/// Transparent rule-based parser. `cameras` is (id, name) so phrases like "gate b" resolve to a camera.
pub fn parse_rules(query: &str, cameras: &[(String, String)]) -> QueryPlan {
    parse_rules_in(query, cameras, chrono_tz::Tz::UTC)
}

/// [`parse_rules`] with the zone that "today"/"yesterday" mean a day IN (#125).
///
/// A calendar day is a wall-clock notion: "yesterday" at a site eight hours ahead is a different
/// 24 hours than "yesterday" in UTC, overlapping by only two thirds. Resolving it in UTC and then
/// showing an operator the results is how a search looks right and answers about the wrong day.
pub fn parse_rules_in(query: &str, cameras: &[(String, String)], tz: chrono_tz::Tz) -> QueryPlan {
    let q = query.to_lowercase();
    let now = Utc::now();
    let mut plan = QueryPlan {
        tz: Some(tz.to_string()),
        ..Default::default()
    };

    // "red zone"/"red area" is a restricted-area phrase, NOT the colour red — don't let it set color.
    let red_zone_phrase = q.contains("red zone") || q.contains("red area");

    // Colour + vehicle type (whole-word matches so "red" doesn't fire inside "covered"/"red zone").
    if let Some(c) = COLORS
        .iter()
        .find(|c| contains_word(&q, c) && !(**c == "red" && red_zone_phrase))
    {
        plan.color = Some(if *c == "grey" {
            "gray".into()
        } else {
            (*c).to_string()
        });
    }
    if let Some(t) = VEHICLE_TYPES.iter().find(|t| contains_word(&q, t)) {
        plan.vehicle_type = Some(if *t == "motorbike" {
            "motorcycle".into()
        } else {
            (*t).to_string()
        });
        plan.subject_type = Some("vehicle".into());
    }
    if [
        "car", "cars", "vehicle", "vehicles", "truck", "trucks", "van",
    ]
    .iter()
    .any(|w| contains_word(&q, w))
    {
        plan.subject_type = Some("vehicle".into());
    }
    if [
        "person",
        "people",
        "pedestrian",
        "customer",
        "customers",
        "visitor",
        "visitors",
    ]
    .iter()
    .any(|w| contains_word(&q, w))
    {
        plan.subject_type = Some("person".into());
    }

    // Authorization wording.
    if q.contains("unauthor")
        || q.contains("without authoriz")
        || q.contains("unmatched")
        || q.contains("unknown")
    {
        plan.auth_status.push("unmatched".into());
    }
    if q.contains("exception") || q.contains("mismatch") {
        plan.auth_status.push("exception".into());
    }
    if q.contains("blocked") || q.contains("blacklist") || q.contains("stolen") {
        plan.auth_status.push("blocked".into());
    }

    // Event / source intent.
    if q.contains("red zone")
        || q.contains("restricted")
        || q.contains("breach")
        || q.contains("intrusion")
    {
        plan.sources.push("breach".into());
    } else if q.contains("enter") || q.contains("entry") || q.contains("arriv") {
        plan.event_type = Some("vehicle_entry".into());
    } else if q.contains("exit") || q.contains("leav") || q.contains("left") {
        plan.event_type = Some("vehicle_exit".into());
    }

    // Camera names (longest name first so "gate b annex" beats "gate b").
    let mut cams: Vec<&(String, String)> = cameras.iter().collect();
    cams.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    for (id, name) in cams {
        let n = name.to_lowercase();
        // Require a whole-word match + a minimum length so a 1-2 char name/id can't match noise.
        if (n.len() >= 3 && contains_word(&q, &n)) || {
            let idl = id.to_lowercase();
            idl.len() >= 3 && contains_word(&q, &idl)
        } {
            plan.cameras.push(id.clone());
        }
    }
    plan.cameras.dedup();

    // Relative date windows.
    // A calendar day starts at the SITE's midnight, and `from_wall_clock` resolves the two days a
    // year where that local midnight is skipped or repeated.
    let local_today = now.with_timezone(&tz).date_naive();
    let midnight = |d: chrono::NaiveDate| {
        heldar_kernel::services::tz::from_wall_clock(tz, d.and_hms_opt(0, 0, 0).unwrap())
    };
    if q.contains("yesterday") {
        let start = midnight(local_today - TimeDelta::try_days(1).unwrap());
        plan.from = Some(start.to_rfc3339());
        plan.to = Some(midnight(local_today).to_rfc3339());
    } else if q.contains("today") {
        plan.from = Some(midnight(local_today).to_rfc3339());
    } else if q.contains("last week") || q.contains("past week") || q.contains("this week") {
        plan.from = Some((now - TimeDelta::try_days(7).unwrap()).to_rfc3339());
    } else if let Some(days) = parse_last_n_days(&q) {
        plan.from = Some((now - TimeDelta::try_days(days).unwrap()).to_rfc3339());
    }

    // Time-of-day: "after 6pm" / "before 9am".
    if let Some(h) = parse_clock_after(&q, "after") {
        plan.hour_min = Some(h);
    }
    if let Some(h) = parse_clock_after(&q, "before") {
        plan.hour_max = Some(h);
    }

    // Plate: any plate-like token.
    for w in q.split(|c: char| !c.is_ascii_alphanumeric()) {
        let p = norm_plate(w);
        if plausible_plate(&p) {
            plan.plate = Some(p);
            break;
        }
    }

    plan
}

fn parse_last_n_days(q: &str) -> Option<i64> {
    let toks: Vec<&str> = q.split_whitespace().collect();
    for i in 0..toks.len() {
        if toks[i] == "last" || toks[i] == "past" {
            if let Some(n) = toks.get(i + 1).and_then(|t| t.parse::<i64>().ok()) {
                if toks
                    .get(i + 2)
                    .map(|t| t.starts_with("day"))
                    .unwrap_or(false)
                {
                    return Some(n.clamp(1, 365));
                }
            }
        }
    }
    None
}

/// Parse "<kw> 6pm" / "<kw> 18:00" / "<kw> 6 pm" → UTC hour 0..23.
fn parse_clock_after(q: &str, kw: &str) -> Option<u32> {
    let idx = q.find(kw)? + kw.len();
    let rest = q[idx..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let mut h: u32 = digits.parse().ok()?;
    // Detect the meridiem in the short remainder after the digits — handles both "6pm" and "6 pm".
    let after: String = rest[digits.len()..].chars().take(5).collect();
    let pm = after.contains("pm");
    let am = after.contains("am");
    if pm && h < 12 {
        h += 12;
    }
    if am && h == 12 {
        h = 0;
    }
    if h <= 23 {
        Some(h)
    } else {
        None
    }
}

/// Optional LLM planner: ask an OpenAI-compatible endpoint to translate the question into a plan JSON.
/// Returns None (caller falls back to rules) on any error. The model NEVER sees or returns data.
pub async fn plan_llm(
    http: &reqwest::Client,
    cfg: &SearchConfig,
    query: &str,
    cameras: &[(String, String)],
) -> Option<QueryPlan> {
    let url = cfg.llm_url.as_ref()?;
    let cam_list: Vec<String> = cameras
        .iter()
        .map(|(id, n)| format!("{id} ({n})"))
        .collect();
    let system = format!(
        "You translate a surveillance question into a STRICT JSON query plan. Output ONLY JSON, no prose. \
         Schema keys (all optional): from,to (RFC3339), hour_min,hour_max (0-23 int), cameras (string[] of \
         camera ids), sources (subset of [\"entry\",\"zone\",\"breach\"]), plate (UPPERCASE alnum), color, \
         vehicle_type, subject_type (\"vehicle\"|\"person\"), auth_status (subset of \
         [\"matched\",\"exception\",\"unmatched\",\"blocked\"]), event_type \
         (\"vehicle_entry\"|\"vehicle_exit\"), zone_kind, text, limit. Known cameras: {}. You ONLY produce \
         the query plan; you never answer the question or invent data.",
        cam_list.join(", ")
    );
    let body = json!({
        "model": cfg.llm_model,
        "temperature": 0,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": query }
        ]
    });
    let mut req = http.post(url).json(&body);
    if let Some(k) = &cfg.llm_api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "search: LLM planner returned non-success; falling back to rules");
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())?;
    match serde_json::from_str::<QueryPlan>(content) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "search: LLM plan JSON did not parse; falling back to rules");
            None
        }
    }
}

/// Convenience: clamp planner-produced hours into range (defensive against an LLM emitting nonsense).
pub fn sanitize(mut plan: QueryPlan) -> QueryPlan {
    // `zone` is a semantic-route-only field (issue #77); the structured executor has no zone-id
    // filter, so clear it here rather than let a caller believe it filtered.
    plan.zone = None;
    if let Some(h) = plan.hour_min {
        if h > 23 {
            plan.hour_min = None;
        }
    }
    if let Some(h) = plan.hour_max {
        if h > 23 {
            plan.hour_max = None;
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_handles_spaced_meridiem() {
        assert_eq!(parse_clock_after("after 6pm", "after"), Some(18));
        assert_eq!(parse_clock_after("after 6 pm", "after"), Some(18));
        assert_eq!(parse_clock_after("before 9 am", "before"), Some(9));
        assert_eq!(parse_clock_after("after 12 am", "after"), Some(0));
        assert_eq!(parse_clock_after("after 18:00", "after"), Some(18));
        assert_eq!(parse_clock_after("after noon", "after"), None);
    }

    #[test]
    fn red_zone_is_not_the_colour_red() {
        let p = parse_rules("people who entered red zones yesterday", &[]);
        assert_eq!(p.color, None);
        assert!(p.sources.iter().any(|s| s == "breach"));
        // but an actual red vehicle still parses as a colour
        assert_eq!(parse_rules("red car", &[]).color.as_deref(), Some("red"));
    }

    #[test]
    fn word_boundary_avoids_false_colour() {
        // "recovered" contains "red"/"cover" but is not a colour match.
        assert_eq!(parse_rules("recovered vehicle", &[]).color, None);
    }

    #[test]
    fn camera_name_resolves_whole_word() {
        let cams = vec![("cam_gate_b".to_string(), "Gate B".to_string())];
        let p = parse_rules("white cars at gate b after 6pm", &cams);
        assert_eq!(p.cameras, vec!["cam_gate_b".to_string()]);
    }
}

#[cfg(test)]
mod tz_tests {
    use super::*;
    use chrono_tz::Tz;

    /// "yesterday" is a wall-clock notion. At a site eight hours ahead it is a different 24 hours
    /// than the UTC one — they overlap by two thirds — so a search resolved in the wrong zone looks
    /// right and answers about the wrong day.
    #[test]
    fn yesterday_is_the_sites_calendar_day_not_utcs() {
        let kl = parse_rules_in("yesterday", &[], Tz::Asia__Kuala_Lumpur);
        let utc = parse_rules_in("yesterday", &[], Tz::UTC);

        let (kl_from, utc_from) = (kl.from.clone().unwrap(), utc.from.clone().unwrap());
        assert_ne!(
            kl_from, utc_from,
            "Kuala Lumpur is +08:00, so its midnight is not UTC's — if these match, the zone is \
             not reaching the window and this test proves nothing"
        );
        // KL midnight is 16:00Z the previous day.
        assert!(
            kl_from.ends_with("+08:00") || kl_from.contains("T16:00:00"),
            "KL's day should start at its own midnight: {kl_from}"
        );
        assert_eq!(
            kl.tz.as_deref(),
            Some("Asia/Kuala_Lumpur"),
            "the plan must record which zone it was resolved in, or a logged plan cannot be re-run"
        );

        // The window is exactly 24 hours in both.
        for p in [&kl, &utc] {
            let f = chrono::DateTime::parse_from_rfc3339(p.from.as_ref().unwrap()).unwrap();
            let t = chrono::DateTime::parse_from_rfc3339(p.to.as_ref().unwrap()).unwrap();
            assert_eq!((t - f).num_hours(), 24, "a calendar day is 24h here");
        }
    }

    /// The zone a plan was resolved in is always recorded, so a `search_log` row from before this
    /// existed (no field) is distinguishable from one resolved to UTC deliberately.
    #[test]
    fn every_plan_records_its_zone() {
        for tz in [Tz::UTC, Tz::Asia__Kuala_Lumpur, Tz::America__New_York] {
            let p = parse_rules_in("red car after 6pm", &[], tz);
            assert_eq!(p.tz.as_deref(), Some(tz.name()));
            assert_eq!(p.hour_min, Some(18), "the hour itself is zone-independent");
        }
    }
}
