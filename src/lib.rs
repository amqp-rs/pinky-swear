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
//!     let (promise, pinky) = PinkySwear::<Result<u32, ()>>::new();
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
#![doc(html_root_url = "https://docs.rs/pinky-swear/2.3.0/")]

doc_comment::doctest!("../README.md");

use log::{trace, warn};
use parking_lot::{Mutex, RwLock};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    task::{Context, Poll, Waker},
};

/// A PinkySwear is a Promise that the other party is supposed to honour at some point.
#[must_use = "PinkySwear should be used or you can miss errors"]
pub struct PinkySwear<T, S = T> {
    recv: Receiver<T>,
    pinky: Pinky<T>,
    inner: Arc<Mutex<Inner<T, S>>>,
}

/// A Pinky allows you to fulfill a Promise that you made.
pub struct Pinky<T> {
    send: Sender<T>,
    subscribers: Arc<Mutex<Subscribers>>,
    marker: Arc<RwLock<Option<String>>>,
}

#[derive(Default)]
struct Subscribers {
    waker: Option<Waker>,
    next: Option<Box<dyn NotifyReady + Send>>,
    tasks: Vec<Box<dyn NotifyReady + Send>>,
}

struct Inner<T, S> {
    barrier: Option<(Box<dyn Promise<S> + Send>, Box<dyn Fn(S) -> T + Send>)>,
    before: Option<Box<dyn Promise<S> + Send>>,
}

impl<T, S> Default for Inner<T, S> {
    fn default() -> Self {
        Self {
            barrier: None,
            before: None,
        }
    }
}

/// A PinkyBroadcaster allows you to broacast a promise resolution to several subscribers.
#[must_use = "PinkyBroadcaster must be subscribed"]
pub struct PinkyBroadcaster<T: Clone, S = T> {
    inner: Arc<Mutex<BroadcasterInner<T, S>>>,
}

struct BroadcasterInner<T, S> {
    promise: PinkySwear<T, S>,
    subscribers: Vec<Pinky<T>>,
}

impl<T: Send + 'static, S: Send + 'static> PinkySwear<T, S> {
    /// Create a new PinkySwear and its associated Pinky.
    pub fn new() -> (Self, Pinky<T>) {
        let (send, recv) = mpsc::channel();
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let inner = Arc::new(Mutex::new(Inner::default()));
        let marker = Default::default();
        let pinky = Pinky { send, subscribers, marker };
        let promise = Self { recv, pinky, inner };
        let pinky = promise.pinky();
        (promise, pinky)
    }

    /// Create a new PinkySwear and honour it at the same time.
    pub fn new_with_data(data: T) -> Self {
        let (promise, pinky) = Self::new();
        pinky.swear(data);
        promise
    }

    fn pinky(&self) -> Pinky<T> {
        self.pinky.clone()
    }

    /// Check whether the Promise has been honoured or not.
    pub fn try_wait(&self) -> Option<T> {
        let mut inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
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
        self.pinky.subscribers.lock().tasks.push(task);
    }

    /// Will someone get notified once the Promise is honoured?
    pub fn has_subscriber(&self) -> bool {
        self.pinky.subscribers.lock().has_subscriber()
    }

    /// Drop all the pending subscribers
    pub fn drop_subscribers(&self) {
        self.pinky.subscribers.lock().tasks.clear()
    }

    /// Apply a tranformation to the result of a PinkySwear.
    pub fn traverse<F: Send + 'static>(
        self,
        transform: Box<dyn Fn(T) -> F + Send>,
    ) -> PinkySwear<F, T> {
        let (promise, _pinky) = PinkySwear::new();
        self.pinky.subscribers.lock().next = Some(Box::new(promise.pinky()));
        promise.inner.lock().barrier = Some((Box::new(self), transform));
        promise
    }

    /// Add a marker to logs
    pub fn set_marker(&self, marker: String) {
        *self.pinky.marker.write() = Some(marker);
    }

    fn set_waker(&self, waker: Waker) {
        trace!("{}Called from future, registering waker", self.pinky.marker());
        self.pinky.subscribers.lock().waker = Some(waker);
    }

    fn backward_waker(&self, waker: Waker) {
        trace!("{}Called from future, registering waker up in chain", self.pinky.marker());
        let inner = self.inner.lock();
        if let Some(before) = inner.before.as_ref() {
            before.register_waker(waker.clone());
        }
        if let Some((barrier, _)) = inner.barrier.as_ref() {
            barrier.register_waker(waker);
        }
    }
}

impl<T: Send + 'static> PinkySwear<T, ()> {
    /// Wait for this promise only once the given one is honoured.
    pub fn after<B: Send + 'static>(promise: PinkySwear<(), B>) -> (Self, Pinky<T>) {
        let (new_promise, new_pinky) = Self::new();
        promise.pinky.subscribers.lock().next = Some(Box::new(new_promise.pinky()));
        new_promise.inner.lock().before = Some(Box::new(promise));
        (new_promise, new_pinky)
    }
}

