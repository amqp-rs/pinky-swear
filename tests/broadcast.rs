use pinky_swear::PinkyErrorBroadcaster;

#[test]
fn broadcaster_ok() {
    let (promise, broadcaster) = PinkyErrorBroadcaster::<u8, ()>::new();
    let sub1 = broadcaster.subscribe();
    let sub2 = broadcaster.subscribe();
    broadcaster.swear(Ok(42));
    assert_eq!(promise.wait(), Ok(42));
    assert_eq!(sub1.wait(), Ok(()));
    assert_eq!(sub2.wait(), Ok(()));
}

#[test]
fn broadcaster_err() {
    let (promise, broadcaster) = PinkyErrorBroadcaster::<(), u8>::new();
    let sub1 = broadcaster.subscribe();
    let sub2 = broadcaster.subscribe();
    broadcaster.swear(Err(42));
    assert_eq!(promise.wait(), Err(42));
    assert_eq!(sub1.wait(), Err(42));
    assert_eq!(sub2.wait(), Err(42));
}
