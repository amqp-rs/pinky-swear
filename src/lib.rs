//! Simple promise library compatible with `std::future` and async/await
//!
//! # Example
//!
//! Create a promise and wait for the result while computing the result in another thread
//!
//! ```rust
//! use pinky_swear::{Pinky, PinkySwear};
//! use std::{thread, time::Duration};
//!
//! fn compute(pinky: Pinky<Result<u32, ()>>) {
//!     thread::sleep(Duration::from_millis(1000));
//!     pinky.swear(Ok(42));
//! }
//!
//! fn main() {
//!     let (promise, pinky) = PinkySwear::new();
//!     thread::spawn(move || {
//!         compute(pinky);
//!     });
//!     assert_eq!(promise.wait(), Ok(42));
//! }
//! ```

#![cfg_attr(feature = "docs", feature(doc_cfg))]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
#![doc(test(attr(deny(rust_2018_idioms, warnings))))]
#![doc(test(attr(allow(unused_extern_crates))))]
#![doc(html_root_url = "https://docs.rs/pinky-swear/1.5.0/")]

doc_comment::doctest!("../README.md");

use log::trace;
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

/// A PinkySwear is a Promise that the other party is supposed to honour at some point.
#[must_use = "PinkySwear should be used or you can miss errors"]
pub struct PinkySwear<T, S = ()> {
    recv: Receiver<T>,
    pinky: Pinky<T, S>,
}

/// A Pinky allows you to fulfill a Promise that you made.
pub struct Pinky<T, S = ()> {
    send: SyncSender<T>,
    inner: Arc<Mutex<Inner<T, S>>>,
}

struct Inner<T, S> {
    waker: Option<Waker>,
    next: Option<Box<dyn NotifyReady + Send>>,
    tasks: Vec<Box<dyn NotifyReady + Send>>,
    barrier: Option<(Box<dyn Promise<S> + Send>, Box<dyn Fn(S) -> T + Send>)>,
    before: Option<Box<dyn Promise<S> + Send>>,
}

impl<T, S> Default for Inner<T, S> {
    fn default() -> Self {
        Self {
            waker: None,
            next: None,
            tasks: Vec::default(),
            barrier: None,
            before: None,
        }
    }
}

/// A PinkyBroadcaster allows you to broacast a promise resolution to several subscribers.
#[must_use = "PinkyBroadcaster must be subscribed"]
pub struct PinkyBroadcaster<T: Clone, S = ()> {
    inner: Arc<Mutex<BroadcasterInner<T, S>>>,
}

struct BroadcasterInner<T, S> {
    promise: PinkySwear<T, S>,
    subscribers: Vec<Pinky<T, S>>,
}

impl<T: Send + 'static, S: 'static> PinkySwear<T, S> {
    /// Create a new PinkySwear and its associated Pinky.
    pub fn new() -> (Self, Pinky<T, S>) {
        let (send, recv) = sync_channel(1);
        let inner = Arc::new(Mutex::new(Inner::default()));
        let pinky = Pinky { send, inner };
        let promise = Self { recv, pinky };
        let pinky = promise.pinky();
        (promise, pinky)
    }

    /// Wait for this promise only once the given one is honoured.
    pub fn after<B: 'static>(promise: PinkySwear<S, B>) -> (Self, Pinky<T, S>) where S: Send {
        let (new_promise, new_pinky) = Self::new();
        promise.pinky.inner.lock().next = Some(Box::new(new_promise.pinky()));
        new_promise.pinky.inner.lock().before = Some(Box::new(promise));
        (new_promise, new_pinky)
    }

    /// Create a new PinkySwear and honour it at the same time.
    pub fn new_with_data(data: T) -> Self {
        let (promise, pinky) = Self::new();
        pinky.swear(data);
        promise
    }

    fn pinky(&self) -> Pinky<T, S> {
        self.pinky.clone()
    }

    /// Check whether the Promise has been honoured or not.
    pub fn try_wait(&self) -> Option<T> {
        let mut inner = self.pinky.inner.lock();
        if let Some(before) = inner.before.as_ref() {
            let _ = before.try_wait()?;
            inner.before = None;
        }
        if let Some((barrier, transform)) = inner.barrier.as_ref() {
            barrier.try_wait().map(transform)
        } else {
            self.recv.try_recv().ok()
        }
    }

    /// Wait until the Promise has been honoured.
    pub fn wait(&self) -> T {
        let mut inner = self.pinky.inner.lock();
        if let Some(before) = inner.before.take() {
            before.wait();
        }
        if let Some((barrier, transform)) = inner.barrier.as_ref() {
            transform(barrier.wait())
        } else {
            self.recv.recv().unwrap()
        }
    }

    /// Get notified once the Promise has been honoured.
    pub fn subscribe(&self, task: Box<dyn NotifyReady + Send>) {
        self.pinky.inner.lock().tasks.push(task);
    }

    /// Will someone get notified once the Promise is honoured?
    pub fn has_subscriber(&self) -> bool {
        let inner = self.pinky.inner.lock();
        inner.waker.is_some() || inner.next.is_some() || !inner.tasks.is_empty()
    }

    /// Drop all the pending subscribers
    pub fn drop_subscribers(&self) {
        self.pinky.inner.lock().tasks.clear()
    }

    /// Apply a tranformation to the result of a PinkySwear.
    pub fn traverse<F: Send + 'static>(
        self,
        transform: Box<dyn Fn(T) -> F + Send>,
    ) -> PinkySwear<F, T> {
        let (promise, pinky) = PinkySwear::new();
        self.pinky.inner.lock().next = Some(Box::new(promise.pinky()));
        pinky.inner.lock().barrier = Some((Box::new(self), transform));
        promise
    }
}

