use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::error::ApiError;

#[derive(Clone, Default)]
pub struct RateLimiter {
    entries: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl RateLimiter {
    pub async fn check(&self, key: String, limit: usize, window: Duration) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        let attempts = entries.entry(key).or_default();
        attempts.retain(|instant| now.duration_since(*instant) < window);
        if attempts.len() >= limit {
            return Err(ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "操作过于频繁，请稍后重试",
            ));
        }
        attempts.push(now);
        Ok(())
    }
}
