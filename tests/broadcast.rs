use pinky_swear::PinkyBroadcaster;

#[test]
fn broadcaster() {
    let broadcaster = PinkyBroadcaster::<Result<(), ()>>::default();
    let sub1 = broadcaster.subscribe();
    let sub2 = broadcaster.subscribe();
    broadcaster.swear(Ok(()));
    assert_eq!(sub1.wait(), Ok(()));
    assert_eq!(sub2.wait(), Ok(()));
}
