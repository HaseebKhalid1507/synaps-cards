//! The table: the wait a new chat is told, the entries a chat keeps, and the
//! memory given back. The rate tests (`tool/tests.rs`) count through it.

use super::*;

fn at(t0: Instant, ms: u64) -> Instant {
    t0 + Duration::from_millis(ms)
}

/// With every place held, a new chat waits until the chat idle longest
/// leaves, to the millisecond, and is in the moment it does; a kept chat
/// pushes whatever the table holds, and reads its entries oldest first.
#[test]
fn the_wait_is_until_the_chat_idle_longest_leaves() {
    let t0 = Instant::now();
    let mut chats = Chats::<u8>::default();
    for i in 0..CHATS_MAX as u64 {
        chats.at(&format!("chat-{i}"), at(t0, 100 * i)).push(1).unwrap();
    }
    chats.at("chat-1", at(t0, 20_000)).push(2).unwrap();
    for (t, wait) in [(30_000, 570_000), (30_001, 569_999), (599_999, 1)] {
        assert_eq!(chats.at("new", at(t0, t)).push(9), Err(Duration::from_millis(wait)), "at {t} ms");
    }
    assert!(chats.window("new").is_none(), "a refused chat isn't kept");
    assert!(chats.at("new", at(t0, 599_999)).entries().is_empty());
    chats.at("new", at(t0, 600_000)).push(9).unwrap();
    assert!(chats.window("chat-0").is_none());
    // chat-1 showed again at 20 s, so chat-2 (200 ms) is idle longest now.
    assert_eq!(chats.at("newer", at(t0, 600_000)).push(9), Err(Duration::from_millis(200)));
    let place = chats.at("chat-1", at(t0, 600_000));
    let got: Vec<(Duration, u8)> = place.entries().iter().map(|&(t, v)| (t - t0, v)).collect();
    assert_eq!(got, [(Duration::from_millis(100), 1), (Duration::from_secs(20), 2)]);
    assert_eq!(chats.len(), CHATS_MAX);
}

/// A burst's memory is given back: a chat's 200 entries once 199 have left
/// (an entry [`KEPT`] old leaves, one a millisecond younger stays), and the
/// table once its chats have.
#[test]
fn memory_is_given_back() {
    let t0 = Instant::now();
    let mut chats = Chats::<()>::default();
    for i in 0..200 {
        chats.at("burst", at(t0, i)).push(()).unwrap();
    }
    for i in 0..CHATS_MAX as u64 - 1 {
        chats.at(&format!("chat-{i}"), at(t0, 199)).push(()).unwrap();
    }
    assert!(chats.window("burst").unwrap().capacity() >= 200);
    let crowd = chats.capacity();
    let _ = chats.at("burst", at(t0, 600_198));
    assert_eq!(chats.window("burst").unwrap().len(), 1);
    assert!(chats.window("burst").unwrap().capacity() <= 16, "{}", chats.window("burst").unwrap().capacity());
    let _ = chats.at("burst", at(t0, 600_199));
    assert_eq!(chats.len(), 0);
    assert!(chats.capacity() <= 16, "{} after {crowd}", chats.capacity());
}
