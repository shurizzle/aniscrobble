//! Client-side token bucket. AniList allows 90 requests per minute and may
//! still count a request that was cancelled mid-flight, so moving traffic is
//! smoothed before it hits the socket.

use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

pub struct RateLimiter {
    state: Mutex<State>,
    capacity: f64,
    refill_interval: Duration,
}

struct State {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    /// `capacity` is the burst size, `refill_interval` the time to regain one
    /// token.
    pub fn new(capacity: f64, refill_interval: Duration) -> Self {
        Self {
            state: Mutex::new(State {
                tokens: capacity,
                last: Instant::now(),
            }),
            capacity,
            refill_interval,
        }
    }

    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self.state.lock().await;
                let per_second = 1.0 / self.refill_interval.as_secs_f64();
                let now = Instant::now();
                let elapsed = now.duration_since(state.last).as_secs_f64();
                state.last = now;
                state.tokens = (state.tokens + elapsed * per_second).min(self.capacity);

                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    None
                } else {
                    Some(Duration::from_secs_f64((1.0 - state.tokens) / per_second))
                }
            };

            match wait {
                None => return,
                Some(delay) => tokio::time::sleep(delay).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::RateLimiter;

    #[tokio::test(start_paused = true)]
    async fn waits_for_a_token_once_the_burst_is_used() {
        let limiter = RateLimiter::new(2.0, Duration::from_millis(600));
        let start = Instant::now();

        limiter.acquire().await;
        limiter.acquire().await;
        assert!(start.elapsed() < Duration::from_millis(10));

        limiter.acquire().await;
        assert!(start.elapsed() >= Duration::from_millis(600));
    }
}
