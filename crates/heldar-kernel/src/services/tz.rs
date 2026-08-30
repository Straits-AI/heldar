//! Which clock a site's operators actually read (#125).
//!
//! Timestamps are stored in UTC everywhere and that does not change. This module answers only the
//! *interpretation* question: when someone says "after 6pm" or schedules a window for "18:00", whose
//! six o'clock did they mean?
//!
//! Getting it wrong is not a cosmetic bug. A search shifted by eight hours returns convincing,
//! WRONG footage — syntactically valid, plausible-looking, and about the wrong part of the night.
//!
//! # Why this returns `Option<Tz>` and no default
//!
//! This is the load-bearing decision in the whole feature, so it is worth stating plainly.
//!
//! The subsystems do not share a historical default:
//!
//! - **recording schedules** evaluate against the SERVER's local zone (`chrono::Local`), which on a
//!   real box is set through `TZ` in the container and is very often already correct;
//! - **search and reports** treat hour filters and relative dates as UTC.
//!
//! Any resolver with a single baked-in fallback therefore silently moves whichever subsystem it did
//! not match. Falling back to UTC would shift every recording window on every box whose operator
//! configured `TZ` — on upgrade day, with no error and no warning, which is the worst thing that can
//! happen to a recorder. Falling back to `Local` would shift every saved search on those same boxes.
//!
//! So there is no fallback here. `None` means "nobody has configured a zone", and each caller states
//! its own historical default in one line at the call site, where the choice is visible:
//!
//! ```ignore
//! let (tz, _) = tz::site_tz(&pool, Some(camera_id)).await;
//! match tz {
//!     Some(tz) => schedule_active_at(s, Utc::now().with_timezone(&tz)),
//!     None => schedule_active_at(s, Local::now()),   // unchanged for an unconfigured box
//! }
//! ```
//!
//! Nothing moves until an operator sets a zone. When they do, everything moves together, which is
//! the point.
//!
//! # Resolution order
//!
//! 1. the camera's site (`cameras.site_id` → `sites.timezone`)
//! 2. the box-wide `default_timezone` setting
//! 3. nothing
//!
//! (1) resolves to nothing on every box today: `sites` has existed since migration 0001 but has no
//! API and no insert path, so no row is ever created. It is wired now anyway, because it costs one
//! query and it means making sites reachable later is pure CRUD rather than a second pass through
//! every caller.
//!
//! An unparseable stored value falls through to the next level rather than panicking — a hand-edited
//! row should degrade, not take the recorder down. Every WRITE path validates and refuses instead,
//! so the fall-through is a safety net and not the normal way a bad zone is handled.

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

pub use chrono_tz::Tz;

/// Where the effective zone came from. Reported alongside the zone itself, because "UTC" and
/// "nobody has said" look identical in a timestamp and mean very different things to an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TzSource {
    /// The camera's site names it.
    Site,
    /// The box-wide `default_timezone` setting.
    Default,
    /// Nothing is configured; the caller's own historical default applies.
    Unset,
}

/// The settings key holding the box-wide default.
pub const DEFAULT_TIMEZONE: &str = "default_timezone";

/// The configured zone for a camera (or for the box, with `camera_id: None`), and where it came
/// from. `None` means nothing is configured — see the module docs for why there is no default here.
pub async fn site_tz(pool: &SqlitePool, camera_id: Option<&str>) -> (Option<Tz>, TzSource) {
    if let Some(id) = camera_id {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT s.timezone FROM cameras c
             JOIN sites s ON s.id = c.site_id
             WHERE c.id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
        if let Some(tz) = row.and_then(|r| parse(&r.0)) {
            return (Some(tz), TzSource::Site);
        }
    }
    if let Some(tz) = crate::services::settings::get_str(pool, DEFAULT_TIMEZONE)
        .await
        .and_then(|s| parse(&s))
    {
        return (Some(tz), TzSource::Default);
    }
    (None, TzSource::Unset)
}

/// Parse an IANA identifier, or `None`. The single place a stored string becomes a zone.
pub fn parse(s: &str) -> Option<Tz> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    s.parse::<Tz>().ok()
}

/// Turn a UTC instant into the given zone. Total by construction: every UTC instant is exactly one
/// wall-clock reading, so there is no ambiguity to resolve here.
///
/// The reverse direction — wall clock to instant — is where daylight saving actually bites: a local
/// time can be skipped (spring forward) or happen twice (autumn back). Anything that needs it must
/// handle [`chrono::LocalResult`] explicitly rather than reaching for `.unwrap()`.
pub fn in_zone(tz: Tz, at: DateTime<Utc>) -> DateTime<Tz> {
    at.with_timezone(&tz)
}

