//! Single-consumer progress streams over Tauri **Channels** rather than events
//! (#65).
//!
//! Tauri's own guidance is that the event system "is not designed for low latency
//! or high throughput situations" and that Channels are the primitive for ordered
//! streamed data. #60 was the throughput half of that lesson (one event per log
//! record froze the UI). This is the *ordering* half, which is a correctness
//! concern rather than a performance one: the docs warn that rapid events
//! delivered to an async listener may be processed out of order, and a progress
//! bar that jumps 40% → 38% → 41% is wrong, not merely ugly.
//!
//! Each of these streams is single-producer, single-consumer by construction —
//! one background job emits, and exactly one React provider consumes and fans out
//! through context. So a stream holds ONE channel slot. Subscribing again
//! replaces it, which is what a webview reload does.
//!
//! Two consequences worth stating, because both look like bugs otherwise:
//!
//! - **Sends before anyone subscribes are dropped.** They were effectively
//!   dropped with events too (an event reaches only a live listener), so this is
//!   not a regression — and it is why every one of these streams pairs with a
//!   status snapshot command the UI reads at mount to re-attach to work already
//!   in flight. The snapshot is the source of truth for "what is happening now";
//!   the stream only carries updates. (Snapshot before subscribe or after both
//!   work: progress ticks again, so a value crossing the two calls self-corrects
//!   on the next update.)
//! - **A send after the consumer is gone fails silently.** That is deliberate:
//!   background work must not fail because a window closed.

use std::sync::{Mutex, OnceLock};

use tauri::ipc::Channel;

/// A single-consumer stream of progress payloads.
///
/// Declared as a `static` per stream, e.g.
/// `static IMPORT: ProgressStream<ImportEvent> = ProgressStream::new();`
pub struct ProgressStream<T: Send + 'static> {
    slot: OnceLock<Mutex<Option<Channel<T>>>>,
}

impl<T: Send + 'static> ProgressStream<T> {
    pub const fn new() -> Self {
        Self {
            slot: OnceLock::new(),
        }
    }

    fn cell(&self) -> &Mutex<Option<Channel<T>>> {
        self.slot.get_or_init(|| Mutex::new(None))
    }

    /// Install the frontend's channel, replacing any previous one. A dev-server
    /// reload would otherwise leave a dead channel installed and the new UI
    /// silently receiving nothing.
    pub fn subscribe(&self, channel: Channel<T>) {
        if let Ok(mut g) = self.cell().lock() {
            *g = Some(channel);
        }
    }
}

impl<T: Clone + Send + Sync + serde::Serialize + 'static> ProgressStream<T> {
    /// Send one payload to the current consumer, if there is one.
    ///
    /// Ignores send failures on purpose: the only reason to fail here is that the
    /// webview went away, and a scan must not die because someone closed a
    /// window. The lock is never held across the send's callers, so a panicking
    /// consumer cannot poison this for the rest of the run.
    pub fn send(&self, payload: T) {
        let ch = match self.cell().lock() {
            Ok(g) => g.clone(),
            Err(e) => e.into_inner().clone(),
        };
        if let Some(ch) = ch {
            let _ = ch.send(payload);
        }
    }
}

impl<T: Send + 'static> Default for ProgressStream<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tauri::ipc::InvokeResponseBody;

    /// A channel that counts what it was sent, standing in for the webview.
    fn counting_channel() -> (Channel<u32>, Arc<AtomicUsize>) {
        let seen = Arc::new(AtomicUsize::new(0));
        let s = seen.clone();
        let ch = Channel::new(move |_: InvokeResponseBody| {
            s.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        (ch, seen)
    }

    #[test]
    fn sends_before_anyone_subscribes_are_dropped_not_buffered() {
        // The snapshot commands, not the stream, are what re-attach a reloaded
        // UI. If this ever started buffering, a subscriber would be handed a
        // burst of stale progress on connect.
        let stream: ProgressStream<u32> = ProgressStream::new();
        stream.send(1);
        let (ch, seen) = counting_channel();
        stream.subscribe(ch);
        assert_eq!(seen.load(Ordering::SeqCst), 0);
        stream.send(2);
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "only the post-subscribe send"
        );
    }

    #[test]
    fn subscribing_again_replaces_the_previous_consumer() {
        // What a webview reload does. Leaving the old channel installed was the
        // failure this replaces: the new UI would sit silent while progress went
        // to a dead channel.
        let stream: ProgressStream<u32> = ProgressStream::new();
        let (first, first_seen) = counting_channel();
        stream.subscribe(first);
        stream.send(1);
        let (second, second_seen) = counting_channel();
        stream.subscribe(second);
        stream.send(2);
        stream.send(3);
        assert_eq!(
            first_seen.load(Ordering::SeqCst),
            1,
            "stops after replacement"
        );
        assert_eq!(
            second_seen.load(Ordering::SeqCst),
            2,
            "gets everything after"
        );
    }

    #[test]
    fn every_send_reaches_the_consumer_in_order() {
        // The reason for the whole change: an ordered stream, not "may process
        // out of order" like rapid events with an async listener.
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let o = order.clone();
        let ch = Channel::new(move |body: InvokeResponseBody| {
            if let InvokeResponseBody::Json(s) = body {
                o.lock().unwrap().push(s);
            }
            Ok(())
        });
        let stream: ProgressStream<u32> = ProgressStream::new();
        stream.subscribe(ch);
        for i in 0..50 {
            stream.send(i);
        }
        let got = order.lock().unwrap().clone();
        assert_eq!(got.len(), 50);
        let expected: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        assert_eq!(got, expected);
    }
}
