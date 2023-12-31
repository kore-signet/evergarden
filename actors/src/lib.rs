#![feature(return_position_impl_trait_in_trait)]

use std::{
    fmt::Debug,
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use futures::FutureExt;
use tokio::{
    sync::{oneshot, watch, Notify},
    task::JoinSet,
};
use tracing::{Instrument, Span};

pub static TASK_COUNT: AtomicUsize = AtomicUsize::new(0);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramState {
    Running,
    Closing,
}

pub trait Actor: Sized + Send + 'static {
    type Input: Send + Sync;
    type Output: Send + Sync;

    type Response<'a>: Future<Output = Self::Output> + Send + 'a
    where
        Self: 'a;
    type CloseFuture<'a>: Future<Output = ()> + Send + 'a
    where
        Self: 'a;

    fn close<'a>(self) -> Self::CloseFuture<'a>;
    fn answer(&mut self, i: Self::Input) -> Self::Response<'_>;

    fn run_async_loop<'a>(
        mut self,
        rx: flume::Receiver<Message<Self::Input, Self::Output>>,
        mut program_state: watch::Receiver<ProgramState>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            loop {
                tokio::select! {
                    Ok(Message { value, output }) = rx.recv_async() => {
                        let result = self.answer(value).in_current_span().await;
                        let _ = output.send(result);
                    },
                    _ = program_state.changed() => {
                        break
                    },
                    else => break
                }
            }

            self.close().await;
        }
        .in_current_span()
    }
}

// pub struct SimpleActor {
//     name:  &'static str,
//     values: Vec<String>,
// }

// impl SimpleActor {
//     pub fn new(name: &'static str) -> SimpleActor {
//         SimpleActor {
//             name,
//             values: Vec::with_capacity(16)
//         }
//     }

//     pub async fn handle_message(&mut self, i: String) -> String {
//         self.values.push(i);
//         format!("values received by {}: {:?}", self.name, self.values)
//     }
// }

// impl Actor for SimpleActor {
//     type Input = String;
//     type Output = String;
//     type Response<'a> = impl Future<Output = Self::Output> + Send + 'a;
//     const IS_BLOCKING: bool = false;

//     fn answer<'a>(&'a mut self, i: Self::Input) -> Self::Response<'a> {
//         self.handle_message(i)
//     }
// }

pub struct Message<I, O> {
    pub value: I,
    pub output: oneshot::Sender<O>,
}

pub struct ActorManager<A: Actor> {
    tasks: JoinSet<()>,
    state: watch::Sender<ProgramState>,
    pub rx: flume::Receiver<Message<A::Input, A::Output>>,
}

impl<A: Actor + Send + 'static> ActorManager<A> {
    pub fn new(capacity: usize) -> (ActorManager<A>, Mailbox<A>) {
        let (tx, rx) = flume::bounded(capacity);
        let (state, _) = watch::channel(ProgramState::Running);

        (
            ActorManager {
                tasks: JoinSet::new(),
                rx,
                state,
            },
            Mailbox {
                notify: Arc::new(Notify::const_new()),
                tx,
            },
        )
    }

    pub async fn close_and_join(&mut self) {
        self.state.send(ProgramState::Closing).unwrap();
        while (self.tasks.join_next().await).is_some() {}
    }

    pub fn get_rx(&self) -> flume::Receiver<Message<A::Input, A::Output>> {
        self.rx.clone()
    }
}

pub struct Mailbox<A: Actor> {
    tx: flume::Sender<Message<A::Input, A::Output>>,
    notify: Arc<Notify>,
}

impl<A: Actor> Debug for Mailbox<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailbox").field("tx", &self.tx).finish()
    }
}

impl<A: Actor> Clone for Mailbox<A> {
    fn clone(&self) -> Self {
        Self {
            notify: Arc::clone(&self.notify),
            tx: self.tx.clone(),
        }
    }
}

impl<A: Actor + Send + 'static> ActorManager<A> {
    pub fn spawn_actor(&mut self, actor: A, span: Span) {
        let rx = self.rx.clone();

        self.tasks.spawn(
            actor
                .run_async_loop(rx, self.state.subscribe())
                .instrument(span),
        );
    }
}

impl<A: Actor + 'static> Mailbox<A> {
    pub fn len(&self) -> usize {
        self.tx.len()
    }

    pub fn subscribe(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    pub async fn deferred_request(
        &self,
        input: A::Input,
    ) -> impl Future<Output = Result<A::Output, oneshot::error::RecvError>> + Send + Sync {
        TASK_COUNT.fetch_add(1, Ordering::Release);

        let (oneshot_tx, oneshot_rx) = oneshot::channel();
        let notifier = Arc::clone(&self.notify);
        notifier.notify_waiters();

        let _ = self
            .tx
            .send_async(Message {
                value: input,
                output: oneshot_tx,
            })
            .await;

        oneshot_rx.inspect(move |_| {
            TASK_COUNT.fetch_sub(1, Ordering::Release);
            notifier.notify_waiters();
        })
    }

    pub async fn request(&self, input: A::Input) -> A::Output {
        let v = self.deferred_request(input).await;
        v.await.unwrap()
    }
}
