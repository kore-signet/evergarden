use std::error::Error;

use clap::builder::TypedValueParser;
use clap::{Parser, Subcommand};
use tracing::metadata::LevelFilter;

mod archiver;
mod export;

#[derive(clap::Parser, Debug)]
#[command(author = "Kore Signet-Yang <kore@cat-girl.gay>")]
#[command(version, about)]
struct Args {
    #[arg(
        long,
        default_value_t = LevelFilter::INFO,
        value_parser = clap::builder::PossibleValuesParser::new(["off", "error", "warn", "info", "debug", "trace"])
            .map(|s| s.parse::<LevelFilter>().unwrap()),
    )]
    log_level: LevelFilter,
    #[command(subcommand)]
    subcommand: EvergardenSubcommand,
}

#[derive(Subcommand, Debug)]
enum EvergardenSubcommand {
    Export(export::run::ExportArgs),
    Archive(archiver::ArchiverArgs),
}

pub fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    match args.subcommand {
        EvergardenSubcommand::Export(export_args) => {
            export::run::export(export_args, args.log_level)
        }
        EvergardenSubcommand::Archive(archiver_args) => {
            let rt = tokio::runtime::Runtime::new()?;

            rt.block_on(archiver::run_archiver(archiver_args, args.log_level))
        }
    }
}
