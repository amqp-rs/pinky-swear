use parking_lot::Mutex;
use std::{
    fmt,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
};

pub trait NotifyReady {
    fn notify(&self);
}

#[must_use = "PinkySwear should be used or you can miss errors"]
pub struct PinkySwear<T> {
    recv: Receiver<T>,
    send: SyncSender<T>,
    task: Arc<Mutex<Option<Box<dyn NotifyReady + Send>>>>,
}

pub trait Pinky<T> {
    fn swear(&self, data: T);
}

#[derive(Clone)]
pub struct SimplePinky<T> {
    send: SyncSender<T>,
    task: Arc<Mutex<Option<Box<dyn NotifyReady + Send>>>>,
}

#[derive(Clone)]
pub struct ComposedPinky<S, T, P: Pinky<T>> {
    send: P,
    transform: Arc<dyn Fn(S) -> T>,
}

impl<T> PinkySwear<T> {
    pub fn new() -> (Self, impl Pinky<T>) {
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

    fn pinky(&self) -> impl Pinky<T> {
        SimplePinky {
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

impl<T: 'static> SimplePinky<T> {
    pub fn compose<S: 'static>(self, f: Box<dyn Fn(S) -> T>) -> impl Pinky<S> {
        ComposedPinky {
            send: self,
            transform: Arc::new(f),
        }
    }
}

impl<T> Pinky<T> for SimplePinky<T> {
    fn swear(&self, data: T) {
        let _ = self.send.send(data);
        if let Some(task) = self.task.lock().take() {
            task.notify();
        }
    }
}

impl<S, T, P: Pinky<T>> Pinky<S> for ComposedPinky<S, T, P> {
    fn swear(&self, data: S) {
        self.send.swear((self.transform)(data));
    }
}

impl<T> fmt::Debug for PinkySwear<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkySwear")
    }
}

impl<T> fmt::Debug for SimplePinky<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SimplePinky")
    }
}

impl<S, T, P: Pinky<T>> fmt::Debug for ComposedPinky<S, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ComposedPinky")
    }
}

#[cfg(feature = "futures")]
pub(crate) mod futures {
    use super::*;

    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    impl<T> Future for PinkySwear<T> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.has_subscriber() {
                self.subscribe(Box::new(Watcher(cx.waker().clone())));
            }
            self.try_wait().map(Poll::Ready).unwrap_or(Poll::Pending)
        }
    }

    pub(crate) struct Watcher(pub(crate) Waker);

    impl NotifyReady for Watcher {
        fn notify(&self) {
            self.0.wake_by_ref();
        }
    }
}
