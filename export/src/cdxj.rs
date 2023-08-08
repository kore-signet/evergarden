use std::{
    fmt::Debug,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

use flate2::{write::GzEncoder, Compression};
use neo_mime::MediaType;
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

use crate::{file_digest, DataPackageEntry};

// static FORMATTING =!_descr
static TIME_FMT: &[FormatItem<'_>] =
    format_description!("[year][month][day][hour repr:24][minute][second]");

const CDX_SPLIT_THRESHOLD: usize = 1000;

pub struct CDXWriter<W: Write + Read + Seek> {
    file_name: String,
    out: BufWriter<W>,
    aux: BufWriter<W>,
    buffer: Vec<CDXRecord>,
}

impl<W: Write + Read + Seek> CDXWriter<W> {
    pub fn new(out: W, aux: W) -> Self {
        CDXWriter {
            file_name: String::from("index.cdx.gz"),
            out: BufWriter::new(out),
            aux: BufWriter::new(aux),
            buffer: Vec::with_capacity(CDX_SPLIT_THRESHOLD),
        }
    }
}

impl<W: Write + Read + Seek + Debug> CDXWriter<W> {
    pub fn write_batch(
        &mut self,
        batch: impl IntoIterator<Item = CDXRecord>,
    ) -> std::io::Result<()> {
        self.buffer.extend(batch);

        if self.buffer.len() >= CDX_SPLIT_THRESHOLD {
            while !self.buffer.is_empty() {
                self.flush_lines()?;
            }
        }

        Ok(())
    }

    pub fn flush_lines(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let mut cdxj_lines = self
            .buffer
            .drain(..std::cmp::min(self.buffer.len(), CDX_SPLIT_THRESHOLD))
            .peekable();

        let mut index_line = CDXStyleRecord {
            key: cdxj_lines.peek().unwrap().key.clone(),
            time: cdxj_lines.peek().unwrap().time,
            block: ZipNumBlock {
                offset: self.out.stream_position()?,
                length: 0,
                digest: [0u8; 32],
                filename: self.file_name.clone(),
            },
        };

        let mut gzip_writer = GzEncoder::new(
            Vec::with_capacity(cdxj_lines.len() * 256),
            Compression::best(),
        );

        for line in cdxj_lines {
            gzip_writer.write_all(&line.to_line())?;
            gzip_writer.write_all(b"\n")?;
        }

        let gzip_slice = gzip_writer.finish()?;

        index_line.block.length = gzip_slice.len() as u64;
        index_line.block.digest = Sha256::digest(&gzip_slice).into();

        self.aux.write_all(&index_line.to_line())?;
        self.aux.write_all(b"\n")?;

        self.out.write_all(&gzip_slice)?;

        Ok(())
    }

    pub fn finalize(
        mut self,
        dir: impl AsRef<Path>,
    ) -> io::Result<((W, DataPackageEntry), (W, DataPackageEntry))> {
        while !self.buffer.is_empty() {
            self.flush_lines()?;
        }

        self.aux.flush()?;
        self.out.flush()?;

        let mut out_file = self.out.into_inner().unwrap();

        let out_digest = file_digest(&mut out_file)?;
        let out_len = out_file.seek(SeekFrom::End(0))?;

        out_file.rewind()?;

        let mut aux_file = self.aux.into_inner().unwrap();
        let aux_digest = file_digest(&mut aux_file)?;
        let aux_len = aux_file.seek(SeekFrom::End(0))?;

        aux_file.rewind()?;

        Ok((
            (
                out_file,
                DataPackageEntry {
                    path: dir
                        .as_ref()
                        .join(&self.file_name)
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    name: self.file_name,
                    hash: out_digest,
                    bytes: out_len,
                },
            ),
            (
                aux_file,
                DataPackageEntry {
                    name: "index.idx".to_owned(),
                    path: dir.as_ref().join("index.idx").to_str().unwrap().to_owned(),
                    hash: aux_digest,
                    bytes: aux_len,
                },
            ),
        ))
    }
}

pub type CDXRecord = CDXStyleRecord<CDXJBlock>;

#[derive(Clone)]
pub struct CDXStyleRecord<T: Serialize> {
    pub key: String,
    pub time: OffsetDateTime,
    pub block: T,
}

impl<S: Serialize> CDXStyleRecord<S> {
    pub fn to_line(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        out.extend_from_slice(self.key.as_bytes());
        out.push(b' ');

        self.time.format_into(&mut out, TIME_FMT).unwrap();
        out.push(b' ');

        serde_json::to_writer(&mut out, &self.block).unwrap();

        out.shrink_to_fit();

        out
    }
}

#[derive(serde::Serialize, Clone)]
pub struct CDXJBlock {
    pub url: String,
    #[serde(serialize_with = "crate::ser_sha256_as_str")]
    pub digest: [u8; 32],
    pub mime: Option<MediaType>,
    pub filename: String,
    pub offset: u64,
    pub length: u64,
    pub status: u16,
}

#[derive(serde::Serialize, Clone)]
pub struct ZipNumBlock {
    pub offset: u64,
    pub length: u64,
    #[serde(serialize_with = "crate::ser_sha256_as_str")]
    pub digest: [u8; 32],
    pub filename: String,
}
