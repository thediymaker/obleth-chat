//! Wakes a wait the moment what it waits for arrives: a file session's job waiting
//! for its next call, or a chat waiting for a call's result. A wake is sent as a
//! Postgres notification, so it reaches every replica, and is delivered here through
//! an in-process map of waiters. Every wait also wakes by itself after a few seconds,
//! so nothing depends on a notification arriving; it only makes the wait instant and
//! the idle time free of queries.
use sqlx::{postgres::PgListener, PgPool};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};
use tokio_util::sync::CancellationToken;

/// The notification channel every replica listens on; the payload is the key.
pub const CHANNEL: &str = "obleth_chat_wakeups";

#[derive(Clone, Default)]
pub struct Wakeups {
    waiters: Arc<Mutex<Waiters>>,
}

#[derive(Default)]
struct Waiters {
    /// Each key's wake, with when the key was first seen here.
    keys: HashMap<String, (Arc<Notify>, Option<Instant>)>,
    swept: Option<Instant>,
}

/// A wake nobody waits for is kept this long. Every replica hears every wake, and
/// most are for a wait on another replica, so they would otherwise stay for good.
const KEPT: Duration = Duration::from_secs(600);

impl Wakeups {
    fn entry(&self, key: &str) -> Arc<Notify> {
        let mut waiters = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if waiters.swept.is_none_or(|at| now.duration_since(at) > KEPT) {
            waiters.swept = Some(now);
            // The map's is the only holder of a wake no one is waiting on.
            waiters.keys.retain(|_, (wake, seen)| {
                Arc::strong_count(wake) > 1 || seen.is_none_or(|at| now.duration_since(at) < KEPT)
            });
        }
        let (wake, seen) = waiters.keys.entry(key.to_owned()).or_default();
        seen.get_or_insert(now);
        wake.clone()
    }

    /// Wakes the wait on this key here. With no one waiting yet, the wake is kept
    /// for the next wait, so a result that lands between a check and a wait is not
    /// missed.
    pub fn wake(&self, key: &str) {
        self.entry(key).notify_one();
    }

    /// Waits for a wake on the key, at most `most`. True when woken.
    pub async fn wait(&self, key: &str, most: Duration) -> bool {
        let notify = self.entry(key);
        let woken = tokio::time::timeout(most, notify.notified()).await.is_ok();
        let mut waiters = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        // Ours and the map's are the only holders: nothing else waits on it.
        if waiters
            .keys
            .get(key)
            .is_some_and(|(held, _)| Arc::strong_count(held) <= 2)
        {
            waiters.keys.remove(key);
        }
        woken
    }

    /// Wakes the key on every replica, this one at once.
    pub async fn send(&self, pool: &PgPool, key: &str) {
        self.wake(key);
        if let Err(error) = sqlx::query("SELECT pg_notify($1, $2)")
            .bind(CHANNEL)
            .bind(key)
            .execute(pool)
            .await
        {
            tracing::warn!("could not send a wakeup: {error}");
        }
    }

    /// Delivers the other replicas' wakes until shutdown, reconnecting when the
    /// listening connection drops.
    pub async fn listen(self, pool: PgPool, shutdown: CancellationToken) {
        loop {
            let listening = async {
                let mut listener = PgListener::connect_with(&pool).await?;
                listener.listen(CHANNEL).await?;
                loop {
                    let notification = listener.recv().await?;
                    self.wake(notification.payload());
                }
                #[allow(unreachable_code)]
                Ok::<(), sqlx::Error>(())
            };
            tokio::select! {
                _ = shutdown.cancelled() => return,
                result = listening => {
                    if let Err(error) = result {
                        tracing::warn!("wakeups listener stopped: {error}");
                    }
                }
            }
            if !crate::worker::pause(&shutdown, crate::worker::WAKEUPS_RECONNECT, None).await {
                return;
            }
        }
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.waiters.lock().unwrap().keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_wait_ends_when_woken_and_a_wake_sent_first_is_kept() {
        let wakeups = Wakeups::default();
        let waiting = wakeups.clone();
        let task =
            tokio::spawn(async move { waiting.wait("call:1", Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        wakeups.wake("call:1");
        assert!(task.await.unwrap());
        // Sent before anyone waits: the next wait returns at once.
        wakeups.wake("call:2");
        assert!(wakeups.wait("call:2", Duration::from_millis(10)).await);
        // Nothing to wake: the wait ends at its limit, and leaves nothing behind.
        assert!(!wakeups.wait("call:3", Duration::from_millis(10)).await);
        assert_eq!(wakeups.waiting(), 0);
        // Another key's wake is not this one's.
        wakeups.wake("call:4");
        assert!(!wakeups.wait("call:5", Duration::from_millis(10)).await);
    }

    #[tokio::test(start_paused = true)]
    async fn wakes_for_waits_on_other_replicas_do_not_pile_up() {
        let wakeups = Wakeups::default();
        for call in 0..1000 {
            wakeups.wake(&format!("call:{call}"));
        }
        assert_eq!(wakeups.waiting(), 1000);
        // Someone waiting through the sweep keeps their place.
        let waiting = wakeups.clone();
        let task = tokio::spawn(async move { waiting.wait("held", KEPT * 3).await });
        tokio::time::sleep(KEPT + Duration::from_secs(1)).await;
        wakeups.wake("late");
        assert_eq!(wakeups.waiting(), 2);
        wakeups.wake("held");
        assert!(task.await.unwrap());
    }
}
