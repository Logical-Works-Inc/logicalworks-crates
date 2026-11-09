use super::Clock;
use std::future::Future as _;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

/// A waker that counts its wakes, so the test can prove the advance
/// reached the parked future rather than only that its condition held.
struct Countdown(AtomicUsize);

impl Wake for Countdown {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A virtual sleep is pending until an advance reaches its deadline, and
/// then ready — with no runtime, no timer and no real time spent.
#[test]
fn a_virtual_sleep_waits_for_the_advance_that_reaches_its_deadline() {
    let clock = Clock::virtual_at(Duration::ZERO);
    let woken = Arc::new(Countdown(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&woken));
    let mut context = Context::from_waker(&waker);
    let mut sleep = pin!(clock.sleep(Duration::from_millis(10)));
    assert!(
        matches!(sleep.as_mut().poll(&mut context), Poll::Pending),
        "a virtual clock nobody advanced must not complete its sleep"
    );
    assert!(
        clock.advance(Duration::from_millis(5)).is_ok(),
        "half the deadline is inside the representable range"
    );
    assert!(
        matches!(sleep.as_mut().poll(&mut context), Poll::Pending),
        "half the deadline is not the deadline"
    );
    assert!(
        clock.advance(Duration::from_millis(5)).is_ok(),
        "the second half is inside the representable range"
    );
    assert!(
        woken.0.load(Ordering::Relaxed) >= 1,
        "the advance must wake the parked sleep, not only satisfy its condition"
    );
    assert!(
        matches!(sleep.as_mut().poll(&mut context), Poll::Ready(())),
        "an advance that reaches the deadline must complete the sleep"
    );
}
