use pinky_swear::{PinkyBroadcaster, PinkySwear};

#[test]
fn broadcaster_external_swear() {
    let (promise, pinky) = PinkySwear::<Result<(), ()>>::new();
    let broadcaster = PinkyBroadcaster::new(promise);
    let sub1 = broadcaster.subscribe();
    let sub2 = broadcaster.subscribe();
    pinky.swear(Ok(()));
    assert_eq!(sub1.wait(), Ok(()));
    assert_eq!(sub2.wait(), Ok(()));
}

#[test]
fn broadcaster_internal_swear() {
    let (promise, _pinky) = PinkySwear::<Result<(), ()>>::new();
    let broadcaster = PinkyBroadcaster::new(promise);
    let sub1 = broadcaster.subscribe();
    let sub2 = broadcaster.subscribe();
    broadcaster.swear(Ok(()));
    assert_eq!(sub1.wait(), Ok(()));
    assert_eq!(sub2.wait(), Ok(()));
}
