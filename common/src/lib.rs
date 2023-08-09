#![feature(impl_trait_in_assoc_type)]
#![feature(return_position_impl_trait_in_trait)]

use std::{
    fmt::{Debug, Display},
    net::SocketAddr,
    sync::Arc,
};

use bytes::Bytes;

use hyper::{http::HeaderValue, HeaderMap, StatusCode, Version};
use serde::{Deserialize, Serialize};

use thiserror::Error;

// pub mod client;
// pub mod recorder;
// pub mod config;
// pub mod recorder;
// pub mod scripting;
pub mod surt;
pub use surt::*;

mod storage;
pub use storage::*;

use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum EvergardenError {
    #[error(transparent)]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    BodyRead(Arc<BodyReadError>),
    #[error(transparent)]
    JSON(#[from] serde_json::Error),
    #[error(transparent)]
    Cache(#[from] cacache::Error),
    #[error(transparent)]
    LZ4(#[from] lz4_flex::frame::Error),
}

impl From<BodyReadError> for EvergardenError {
    fn from(value: BodyReadError) -> Self {
        Self::BodyRead(Arc::new(value))
    }
}

impl From<Arc<BodyReadError>> for EvergardenError {
    fn from(value: Arc<BodyReadError>) -> Self {
        Self::BodyRead(value)
    }
}

#[derive(Error, Debug)]
pub enum BodyReadError {
    #[error(transparent)]
    Client(#[from] hyper::Error),
    #[error(transparent)]
    IOError(#[from] std::io::Error),
    #[error("response timed out")]
    TimedOut,
    #[error("response body excedeed limit")]
    BodyTooLarge,
}

pub type EvergardenResult<T> = Result<T, EvergardenError>;
pub type BodyResult<T> = Result<T, Arc<BodyReadError>>;

#[derive(Clone, Serialize, Deserialize)]
pub struct UrlInfo {
    pub url: Url,
    pub discovered_in: Url,
    pub hops: usize,
}

impl Debug for UrlInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlInfo")
            .field("url", &self.url.as_str())
            .field("discovered_in", &self.discovered_in.as_str())
            .field("hops", &self.hops)
            .finish()
    }
}

impl Display for UrlInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (discovered in {}, hops:{})",
            self.url.as_str(),
            self.discovered_in.as_str(),
            self.hops
        )
    }
}

impl UrlInfo {
    pub fn start(url: &str) -> Option<UrlInfo> {
        let url = Url::parse(url).ok()?;
        Some(UrlInfo {
            url: url.clone(),
            discovered_in: url,
            hops: 0,
        })
    }
    pub fn hop(mut self, new_url: &str) -> Option<UrlInfo> {
        let new_url = self.url.join(new_url).ok()?;

        if new_url.host() != self.url.host() {
            self.hops += 1;
        }

        self.discovered_in = self.url;
        self.url = new_url;

        Some(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseMetadata {
    pub url: UrlInfo,
    #[serde(with = "http_serde::status_code")]
    pub status: StatusCode,
    #[serde(with = "http_serde::version")]
    pub version: Version,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap<HeaderValue>,
    pub remote_addr: Option<SocketAddr>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
    pub id: Uuid,
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub meta: Arc<ResponseMetadata>,
    pub body: async_broadcast::Receiver<BodyResult<Bytes>>,
}

impl Display for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.meta.status)
        // write!(f, "{} {}", self.meta.url.url.as_str(), self.meta.status)
    }
}

#[derive(Serialize, Deserialize)]
pub struct CrawlInfo {
    pub config: String,
    pub entry_points: Vec<String>,
}
