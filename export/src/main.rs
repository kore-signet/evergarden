use std::{
    fs::File,
    io::{self, BufWriter, Read, Write, Seek, BufReader}, error::Error,
};

use evergarden_common::{ResponseMetadata, Storage};
use flate2::{write::GzEncoder, Compression};
use tempfile::tempfile;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

pub trait RecordWriter: Write {
    fn line_end(&mut self) -> io::Result<()> {
        self.write_all(b"\r\n")
    }

    fn line(&mut self, line: impl AsRef<[u8]>) -> io::Result<()> {
        self.write_all(line.as_ref())?;
        self.line_end()
    }

    fn header(&mut self, name: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> io::Result<()> {
        self.write_all(name.as_ref())?;
        self.write_all(b": ")?;
        self.write_all(value.as_ref())?;
        self.line_end()
    }
}

impl<T> RecordWriter for T where T: Write {}

fn write_http_response(meta: &ResponseMetadata, body: &mut impl Read, out: &mut (impl Write + Seek)) -> std::io::Result<u64> {
    out.line(format!(
        "{:?} {} {}",
        meta.version,
        meta.status,
        meta.status
            .canonical_reason()
            .unwrap_or("<unknown status code>")
    ))?;

    for (name, value) in meta.headers.iter() {
        out.header(name.as_str(), value.as_bytes())?;
    }

    out.line("")?;

    std::io::copy(body, out)?;

    out.flush()?;

    let end = out.stream_position()?;

    Ok(end)
}


struct WarcRecorder {
    output: BufWriter<File>,
}

impl WarcRecorder {
    pub fn write_warc(&mut self, meta: &ResponseMetadata, body: &mut impl Read) -> std::io::Result<()> {
        use http::Version;

        let mut http_block_out = BufWriter::new(tempfile()?);
        let content_len = write_http_response(meta, body, &mut http_block_out)?;
        http_block_out.flush()?;

        let mut http_block_out = http_block_out.into_inner().unwrap();
        http_block_out.rewind()?;


        let mut out = GzEncoder::new(&mut self.output, Compression::new(5));

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

        out.header("Content-Length", content_len.to_string())?;

        out.line("")?;

        std::io::copy(&mut BufReader::new(http_block_out), &mut out)?;

        out.flush()?;
        out.finish()?;
  
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut output = WarcRecorder {
        output: BufWriter::new(File::create("out.warc.gz")?)
    };
    
    let storage = Storage::new("results.db", false)?;

    for res in storage.list() {
        let (key, hash, meta) = res?;

        println!("writing {key}");

        let mut body = storage.read_body_sync(hash)?.unwrap();
        output.write_warc(&meta, &mut body)?;
    }
    Ok(())
}
