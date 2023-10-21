use crate::{AppContext, PlatformDispatcher};
use smol::prelude::*;
use std::{
    fmt::Debug,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use util::TryFutureExt;

#[derive(Clone)]
pub struct Executor {
    dispatcher: Arc<dyn PlatformDispatcher>,
}

#[must_use]
pub enum Task<T> {
    Ready(Option<T>),
    Spawned(async_task::Task<T>),
}

impl<T> Task<T> {
    pub fn ready(val: T) -> Self {
        Task::Ready(Some(val))
    }

    pub fn detach(self) {
        match self {
            Task::Ready(_) => {}
            Task::Spawned(task) => task.detach(),
        }
    }
}

impl<E, T> Task<Result<T, E>>
where
    T: 'static + Send,
    E: 'static + Send + Debug,
{
    pub fn detach_and_log_err(self, cx: &mut AppContext) {
        cx.executor().spawn(self.log_err()).detach();
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        match unsafe { self.get_unchecked_mut() } {
            Task::Ready(val) => Poll::Ready(val.take().unwrap()),
            Task::Spawned(task) => task.poll(cx),
        }
    }
}

impl Executor {
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Enqueues the given closure to be run on any thread. The closure returns
    /// a future which will be run to completion on any available thread.
    pub fn spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        let dispatcher = self.dispatcher.clone();
        let (runnable, task) =
            async_task::spawn(future, move |runnable| dispatcher.dispatch(runnable));
        runnable.schedule();
        Task::Spawned(task)
    }

    /// Enqueues the given closure to run on the application's event loop.
    /// Returns the result asynchronously.
    pub fn run_on_main<F, R>(&self, func: F) -> Task<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if self.dispatcher.is_main_thread() {
            Task::ready(func())
        } else {
            self.spawn_on_main(move || async move { func() })
        }
    }

    /// Enqueues the given closure to be run on the application's event loop. The
    /// closure returns a future which will be run to completion on the main thread.
    pub fn spawn_on_main<F, R>(&self, func: impl FnOnce() -> F + Send + 'static) -> Task<R>
    where
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let (runnable, task) = async_task::spawn(
            {
                let this = self.clone();
                async move {
                    let task = this.spawn_on_main_local(func());
                    task.await
                }
            },
            {
                let dispatcher = self.dispatcher.clone();
                move |runnable| dispatcher.dispatch_on_main_thread(runnable)
            },
        );
        runnable.schedule();
        Task::Spawned(task)
    }

    /// Enqueues the given closure to be run on the application's event loop. Must
    /// be called on the main thread.
    pub fn spawn_on_main_local<R>(&self, future: impl Future<Output = R> + 'static) -> Task<R>
    where
        R: 'static,
    {
        assert!(
            self.dispatcher.is_main_thread(),
            "must be called on main thread"
        );

        let dispatcher = self.dispatcher.clone();
        let (runnable, task) = async_task::spawn_local(future, move |runnable| {
            dispatcher.dispatch_on_main_thread(runnable)
        });
        runnable.schedule();
        Task::Spawned(task)
    }

    pub fn block<R>(&self, future: impl Future<Output = R>) -> R {
        // todo!("integrate with deterministic dispatcher")
        futures::executor::block_on(future)
    }

    pub fn is_main_thread(&self) -> bool {
        self.dispatcher.is_main_thread()
    }
}
