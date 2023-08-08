use std::{
    fs::{File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, Write},
    path::{Path, PathBuf},
};

use evergarden_common::ResponseMetadata;
use flate2::{write::GzEncoder, Compression};
use http::header::CONTENT_TYPE;
use neo_mime::MediaType;

use tempfile::tempfile;
use time::format_description::well_known::Rfc3339;

use crate::{
    cdxj::{self, CDXRecord},
    file_digest, sha256_as_string,
    writer::{HttpResponseWriter, RecordWriter},
    DataPackageEntry,
};

pub trait WarcRecorder {
    fn write_warc(
        &mut self,
        surt: &str,
        meta: &ResponseMetadata,
        body: &mut impl Read,
    ) -> std::io::Result<CDXRecord>;

    fn write_raw_warc(
        &mut self,
        meta: &ResponseMetadata,
        http_block: &mut impl Read,
        digest: &[u8; 32],
        content_len: u64,
    ) -> std::io::Result<()>;
}

impl WarcRecorder for BufWriter<File> {
    fn write_warc(
        &mut self,
        surt: &str,
        meta: &ResponseMetadata,
        body: &mut impl Read,
    ) -> std::io::Result<CDXRecord> {
        let mut http_block_out = BufWriter::new(tempfile()?);
        let content_len = http_block_out.write_http_response(meta, body)?;
        http_block_out.flush()?;

        let mut http_block_out = http_block_out.into_inner().unwrap();
        http_block_out.sync_data()?;

        let block_digest = file_digest(&mut http_block_out)?;

        http_block_out.rewind()?;

        let start_position = self.stream_position()?;

        self.write_raw_warc(
            meta,
            &mut BufReader::new(http_block_out),
            &block_digest,
            content_len,
        )?;
        self.flush()?;

        let end_position = self.stream_position()?;

        Ok(CDXRecord {
            key: surt.to_owned(),
            time: meta.fetched_at,
            block: cdxj::CDXJBlock {
                url: meta.url.url.to_string(),
                digest: block_digest,
                mime: meta
                    .headers
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| MediaType::parse(v).ok())
                    .map(|v| v.without_params()),
                filename: String::new(),
                offset: start_position,
                length: end_position - start_position,
                status: meta.status.as_u16(),
            },
        })
    }

    fn write_raw_warc(
        &mut self,
        meta: &ResponseMetadata,
        http_block: &mut impl Read,
        digest: &[u8; 32],
        content_len: u64,
    ) -> std::io::Result<()> {
        use http::Version;

        let mut out = GzEncoder::new(self, Compression::new(5));

        out.line("WARC/1.1")?;

        out.header("WARC-Target-URI", meta.url.url.as_str())?;
        out.header("Content-Type", "application/http;msgtype=response")?;
        out.header("WARC-Type", "response")?;
        out.header("WARC-Date", meta.fetched_at.format(&Rfc3339).unwrap())?;
        out.header(
            "WARC-Record-ID",
            format!("<urn:uuid:{}>", meta.id.hyphenated()),
        )?;

        if let Some(ip) = meta.remote_addr {
            out.header("WARC-IP-Address", ip.to_string())?;
        }

        out.header(
            "WARC-Protocol",
            match meta.version {
                Version::HTTP_09 => "http/0.9",
                Version::HTTP_10 => "http/1.0",
                Version::HTTP_11 => "http/1.1",
                Version::HTTP_2 => "h2",
                Version::HTTP_3 => "h3",
                _ => unreachable!(),
            },
        )?;

        out.header("WARC-Block-Digest", sha256_as_string(digest))?;
        out.header("Content-Length", content_len.to_string())?;

        out.line("")?;

        std::io::copy(http_block, &mut out)?;

        out.flush()?;
        out.finish()?;

        Ok(())
    }
}

pub struct RotatingWarcRecorder {
    threshold: u64,
    counter: usize,
    packaged_path: PathBuf,
    dir: PathBuf,
    current_file: BufWriter<File>,
    digests: Vec<(usize, [u8; 32], u64)>,
}

impl RotatingWarcRecorder {
    pub fn new(
        dir: impl AsRef<Path>,
        packaged_path: impl AsRef<Path>,
        threshold: u64,
    ) -> std::io::Result<RotatingWarcRecorder> {
        let first_file_name = dir.as_ref().join(format!("{:05}.warc.gz", 0));

        let first_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(first_file_name)?;

        Ok(RotatingWarcRecorder {
            threshold,
            counter: 0,
            packaged_path: packaged_path.as_ref().to_path_buf(),
            dir: dir.as_ref().to_path_buf(),
            current_file: BufWriter::new(first_file),
            digests: Vec::new(),
        })
    }

    pub fn rotate(&mut self) -> std::io::Result<()> {
        self.counter += 1;

        self.current_file.flush()?;

        let next_file_name = self.dir.join(format!("{:05}.warc.gz", self.counter));
        let next_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(next_file_name)?;
        let old_file = std::mem::replace(&mut self.current_file, BufWriter::new(next_file));

        self.add_digest(
            self.counter.saturating_sub(1),
            &mut old_file.into_inner().unwrap(),
        )?;

        Ok(())
    }

    pub fn add_digest(&mut self, idx: usize, file: &mut File) -> io::Result<()> {
        let len = file.metadata()?.len();

        let digest = file_digest(file)?;

        self.digests.push((idx, digest, len));

        Ok(())
    }

    pub fn finalize(self) -> std::io::Result<Vec<DataPackageEntry>> {
        let Self {
            counter,

            mut current_file,
            mut digests,
            ..
        } = self;

        current_file.flush()?;

        let mut current_file = current_file.into_inner().unwrap();

        digests.push((
            counter,
            file_digest(&mut current_file)?,
            current_file.metadata()?.len(),
        ));

        Ok(digests
            .into_iter()
            .map(|(index, digest, len)| DataPackageEntry {
                name: format!("{:05}.warc.gz", index),
                path: self
                    .packaged_path
                    .join(format!("{:05}.warc.gz", index))
                    .to_str()
                    .unwrap()
                    .to_owned(),
                hash: digest,
                bytes: len,
            })
            .collect())
    }
}

impl WarcRecorder for RotatingWarcRecorder {
    fn write_warc(
        &mut self,
        surt: &str,
        meta: &ResponseMetadata,
        body: &mut impl Read,
    ) -> std::io::Result<CDXRecord> {
        let mut cdx = self.current_file.write_warc(surt, meta, body)?;
        cdx.block.filename = format!("{:05}.warc.gz", self.counter);

        if cdx.block.offset + cdx.block.length > self.threshold {
            self.rotate()?;
        }

        Ok(cdx)
    }

    fn write_raw_warc(
        &mut self,
        meta: &ResponseMetadata,
        http_block: &mut impl Read,
        digest: &[u8; 32],
        content_len: u64,
    ) -> std::io::Result<()> {
        self.current_file
            .write_raw_warc(meta, http_block, digest, content_len)
    }
}
