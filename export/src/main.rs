use std::{
    error::Error,
    fs::{create_dir_all, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use evergarden_common::{CrawlInfo, EvergardenResult, ResponseMetadata, Storage};
use evergarden_export::{
    cdxj::CDXWriter,
    pages::PagesWriter,
    warc::{RotatingWarcRecorder, WarcRecorder},
    DataPackage, DataPackageEntry,
};
use itertools::Itertools;
use log::{debug, info};
use ssri::Integrity;

use clap::builder::TypedValueParser;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use ubyte::ByteUnit;
use zip::{write::FileOptions, CompressionMethod, ZipWriter};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(short, long)]
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(
        long,
        value_parser = clap::builder::PossibleValuesParser::new(["off", "error", "warn", "info", "debug", "trace"])
            .map(|s| s.parse::<log::LevelFilter>().unwrap()),
    )]
    log_level: Option<log::LevelFilter>,
}

fn open(path: impl AsRef<Path>) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path.as_ref())
}

trait ZipWriterExt {
    fn add_file(
        &mut self,
        path: &str,
        reader: impl Read,
        compression: Option<i32>,
    ) -> io::Result<()>;
}

impl<W: Write + Seek> ZipWriterExt for ZipWriter<W> {
    fn add_file(
        &mut self,
        path: &str,
        reader: impl Read,
        compression: Option<i32>,
    ) -> io::Result<()> {
        let opts = if let Some(level) = compression {
            FileOptions::default()
                .compression_level(Some(level))
                .compression_method(CompressionMethod::Deflated)
        } else {
            FileOptions::default().compression_method(CompressionMethod::Stored)
        };

        self.start_file(path, opts)?;
        std::io::copy(&mut BufReader::new(reader), self)?;

        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    if let Some(level) = args.log_level {
        pretty_env_logger::formatted_builder()
            .filter_module(module_path!(), level)
            .init();
    } else {
        pretty_env_logger::init();
    }

    debug!("opening storage");

    let storage = Storage::new(&args.input, false)?;

    let output_dir = tempfile::tempdir_in("./")?;
    let output_path = PathBuf::from(output_dir.path());

    let _ = create_dir_all(output_path.join("archive"));
    let _ = create_dir_all(output_path.join("indexes"));
    let _ = create_dir_all(output_path.join("pages"));

    // set up our writers

    debug!("opening output files");

    let mut warc_writer = RotatingWarcRecorder::new(
        output_path.join("archive"),
        "archive/",
        ByteUnit::Gigabyte(1).as_u64(),
    )?;

    let mut cdx_writer = CDXWriter::new(
        open(output_path.join("indexes/index.cdx.gz"))?,
        open(output_path.join("indexes/index.idx"))?,
    );

    let mut pages_writer = PagesWriter::new(
        open(output_path.join("pages/pages.jsonl"))?,
        open(output_path.join("pages/extraPages.jsonl"))?,
    )?;

    // get a list of records from our storage

    let mut records = storage
        .list()?
        .collect::<EvergardenResult<Vec<(String, Integrity, ResponseMetadata)>>>()
        .unwrap();

    info!("found {} WARC records!", records.len());

    // sort our records by time, key

    records.sort_unstable_by(|(lkey, _, lmeta), (rkey, _, rmeta)| {
        (lkey, lmeta.fetched_at.to_hms()).cmp(&(rkey, rmeta.fetched_at.to_hms()))
    });

    let CrawlInfo {
        mut entry_points, ..
    } = storage.read_info_sync()?;
    entry_points.sort();

    // writes records, batch by batch. ensures resulting CDXJ will be sorted
    for (_, group) in &records
        .into_iter()
        .group_by(|(lkey, _, lmeta)| (lkey.clone(), lmeta.fetched_at.to_hms()))
    {
        let mut records = Vec::with_capacity(8);

        for (key, hash, meta) in group {
            debug!("writing record {key}");

            pages_writer.add_entry(&meta, entry_points.binary_search(&key).is_ok())?;

            let cdx =
                warc_writer.write_warc(&key, &meta, &mut storage.read_body_sync(hash)?.unwrap())?;
            records.push(cdx.clone());
        }

        cdx_writer.write_batch(records)?;
    }

    // get our metadata in order

    info!("finishing up WARC/CDX export");

    let warc_entries = warc_writer.finalize()?;

    let mut all_entries = Vec::new();
    all_entries.extend_from_slice(&warc_entries);

    // TODO: compressed cdx files. this seems to use something called zipnum index?https://github.com/harvard-lil/js-wacz/blob/0ccad603752d91545519109851937620a593251a/index.js#L458C2-L458C2
    let ((cdx_file, cdx_entry), (idx_file, idx_entry)) = cdx_writer.finalize("indexes/")?;
    all_entries.push(cdx_entry);
    all_entries.push(idx_entry);

    let ((pages_file, pages_entry), (extrapages_file, extrapages_entry)) =
        pages_writer.finalize("pages/")?;
    all_entries.push(pages_entry);
    all_entries.push(extrapages_entry);

    let package_metadata = DataPackage {
        profile: "data-package",
        wacz_version: "1.1.1",
        software: "Evergarden (https://github.com/kore-signet/evergarden)",
        created: OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
        resources: all_entries,
    };

    info!("building WACZ package");

    let mut package = ZipWriter::new(BufWriter::new(File::create(args.output)?));

    package.add_directory(
        "archive",
        FileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    package.add_directory(
        "indexes",
        FileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    package.add_directory(
        "pages",
        FileOptions::default().compression_method(CompressionMethod::Deflated),
    )?;

    package.add_file(
        "datapackage.json",
        &serde_json::to_vec_pretty(&package_metadata)?[..],
        Some(9),
    )?;

    info!("copying indexes..");

    package.add_file("indexes/index.cdx.gz", cdx_file, None)?;
    package.add_file("indexes/index.idx", idx_file, Some(9))?;

    package.add_file("pages/pages.jsonl", pages_file, Some(9))?;
    package.add_file("pages/extraPages.jsonl", extrapages_file, Some(9))?;

    info!("copying WARC files");

    for DataPackageEntry { path, .. } in warc_entries {
        debug!("copying WARC: {path}");
        let file = File::open(output_path.join(&path))?;
        package.add_file(&path, file, None)?;
    }

    info!("finishing WACZ package!");

    package.finish()?;

    Ok(())
}
