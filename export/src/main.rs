use std::{
    error::Error,
    fs::{create_dir_all, File, OpenOptions},
    io::{BufReader, BufWriter},
    path::PathBuf,
};

use clap::Parser;
use evergarden_common::{EvergardenResult, ResponseMetadata, Storage};
use evergarden_export::{
    cdxj::{CDXRecord, CDXWriter},
    warc::{RotatingWarcRecorder, WarcRecorder},
    DataPackage, DataPackageEntry,
};
use itertools::Itertools;
use ssri::Integrity;

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
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let storage = Storage::new(&args.input, false)?;

    let output_dir = tempfile::tempdir_in("./")?;
    let output_path = PathBuf::from(output_dir.path());

    let _ = create_dir_all(output_path.join("archive"));
    let _ = create_dir_all(output_path.join("indexes"));
    let _ = create_dir_all(output_path.join("pages"));

    // set up our writers

    let mut warc_writer = RotatingWarcRecorder::new(
        output_path.join("archive"),
        "archive/",
        ByteUnit::Gigabyte(1).as_u64(),
    )?;

    // let mut pages_writer = PageEntryWriter::new(
    //     OpenOptions::new()
    //         .create(true)
    //         .read(true)
    //         .write(true)
    //         .open(output_path.join("pages/pages.jsonl"))?,
    // )?;

    let mut cdx_writer = CDXWriter::new(
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(output_path.join("indexes/index.cdx"))?,
    );

    // get a list of records from our storage

    let mut records = storage
        .list()
        .collect::<EvergardenResult<Vec<(String, Integrity, ResponseMetadata)>>>()
        .unwrap();

    // sort our records by time, key

    records.sort_unstable_by(|(lkey, _, lmeta), (rkey, _, rmeta)| {
        (lkey, lmeta.fetched_at.to_hms()).cmp(&(rkey, rmeta.fetched_at.to_hms()))
    });

    let mut cdx_records: Vec<CDXRecord> = Vec::with_capacity(records.len());

    // writes records, batch by batch. ensures resulting CDXJ will be sorted
    for (_, group) in &records
        .into_iter()
        .group_by(|(lkey, _, lmeta)| (lkey.clone(), lmeta.fetched_at.to_hms()))
    {
        let mut records = Vec::with_capacity(8);

        for (key, hash, meta) in group {
            let cdx =
                warc_writer.write_warc(&key, &meta, &mut storage.read_body_sync(hash)?.unwrap())?;
            records.push(cdx.clone());
            cdx_records.push(cdx);
            // pages_writer.entry(&meta)?;
            // records.push((key))
        }

        cdx_writer.write_batch(&records[..])?;
    }

    // get our metadata in order

    let warc_entries = warc_writer.finalize()?;

    let mut all_entries = Vec::new();
    all_entries.extend_from_slice(&warc_entries);

    // TODO: compressed cdx files. this seems to use something called zipnum index?https://github.com/harvard-lil/js-wacz/blob/0ccad603752d91545519109851937620a593251a/index.js#L458C2-L458C2
    let (cdx_file, cdx_entry) = cdx_writer.finalize("index.cdx", "indexes/index.cdx")?;
    all_entries.push(cdx_entry);

    let package_metadata = DataPackage {
        profile: "data-package",
        wacz_version: "1.1.1",
        software: "Evergarden (https://github.com/kore-signet/evergarden)",
        created: OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
        resources: all_entries,
    };

    //

    let mut package = ZipWriter::new(BufWriter::new(File::create(args.output)?));

    package.add_directory(
        "archive",
        FileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    package.add_directory(
        "indexes",
        FileOptions::default().compression_method(CompressionMethod::Stored),
    )?;

    package.start_file(
        "datapackage.json",
        FileOptions::default().compression_level(Some(9)),
    )?;
    serde_json::to_writer_pretty(&mut package, &package_metadata)?;

    package.start_file(
        "indexes/index.cdx",
        FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(8)),
    )?;

    std::io::copy(&mut BufReader::new(cdx_file), &mut package)?;

    for DataPackageEntry { path, .. } in warc_entries {
        package.start_file(
            &path,
            FileOptions::default().compression_method(CompressionMethod::Stored),
        )?;
        let file = File::open(output_path.join(&path))?;

        std::io::copy(&mut BufReader::new(file), &mut package)?;
    }

    package.finish()?;

    Ok(())
}