/// The UTC instant for a wall-clock reading in `tz`, resolving daylight-saving edges explicitly.
///
/// - a **skipped** local time (spring forward) resolves to the instant the clock jumps to, so a
///   window whose start does not exist still opens;
/// - a **repeated** local time (autumn back) resolves to the FIRST occurrence, so a range covers the
///   repeated hour rather than starting inside it.
///
/// Both are choices, not facts, which is why they are made here once and named — leaving them to
/// `.unwrap()` at half a dozen call sites is how a recorder ends up with an hour of missing footage
/// one Sunday a year.
pub fn from_wall_clock(tz: Tz, naive: chrono::NaiveDateTime) -> DateTime<Utc> {
    use chrono::LocalResult;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(t) => t.with_timezone(&Utc),
        // Repeated: both are real instants; the earlier one is the start of the repeated hour.
        LocalResult::Ambiguous(earlier, _later) => earlier.with_timezone(&Utc),
        // Skipped: no instant maps to this reading. Step forward a minute at a time until one does;
        // the gap is at most an hour in every zone tzdb has ever carried.
        LocalResult::None => {
            let mut probe = naive;
            for _ in 0..(60 * 3) {
                probe += chrono::Duration::minutes(1);
                if let LocalResult::Single(t) = tz.from_local_datetime(&probe) {
                    return t.with_timezone(&Utc);
                }
            }
            // Unreachable for any real zone; better than a panic in the recorder.
            DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn nothing_configured_resolves_to_nothing() {
        let p = pool().await;
        assert_eq!(site_tz(&p, None).await, (None, TzSource::Unset));
        assert_eq!(
            site_tz(&p, Some("cam_a")).await,
            (None, TzSource::Unset),
            "an unknown camera must not invent a zone"
        );
    }

    #[tokio::test]
    async fn the_box_setting_is_used_when_no_site_names_one() {
        let p = pool().await;
        crate::services::settings::set_str(&p, DEFAULT_TIMEZONE, "Asia/Kuala_Lumpur")
            .await
            .unwrap();
        assert_eq!(
            site_tz(&p, None).await,
            (Some(Tz::Asia__Kuala_Lumpur), TzSource::Default)
        );
    }

    #[tokio::test]
    async fn a_site_beats_the_box_setting() {
        let p = pool().await;
        crate::services::settings::set_str(&p, DEFAULT_TIMEZONE, "UTC")
            .await
            .unwrap();
        let now = Utc::now();
        sqlx::query("INSERT INTO sites (id, name, timezone, created_at) VALUES (?,?,?,?)")
            .bind("site_kl")
            .bind("KL")
            .bind("Asia/Kuala_Lumpur")
            .bind(now)
            .execute(&p)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO cameras (id, site_id, name, created_at, updated_at) VALUES (?,?,?,?,?)",
        )
        .bind("cam_a")
        .bind("site_kl")
        .bind("A")
        .bind(now)
        .bind(now)
        .execute(&p)
        .await
        .unwrap();
        assert_eq!(
            site_tz(&p, Some("cam_a")).await,
            (Some(Tz::Asia__Kuala_Lumpur), TzSource::Site),
            "a camera's own site must win over the box default"
        );
    }

    /// A hand-edited or corrupted value must degrade to the next level, never take the recorder
    /// down and never silently answer in a zone nobody chose.
    #[tokio::test]
    async fn an_unparseable_stored_zone_falls_through_rather_than_panicking() {
        let p = pool().await;
        crate::services::settings::set_str(&p, DEFAULT_TIMEZONE, "Middle-earth/Shire")
            .await
            .unwrap();
        assert_eq!(site_tz(&p, None).await, (None, TzSource::Unset));
    }

    #[test]
    fn parse_accepts_iana_and_refuses_plausible_nonsense() {
        assert_eq!(parse("Asia/Kuala_Lumpur"), Some(Tz::Asia__Kuala_Lumpur));
        assert_eq!(parse("UTC"), Some(Tz::UTC));
        assert_eq!(parse("  Europe/London  "), Some(Tz::Europe__London));
        for bad in [
            "Asia/KL",
            "GMT+8",
            "",
            "   ",
            "Middle-earth/Shire",
            "+08:00",
        ] {
            assert_eq!(parse(bad), None, "{bad:?} must not parse as a zone");
        }
    }

    /// The whole reason chrono-tz is a dependency. A fixed offset gets these wrong twice a year, and
    /// those are exactly the hours where a shifted search returns confident, wrong footage.
    #[test]
    fn daylight_saving_edges_are_resolved_deliberately() {
        // Europe/London, 2026-03-29: 01:00 → 02:00. 01:30 never happens.
        let skipped = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let got = from_wall_clock(Tz::Europe__London, skipped);
        assert_eq!(
            got.to_rfc3339(),
            "2026-03-29T01:00:00+00:00",
            "a skipped wall clock must resolve to the instant the clock jumps to, so a window whose \
             start does not exist still opens"
        );

        // Europe/London, 2026-10-25: 02:00 → 01:00. 01:30 happens twice.
        let repeated = NaiveDate::from_ymd_opt(2026, 10, 25)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let got = from_wall_clock(Tz::Europe__London, repeated);
        assert_eq!(
            got.to_rfc3339(),
            "2026-10-25T00:30:00+00:00",
            "a repeated wall clock must resolve to the FIRST occurrence, so a range covers the \
             repeated hour instead of starting inside it"
        );

        // A negative-offset DST zone, and a zone with no DST at all.
        let ny = NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        assert_eq!(
            from_wall_clock(Tz::America__New_York, ny).to_rfc3339(),
            "2026-03-08T07:00:00+00:00"
        );
        let kl = NaiveDate::from_ymd_opt(2026, 6, 1)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();
        assert_eq!(
            from_wall_clock(Tz::Asia__Kuala_Lumpur, kl).to_rfc3339(),
            "2026-06-01T10:00:00+00:00",
            "18:00 in Kuala Lumpur is 10:00 UTC — the eight hours that turn a valid search into the \
             wrong footage"
        );
    }

    /// Half-hour and quarter-hour offsets exist. Nothing in the codebase may assume a whole hour.
    #[test]
    fn sub_hour_offsets_round_trip() {
        let t = NaiveDate::from_ymd_opt(2026, 6, 1)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();
        assert_eq!(
            from_wall_clock(Tz::Asia__Kathmandu, t).to_rfc3339(),
            "2026-06-01T12:15:00+00:00"
        );
        assert_eq!(
            from_wall_clock(Tz::Australia__Adelaide, t).to_rfc3339(),
            "2026-06-01T08:30:00+00:00"
        );
    }
}
