use std::{fmt::Display, process::Stdio, sync::Arc, time::Duration};

use actors::{Actor, ActorManager, Mailbox};

use evergarden_common::{EvergardenResult, HttpResponse};
use futures_util::{stream::FuturesUnordered, Future, FutureExt, StreamExt};

use tokio::{
    io::{BufReader, BufWriter},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use tracing::{debug, info};

use crate::{
    client::HttpClient,
    config::{GlobalState, ScriptConfig, ScriptFilter},
    scripting::protocol::ClientRequest,
};

use super::protocol::{ClientReader, ClientWriter};

pub struct ScriptId {
    pub name: Arc<str>,
    pub counter: usize,
}

impl Display for ScriptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{:03}]", self.name, self.counter)
    }
}

pub struct ScriptManager {
    scripts: Vec<Script>,
}

impl ScriptManager {
    pub fn new(
        scripts: impl IntoIterator<Item = (Arc<str>, ScriptConfig)>,
        global: &GlobalState,
    ) -> EvergardenResult<ScriptManager> {
        Ok(ScriptManager {
            scripts: scripts
                .into_iter()
                .map(|(name, cfg)| Script::spawn(name, cfg, global))
                .collect::<EvergardenResult<Vec<Script>>>()?,
        })
    }

    pub async fn close_all(self) {
        let mut stream = self
            .scripts
            .into_iter()
            .map(|v| v.close_all())
            .collect::<FuturesUnordered<_>>();

        while let Some(_) = stream.next().await {}
    }

    pub async fn process(&self, data: HttpResponse) -> EvergardenResult<()> {
        let mut stream = self
            .scripts
            .iter()
            .filter(|s| s.filter.matches(&data))
            .map(|v| v.mailbox.request(data.clone()))
            .collect::<FuturesUnordered<_>>();

        while let Some(v) = stream.next().await {
            v?;
        }

        Ok(())
    }
}

impl Actor for ScriptManager {
    type Input = HttpResponse;

    type Output = EvergardenResult<()>;

    type Response<'a> = impl Future<Output = Self::Output> + Send + 'a
    where
        Self: 'a;

    fn answer<'a>(&'a mut self, data: Self::Input) -> Self::Response<'a> {
        self.process(data)
    }

    type CloseFuture<'a> = impl Future<Output = ()> + Send + 'a where Self: 'a ;

    fn close<'a>(self) -> Self::CloseFuture<'a> {
        self.close_all()
    }
}

pub struct Script {
    filter: ScriptFilter,
    #[allow(dead_code)]
    manager: ActorManager<ScriptInstance>,
    mailbox: Mailbox<ScriptInstance>,
}

impl Script {
    pub fn spawn(
        name: Arc<str>,
        cfg: ScriptConfig,
        global: &GlobalState,
    ) -> EvergardenResult<Script> {
        let (mut manager, mailbox) = ActorManager::<ScriptInstance>::new(256);
        for idx in 0..cfg.workers {
            manager.spawn_actor(ScriptInstance::spawn(
                ScriptId {
                    name: Arc::clone(&name),
                    counter: idx,
                },
                &cfg,
                global,
            )?);
        }

        Ok(Script {
            filter: cfg.filter,
            manager,
            mailbox,
        })
    }

    pub async fn close_all(mut self) {
        self.manager.close_and_join().await;
    }
}

pub struct ScriptInstance {
    id: ScriptId,
    client: Mailbox<HttpClient>,
    #[allow(dead_code)]
    proc: Child,
    proc_in: ClientWriter<BufWriter<ChildStdin>>,
    proc_out: ClientReader<BufReader<ChildStdout>>,
    max_hops: usize,
}

impl ScriptInstance {
    #[tracing::instrument(skip(id, script, global), fields(
        id = %id,
        script = ?script
    ))]
    pub fn spawn(
        id: ScriptId,
        script: &ScriptConfig,
        global: &GlobalState,
    ) -> EvergardenResult<ScriptInstance> {
        let mut proc = Command::new(&script.command)
            .args(&script.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let proc_in = BufWriter::new(proc.stdin.take().unwrap());
        let proc_out = BufReader::new(proc.stdout.take().unwrap());

        Ok(ScriptInstance {
            id,
            client: global.client.clone(),
            proc,
            proc_in: ClientWriter::new(proc_in),
            proc_out: ClientReader::new(proc_out),
            max_hops: global.config.max_hops,
        })
    }

    pub async fn close_script(mut self) -> EvergardenResult<()> {
        self.proc_in.close_script().await?;
        let _ = tokio::time::timeout(Duration::from_millis(100), self.proc.wait()).await;

        Ok(())
    }

    #[tracing::instrument(skip(self, data), fields(
        script = %self.id,
        url = ?data.meta.url,
    ))]
    pub async fn submit(&mut self, data: HttpResponse) -> EvergardenResult<()> {
        use ClientRequest::*;

        self.proc_in.submit(&data).await?;

        loop {
            match self.proc_out.read_op().await.unwrap() {
                Submit { url } => {
                    let Some(url) = data.meta.url.clone().hop(&url) else {
                        debug!("script result skipped: invalid url {}", &url);
                        continue;
                    };

                    if url.hops > self.max_hops {
                        debug!(
                            "script result skipped: url {} exceeded max hops",
                            url.url.as_str()
                        );

                        continue;
                    }

                    info!(?url, "script yielded url");

                    let v = self.client.deferred_request(url).await;
                    tokio::task::spawn(v);
                }
                Fetch { url } => {
                    let Some(url) = data.meta.url.clone().hop(&url) else {
                        self.proc_in.error_fetch("invalid_url").await?;
                        continue;
                    };

                    info!(?url, "fetching url for script");

                    match self.client.request(url).await {
                        Ok(res) => self.proc_in.answer_fetch(&res).await?,
                        Err(e) => self.proc_in.error_fetch(&e.to_string()).await?,
                    }
                }
                EndFile => {
                    break;
                }
            }
        }

        Ok(())
    }
}

impl Actor for ScriptInstance {
    type Input = HttpResponse;
    type Output = EvergardenResult<()>;

    type Response<'a> = impl Future<Output = EvergardenResult<()>> + Send + 'a
    where
        Self: 'a;

    fn answer<'a>(&'a mut self, i: Self::Input) -> Self::Response<'a> {
        self.submit(i)
    }

    type CloseFuture<'a> = impl Future<Output = ()> + Send + 'a where Self: 'a;

    fn close<'a>(self) -> Self::CloseFuture<'a> {
        self.close_script().map(|_| ())
    }
}
