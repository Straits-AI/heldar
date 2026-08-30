//! Sites (#125).
//!
//! The `sites` table has existed since migration 0001 with no API and no insert path, so no row was
//! ever created. That made the timezone resolver's first arm — the camera's own site — permanently
//! unreachable, and left a multi-site box unable to say that its Kuala Lumpur cameras and its London
//! cameras keep different hours.
//!
//! # Changing a site's timezone is an operational act, not a label edit
//!
//! Every recording schedule on the site's cameras is a WALL-CLOCK rule, so moving the zone moves the
//! hours those cameras actually record. A 200 with no indication of that is how an operator relabels
//! a site at 5pm and discovers at midnight that nothing recorded.
//!
//! So the write path reports what it moved (`cameras_affected`, `previous_timezone`) and audits it.
//! It does not refuse — the operator is fixing something, and a box that will not let you correct a
//! wrong zone is worse than one that tells you what changed.
//!
//! # Deleting a site is not a label edit either
//!
//! `cameras.site_id` is `ON DELETE SET NULL` (migration 0001), so deleting a site silently drops
//! every camera on it back to the box default — reinterpreting their windows with no event, no
//! warning and no way to notice. A DELETE is therefore REFUSED while any camera references the site.
//! Move the cameras first, deliberately, and the reinterpretation happens one camera at a time where
//! it is visible.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sites", post(create).get(list))
        .route(
            "/api/v1/sites/{id}",
            get(get_one).patch(update).delete(delete_site),
        )
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SiteRow {
    pub id: String,
    pub name: String,
    /// IANA identifier, or `null` when the site has not chosen one.
    ///
    /// Null is a real state, not a placeholder: migration 0019 removed the `NOT NULL DEFAULT 'UTC'`
    /// precisely so that "nobody has chosen" is distinguishable from "chose UTC". A site with no
    /// zone falls through to the box-wide default.
    pub timezone: Option<String>,
    /// Typed, not a raw column string: sqlx hands back `+00:00` while every other model in the API
    /// re-serializes as `Z`, and one box speaking two timestamp dialects is a trap a generated
    /// client will not catch — OpenAPI types both as plain `string`.
    #[schema(value_type = String)]
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SiteCreate {
    pub id: String,
    pub name: String,
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SiteUpdate {
    pub name: Option<String>,
    /// An IANA identifier, or explicit `null` to clear it back to the box default.
    #[serde(default, deserialize_with = "crate::util::double_option")]
    pub timezone: Option<Option<String>>,
}

fn validate_tz(raw: &Option<String>) -> AppResult<()> {
    if let Some(tz) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if crate::services::tz::parse(tz).is_none() {
            return Err(AppError::BadRequest(format!(
                "`timezone` must be an IANA identifier such as `Asia/Kuala_Lumpur` (got {tz:?}). \
                 Abbreviations and fixed offsets are not accepted: `GMT+8` and `+08:00` cannot \
                 express daylight saving."
            )));
        }
    }
    Ok(())
}

async fn camera_count(st: &AppState, site: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM cameras WHERE site_id = ?")
        .bind(site)
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0)
}

