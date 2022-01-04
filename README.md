Simple promise library compatible with `std::future` and async/await

[![API Docs](https://docs.rs/pinky-swear/badge.svg)](https://docs.rs/pinky-swear)
[![Build status](https://github.com/amqp-rs/pinky-swear/workflows/Build%20and%20test/badge.svg)](https://github.com/amqp-rs/pinky-swear/actions)
[![Downloads](https://img.shields.io/crates/d/pinky-swear.svg)](https://crates.io/crates/pinky-swear)

# Example

Create a promise and wait for the result while computing the result in another thread

```rust
use pinky_swear::{Pinky, PinkySwear};
use std::{thread, time::Duration};

fn compute(pinky: Pinky<Result<u32, ()>>) {
    thread::sleep(Duration::from_millis(1000));
    pinky.swear(Ok(42));
}

fn main() {
    let (promise, pinky) = PinkySwear::new();
    thread::spawn(move || {
        compute(pinky);
    });
    assert_eq!(promise.wait(), Ok(42));
}
```
