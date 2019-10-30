use parking_lot::Mutex;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
    task::{Context, Poll, Waker},
};

#[must_use = "PinkySwear should be used or you can miss errors"]
pub struct PinkySwear<T, S = ()> {
    recv: Receiver<T>,
    send: SyncSender<T>,
    inner: Arc<Mutex<Inner<T, S>>>,
}

#[derive(Clone)]
pub struct Pinky<T, S = ()> {
    send: SyncSender<T>,
    inner: Arc<Mutex<Inner<T, S>>>,
}

struct Inner<T, S> {
    task: Option<Box<dyn NotifyReady + Send>>,
    barrier: Option<(Box<dyn Promise<S>>, Box<dyn Fn(S) -> T>)>,
}

impl<T, S> Default for Inner<T, S> {
    fn default() -> Self {
        Self {
            task: None,
            barrier: None,
        }
    }
}

impl<T: 'static, S: 'static> PinkySwear<T, S> {
    pub fn new() -> (Self, Pinky<T, S>) {
        Self::new_with_inner(Inner::default())
    }

    fn new_with_inner(inner: Inner<T, S>) -> (Self, Pinky<T, S>) {
        let (send, recv) = sync_channel(1);
        let promise = Self {
            recv,
            send,
            inner: Arc::new(Mutex::new(inner)),
        };
        let pinky = promise.pinky();
        (promise, pinky)
    }

    pub fn new_with_data(data: T) -> Self {
        let (promise, pinky) = Self::new();
        pinky.swear(data);
        promise
    }

    fn pinky(&self) -> Pinky<T, S> {
        Pinky {
            send: self.send.clone(),
            inner: self.inner.clone(),
        }
    }

    pub fn try_wait(&self) -> Option<T> {
        if let Some((barrier, transform)) = self.inner.lock().barrier.as_ref() {
            barrier.try_wait().map(transform)
        } else {
            self.recv.try_recv().ok()
        }
    }

    pub fn wait(&self) -> T {
        if let Some((barrier, transform)) = self.inner.lock().barrier.as_ref() {
            transform(barrier.wait())
        } else {
            self.recv.recv().unwrap()
        }
    }

    pub fn subscribe(&self, task: Box<dyn NotifyReady + Send>) {
        self.inner.lock().task = Some(task);
    }

    pub fn has_subscriber(&self) -> bool {
        self.inner.lock().task.is_some()
    }

    pub fn traverse<F: 'static>(
        self,
        transform: Box<dyn Fn(T) -> F>,
    ) -> (PinkySwear<F, T>, Pinky<F, T>) {
        let inner = Inner {
            task: None,
            barrier: Some((Box::new(self), transform)),
        };
        PinkySwear::new_with_inner(inner)
    }
}

impl<T, S> Pinky<T, S> {
    pub fn swear(&self, data: T) {
        let _ = self.send.send(data);
        if let Some(task) = self.inner.lock().task.as_ref() {
            task.notify();
        }
    }
}

impl<T, S> fmt::Debug for PinkySwear<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkySwear")
    }
}

impl<T, S> fmt::Debug for Pinky<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pinky")
    }
}

impl<T: 'static, S: 'static> Future for PinkySwear<T, S> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.has_subscriber() {
            self.subscribe(Box::new(cx.waker().clone()));
        }
        self.try_wait().map(Poll::Ready).unwrap_or(Poll::Pending)
    }
}

trait Promise<T> {
    fn try_wait(&self) -> Option<T>;
    fn wait(&self) -> T;
}

impl<T: 'static, S: 'static> Promise<T> for PinkySwear<T, S> {
    fn try_wait(&self) -> Option<T> {
        self.try_wait()
    }

    fn wait(&self) -> T {
        self.wait()
    }
}

pub trait Cancellable<E> {
    fn cancel(&self, err: E);
}

impl<T, S, E> Cancellable<E> for Pinky<Result<T, E>, S> {
    fn cancel(&self, err: E) {
        self.swear(Err(err))
    }
}

pub trait NotifyReady {
    fn notify(&self);
}

impl NotifyReady for Waker {
    fn notify(&self) {
        self.wake_by_ref();
    }
}
