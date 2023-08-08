use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use actors::Actor;
use bytes::BytesMut;
use cacache::{SyncReader, WriteOpts};
use futures_util::{Future, TryFutureExt, TryStreamExt};
use lz4_flex::frame::{FrameDecoder, FrameEncoder};

use ssri::Integrity;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use url::Url;

use crate::{surt, EvergardenError, EvergardenResult};
use crate::{BodyReadError, HttpResponse, ResponseMetadata};

struct SyncBridge<T> {
    inner: T,
    handle: Handle,
}

impl<T> SyncBridge<T> {
    pub fn new(inner: T) -> SyncBridge<T> {
        SyncBridge {
            inner,
            handle: Handle::current(),
        }
    }
}

impl<T> Write for SyncBridge<T>
where
    T: AsyncWrite + Unpin,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.handle.block_on(self.inner.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.handle.block_on(self.inner.flush())
    }
}

impl<T> Read for SyncBridge<T>
where
    T: AsyncRead + Unpin,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.handle.block_on(self.inner.read(buf))
    }
}

#[derive(Clone)]
pub struct Storage {
    path: PathBuf,
}

impl Storage {
    pub fn new(path: impl AsRef<Path>, drop_tables: bool) -> EvergardenResult<Storage> {
        let path = PathBuf::from(path.as_ref());

        if drop_tables {
            cacache::clear_sync(&path)?;
        }

        Ok(Storage { path })
    }

    pub async fn write_res(&self, res: HttpResponse) -> EvergardenResult<()> {
        let key = surt(res.meta.url.url.clone());
        self.write_by_key(&key, res).await
    }

    pub async fn write_by_key(&self, key: &str, res: HttpResponse) -> EvergardenResult<()> {
        tokio::task::block_in_place(|| -> EvergardenResult<()> {
            let handle = Handle::current();
            let HttpResponse { meta, mut body } = res;

            let json_header = serde_json::to_value(meta.as_ref())?;

            let write_opts = WriteOpts::new()
                .algorithm(cacache::Algorithm::Xxh3)
                .metadata(json_header)
                .time(meta.fetched_at.unix_timestamp_nanos() as u128);

            let file = SyncBridge::new(handle.block_on(write_opts.open(&self.path, key))?);

            let mut encoder = FrameEncoder::new(file);

            while let Some(chunk) = handle.block_on(body.try_next())? {
                encoder.write_all(&chunk)?;
            }

            let mut finished = encoder.finish()?.inner;
            handle.block_on(finished.flush())?;
            handle.block_on(finished.commit())?;

            Ok(())
        })
    }

    pub async fn retrieve_by_url(&self, url: Url) -> EvergardenResult<Option<HttpResponse>> {
        let key = surt(url);
        self.retrieve_by_key(&key).await
    }

    pub async fn retrieve_by_key(&self, key: &str) -> EvergardenResult<Option<HttpResponse>> {
        let Some(metadata) = cacache::metadata(&self.path, key).await? else {
            return Ok(None);
        };

        let metadata: ResponseMetadata = serde_json::from_value(metadata.metadata)?;

        let reader = SyncBridge::new(cacache::Reader::open(&self.path, key).await?);
        let mut decoder = FrameDecoder::new(reader);
        let (tx, rx) = async_broadcast::broadcast(1024);

        tokio::task::spawn_blocking(move || {
            let handle = Handle::current();

            loop {
                let mut buffer = BytesMut::zeroed(8096);
                let n = match decoder.read(&mut buffer) {
                    Ok(n) => n,
                    Err(e) => {
                        let _ =
                            handle.block_on(tx.broadcast(Err(Arc::new(BodyReadError::IOError(e)))));
                        tx.close();
                        return;
                    }
                };

                if n == 0 {
                    tx.close();
                    return;
                }

                buffer.truncate(n);
                let _ = handle.block_on(tx.broadcast(Ok(buffer.freeze())));
            }
        });

        Ok(Some(HttpResponse {
            meta: Arc::new(metadata),
            body: rx,
        }))
    }

    pub fn read_body_sync(
        &self,
        hash: Integrity,
    ) -> EvergardenResult<Option<FrameDecoder<cacache::SyncReader>>> {
        if !cacache::exists_sync(&self.path, &hash) {
            return Ok(None);
        }

        Ok(Some(FrameDecoder::new(SyncReader::open_hash(
            &self.path, hash,
        )?)))
    }

    pub fn list(
        &self,
    ) -> impl Iterator<Item = EvergardenResult<(String, Integrity, ResponseMetadata)>> + '_ {
        cacache::list_sync(&self.path).map(
            |res| -> EvergardenResult<(String, Integrity, ResponseMetadata)> {
                let res = match res {
                    Ok(v) => v,
                    Err(e) => return Err(EvergardenError::Cache(e)),
                };

                let headers: ResponseMetadata = serde_json::from_value(res.metadata)?;

                Ok((res.key, res.integrity, headers))
            },
        )
    }

    async fn answer_request(&mut self, i: StorageMessage) -> EvergardenResult<StorageResponse> {
        match i {
            StorageMessage::Retrieve(key) => {
                self.retrieve_by_url(key)
                    .map_ok(|v| StorageResponse::Retrieve(v))
                    .await
            }
            StorageMessage::Store(res) => {
                self.write_res(res)
                    .map_ok(|_| StorageResponse::Stored)
                    .await
            }
        }
    }
}

pub enum StorageMessage {
    Retrieve(Url),
    Store(HttpResponse),
}

pub enum StorageResponse {
    Retrieve(Option<HttpResponse>),
    Stored,
}

impl Actor for Storage {
    type Input = StorageMessage;

    type Output = EvergardenResult<StorageResponse>;

    type Response<'a> = impl Future<Output = Self::Output> + Send + 'a
    where
        Self: 'a;

    type CloseFuture<'a> = futures_util::future::Ready<()>
    where
        Self: 'a;

    fn close<'a>(self) -> Self::CloseFuture<'a> {
        futures_util::future::ready(())
    }

    fn answer(&mut self, i: Self::Input) -> Self::Response<'_> {
        self.answer_request(i)
    }
}
