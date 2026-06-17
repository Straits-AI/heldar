//! Semantic-search configuration (loaded from env by the composing server).

fn parse_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Debug)]
pub struct SearchConfig {
    /// Optional OpenAI-compatible chat-completions endpoint used ONLY to translate a question into a
    /// structured query plan (never to produce answers). Empty ⇒ the rule-based planner is used.
    pub llm_url: Option<String>,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
    /// Max hits returned per search.
    pub max_results: i64,
    /// How long the `search_log` query-history/audit rows are kept before retention prunes them.
    pub query_log_retention_days: i64,
}

impl SearchConfig {
    pub fn from_env() -> Self {
        SearchConfig {
            llm_url: std::env::var("HELDAR_SEARCH_LLM_URL")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            llm_api_key: std::env::var("HELDAR_SEARCH_LLM_API_KEY")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            llm_model: std::env::var("HELDAR_SEARCH_LLM_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "gpt-4o-mini".to_string()),
            max_results: parse_or::<i64>("HELDAR_SEARCH_MAX_RESULTS", 200).clamp(1, 5000),
            query_log_retention_days: parse_or::<i64>("HELDAR_SEARCH_QUERY_LOG_RETENTION_DAYS", 90)
                .max(1),
        }
    }
}
