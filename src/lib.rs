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
#![doc(html_root_url = "https://docs.rs/amq-protocol/1.1.0/")]

doc_comment::doctest!("../README.md");

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
    main_task: Option<Box<dyn NotifyReady + Send>>,
    tasks: Vec<Box<dyn NotifyReady + Send>>,
    barrier: Option<(Box<dyn Promise<S> + Send>, Box<dyn Fn(S) -> T + Send>)>,
    before: Option<Box<dyn Promise<S> + Send>>,
}

impl<T, S> Default for Inner<T, S> {
    fn default() -> Self {
        Self {
            main_task: None,
            tasks: Vec::default(),
            barrier: None,
            before: None,
        }
    }
}

impl<T: Send + 'static, S: 'static> PinkySwear<T, S> {
    /// Create a new PinkySwear and its associated Pinky.
    pub fn new() -> (Self, Pinky<T, S>) {
        let promise = Self::new_with_inner(Inner::default());
        let pinky = promise.pinky();
        (promise, pinky)
    }

    /// Wait for this promise only once the given one is honoured.
    pub fn after<B: 'static>(promise: PinkySwear<S, B>) -> (Self, Pinky<T, S>) where S: Send {
        let inner = Inner {
            before: Some(Box::new(promise)),
            ..Inner::default()
        };
        let promise = Self::new_with_inner(inner);
        let pinky = promise.pinky();
        (promise, pinky)
    }

    fn new_with_inner(inner: Inner<T, S>) -> Self {
        let (send, recv) = sync_channel(1);
        let inner = Arc::new(Mutex::new(inner));
        let pinky = Pinky { send, inner };
        Self { recv, pinky }
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
        inner.main_task.is_some() || !inner.tasks.is_empty()
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
        let inner = Inner {
            barrier: Some((Box::new(self), transform)),
            ..Inner::default()
        };
        PinkySwear::new_with_inner(inner)
    }
}

impl<T, S> Pinky<T, S> {
    /// Honour your PinkySwear by giving the promised data.
    pub fn swear(&self, data: T) {
        let _ = self.send.send(data);
        let inner = self.inner.lock();
        if let Some(task) = inner.main_task.as_ref() {
            task.notify();
        }
        for task in inner.tasks.iter() {
            task.notify();
        }
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

impl<T: Send + 'static, S: 'static> Future for PinkySwear<T, S> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        {
            let mut inner = self.pinky.inner.lock();
            inner.main_task = Some(Box::new(cx.waker().clone()));
        }
        self.try_wait().map(Poll::Ready).unwrap_or(Poll::Pending)
    }
}

trait Promise<T> {
    fn try_wait(&self) -> Option<T>;
    fn wait(&self) -> T;
}

impl<T: Send + 'static, S: 'static> Promise<T> for PinkySwear<T, S> {
    fn try_wait(&self) -> Option<T> {
        self.try_wait()
    }

    fn wait(&self) -> T {
        self.wait()
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

impl NotifyReady for Waker {
    fn notify(&self) {
        self.wake_by_ref();
    }
}
