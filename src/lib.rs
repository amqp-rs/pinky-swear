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
pub struct PinkySwear<T> {
    recv: Receiver<T>,
    send: SyncSender<T>,
    task: Arc<Mutex<Option<Box<dyn NotifyReady + Send>>>>,
}

#[derive(Clone)]
pub struct Pinky<T> {
    send: SyncSender<T>,
    task: Arc<Mutex<Option<Box<dyn NotifyReady + Send>>>>,
}

impl<T> PinkySwear<T> {
    pub fn new() -> (Self, Pinky<T>) {
        let (send, recv) = sync_channel(1);
        let promise = Self {
            recv,
            send,
            task: Arc::new(Mutex::new(None)),
        };
        let pinky = promise.pinky();
        (promise, pinky)
    }

    pub fn new_with_data(data: T) -> Self {
        let (promise, pinky) = Self::new();
        pinky.swear(data);
        promise
    }

    fn pinky(&self) -> Pinky<T> {
        Pinky {
            send: self.send.clone(),
            task: self.task.clone(),
        }
    }

    pub fn try_wait(&self) -> Option<T> {
        self.recv.try_recv().ok()
    }

    pub fn wait(&self) -> T {
        self.recv.recv().unwrap()
    }

    pub fn subscribe(&self, task: Box<dyn NotifyReady + Send>) {
        *self.task.lock() = Some(task);
    }

    pub fn has_subscriber(&self) -> bool {
        self.task.lock().is_some()
    }
}

impl<T> Pinky<T> {
    fn swear(&self, data: T) {
        let _ = self.send.send(data);
        if let Some(task) = self.task.lock().take() {
            task.notify();
        }
    }
}

impl<T> fmt::Debug for PinkySwear<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkySwear")
    }
}

impl<T> fmt::Debug for Pinky<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pinky")
    }
}

impl<T> Future for PinkySwear<T> {
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

impl<T> Promise<T> for PinkySwear<T> {
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

impl<T, E> Cancellable<E> for Pinky<Result<T, E>> {
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
