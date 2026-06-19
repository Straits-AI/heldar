use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;

/// Deserialize a PRESENT field into `Some(inner)`. Combined with `#[serde(default)]` (which leaves a
/// missing field as `None`), this yields three states: omitted = `None`, null = `Some(None)`,
/// value = `Some(Some(v))`.
fn de_field_present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

/// Mask a webhook URL for display: keep only `scheme://host[:port]` and append `/…` so the path/token
/// is never revealed. Returns None for an empty url; a url without a scheme is masked to `…` (it may
/// be a bare token).
pub fn mask_webhook_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..authority_end];
            if authority_end < rest.len() {
                Some(format!("{scheme}://{authority}/…"))
            } else {
                Some(format!("{scheme}://{authority}"))
            }
        }
        None => Some("…".to_string()),
    }
}

/// A webhook subscription row as stored. `secret` (the HMAC signing key) is never serialized; use
/// [`WebhookSubscriptionView`] for output. `event_types` is a JSON array of type names; the sentinel
/// `["*"]` matches every event type, otherwise it is an exact-membership set. `cursor_at` is the
/// per-subscription delivery cursor (an `events.created_at`); NULL means "start at now" (no backlog).
#[derive(Debug, Clone, FromRow)]
pub struct WebhookSubscription {
    pub id: String,
    pub name: String,
    pub url: String,
    pub event_types: Json<Vec<String>>,
    pub min_severity: String,
    pub secret: Option<String>,
    pub enabled: bool,
    pub cursor_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Client-facing subscription view: the `secret` is replaced by a `has_secret` flag and never echoed.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookSubscriptionView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub min_severity: String,
    /// Whether an HMAC signing secret is configured (the value itself is never returned).
    pub has_secret: bool,
    pub enabled: bool,
    pub cursor_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<WebhookSubscription> for WebhookSubscriptionView {
    fn from(s: WebhookSubscription) -> Self {
        WebhookSubscriptionView {
            id: s.id,
            name: s.name,
            url: s.url,
            event_types: s.event_types.0,
            min_severity: s.min_severity,
            has_secret: s.secret.as_deref().map(|v| !v.is_empty()).unwrap_or(false),
            enabled: s.enabled,
            cursor_at: s.cursor_at,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WebhookSubscriptionCreate {
    pub name: String,
    pub url: String,
    /// Omitted/empty = all types (`["*"]`).
    pub event_types: Option<Vec<String>>,
    /// `info` | `warning` | `critical` (default `info`).
    pub min_severity: Option<String>,
    /// Optional HMAC-SHA256 signing secret.
    pub secret: Option<String>,
    pub enabled: Option<bool>,
}

/// Partial update; an ABSENT field is left unchanged. `secret` is three-state: omitted = unchanged,
/// null = clear the secret, a value = set it (the outer `Option` distinguishes "field omitted" from
/// an explicit null — see [`de_field_present`]).
#[derive(Debug, Deserialize, Default)]
pub struct WebhookSubscriptionUpdate {
    pub name: Option<String>,
    pub url: Option<String>,
    pub event_types: Option<Vec<String>>,
    pub min_severity: Option<String>,
    #[serde(default, deserialize_with = "de_field_present")]
    pub secret: Option<Option<String>>,
    pub enabled: Option<bool>,
}

/// One webhook delivery attempt (the at-least-once retry ledger). `status` is `delivered` | `failed`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WebhookDelivery {
    pub id: String,
    pub subscription_id: String,
    pub event_id: Option<String>,
    pub event_type: Option<String>,
    pub status: String,
    pub attempts: i64,
    pub response_code: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_webhook_url_hides_path_and_token() {
        // Path/query/fragment are dropped behind an ellipsis; scheme + host (and port) are kept.
        assert_eq!(
            mask_webhook_url("https://hooks.slack.com/services/T000/B000/XXXXSECRET"),
            Some("https://hooks.slack.com/…".to_string())
        );
        assert_eq!(
            mask_webhook_url("https://example.com:8443/alert?token=abc"),
            Some("https://example.com:8443/…".to_string())
        );
        // Host-only urls keep just scheme://host.
        assert_eq!(
            mask_webhook_url("https://example.com"),
            Some("https://example.com".to_string())
        );
        // Empty/whitespace => None; schemeless => fully masked (may be a bare token).
        assert_eq!(mask_webhook_url("   "), None);
        assert_eq!(mask_webhook_url("not-a-url"), Some("…".to_string()));
    }

    #[test]
    fn webhook_update_secret_is_three_state() {
        // Omitted => None (leave the signing secret unchanged).
        let u: WebhookSubscriptionUpdate = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(u.secret.is_none());
        assert_eq!(u.enabled, Some(true));
        // Explicit null => Some(None) (clear the secret).
        let u: WebhookSubscriptionUpdate = serde_json::from_str(r#"{"secret": null}"#).unwrap();
        assert_eq!(u.secret, Some(None));
        // A value => Some(Some(v)) (set the secret).
        let u: WebhookSubscriptionUpdate = serde_json::from_str(r#"{"secret": "s3cr3t"}"#).unwrap();
        assert_eq!(u.secret, Some(Some("s3cr3t".to_string())));
    }
}
