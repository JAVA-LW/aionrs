use std::future::Future;
use std::time::Duration;

use super::ProviderError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryAttempt {
    pub attempt: u32,
    pub max_retries: u32,
    pub delay: Duration,
    pub error: String,
}

/// Retry a fallible async operation with exponential backoff
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    with_retry_if(max_retries, |error| error.is_retryable(), f).await
}

/// Retry a fallible async operation when `should_retry` accepts the error.
pub async fn with_retry_if<F, Fut, T, P>(
    max_retries: u32,
    should_retry: P,
    f: F,
) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
    P: Fn(&ProviderError) -> bool,
{
    with_retry_if_notify(max_retries, should_retry, |_| {}, f).await
}

/// Retry a fallible async operation and notify before each backoff sleep.
pub async fn with_retry_if_notify<F, Fut, T, P, N>(
    max_retries: u32,
    should_retry: P,
    on_retry: N,
    f: F,
) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
    P: Fn(&ProviderError) -> bool,
    N: Fn(RetryAttempt),
{
    let mut backoff = Duration::from_secs(1);
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if should_retry(&e) && attempt < max_retries => {
                eprintln!("[retry] attempt {}/{}: {}", attempt + 1, max_retries, e);
                let delay = retry_delay(&e, backoff);
                on_retry(RetryAttempt {
                    attempt: attempt + 1,
                    max_retries,
                    delay,
                    error: e.to_string(),
                });
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn retry_delay(error: &ProviderError, fallback: Duration) -> Duration {
    match error {
        ProviderError::RateLimited { retry_after_ms } => {
            Duration::from_millis(*retry_after_ms).min(Duration::from_secs(30))
        }
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::ProviderError;

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let result = with_retry(2, || async { Ok::<_, ProviderError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        // Pause tokio time so sleep calls return immediately
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(ProviderError::Connection("timeout".into()))
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        tokio::time::pause();

        let result = with_retry(2, || async {
            Err::<(), _>(ProviderError::Connection("always fails".into()))
        })
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Connection(_)));
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error_fails_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::Api {
                    status: 401,
                    message: "unauthorized".into(),
                })
            }
        })
        .await;

        // Non-retryable errors should fail immediately without retrying
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_retries_transient_api_errors() {
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err(ProviderError::Api {
                        status: 503,
                        message: "service unavailable".into(),
                    })
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_notifies_before_backoff() {
        tokio::time::pause();

        let attempts = Arc::new(AtomicU32::new(0));
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let notifications_for_callback = Arc::clone(&notifications);

        let result = with_retry_if_notify(
            2,
            |error| error.is_retryable(),
            |retry| {
                notifications_for_callback.lock().unwrap().push(retry);
            },
            || {
                let attempts = Arc::clone(&attempts);
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Err(ProviderError::RateLimited {
                            retry_after_ms: 5000,
                        })
                    } else {
                        Ok(attempt)
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        let notifications = notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].attempt, 1);
        assert_eq!(notifications[0].max_retries, 2);
        assert_eq!(notifications[0].delay.as_millis(), 5000);
        assert_eq!(notifications[0].error, "Rate limited, retry after 5000ms");
    }
}
