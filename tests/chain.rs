use pinky_swear::{PinkySwear};

#[test]
fn chain_ok() {
    let (promise, pinky) = PinkySwear::<()>::new();
    pinky.swear(());
    let (promise, pinky) = PinkySwear::<Result<(), ()>, ()>::after(promise);
    pinky.swear(Ok(()));
    let (promise, pinky) = PinkySwear::<Result<(), ()>>::after(promise);
    pinky.swear(Ok(()));
    assert_eq!(promise.wait(), Ok(()));
}

#[test]
fn chain_err_end() {
    let (promise, pinky) = PinkySwear::<()>::new();
    pinky.swear(());
    let (promise, pinky) = PinkySwear::<Result<(), &'static str>, ()>::after(promise);
    pinky.swear(Ok(()));
    let (promise, pinky) = PinkySwear::<Result<(), &'static str>>::after(promise);
    pinky.swear(Err("end"));
    assert_eq!(promise.wait(), Err("end"));
}

#[test]
fn chain_err_middle() {
    let (promise, pinky) = PinkySwear::<()>::new();
    pinky.swear(());
    let (promise, pinky) = PinkySwear::<Result<(), &'static str>, ()>::after(promise);
    pinky.swear(Err("middle"));
    let (promise, pinky) = PinkySwear::<Result<(), &'static str>>::after(promise);
    pinky.swear(Ok(()));
    assert_eq!(promise.wait(), Err("middle"));
}

#[test]
fn chain_err_middle_end() {
    let (promise, pinky) = PinkySwear::<()>::new();
    pinky.swear(());
    let (promise, pinky) = PinkySwear::<Result<(), &'static str>, ()>::after(promise);
    pinky.swear(Err("middle"));
    let (promise, pinky) = PinkySwear::<Result<(), &'static str>>::after(promise);
    pinky.swear(Err("end"));
    assert_eq!(promise.wait(), Err("middle"));
}