impl<T, S> Pinky<T, S> {
    /// Honour your PinkySwear by giving the promised data.
    pub fn swear(&self, data: T) {
        trace!("Resolving promise");
        let res = self.send.send(data);
        trace!("Resolving promise gave: {:?}", res);
        self.inner.lock().notify();
    }
}

impl<T, S> Clone for Pinky<T, S> {
    fn clone(&self) -> Self {
        Self {
            send: self.send.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<T, S> Inner<T, S> {
    fn notify(&self) {
        if let Some(waker) = self.waker.as_ref() {
            trace!("Got data, waking our waker");
            waker.wake_by_ref();
        }
        if let Some(next) = self.next.as_ref() {
            trace!("Got data, notifying next in chain");
            next.notify();
        }
        for task in self.tasks.iter() {
            trace!("Got data, notifying task");
            task.notify();
        }
    }
}

impl<T: Send + Clone + 'static, S: 'static> PinkyBroadcaster<T, S> {
    /// Create a new PinkyBroadcaster from a PinkySwear.
    pub fn new(promise: PinkySwear<T, S>) -> Self {
        let pinky = promise.pinky();
        let broadcaster = Self {
            inner: Arc::new(Mutex::new(BroadcasterInner {
                promise,
                subscribers: Vec::default(),
            })),
        };
        pinky.inner.lock().next = Some(Box::new(broadcaster.clone()));
        broadcaster
    }

    /// Subscribe to receive a broacast when the underlying promise get henoured.
    pub fn subscribe(&self) -> PinkySwear<T, S> {
        let (promise, pinky) = PinkySwear::new();
        self.inner.lock().subscribers.push(pinky);
        promise
    }

    /// Resolve the underlying promise and broadcast the result to subscribers.
    pub fn swear(&self, data: T) {
        let pinky = self.inner.lock().promise.pinky();
        pinky.swear(data);
    }
}

impl<T: Clone, S> Clone for PinkyBroadcaster<T, S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T, S> fmt::Debug for PinkySwear<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkySwear")
    }
}

impl<T: Clone, S> fmt::Debug for PinkyBroadcaster<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkyBroadcaster")
    }
}

impl<T, S> fmt::Debug for Pinky<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pinky")
    }
}

impl<T: Send + 'static, S: 'static> Future for PinkySwear<T, S> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        {
            trace!("Called from future, registering waker");
            let mut inner = self.pinky.inner.lock();
            if let Some(before) = inner.before.as_ref() {
                before.register_waker(cx.waker().clone());
            }
            if let Some((barrier, _)) = inner.barrier.as_ref() {
                barrier.register_waker(cx.waker().clone());
            }
            inner.waker = Some(cx.waker().clone());
        }
        self.try_wait().map(Poll::Ready).unwrap_or(Poll::Pending)
    }
}

trait Promise<T> {
    fn try_wait(&self) -> Option<T>;
    fn wait(&self) -> T;
    fn register_waker(&self, waker: Waker);
}

impl<T: Send + 'static, S: 'static> Promise<T> for PinkySwear<T, S> {
    fn try_wait(&self) -> Option<T> {
        self.try_wait()
    }

    fn wait(&self) -> T {
        self.wait()
    }

    fn register_waker(&self, waker: Waker) {
        self.pinky.inner.lock().waker = Some(waker);
    }
}

/// Sometimes you just cannot keep your Promises.
pub trait Cancellable<E> {
    /// Cancel the Promise you made, explaining why with an Error.
    fn cancel(&self, err: E);
}

impl<T, S, E> Cancellable<E> for Pinky<Result<T, E>, S> {
    fn cancel(&self, err: E) {
        self.swear(Err(err))
    }
}

/// Notify once a Promise is honoured.
pub trait NotifyReady {
    /// Notify once a Promise is honoured.
    fn notify(&self);
}

impl<T, S> NotifyReady for Pinky<T, S> {
    fn notify(&self) {
        self.inner.lock().notify();
    }
}

impl<T: Send + Clone + 'static, S: 'static> NotifyReady for PinkyBroadcaster<T, S> {
    fn notify(&self) {
        let inner = self.inner.lock();
        let data = inner.promise.wait();
        for subscriber in inner.subscribers.iter() {
            subscriber.swear(data.clone())
        }
    }
}

impl NotifyReady for Waker {
    fn notify(&self) {
        self.wake_by_ref();
    }
}
