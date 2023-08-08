use std::{str::FromStr, sync::Arc, time::Duration};

use actors::{Actor, Mailbox, Message, ProgramState};

use bytes::Bytes;
use evergarden_common::Storage;
use futures_util::{Future, TryStreamExt};
use governor::{Jitter, RateLimiter};
use hyper::{
    client::{connect::HttpInfo, HttpConnector},
    http::{HeaderName, HeaderValue},
    Body, Client, Request,
};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_trust_dns::TrustDnsResolver;

use time::OffsetDateTime;
use tokio::{
    sync::{watch, OwnedSemaphorePermit, Semaphore, SemaphorePermit},
    time::timeout,
};
use uuid::Uuid;

use crate::{
    config::{HeaderPair, HttpConfig, RateLimitingConfig},
    scripting::script::ScriptManager,
};

use evergarden_common::*;

type HttpsConn = HttpsConnector<HttpConnector<TrustDnsResolver>>;

#[derive(Clone)]
pub struct HttpRateLimiter {
    total_permits: usize,
    permits: Arc<Semaphore>,
    limiter: Arc<
        RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
            governor::middleware::NoOpMiddleware,
        >,
    >,
    jitter: Duration,
}

impl HttpRateLimiter {
    pub fn new(config: RateLimitingConfig) -> HttpRateLimiter {
        HttpRateLimiter {
            total_permits: config.max_tasks_per_worker.into(),
            permits: Arc::new(Semaphore::new(config.max_tasks_per_worker.into())),
            limiter: Arc::new(RateLimiter::direct(config.as_quota())),
            jitter: config.jitter,
        }
    }

    pub async fn acquire(&self) -> SemaphorePermit<'_> {
        let (permit, _) = tokio::join! {
            self.permits.acquire(),
            self.limiter.until_ready_with_jitter(Jitter::up_to(self.jitter))
        };

        permit.unwrap()
    }

    pub async fn acquire_owned(&self) -> OwnedSemaphorePermit {
        let (permit, _) = tokio::join! {
            self.permits.clone().acquire_owned(),
            self.limiter.until_ready_with_jitter(Jitter::up_to(self.jitter))
        };

        permit.unwrap()
    }

    pub fn is_idle(&self) -> bool {
        self.total_permits == self.permits.available_permits()
    }
}

#[derive(Clone)]
pub struct HttpClient {
    headers: Vec<(HeaderName, HeaderValue)>,
    limiter: HttpRateLimiter,
    client: Client<HttpsConn>,
    max_body_length: Option<usize>,
    timeout: Duration,
    storage: Mailbox<Storage>,
    scrapers: Mailbox<ScriptManager>,
}

impl HttpClient {
    pub fn new(
        http_config: &HttpConfig,
        rate: HttpRateLimiter,
        storage: Mailbox<Storage>,
        scripts: Mailbox<ScriptManager>,
    ) -> EvergardenResult<HttpClient> {
        let (dns_config, dns_options) =
            trust_dns_resolver::system_conf::read_system_conf().unwrap_or_default();
        let mut resolver = TrustDnsResolver::with_config_and_options(dns_config, dns_options)
            .into_http_connector();
        resolver.enforce_http(false);

        let connector = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http1()
            .wrap_connector(resolver);

        let hyper_client = Client::builder().build::<_, hyper::Body>(connector);

        Ok(HttpClient {
            storage,
            headers: http_config
                .headers
                .iter()
                .map(|HeaderPair { name, value }| {
                    (
                        HeaderName::from_str(&name).unwrap(),
                        HeaderValue::from_str(&value).unwrap(),
                    )
                })
                .collect::<Vec<_>>(),
            limiter: rate,
            client: hyper_client,
            max_body_length: http_config.max_body_length,
            timeout: http_config.timeout,
            scrapers: scripts,
        })
    }

    // pub (crate) fn write_body(&self, key: &str, mut body: hyper::Body) -> HttpResult<()> {

    // // }
    // pub(crate) async fn body_to_ivec(&self, mut body: hyper::Body) -> HttpResult<IVec> {
    //     let mut out = Vec::with_capacity(body.size_hint().lower() as usize);
    //     while let Some(data) = body.try_next().await? {
    //         out.put(data);