/// Sites this credential may see.
///
/// A camera-scoped credential sees the sites its own cameras belong to — that is information about
/// its cameras, not the fleet roster. It does NOT see sites it holds no camera on, for the same
/// reason it does not see those cameras.
#[utoipa::path(
    get, path = "/api/v1/sites", tag = "sites",
    responses(
        (status = 200, description = "Sites visible to this credential"),
        (status = 403, description = "Missing `camera:read`", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<serde_json::Value>> {
    principal.require_cap(Cap::CameraRead, "list sites")?;
    let mut sql = String::from("SELECT id, name, timezone, created_at FROM sites WHERE 1=1");
    let mut binds: Vec<String> = Vec::new();
    if let crate::auth::Scope::Cameras(allowed) = &principal.scope {
        if allowed.is_empty() {
            return Ok(Json(json!({ "sites": [] })));
        }
        sql.push_str(
            " AND id IN (SELECT site_id FROM cameras WHERE site_id IS NOT NULL AND id IN (",
        );
        sql.push_str(&vec!["?"; allowed.len()].join(","));
        sql.push_str("))");
        binds.extend(allowed.iter().cloned());
    }
    sql.push_str(" ORDER BY id ASC");
    let mut q = sqlx::query_as::<_, SiteRow>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    Ok(Json(json!({ "sites": q.fetch_all(&st.pool).await? })))
}

/// A site's own record. An out-of-scope site answers exactly as an unknown one.
#[utoipa::path(
    get, path = "/api/v1/sites/{id}", tag = "sites",
    params(("id" = String, Path, description = "Site id")),
    responses(
        (status = 200, description = "The site", body = SiteRow),
        (status = 404, description = "Unknown site, or one this credential holds no camera on", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_one(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<SiteRow>> {
    principal.require_cap(Cap::CameraRead, "read a site")?;
    let row: Option<SiteRow> =
        sqlx::query_as("SELECT id, name, timezone, created_at FROM sites WHERE id = ?")
            .bind(&id)
            .fetch_optional(&st.pool)
            .await?;
    let row = row.ok_or_else(|| AppError::NotFound(format!("site {id} not found")))?;
    if !visible(&st, &principal, &id).await? {
        // 404, not 403: answering differently for "exists but not yours" turns site ids into an
        // enumeration oracle, exactly as it would for cameras.
        return Err(AppError::NotFound(format!("site {id} not found")));
    }
    Ok(Json(row))
}

/// Whether a scoped credential holds any camera on this site. Fleet-wide credentials see everything.
async fn visible(st: &AppState, principal: &Principal, site: &str) -> AppResult<bool> {
    let crate::auth::Scope::Cameras(allowed) = &principal.scope else {
        return Ok(true);
    };
    if allowed.is_empty() {
        return Ok(false);
    }
    let sql = format!(
        "SELECT COUNT(*) FROM cameras WHERE site_id = ? AND id IN ({})",
        vec!["?"; allowed.len()].join(",")
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql).bind(site);
    for c in allowed.iter() {
        q = q.bind(c);
    }
    Ok(q.fetch_one(&st.pool).await? > 0)
}

/// Create a site. Fleet-wide by nature — a site is not a camera, and its zone reinterprets every
/// camera later assigned to it.
#[utoipa::path(
    post, path = "/api/v1/sites", tag = "sites",
    request_body = SiteCreate,
    responses(
        (status = 200, description = "The created site", body = SiteRow),
        (status = 400, description = "Bad id or timezone", body = crate::openapi::ErrorBody),
        (status = 409, description = "A site with this id already exists", body = crate::openapi::ErrorBody),
        (status = 403, description = "Not an admin, or a camera-scoped credential", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn create(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<SiteCreate>,
) -> AppResult<Json<SiteRow>> {
    principal.require(principal.can_admin(), "create a site")?;
    crate::routes::cameras::require_fleet_scope(&principal, "create a site")?;

    let id = body.id.trim().to_string();
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(AppError::BadRequest(
            "`id` must be 1-64 characters of [A-Za-z0-9_-]".into(),
        ));
    }
    if id.len() > 64 {
        return Err(AppError::BadRequest(
            "`id` must be 1-64 characters of [A-Za-z0-9_-]".into(),
        ));
    }
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    validate_tz(&body.timezone)?;
    let tz = body
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM sites WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?;
    if exists.is_some() {
        return Err(AppError::Conflict(format!("site {id} already exists")));
    }

    sqlx::query("INSERT INTO sites (id, name, timezone, created_at) VALUES (?,?,?,?)")
        .bind(&id)
        .bind(body.name.trim())
        .bind(tz)
        .bind(chrono::Utc::now())
        .execute(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_site",
        "site",
        &id,
        json!({ "name": body.name, "timezone": tz }),
    )
    .await;

    let row = sqlx::query_as("SELECT id, name, timezone, created_at FROM sites WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(row))
}

/// Update a site. Changing its timezone MOVES the recording windows of every camera on it.
#[utoipa::path(
    patch, path = "/api/v1/sites/{id}", tag = "sites",
    params(("id" = String, Path, description = "Site id")),
    request_body = SiteUpdate,
    responses(
        (status = 200, description = "The site, plus what the change moved"),
        (status = 400, description = "Bad timezone", body = crate::openapi::ErrorBody),
        (status = 403, description = "Not an admin, or a camera-scoped credential", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown site", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn update(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<SiteUpdate>,
) -> AppResult<Json<serde_json::Value>> {
    principal.require(principal.can_admin(), "update a site")?;
    crate::routes::cameras::require_fleet_scope(&principal, "update a site")?;

    let cur: Option<SiteRow> =
        sqlx::query_as("SELECT id, name, timezone, created_at FROM sites WHERE id = ?")
            .bind(&id)
            .fetch_optional(&st.pool)
            .await?;
    let cur = cur.ok_or_else(|| AppError::NotFound(format!("site {id} not found")))?;

    if let Some(tz) = &body.timezone {
        validate_tz(tz)?;
    }
    let new_tz = match &body.timezone {
        // Absent: leave it alone. Present-and-null: clear it. `double_option` is what keeps a rename
        // from silently wiping the zone.
        None => cur.timezone.clone(),
        Some(v) => v
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    };
    // Trimmed, and a blank rename keeps the old name rather than storing whitespace. `create`
    // trimmed and `update` did not, which is how a site ends up named "   ".
    let name = body
        .name
        .clone()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| cur.name.clone());

    sqlx::query("UPDATE sites SET name = ?, timezone = ? WHERE id = ?")
        .bind(&name)
        .bind(&new_tz)
        .bind(&id)
        .execute(&st.pool)
        .await?;

    let moved = new_tz != cur.timezone;
    let affected = if moved {
        camera_count(&st, &id).await
    } else {
        0
    };
    auth::audit(
        &st.pool,
        &principal,
        "update_site",
        "site",
        &id,
        json!({
            "name": name,
            "timezone": new_tz,
            "previous_timezone": cur.timezone,
            "cameras_affected": affected,
        }),
    )
    .await;
    if moved && affected > 0 {
        tracing::warn!(
            target: "heldar::security",
            site = %id,
            from = ?cur.timezone,
            to = ?new_tz,
            cameras = affected,
            "sites: timezone changed — recording windows for these cameras now follow a different clock"
        );
    }

    Ok(Json(json!({
        "site": SiteRow { id: id.clone(), name, timezone: new_tz.clone(), created_at: cur.created_at },
        // The operator is told what they just moved. A schedule is a wall-clock rule, so changing
        // the zone changes the hours these cameras record — silently, unless someone says so.
        "timezone_changed": moved,
        "previous_timezone": cur.timezone,
        "cameras_affected": affected,
        "note": if moved && affected > 0 {
            "Recording schedules on these cameras are wall-clock rules and now follow the new zone. \
             Check any window that must not move."
        } else {
            "No recording windows moved."
        },
    })))
}

/// Delete a site, but only once nothing depends on it.
#[utoipa::path(
    delete, path = "/api/v1/sites/{id}", tag = "sites",
    params(("id" = String, Path, description = "Site id")),
    responses(
        (status = 200, description = "Deleted"),
        (status = 409, description = "Cameras still reference this site", body = crate::openapi::ErrorBody),
        (status = 403, description = "Not an admin, or a camera-scoped credential", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown site", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn delete_site(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    principal.require(principal.can_admin(), "delete a site")?;
    crate::routes::cameras::require_fleet_scope(&principal, "delete a site")?;

    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM sites WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound(format!("site {id} not found")));
    }

    // `cameras.site_id` is ON DELETE SET NULL, so deleting a populated site would drop every camera
    // on it back to the box default and reinterpret its recording windows — with no event and
    // nothing to notice. Refusing makes the operator move the cameras deliberately.
    //
    // THE CHECK AND THE DELETE ARE ONE STATEMENT. Counting first and then deleting is a TOCTOU on
    // separate pool connections (the pool is 16 by default, never 1): a camera assigned in between
    // is silently detached, which is precisely the outcome the guard exists to prevent. Measured at
    // 23 of 40 attempts before this was one statement.
    let deleted = sqlx::query(
        "DELETE FROM sites
          WHERE id = ? AND NOT EXISTS (SELECT 1 FROM cameras WHERE site_id = ?)",
    )
    .bind(&id)
    .bind(&id)
    .execute(&st.pool)
    .await?
    .rows_affected();
    if deleted == 0 {
        let n = camera_count(&st, &id).await;
        return Err(AppError::Conflict(format!(
            "{n} camera(s) still belong to site {id}. Deleting it would drop them to the box-wide \
             timezone and silently reinterpret their recording schedules — reassign them first \
             with `PATCH /api/v1/cameras/{{id}}`."
        )));
    }
    auth::audit(&st.pool, &principal, "delete_site", "site", &id, json!({})).await;
    Ok(Json(json!({ "deleted": id })))
}
