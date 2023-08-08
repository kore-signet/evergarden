use std::{
    io,
    ops::{Deref, DerefMut},
};

use evergarden_common::{EvergardenResult, HttpResponse};
use futures_util::TryStreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug)]
pub enum ClientRequest {
    Submit {
        // OPCODE = 0
        url: String,
    },
    Fetch {
        // OPCODE = 1
        url: String,
    },
    EndFile, // OPCODE = 2
}

#[repr(u8)]
pub enum ServerRequest {
    Submit = 0,
    AnswerFetch = 1,
    CloseScript = 2,
}

#[repr(transparent)]
pub struct ClientReader<R: AsyncRead> {
    reader: R,
}

impl<R: AsyncRead> Deref for ClientReader<R> {
    type Target = R;

    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

impl<R: AsyncRead> DerefMut for ClientReader<R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.reader
    }
}

impl<R: AsyncRead + Unpin> ClientReader<R> {
    pub fn new(reader: R) -> ClientReader<R> {
        ClientReader { reader }
    }

    pub async fn read_op(&mut self) -> std::io::Result<ClientRequest> {
        match self.reader.read_u8().await? {
            0 => {
                // SUBMIT
                let len = self.reader.read_u16_le().await?;
                let mut buffer = vec![0u8; len as usize];
                // dbg!(String::from_utf8_lossy(&buffer));
                // println!("reading...");
                self.read_exact(&mut buffer[..]).await?;

                // dbg!(String::from_utf8_lossy(&buffer));

                Ok(ClientRequest::Submit {
                    url: String::from_utf8(buffer)
                        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
                })
            }
            1 => {
                // FETCH
                let len = self.reader.read_u16_le().await?;
                let mut buffer = vec![0u8; len as usize];
                self.read_exact(&mut buffer[..]).await?;
                Ok(ClientRequest::Fetch {
                    url: String::from_utf8(buffer)
                        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
                })
            }
            2 => Ok(ClientRequest::EndFile),
            _ => Err(io::Error::from(io::ErrorKind::InvalidData)),
        }
    }
}

#[repr(transparent)]
pub struct ClientWriter<W: AsyncWrite> {
    writer: W,
}

impl<W: AsyncWrite> Deref for ClientWriter<W> {
    type Target = W;

    fn deref(&self) -> &Self::Target {
        &self.writer
    }
}

impl<W: AsyncWrite> DerefMut for ClientWriter<W> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.writer
    }
}

impl<W: AsyncWrite + Unpin> ClientWriter<W> {
    pub fn new(writer: W) -> ClientWriter<W> {
        ClientWriter { writer }
    }

    pub async fn submit(&mut self, res: &HttpResponse) -> EvergardenResult<()> {
        self.writer.write_u8(ServerRequest::Submit as u8).await?;
        self.write_res(res).await
    }

    pub async fn close_script(&mut self) -> io::Result<()> {
        self.writer
            .write_u8(ServerRequest::CloseScript as u8)
            .await?;
        self.writer.flush().await
    }

    pub async fn error_fetch(&mut self, err: &str) -> io::Result<()> {
        self.writer
            .write_u8(ServerRequest::AnswerFetch as u8)
            .await?;
        self.writer.write_u8(1).await?; // IS AN ERROR
        self.writer.write_u64_le(err.len() as u64).await?;
        self.writer.write_all(err.as_bytes()).await?;
        self.writer.flush().await?;

        Ok(())
    }

    pub async fn answer_fetch(&mut self, res: &HttpResponse) -> EvergardenResult<()> {
        self.writer
            .write_u8(ServerRequest::AnswerFetch as u8)
            .await?;
        self.writer.write_u8(0).await?; // NOT AN ERROR

        self.write_res(res).await
    }

    async fn write_res(&mut self, res: &HttpResponse) -> EvergardenResult<()> {
        let meta_json = serde_json::to_vec(res.meta.as_ref()).unwrap();

        self.writer.write_u64_le(meta_json.len() as u64).await?;
        self.writer.write_all(&meta_json).await?;

        let mut body = res.body.clone();

        while let Some(chunk) = body.try_next().await? {
            self.writer.write_u64_le(chunk.len() as u64).await?;
            self.writer.write_all(&chunk).await?;
            self.writer.flush().await?;
        }

        self.writer.write_u64_le(0).await?;

        // self.writer.write_all(&res.body).await?;

        self.writer.flush().await?;

        Ok(())
    }
}
