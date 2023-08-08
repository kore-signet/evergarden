use std::{
    collections::BTreeMap,
    num::{NonZeroU32, NonZeroUsize},
    sync::Arc,
    time::Duration,
};

use actors::Mailbox;
use evergarden_common::{HttpResponse, ResponseMetadata};
use governor::Quota;
use hyper::header::CONTENT_TYPE;
use neo_mime::{MediaRange, MediaType};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::client::HttpClient;

#[derive(Clone)]
pub struct GlobalState {
    pub config: GlobalConfig,
    pub client: Mailbox<HttpClient>,
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub max_hops: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default)]
    pub max_body_length: Option<usize>,
    #[serde(default)]
    pub headers: Vec<HeaderPair>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ScriptConfig {
    pub filter: ScriptFilter,
    pub command: String,
    pub args: Vec<String>,
    pub workers: usize,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ScriptFilter {
    #[serde(with = "serde_regex", default)]
    pub(crate) url_pattern: Option<Regex>,
    #[serde(default)]
    pub(crate) mime_types: Vec<MediaRange>,
}

impl ScriptFilter {
    pub fn matches(&self, data: &HttpResponse) -> bool {
        self.matches_url(data.meta.url.url.as_str()) && self.matches_types(&data.meta)
    }

    fn matches_url(&self, url: &str) -> bool {
        self.url_pattern
            .as_ref()
            .map(|pat| pat.is_match(url))
            .unwrap_or(true)
    }

    fn matches_types(&self, data: &ResponseMetadata) -> bool {
        data.headers
            .get(CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| MediaType::parse(header).ok())
            .map(|header| self.mime_types.iter().any(|range| range.matches(&header)))
            .unwrap_or(true)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitingDuration {
    Second,
    Minute,
    Hour,
}

impl RateLimitingDuration {
    pub fn as_duration(&self) -> Duration {
        match self {
            RateLimitingDuration::Second => Duration::from_secs(1),
            RateLimitingDuration::Minute => Duration::from_secs(60),
            RateLimitingDuration::Hour => Duration::from_secs(60 * 60),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct RateLimitingConfig {
    pub max_tasks_per_worker: NonZeroUsize,
    pub n: NonZeroU32,
    pub per: RateLimitingDuration,
    #[serde(with = "humantime_serde")]
    pub jitter: Duration,
}

impl Default for RateLimitingConfig {
    fn default() -> Self {
        Self {
            max_tasks_per_worker: NonZeroUsize::new(16).unwrap(),
            n: NonZeroU32::new(200).unwrap(),
            per: RateLimitingDuration::Second,
            jitter: Duration::from_millis(50),
        }
    }
}

impl RateLimitingConfig {
    pub fn as_quota(&self) -> Quota {
        let replenish_interval_ns = self.per.as_duration().as_nanos() / (self.n.get() as u128);
        Quota::with_period(Duration::from_nanos(replenish_interval_ns as u64))
            .unwrap()
            .allow_burst(self.n)
    }
}

#[derive(Serialize, Deserialize)]
pub struct FullConfig {
    pub general: GlobalConfig,
    pub ratelimiter: RateLimitingConfig,
    pub http: HttpConfig,
    pub scripts: BTreeMap<Arc<str>, ScriptConfig>,
}
