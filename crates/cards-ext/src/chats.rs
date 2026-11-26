//! What each chat did in the last ten minutes, bounded across chats: the
//! table a rate counts in (`cards_ui`'s cards, design §5.5). One process
//! serves every chat of a daemon, so the table is shared, and a cap on each
//! chat alone leaves the number of chats open.
//!
//! **The bound.** At most [`CHATS_MAX`] chats are kept, each under a 64-bit
//! keyed hash of its name, so a long host id costs what a short one does
//! (two names that collide share one limit, which errs strict). A chat
//! leaves when its last entry is [`KEPT`] old, so the chats idle longest
//! leave first, and never sooner: by then nothing it did still counts, and
//! leaving resets nothing. No chat with an entry in its window is evicted,
//! so no crowd of other chats can buy one a fresh budget; while all
//! [`CHATS_MAX`] have one, a new chat is refused until the one idle longest
//! leaves ([`Place::push`]). Every look prunes, so a chat's place is free the
//! moment it leaves, and the memory a crowd or a burst took is given back
//! once the table, or a chat's entries, are a quarter full.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, RandomState};
use std::time::{Duration, Instant};

/// The most chats kept at once.
pub const CHATS_MAX: usize = 256;
/// How long an entry is kept: the longest window a rate counts.
pub const KEPT: Duration = Duration::from_secs(600);

/// The chats, each with its entries of the last [`KEPT`], oldest first.
pub struct Chats<T> {
    windows: HashMap<u64, VecDeque<(Instant, T)>>,
    names: RandomState,
    none: VecDeque<(Instant, T)>,
}

impl<T> Default for Chats<T> {
    fn default() -> Self {
        Chats { windows: HashMap::new(), names: RandomState::new(), none: VecDeque::new() }
    }
}

impl<T> Chats<T> {
    /// Forget every entry [`KEPT`] old at `now`, and every chat left with
    /// none, giving back the memory that frees; then `chat`'s place.
    pub fn at(&mut self, chat: &str, now: Instant) -> Place<'_, T> {
        self.windows.retain(|_, entries| {
            while entries.front().is_some_and(|&(t, _)| now.saturating_duration_since(t) >= KEPT) {
                entries.pop_front();
            }
            if let Some(n) = give_back(entries.capacity(), entries.len()) {
                entries.shrink_to(n);
            }
            !entries.is_empty()
        });
        if let Some(n) = give_back(self.windows.capacity(), self.windows.len()) {
            self.windows.shrink_to(n);
        }
        let chat = self.names.hash_one(chat);
        Place { chats: self, chat, now }
    }
}

/// A chat's place in the table at one instant ([`Chats::at`]).
pub struct Place<'a, T> {
    chats: &'a mut Chats<T>,
    chat: u64,
    now: Instant,
}

impl<T> Place<'_, T> {
    /// The chat's entries of the last [`KEPT`], oldest first: none for a
    /// chat that isn't kept.
    pub fn entries(&self) -> &VecDeque<(Instant, T)> {
        self.chats.windows.get(&self.chat).unwrap_or(&self.chats.none)
    }

    /// Count `value` for the chat at this instant. `Err(wait)` when the chat
    /// isn't kept and every place is held by a chat with an entry in its
    /// window: `wait` is how long until the one idle longest leaves. A
    /// refused entry isn't kept, and neither is the chat.
    pub fn push(self, value: T) -> Result<(), Duration> {
        let Place { chats, chat, now } = self;
        if !chats.windows.contains_key(&chat) && chats.windows.len() >= CHATS_MAX {
            let last = chats.windows.values().filter_map(|e| e.back()).map(|&(t, _)| t).min().unwrap_or(now);
            return Err(KEPT.saturating_sub(now.saturating_duration_since(last)));
        }
        chats.windows.entry(chat).or_default().push_back((now, value));
        Ok(())
    }
}

/// The capacity to shrink a collection to once it's a quarter full: twice
/// its length, so the next entries don't grow it straight back. Small ones
/// are left alone.
fn give_back(capacity: usize, len: usize) -> Option<usize> {
    (capacity > 16 && capacity >= 4 * len).then_some(2 * len)
}

#[cfg(test)]
impl<T> Chats<T> {
    pub(crate) fn window(&self, chat: &str) -> Option<&VecDeque<(Instant, T)>> {
        self.windows.get(&self.names.hash_one(chat))
    }

    pub(crate) fn len(&self) -> usize {
        self.windows.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.windows.capacity()
    }
}

#[cfg(test)]
mod tests;
