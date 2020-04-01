Simple promise library compatible with `std::future` and async/await

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
    let (promise, pinky) = PinkySwear::<Result<u32, ()>>::new();
    thread::spawn(move || {
        compute(pinky);
    });
    assert_eq!(promise.wait(), Ok(42));
}
```
