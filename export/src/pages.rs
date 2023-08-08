use core::fmt;
use std::{
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

use evergarden_common::{EvergardenResult, ResponseMetadata};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{file_digest, DataPackageEntry};

#[derive(Serialize)]
struct PageHeader<'a> {
    format: &'static str,
    id: &'a str,
    title: &'a str,
}

#[derive(Serialize)]
struct PageEntry<'a> {
    id: Uuid,
    url: &'a str,
    #[serde(with = "time::serde::rfc3339")]
    ts: OffsetDateTime,
}

pub struct PagesWriter<W: Write + Read + Seek> {
    main: BufWriter<W>,
    extra: BufWriter<W>,
}

impl<W: Write + Read + Seek + fmt::Debug> PagesWriter<W> {
    pub fn new(main: W, extra: W) -> EvergardenResult<Self> {
        let mut main = BufWriter::new(main);
        let mut extra = BufWriter::new(extra);

        main.start_pages("entrypoint-pages", "main pages!")?;
        extra.start_pages("extra-pages", "crawled pages")?;

        Ok(PagesWriter { main, extra })
    }

    pub fn add_entry(&mut self, record: &ResponseMetadata, is_main: bool) -> EvergardenResult<()> {
        if is_main {
            self.main.pages_entry(record)
        } else {
            self.extra.pages_entry(record)
        }
    }

    pub fn finalize(
        mut self,
        path: impl AsRef<Path>,
    ) -> EvergardenResult<((W, DataPackageEntry), (W, DataPackageEntry))> {
        self.main.flush()?;
        self.extra.flush()?;

        let mut main_file = self.main.into_inner().unwrap();

        let main_digest = file_digest(&mut main_file)?;
        let main_len = main_file.seek(SeekFrom::End(0))?;

        main_file.rewind()?;

        let mut extra_file = self.extra.into_inner().unwrap();
        let extra_digest = file_digest(&mut extra_file)?;
        let extra_len = extra_file.seek(SeekFrom::End(0))?;

        extra_file.rewind()?;

        Ok((
            (
                main_file,
                DataPackageEntry {
                    name: "pages.jsonl".to_owned(),
                    path: path
                        .as_ref()
                        .join("pages.jsonl")
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    hash: main_digest,
                    bytes: main_len,
                },
            ),
            (
                extra_file,
                DataPackageEntry {
                    name: "extraPages.jsonl".to_owned(),
                    path: path
                        .as_ref()
                        .join("extraPages.jsonl")
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    hash: extra_digest,
                    bytes: extra_len,
                },
            ),
        ))
    }
}
pub trait PageEntryWriter: Write {
    fn start_pages(&mut self, id: &str, title: &str) -> EvergardenResult<()> {
        self.write_all(&serde_json::to_vec(&PageHeader {
            format: "json-pages-1.0",
            id,
            title,
        })?)?;

        self.write_all(b"\n")?;

        Ok(())
    }

    fn pages_entry(&mut self, record: &ResponseMetadata) -> EvergardenResult<()> {
        self.write_all(&serde_json::to_vec(&PageEntry {
            id: record.id,
            url: record.url.url.as_str(),
            ts: record.fetched_at,
        })?)?;

        self.write_all(b"\n")?;
        Ok(())
    }
}

impl<W> PageEntryWriter for W where W: Write {}
