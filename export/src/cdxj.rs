use std::{
    fmt::Debug,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
};

use neo_mime::MediaType;
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

use crate::{file_digest, DataPackageEntry};

// static FORMATTING =!_descr
static TIME_FMT: &[FormatItem<'_>] =
    format_description!("[year][month][day][hour repr:24][minute][second]");

pub struct CDXWriter<W: Write + Read + Seek> {
    out: BufWriter<W>,
}

impl<W: Write + Read + Seek> CDXWriter<W> {
    pub fn new(out: W) -> Self {
        CDXWriter {
            out: BufWriter::new(out),
        }
    }
}

impl<W: Write + Read + Seek + Debug> CDXWriter<W> {
    pub fn write_batch(&mut self, batch: &[CDXRecord]) -> std::io::Result<()> {
        let mut lines: Vec<Vec<u8>> = batch.iter().map(CDXRecord::to_line).collect();
        lines.sort();

        for line in lines {
            self.out.write_all(&line)?;
            self.out.write_all(b"\n")?;
        }

        Ok(())
    }

    pub fn finalize(mut self, name: &str, path: &str) -> io::Result<(W, DataPackageEntry)> {
        self.out.flush()?;

        let mut file = self.out.into_inner().unwrap();

        let digest = file_digest(&mut file)?;
        let len = file.seek(SeekFrom::End(0))?;

        file.rewind()?;

        Ok((
            file,
            DataPackageEntry {
                name: name.to_owned(),
                path: path.to_owned(),
                hash: digest,
                bytes: len,
            },
        ))
    }
}

#[derive(Clone)]
pub struct CDXRecord {
    pub key: String,
    pub time: OffsetDateTime,
    pub block: CDXBlock,
}

impl CDXRecord {
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
pub struct CDXBlock {
    pub url: String,
    #[serde(serialize_with = "crate::ser_sha256_as_str")]
    pub digest: [u8; 32],
    pub mime: MediaType,
    pub filename: String,
    pub offset: u64,
    pub length: u64,
    pub status: u16,
}
