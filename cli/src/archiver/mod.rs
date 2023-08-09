use std::{error::Error, path::PathBuf, sync::atomic::Ordering, time::Duration};

use actors::ActorManager;
use evergarden_client::{
    client::{HttpClient, HttpRateLimiter},
    config::{FullConfig, GlobalState},
    scripting::script::ScriptManager,
};
use evergarden_common::{surt, CrawlInfo, Storage, UrlInfo};
use futures_util::{stream::FuturesUnordered, StreamExt};
use tracing::{info, info_span, metadata::LevelFilter};

use clap::builder::TypedValueParser;
use tracing_subscriber::{filter::Targets, fmt::format, prelude::*};
use url::Url;

#[derive(clap::Args, Debug)]
pub(crate) struct ArchiverArgs {
    #[arg(short, long, help = "crawl configuration")]
    config: PathBuf,
    #[arg(short, long, help = "output folder")]
    output: PathBuf,
    #[arg(long, help = "Doesn't overwrite existing records in <output>, except for seed urls.")]
    no_clobber: bool,
    #[arg(
        long,
        help = "Logging level for HTTP tasks",
        default_value_t = LevelFilter::WARN,
        value_parser = clap::builder::PossibleValuesParser::new(["off", "error", "warn", "info", "debug", "trace"])
            .map(|s| s.parse::<LevelFilter>().unwrap()),
    )]
    http_log: LevelFilter,
    #[arg(
        long,
        help = "Logging level for script tasks",
        default_value_t = LevelFilter::WARN,
        value_parser = clap::builder::PossibleValuesParser::new(["off", "error", "warn", "info", "debug", "trace"])
            .map(|s| s.parse::<LevelFilter>().unwrap()),
    )]
    script_log: LevelFilter,
    #[arg(help = "URLs for start of crawl", required = true)]
    seed_urls: Vec<String>,
}

pub(crate) async fn run_archiver(
    args: ArchiverArgs,
    log_level: LevelFilter,
) -> Result<(), Box<dyn Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().event_format(
                format()
                    .pretty()
                    .with_line_number(false)
                    .with_source_location(false),
            ),
        )
        .with(
            Targets::new()
                .with_default(log_level)
                .with_target("evergarden::http", args.http_log)
                .with_target("evergarden_client::scripting", args.script_log),
        )
        .init();

    let cfg: FullConfig = toml::from_str(&tokio::fs::read_to_string(args.config).await?)?;
    let storage: Storage = Storage::new(args.output, !args.no_clobber)?;

    let seed_urls: Vec<Url> = args
        .seed_urls
        .into_iter()
        .filter_map(|v| v.parse::<Url>().ok())
        .collect();

    storage
        .write_info(&CrawlInfo {
            config: serde_json::to_string(&cfg)?,
            entry_points: seed_urls.iter().cloned().map(surt).collect(),
        })
        .await?;

    for url in seed_urls.iter().cloned().map(surt) {
        storage.del_by_key(&url).await?;
    }

    let FullConfig {
        general,
        ratelimiter,
        http,
        scripts,
    } = cfg;

    let rate_limiter = HttpRateLimiter::new(ratelimiter);

    let (mut http_manager, http_mailbox) = ActorManager::new(10_000);
    let (mut script_runner, script_mailbox) = ActorManager::new(256);
    let (mut storage_manager, storage_mailbox) = ActorManager::new(256);

    storage_manager.spawn_actor(
        storage,
        info_span!(target: "evergarden::storage", "Storage"),
    );

    http_manager.spawn_actor(
        HttpClient::new(
            &http,
            rate_limiter,
            storage_mailbox.clone(),
            script_mailbox.clone(),
        )?,
        info_span!(target: "evergarden::http", "HTTP"),
    );

    let global_state = GlobalState {
        config: general,
        client: http_mailbox.clone(),
    };

    let script_span = info_span!(target: "evergarden::scripting", "Scripts");
    script_runner.spawn_actor(ScriptManager::new(scripts, &global_state)?, script_span);

    let mail = http_mailbox.clone();
    let submitter_task = tokio::task::spawn(async move {
        let mut futures = seed_urls
            .into_iter()
            .map(|v| UrlInfo {
                url: v.clone(),
                discovered_in: v,
                hops: 0,
            })
            .map(|u| mail.request(u))
            .collect::<FuturesUnordered<_>>();

        while futures.next().await.is_some() {}
    });

    let mut ticker = tokio::time::interval(Duration::from_millis(200));
    ticker.tick().await;

    let queue_notifier = http_mailbox.subscribe();

    let queue_task = tokio::task::spawn(async move {
        loop {
            queue_notifier.notified().await;
            info!(
                "HTTP Queue Size {} | Actor System Queue Size {}",
                http_mailbox.len(),
                actors::TASK_COUNT.load(Ordering::Acquire)
            );
        }
    });

    loop {
        ticker.tick().await;

        if submitter_task.is_finished() && actors::TASK_COUNT.load(Ordering::Acquire) == 0 {
            break;
        }
    }

    script_runner.close_and_join().await;
    http_manager.close_and_join().await;

    queue_task.abort();

    Ok(())
}
