//! When Chat's background workers run, in one place, and the timing and shutdown
//! they share. How replicas share each worker's work (an advisory lock, claimed
//! rows, a stamp) stays with the worker.

use crate::connections::wakeups::Wakeups;
use std::{future::Future, time::Duration};
use tokio::time::{interval_at, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// Temporary chats that were left or went quiet are deleted (retention.rs).
pub const TEMPORARY_CHATS: Duration = Duration::from_secs(60);
/// Retention: expired chats, files, sign-in records and tokens (retention.rs).
pub const RETENTION: Duration = Duration::from_secs(60 * 60);
/// Memory embeds what waits and reads chats that went quiet (memory.rs).
pub const MEMORY: Duration = Duration::from_secs(30);
/// Insights label chats and place workstreams (insights.rs).
pub const INSIGHTS: Duration = Duration::from_secs(60);
/// The project log notes days with work and writes the ones that are due
/// (project_log/write.rs).
pub const PROJECT_LOG: Duration = Duration::from_secs(60);
/// Notebook guides are made for new documents in projects, and notebook items a
/// stopped replica left running are taken up (notebook/).
pub const NOTEBOOK: Duration = Duration::from_secs(20);
/// Auto mode's watchdog nudges stopped turns (autopilot.rs).
pub const AUTO_MODE: Duration = Duration::from_secs(60);
/// Running research experiments are looked at; each run's own pace is in
/// research/mod.rs.
pub const RESEARCH: Duration = Duration::from_secs(5);
/// Workspace pods are reconciled with Kubernetes (sandbox/mod.rs).
pub const WORKSPACES: Duration = Duration::from_secs(60);
/// Browser pods are reconciled with Kubernetes (browser/mod.rs).
pub const BROWSERS: Duration = Duration::from_secs(60);
/// Provider grants are refreshed and ended sign-ins revoked (auth/grants.rs).
pub const GRANTS: Duration = Duration::from_secs(30);
/// Connection tokens move to the secret key (connections/mod.rs).
pub const RESEAL: Duration = Duration::from_secs(5 * 60);
/// Knowledge looks for queued work without a wake-up (knowledge/mod.rs).
pub const KNOWLEDGE_IDLE: Duration = Duration::from_secs(30);
/// Files with nothing to move wait this long at first, doubling up to
/// FILES_IDLE_MAX (files.rs).
pub const FILES_IDLE: Duration = Duration::from_secs(30);
pub const FILES_IDLE_MAX: Duration = Duration::from_secs(5 * 60);
/// A lost wake-up listener connects again (connections/wakeups.rs).
pub const WAKEUPS_RECONNECT: Duration = Duration::from_secs(5);

/// When a worker's first pass runs.
#[derive(Clone, Copy, Debug)]
pub enum Start {
    /// At startup.
    Now,
    /// One period after startup.
    AfterPeriod,
}

/// Runs `pass` every `period` until shutdown. A pass under way finishes first, as a
/// pass may have done something outside Chat it has yet to record, such as a provider
/// handing out a new refresh token. A pass that overruns skips the ticks it missed
/// rather than running again at once.
pub async fn every<F, Fut>(period: Duration, start: Start, shutdown: CancellationToken, mut pass: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let first = match start {
        Start::Now => Instant::now(),
        Start::AfterPeriod => Instant::now() + period,
    };
    let mut timer = interval_at(first, period);
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            _ = timer.tick() => {}
        }
        pass().await;
    }
}

/// Waits `wait`, or less when `wake` names a key that is woken. False at shutdown.
pub async fn pause(
    shutdown: &CancellationToken,
    wait: Duration,
    wake: Option<(&Wakeups, &str)>,
) -> bool {
    let waited = async {
        match wake {
            Some((wakeups, key)) => {
                wakeups.wait(key, wait).await;
            }
            None => tokio::time::sleep(wait).await,
        }
    };
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => false,
        _ = waited => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    const PERIOD: Duration = Duration::from_secs(10);

    /// Runs `every` with passes that take `takes`, counting them.
    fn counted(start: Start, takes: Duration) -> (Arc<AtomicU32>, CancellationToken) {
        let passes = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let counter = passes.clone();
        tokio::spawn(every(PERIOD, start, shutdown.clone(), move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(takes).await;
            }
        }));
        (passes, shutdown)
    }

    async fn advance(by: Duration) {
        tokio::time::sleep(by).await;
    }

    #[tokio::test(start_paused = true)]
    async fn runs_first_or_waits_a_period() {
        let (now, _a) = counted(Start::Now, Duration::ZERO);
        let (later, _b) = counted(Start::AfterPeriod, Duration::ZERO);
        advance(Duration::from_secs(1)).await;
        assert_eq!(now.load(Ordering::SeqCst), 1);
        assert_eq!(later.load(Ordering::SeqCst), 0);
        advance(PERIOD).await;
        assert_eq!(now.load(Ordering::SeqCst), 2);
        assert_eq!(later.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pass_that_overruns_skips_the_ticks_it_missed() {
        // Each pass takes two and a half periods: passes start at 0, 30 and 60 s.
        let (passes, _shutdown) = counted(Start::Now, PERIOD * 5 / 2);
        advance(PERIOD * 6 + Duration::from_secs(1)).await;
        assert_eq!(passes.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_lets_a_pass_finish_and_ends_the_wait_at_once() {
        let shutdown = CancellationToken::new();
        let finished = Arc::new(AtomicU32::new(0));
        let done = finished.clone();
        let task = tokio::spawn(every(PERIOD, Start::Now, shutdown.clone(), move || {
            let done = done.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                done.fetch_add(1, Ordering::SeqCst);
            }
        }));
        advance(Duration::from_secs(1)).await;
        shutdown.cancel();
        let began = Instant::now();
        tokio::time::timeout(PERIOD, task)
            .await
            .expect("the worker stops once its pass is done")
            .unwrap();
        assert_eq!(finished.load(Ordering::SeqCst), 1);
        assert!(
            began.elapsed() < Duration::from_secs(3),
            "{:?}",
            began.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_ends_early_when_woken_and_at_shutdown() {
        let shutdown = CancellationToken::new();
        let wakeups = Wakeups::default();
        let waiting = {
            let (shutdown, wakeups) = (shutdown.clone(), wakeups.clone());
            tokio::spawn(async move {
                let began = Instant::now();
                let went_on = pause(&shutdown, PERIOD, Some((&wakeups, "queue"))).await;
                (went_on, began.elapsed())
            })
        };
        advance(Duration::from_secs(1)).await;
        wakeups.wake("queue");
        let (went_on, took) = waiting.await.unwrap();
        assert!(went_on && took < PERIOD, "{took:?}");
        assert!(pause(&shutdown, PERIOD, None).await);
        shutdown.cancel();
        assert!(!pause(&shutdown, PERIOD, None).await);
    }
}
