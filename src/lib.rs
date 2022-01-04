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
#![doc(html_root_url = "https://docs.rs/pinky-swear/6.0.0/")]

doc_comment::doctest!("../README.md");

use flume::{Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use tracing::{trace, warn};

/// Sometimes you just cannot keep your Promises.
pub trait Cancellable<E> {
    /// Cancel the Promise you made, explaining why with an Error.
    fn cancel(&self, err: E);
}

/// A PinkySwear is a Promise that the other party is supposed to honour at some point.
#[must_use = "PinkySwear should be used or you can miss errors"]
pub struct PinkySwear<T> {
    recv: Receiver<T>,
    pinky: Pinky<T>,
}

impl<T: Send + 'static> PinkySwear<T> {
    /// Create a new PinkySwear and its associated Pinky.
    pub fn new() -> (Self, Pinky<T>) {
        let (send, recv) = flume::unbounded();
        let pinky = Pinky {
            send,
            waker: Default::default(),
            marker: Default::default(),
        };
        let promise = Self { recv, pinky };
        let pinky = promise.pinky.clone();
        (promise, pinky)
    }

    /// Create a new PinkySwear and honour it at the same time.
    pub fn new_with_data(data: T) -> Self {
        let (promise, pinky) = Self::new();
        pinky.swear(data);
        promise
    }

    /// Check whether the Promise has been honoured or not.
    pub fn try_wait(&self) -> Option<T> {
        self.recv.try_recv().ok()
    }

    /// Wait until the Promise has been honoured.
    pub fn wait(&self) -> T {
        self.recv.recv().unwrap()
    }

    /// Add a marker to logs
    pub fn set_marker(&self, marker: String) {
        self.pinky.set_marker(marker);
    }

    fn set_waker(&self, waker: Waker) {
        trace!(
            promise = %self.pinky.marker(),
            "Called from future, registering waker.",
        );
        *self.pinky.waker.lock() = Some(waker);
    }
}

impl<T: Send + 'static> Future for PinkySwear<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.set_waker(cx.waker().clone());
        self.try_wait().map(Poll::Ready).unwrap_or(Poll::Pending)
    }
}

impl<T> fmt::Debug for PinkySwear<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkySwear")
    }
}

impl<T> Drop for PinkySwear<T> {
    fn drop(&mut self) {
        trace!(
            promise = %self.pinky.marker(),
            "Dropping promise.",
        );
    }
}

/// A Pinky allows you to fulfill a Promise that you made.
pub struct Pinky<T> {
    send: Sender<T>,
    waker: Arc<Mutex<Option<Waker>>>,
    marker: Arc<RwLock<Option<String>>>,
}

impl<T> Pinky<T> {
    /// Honour your PinkySwear by giving the promised data.
    pub fn swear(&self, data: T) {
        trace!(
            promise = %self.marker(),
            "Resolving promise.",
        );
        if let Err(err) = self.send.send(data) {
            warn!(
                promise = %self.marker(),
                error = %err,
                "Failed resolving promise, promise has vanished.",
            );
        }
        if let Some(waker) = self.waker.lock().as_ref() {
            trace!("Got data, waking our waker.");
            waker.wake_by_ref();
        } else {
            trace!("Got data but we have no one to notify.");
        }
    }

    fn set_marker(&self, marker: String) {
        *self.marker.write() = Some(marker);
    }

    fn marker(&self) -> String {
        self.marker
            .read()
            .as_ref()
            .map_or(String::default(), |marker| format!("[{}] ", marker))
    }
}

impl<T> Clone for Pinky<T> {
    fn clone(&self) -> Self {
        Self {
            send: self.send.clone(),
            waker: self.waker.clone(),
            marker: self.marker.clone(),
        }
    }
}

impl<T, E> Cancellable<E> for Pinky<Result<T, E>> {
    fn cancel(&self, err: E) {
        self.swear(Err(err))
    }
}

impl<T> PartialEq for Pinky<T> {
    fn eq(&self, other: &Pinky<T>) -> bool {
        Arc::ptr_eq(&self.waker, &other.waker)
    }
}

impl<T> fmt::Debug for Pinky<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pinky")
    }
}

/// A PinkyErrorBroadcaster allows you to broacast the success/error of a promise resolution to several subscribers.
pub struct PinkyErrorBroadcaster<T, E: Clone> {
    marker: Arc<RwLock<Option<String>>>,
    inner: Arc<Mutex<ErrorBroadcasterInner<E>>>,
    pinky: Pinky<Result<T, E>>,
}

impl<T: Send + 'static, E: Send + Clone + 'static> PinkyErrorBroadcaster<T, E> {
    /// Create a new promise with associated error broadcaster
    pub fn new() -> (PinkySwear<Result<T, E>>, Self) {
        let (promise, pinky) = PinkySwear::new();
        (
            promise,
            Self {
                marker: Default::default(),
                inner: Arc::new(Mutex::new(ErrorBroadcasterInner(Vec::default()))),
                pinky,
            },
        )
    }

    /// Add a marker to logs
    pub fn set_marker(&self, marker: String) {
        for subscriber in self.inner.lock().0.iter() {
            subscriber.set_marker(marker.clone());
        }
        *self.marker.write() = Some(marker);
    }

    /// Subscribe to receive a broacast when the underlying promise get henoured.
    pub fn subscribe(&self) -> PinkySwear<Result<(), E>> {
        self.inner.lock().subscribe(self.marker.read().clone())
    }

    /// Unsubscribe a promise from the broadcast.
    pub fn unsubscribe(&self, promise: PinkySwear<Result<(), E>>) {
        self.inner.lock().unsubscribe(promise);
    }

    /// Resolve the underlying promise and broadcast the result to subscribers.
    pub fn swear(&self, data: Result<T, E>) {
        for subscriber in self.inner.lock().0.iter() {
            subscriber.swear(data.as_ref().map(|_| ()).map_err(Clone::clone))
        }
        self.pinky.swear(data);
    }
}

impl<T, E: Clone> fmt::Debug for PinkyErrorBroadcaster<T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinkyErrorBroadcaster")
    }
}

struct ErrorBroadcasterInner<E>(Vec<Pinky<Result<(), E>>>);

impl<E: Send + 'static> ErrorBroadcasterInner<E> {
    fn subscribe(&mut self, marker: Option<String>) -> PinkySwear<Result<(), E>> {
        let (promise, pinky) = PinkySwear::new();
        self.0.push(pinky);
        if let Some(marker) = marker {
            promise.set_marker(marker);
        }
        promise
    }

    fn unsubscribe(&mut self, promise: PinkySwear<Result<(), E>>) {
        self.0.retain(|pinky| pinky != &promise.pinky)
    }
}