    //         if let Some(max_body_length) = self.max_body_length {
    //             if out.len() > max_body_length {
    //                 return Err(HttpError::BodyTooLarge);
    //             }
    //         }
    //     }

    //     Ok(IVec::from(out))
    // }

    pub async fn get(&self, url: UrlInfo) -> EvergardenResult<HttpResponse> {
        println!("fetching {}...", url.url.as_str());

        let mut request = Request::get(url.url.as_str());
        request
            .headers_mut()
            .unwrap()
            .extend(self.headers.iter().cloned());

        let fetched_at = OffsetDateTime::now_utc();

        let (header, body) = match timeout(
            self.timeout,
            self.client.request(request.body(Body::empty()).unwrap()),
        )
        .await
        {
            Ok(Ok(res)) => res.into_parts(),
            Ok(Err(e)) => return Err(BodyReadError::Client(e).into()),
            Err(_) => return Err(BodyReadError::TimedOut.into()),
        };

        let (body_tx, body_rx) = async_broadcast::broadcast(1024);
        let body_task = tokio::task::spawn(broadcast_body(self.max_body_length, body, body_tx));

        let res = HttpResponse {
            meta: Arc::new(ResponseMetadata {
                url,
                id: Uuid::new_v4(),
                status: header.status,
                version: header.version,
                headers: header.headers,
                remote_addr: header.extensions.get::<HttpInfo>().map(|v| v.remote_addr()),
                fetched_at,
            }),
            body: body_rx,
        };

        let (body, storage, scraper) = tokio::join!(
            body_task,
            self.storage.request(StorageMessage::Store(res.clone())),
            self.scrapers.request(res.clone())
        );

        body.unwrap()?;
        storage?;
        scraper?;

        // self.storage.insert(&res)?;
        // .unwrap();

        Ok(res)
    }
}

impl Actor for HttpClient {
    type Input = UrlInfo;

    type Output = EvergardenResult<HttpResponse>;

    type Response<'a> = futures_util::future::Ready<EvergardenResult<HttpResponse>>;

    fn answer<'a>(&'a mut self, _i: Self::Input) -> Self::Response<'a> {
        #[allow(unreachable_code)]
        futures_util::future::ready(unreachable!())
    }

    fn run_async_loop(
        self,
        rx: flume::Receiver<actors::Message<Self::Input, Self::Output>>,
        mut program_state: watch::Receiver<ProgramState>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            loop {
                tokio::select! {
                    Ok(Message { value, output }) = rx.recv_async() => {
                        if let Ok(StorageResponse::Retrieve(Some(res))) = self.storage.request(StorageMessage::Retrieve(value.url.clone())).await {
                            let _ = output.send(Ok(res)).unwrap();
                            continue;
                        }

                        let cli = self.clone();
                        let permit = cli.limiter.acquire_owned().await;
                        tokio::task::spawn(async move {
                            let res = cli.get(value).await;
                            let _ = output.send(res).unwrap();
                            drop(permit);
                        });
                    },
                    _ = program_state.changed() => {
                        break
                    },
                    else => break
                }
            }

            self.close().await;
        }
    }

    type CloseFuture<'a> = futures_util::future::Ready<()> where Self: 'a;

    fn close<'a>(self) -> Self::CloseFuture<'a> {
        futures_util::future::ready(())
    }
}

pub async fn broadcast_body(
    max_length: Option<usize>,
    mut body: hyper::Body,
    into: async_broadcast::Sender<BodyResult<Bytes>>,
) -> EvergardenResult<()> {
    let mut received = 0;
    loop {
        match body.try_next().await {
            Ok(Some(chunk)) => {
                received += chunk.len();
                if let Some(max_length) = max_length {
                    if received > max_length {
                        let _ = into
                            .broadcast(Err(Arc::new(BodyReadError::BodyTooLarge)))
                            .await;
                        into.close();
                        return Err(BodyReadError::BodyTooLarge.into());
                    }
                }

                let _ = into.broadcast(Ok(chunk)).await;
            }
            Ok(None) => {
                into.close();
                return Ok(());
            }
            Err(e) => {
                let e = Arc::new(BodyReadError::Client(e));
                let _ = into.broadcast(Err(Arc::clone(&e))).await;
                into.close();
                return Err(Arc::clone(&e).into());
            }
        }
    }
}