impl<T> Pinky<T> {
    /// Honour your PinkySwear by giving the promised data.
    pub fn swear(&self, data: T) {
        trace!("{}Resolving promise", self.marker());
        if let Err(err) = self.send.send(data) {
            warn!("{}Failed resolving promise, promise has vanished: {:?}", self.marker(), err);
        }
        self.subscribers.lock().notify();
    }

    fn marker(&self) -> String {
        self.marker.read().as_ref().map_or(String::default(), |marker| format!("[{}] ", marker))
    }
}

impl<T> PartialEq for Pinky<T> {
    fn eq(&self, other: &Pinky<T>) -> bool {
        Arc::ptr_eq(&self.subscribers, &other.subscribers)
    }
}

impl<T> Clone for Pinky<T> {
    fn clone(&self) -> Self {
        Self {
            send: self.send.clone(),
            subscribers: self.subscribers.clone(),
            marker: self.marker.clone(),
        }
    }
}

impl Subscribers {
    fn has_subscriber(&self) -> bool {
        self.waker.is_some() || self.next.is_some() || !self.tasks.is_empty()
    }

    fn notify(&self) {
        let mut notified = false;
        if let Some(waker) = self.waker.as_ref() {
            trace!("Got data, waking our waker");
            waker.wake_by_ref();
            notified = true;
        }
        if let Some(next) = self.next.as_ref() {
            trace!("Got data, notifying next in chain");
            next.notify();
            notified = true;
        }
        for task in self.tasks.iter() {
            trace!("Got data, notifying task");
            task.notify();
            notified = true;
        }
        if !notified {
            trace!("Got data but we have no one to notify");
        }
    }
}

impl<T: Send + Clone + 'static, S: Send + 'static> PinkyBroadcaster<T, S> {
    /// Create a new PinkyBroadcaster from a PinkySwear.
    pub fn new(promise: PinkySwear<T, S>) -> Self {
        let pinky = promise.pinky();
        let broadcaster = Self {
            inner: Arc::new(Mutex::new(BroadcasterInner {
                promise,
                subscribers: Vec::default(),
            })),
        };
        pinky.subscribers.lock().next = Some(Box::new(broadcaster.clone()));
        broadcaster
    }

    /// Subscribe to receive a broacast when the underlying promise get henoured.
    pub fn subscribe(&self) -> PinkySwear<T, S> {
        self.inner.lock().subscribe()
    }

    /// Unsubscribe a promise from the broadcast.
    pub fn unsubscribe(&self, promise: PinkySwear<T, S>) {
        self.inner.lock().unsubscribe(promise);
    }

    /// Resolve the underlying promise and broadcast the result to subscribers.
    pub fn swear(&self, data: T) {
        let pinky = self.inner.lock().promise.pinky();
        pinky.swear(data);
    }
}

impl <T: Send + 'static, S: Send + 'static> BroadcasterInner<T, S> {
    fn subscribe(&mut self) -> PinkySwear<T, S> {
        let (promise, pinky) = PinkySwear::new();
        self.subscribers.push(pinky);
        if let Some(marker) = self.promise.pinky.marker.read().clone() {
            promise.set_marker(marker);
        }
        promise
    }

    fn unsubscribe(&mut self, promise: PinkySwear<T, S>) {
        self.subscribers.retain(|pinky| pinky != &promise.pinky)
    }
}

impl<T: Send + Clone + 'static, S: Send + 'static> Default for PinkyBroadcaster<T, S> {
    fn default() -> Self {
        let (promise, _) = PinkySwear::new();
        Self::new(promise)
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

impl<T> fmt::Debug for Pinky<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pinky")
    }
}

impl<T: Send + 'static, S: Send + 'static> Future for PinkySwear<T, S> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.set_waker(cx.waker().clone());
        self.backward_waker(cx.waker().clone());
        self.try_wait().map(Poll::Ready).unwrap_or(Poll::Pending)
    }
}

trait Promise<T> {
    fn try_wait(&self) -> Option<T>;
    fn wait(&self) -> T;
    fn register_waker(&self, waker: Waker);
}

impl<T: Send + 'static, S: Send + 'static> Promise<T> for PinkySwear<T, S> {
    fn try_wait(&self) -> Option<T> {
        self.try_wait()
    }

    fn wait(&self) -> T {
        self.wait()
    }

    fn register_waker(&self, waker: Waker) {
        self.set_waker(waker);
    }
}

/// Sometimes you just cannot keep your Promises.
pub trait Cancellable<E> {
    /// Cancel the Promise you made, explaining why with an Error.
    fn cancel(&self, err: E);
}

impl<T, E> Cancellable<E> for Pinky<Result<T, E>> {
    fn cancel(&self, err: E) {
        self.swear(Err(err))
    }
}

/// Notify once a Promise is honoured.
pub trait NotifyReady {
    /// Notify once a Promise is honoured.
    fn notify(&self);
}

impl<T> NotifyReady for Pinky<T> {
    fn notify(&self) {
        self.subscribers.lock().notify();
    }
}

impl<T: Send + Clone + 'static, S: Send + 'static> NotifyReady for PinkyBroadcaster<T, S> {
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

impl<T, S> Drop for PinkySwear<T, S> {
    fn drop(&mut self) {
        trace!("{}Dropping promise", self.pinky.marker());
    }
}
