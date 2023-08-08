use std::{error::Error, path::PathBuf, sync::atomic::Ordering, time::Duration};

use actors::ActorManager;
use clap::builder::TypedValueParser;
use clap::Parser;
use evergarden_client::{
    client::{HttpClient, HttpRateLimiter},
    config::{FullConfig, GlobalState},
    scripting::script::ScriptManager,
};
use evergarden_common::{Storage, UrlInfo};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(short, long)]
    config: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(short, long)]
    start_point: String,
    #[arg(long)]
    no_clobber: bool, // #[arg(
                      //     long,
                      //     value_parser = clap::builder::PossibleValuesParser::new(["off", "error", "warn", "info", "debug", "trace"])
                      //         .map(|s| s.parse::<trac::LevelFilter>().unwrap()),
                      // )]
                      // log_level: Option<log::LevelFilter>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();

    let cfg: FullConfig = toml::from_str(&tokio::fs::read_to_string(args.config).await?)?;
    let FullConfig {
        general,
        ratelimiter,
        http,
        scripts,
    } = cfg;

    let storage = Storage::new(args.output, !args.no_clobber)?;

    let rate_limiter = HttpRateLimiter::new(ratelimiter);

    let (mut http_manager, http_mailbox) = ActorManager::new(256);
    let (mut script_runner, script_mailbox) = ActorManager::new(256);
    let (mut storage_manager, storage_mailbox) = ActorManager::new(256);

    storage_manager.spawn_actor(storage);

    http_manager.spawn_actor(HttpClient::new(
        &http,
        rate_limiter,
        storage_mailbox.clone(),
        script_mailbox.clone(),
    )?);

    let global_state = GlobalState {
        config: general,
        client: http_mailbox.clone(),
    };

    script_runner.spawn_actor(ScriptManager::new(
        scripts.into_values().collect(),
        &global_state,
    )?);

    let mail = http_mailbox.clone();
    tokio::task::spawn(async move {
        mail.request(UrlInfo::start(&args.start_point).unwrap())
            .await
    });

    let mut ticker = tokio::time::interval(Duration::from_millis(200));
    ticker.tick().await;

    loop {
        ticker.tick().await;

        if actors::TASK_COUNT.load(Ordering::Acquire) == 0 {
            break;
        }
    }

    script_runner.close_and_join().await;
    http_manager.close_and_join().await;

    Ok(())
}
