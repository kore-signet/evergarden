use std::io::{self, Read, Seek, Write};

use evergarden_common::ResponseMetadata;

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

pub trait HttpResponseWriter: Write + Seek + RecordWriter {
    fn write_http_response(
        &mut self,
        meta: &ResponseMetadata,
        body: &mut impl Read,
    ) -> std::io::Result<u64> {
        self.line(format!(
            "{:?} {} {}",
            meta.version,
            meta.status,
            meta.status
                .canonical_reason()
                .unwrap_or("<unknown status code>")
        ))?;

        for (name, value) in meta.headers.iter() {
            self.header(name.as_str(), value.as_bytes())?;
        }

        self.line("")?;

        std::io::copy(body, self)?;

        self.flush()?;

        let end = self.stream_position()?;

        Ok(end)
    }
}

impl<T> RecordWriter for T where T: Write {}
impl<T> HttpResponseWriter for T where T: Write + Seek {}
