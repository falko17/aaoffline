//! Contains middleware for the [reqwest] client.

use std::error::Error;
use std::fmt::{Debug, Display};
use std::str::FromStr;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
#[cfg(not(target_arch = "wasm32"))]
use http::Extensions;
#[cfg(not(target_arch = "wasm32"))]
use log::{debug, warn};
use reqwest::Url;
#[cfg(not(target_arch = "wasm32"))]
use reqwest::{Request, Response, StatusCode, header::RETRY_AFTER};
use reqwest_middleware::RequestInitialiser;
#[cfg(not(target_arch = "wasm32"))]
use reqwest_middleware::{Middleware, Next};
#[cfg(not(target_arch = "wasm32"))]
use reqwest_retry::{
    Retryable, RetryableStrategy, default_on_request_failure, default_on_request_success,
};

use crate::args::Args;

/// Asks the user to solve a bot challenge (a captcha) from Ace Attorney Online.
///
/// Ace Attorney Online sends clients with too many requests to a challenge page.
/// When the user solves the challenge in a browser, the site accepts requests from their IP again.
#[async_trait]
pub trait ChallengeHandler: Debug + Send + Sync {
    /// Asks the user to solve the challenge at the given [url] in a browser.
    ///
    /// Returns `true` if the user solved the challenge and aaoffline must retry the request.
    async fn solve(&self, url: &Url) -> bool;
}

/// Error for a request that Ace Attorney Online redirected to a bot challenge.
#[derive(Debug, Clone)]
pub struct ChallengeRequired {
    /// The URL of the challenge page.
    pub url: Url,
}

impl Display for ChallengeRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Ace Attorney Online wants to verify that you are human. Open {} in your browser, solve the captcha, then try again.",
            self.url
        )
    }
}

impl Error for ChallengeRequired {}

impl ChallengeRequired {
    /// Finds a [`ChallengeRequired`] in the cause chain of the given [err].
    #[must_use]
    pub fn find(err: &anyhow::Error) -> Option<&ChallengeRequired> {
        err.chain().find_map(|cause| {
            cause.downcast_ref::<ChallengeRequired>().or_else(|| {
                match cause.downcast_ref::<reqwest_middleware::Error>() {
                    Some(reqwest_middleware::Error::Middleware(inner)) => inner.downcast_ref(),
                    _ => None,
                }
            })
        })
    }
}

/// State that all requests of a [`ChallengeMiddleware`] share.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
struct ChallengeState {
    /// How often the user solved a challenge.
    solved: u64,
    /// Whether the user declined to solve a challenge.
    declined: bool,
}

/// A middleware that detects bot challenges from Ace Attorney Online.
///
/// On a challenge, it asks the [`ChallengeHandler`] and then retries the request.
/// Concurrent requests that run into the same challenge ask only once.
/// Without a handler, or if the user declines, the request fails with [`ChallengeRequired`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ChallengeMiddleware {
    handler: Option<Arc<dyn ChallengeHandler>>,
    state: tokio::sync::Mutex<ChallengeState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ChallengeMiddleware {
    pub(crate) fn new(handler: Option<Arc<dyn ChallengeHandler>>) -> Self {
        ChallengeMiddleware {
            handler,
            state: tokio::sync::Mutex::default(),
        }
    }

    /// Returns the challenge URL if the given [response] is a challenge page.
    fn challenge_url(response: &Response) -> Option<Url> {
        let url = response.url();
        (url.path() == "/challenge" && url.query_pairs().any(|(key, _)| key == "destination"))
            .then(|| url.clone())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl Middleware for ChallengeMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        loop {
            let solved_before = self.state.lock().await.solved;
            let Some(attempt) = req.try_clone() else {
                // We cannot retry requests with streamed bodies.
                return next.run(req, extensions).await;
            };
            let response = next.clone().run(attempt, extensions).await?;
            let Some(url) = Self::challenge_url(&response) else {
                return Ok(response);
            };
            debug!("Request to {} was redirected to challenge {url}", req.url());

            let mut state = self.state.lock().await;
            if state.declined {
                return Err(reqwest_middleware::Error::middleware(ChallengeRequired {
                    url,
                }));
            }
            if state.solved == solved_before {
                // No concurrent request solved the challenge in the meantime, so we ask.
                let Some(handler) = &self.handler else {
                    return Err(reqwest_middleware::Error::middleware(ChallengeRequired {
                        url,
                    }));
                };
                if handler.solve(&url).await {
                    state.solved += 1;
                } else {
                    state.declined = true;
                    return Err(reqwest_middleware::Error::middleware(ChallengeRequired {
                        url,
                    }));
                }
            }
        }
    }
}

/// A middleware that retries requests that failed with status 429 (too many requests).
///
/// It waits for the time in the `Retry-After` header. Without this header, it uses an exponential
/// backoff. It never waits longer than its maximum delay.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RateLimitMiddleware {
    max_retries: u32,
    base_delay: Duration,
    max_delay: Duration,
}

#[cfg(not(target_arch = "wasm32"))]
impl RateLimitMiddleware {
    pub(crate) fn new(max_retries: u32, base_delay: Duration, max_delay: Duration) -> Self {
        RateLimitMiddleware {
            max_retries,
            base_delay,
            max_delay,
        }
    }

    /// Returns how long to wait before the given [retry] (starting at zero) of [response].
    fn delay(&self, response: &Response, retry: u32) -> Duration {
        retry_after(response)
            .unwrap_or_else(|| self.base_delay.saturating_mul(2u32.saturating_pow(retry)))
            .min(self.max_delay)
    }
}

/// Returns the wait time from the `Retry-After` header of the given [response].
///
/// The header contains either a number of seconds or an HTTP date.
#[cfg(not(target_arch = "wasm32"))]
fn retry_after(response: &Response) -> Option<Duration> {
    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let target = Duration::from_secs(u64::try_from(date.timestamp()).ok()?);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(target.saturating_sub(now))
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl Middleware for RateLimitMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        let mut retry = 0;
        loop {
            let Some(attempt) = req.try_clone() else {
                // We cannot retry requests with streamed bodies.
                return next.run(req, extensions).await;
            };
            let response = next.clone().run(attempt, extensions).await?;
            if response.status() != StatusCode::TOO_MANY_REQUESTS || retry >= self.max_retries {
                return Ok(response);
            }
            let delay = self.delay(&response, retry);
            warn!(
                "Too many requests to {}, retrying in {} seconds.",
                response.url().host_str().unwrap_or("[UNKNOWN HOST]"),
                delay.as_secs_f32()
            );
            retry += 1;
            tokio::time::sleep(delay).await;
        }
    }
}

/// The default retry strategy, except that it leaves status 429 to [`RateLimitMiddleware`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct NoRateLimitRetryStrategy;

#[cfg(not(target_arch = "wasm32"))]
impl RetryableStrategy for NoRateLimitRetryStrategy {
    fn handle(&self, res: &reqwest_middleware::Result<Response>) -> Option<Retryable> {
        match res {
            Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS => {
                Some(Retryable::Fatal)
            }
            Ok(response) => default_on_request_success(response),
            Err(error) => default_on_request_failure(error),
        }
    }
}

/// A middleware that modifies outgoing HTTP requests from aaoffline.
pub(crate) struct AaofflineMiddleware {
    /// Whether photobucket watermarks should be automatically removed.
    fix_photobucket: bool,
    proxy: Option<String>,
}

impl From<&Args> for AaofflineMiddleware {
    fn from(args: &Args) -> Self {
        AaofflineMiddleware {
            fix_photobucket: !args.disable_photobucket_fix,
            proxy: args.proxy.clone(),
        }
    }
}

impl RequestInitialiser for AaofflineMiddleware {
    fn init(
        &self,
        mut req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        if let Some((client, mut request)) = req
            .try_clone()
            .map(|x| x.build_split())
            .and_then(|x| x.1.map(|y| (x.0, y)).ok())
        {
            if self.fix_photobucket
                && request
                    .url()
                    .host_str()
                    .is_some_and(|x| x.contains("photobucket.com"))
            {
                req = req.header("Referer", "https://photobucket.com/");
            }
            if let Some(proxy) = self.proxy.as_ref() {
                let url = request.url_mut();
                *url =
                    Url::from_str(&format!("{proxy}{}", url.as_str())).expect("invalid proxy URL");
                req = reqwest_middleware::RequestBuilder::from_parts(client, request)
            }
        }
        req
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use reqwest::Url;
    use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Middleware};
    use reqwest_retry::RetryTransientMiddleware;
    use reqwest_retry::policies::ExponentialBackoff;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        ChallengeHandler, ChallengeMiddleware, ChallengeRequired, NoRateLimitRetryStrategy,
        RateLimitMiddleware,
    };

    const SCRIPT: &str = "var trial_information;";

    /// Handler that counts how often it was asked and always gives the same [answer].
    #[derive(Debug)]
    struct CountingHandler {
        calls: AtomicUsize,
        answer: bool,
    }

    impl CountingHandler {
        fn new(answer: bool) -> Arc<CountingHandler> {
            Arc::new(CountingHandler {
                calls: AtomicUsize::new(0),
                answer,
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ChallengeHandler for CountingHandler {
        async fn solve(&self, url: &Url) -> bool {
            assert_eq!(url.path(), "/challenge");
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Give concurrent requests time to run into the challenge, too.
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.answer
        }
    }

    fn client(middleware: impl Middleware) -> ClientWithMiddleware {
        ClientBuilder::new(reqwest::Client::new())
            .with(middleware)
            .build()
    }

    fn challenge_client(handler: Option<Arc<CountingHandler>>) -> ClientWithMiddleware {
        client(ChallengeMiddleware::new(
            handler.map(|x| x as Arc<dyn ChallengeHandler>),
        ))
    }

    /// Starts a server that redirects the first [challenges] requests to the challenge page.
    async fn challenge_server(challenges: u64) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/trial.js.php"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "/challenge?destination=%2Ftrial.js.php"),
            )
            .up_to_n_times(challenges)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/trial.js.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SCRIPT))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<title>Verifying</title>"))
            .mount(&server)
            .await;
        server
    }

    fn challenge_of(result: reqwest_middleware::Result<reqwest::Response>) -> ChallengeRequired {
        let err = anyhow::Error::from(result.expect_err("expected challenge error"));
        ChallengeRequired::find(&err)
            .expect("error must contain ChallengeRequired")
            .clone()
    }

    #[tokio::test]
    async fn challenge_without_handler_fails() {
        let server = challenge_server(1).await;
        let url = format!("{}/trial.js.php", server.uri());

        let challenge = challenge_of(challenge_client(None).get(&url).send().await);

        assert_eq!(challenge.url.path(), "/challenge");
        assert_eq!(challenge.url.query(), Some("destination=%2Ftrial.js.php"));
    }

    #[tokio::test]
    async fn challenge_is_retried_after_solve() {
        let server = challenge_server(1).await;
        let handler = CountingHandler::new(true);
        let client = challenge_client(Some(Arc::clone(&handler)));

        let response = client
            .get(format!("{}/trial.js.php", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.text().await.unwrap(), SCRIPT);
        assert_eq!(handler.calls(), 1);
    }

    #[tokio::test]
    async fn declined_challenge_fails() {
        let server = challenge_server(1).await;
        let handler = CountingHandler::new(false);
        let client = challenge_client(Some(Arc::clone(&handler)));

        let result = client
            .get(format!("{}/trial.js.php", server.uri()))
            .send()
            .await;

        challenge_of(result);
        assert_eq!(handler.calls(), 1);
    }

    #[tokio::test]
    async fn concurrent_challenges_ask_once() {
        let server = challenge_server(2).await;
        let handler = CountingHandler::new(true);
        let client = challenge_client(Some(Arc::clone(&handler)));
        let url = format!("{}/trial.js.php", server.uri());

        let (first, second) = tokio::join!(client.get(&url).send(), client.get(&url).send());

        assert_eq!(first.unwrap().text().await.unwrap(), SCRIPT);
        assert_eq!(second.unwrap().text().await.unwrap(), SCRIPT);
        assert_eq!(handler.calls(), 1);
    }

    #[tokio::test]
    async fn concurrent_challenges_after_decline_ask_once() {
        let server = challenge_server(2).await;
        let handler = CountingHandler::new(false);
        let client = challenge_client(Some(Arc::clone(&handler)));
        let url = format!("{}/trial.js.php", server.uri());

        let (first, second) = tokio::join!(client.get(&url).send(), client.get(&url).send());

        challenge_of(first);
        challenge_of(second);
        assert_eq!(handler.calls(), 1);
    }

    #[tokio::test]
    async fn other_redirects_pass() {
        let server = MockServer::start().await;
        Mock::given(path("/old"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/new"))
            .mount(&server)
            .await;
        Mock::given(path("/new"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SCRIPT))
            .mount(&server)
            .await;
        let handler = CountingHandler::new(true);

        let response = challenge_client(Some(Arc::clone(&handler)))
            .get(format!("{}/old", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.text().await.unwrap(), SCRIPT);
        assert_eq!(handler.calls(), 0);
    }

    /// Starts a server that answers the first [limited] requests with 429.
    async fn rate_limited_server(limited: u64, retry_after: Option<&str>) -> MockServer {
        let server = MockServer::start().await;
        let mut too_many = ResponseTemplate::new(429);
        if let Some(retry_after) = retry_after {
            too_many = too_many.insert_header("Retry-After", retry_after);
        }
        Mock::given(path("/asset.png"))
            .respond_with(too_many)
            .up_to_n_times(limited)
            .mount(&server)
            .await;
        Mock::given(path("/asset.png"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        server
    }

    fn rate_limit_client(max_retries: u32, max_delay: Duration) -> ClientWithMiddleware {
        client(RateLimitMiddleware::new(
            max_retries,
            Duration::from_millis(10),
            max_delay,
        ))
    }

    #[tokio::test]
    async fn rate_limit_waits_for_retry_after_seconds() {
        let server = rate_limited_server(1, Some("1")).await;
        let start = Instant::now();

        let response = rate_limit_client(3, Duration::from_secs(5))
            .get(format!("{}/asset.png", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert!(start.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test]
    async fn rate_limit_waits_for_retry_after_date() {
        // An HTTP date in the past means "retry now".
        let server = rate_limited_server(1, Some("Sun, 06 Nov 1994 08:49:37 GMT")).await;

        let response = rate_limit_client(3, Duration::from_secs(5))
            .get(format!("{}/asset.png", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn rate_limit_backs_off_without_retry_after() {
        let server = rate_limited_server(2, None).await;

        let response = rate_limit_client(3, Duration::from_secs(5))
            .get(format!("{}/asset.png", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn rate_limit_caps_delay() {
        let server = rate_limited_server(1, Some("3600")).await;
        let start = Instant::now();

        let response = rate_limit_client(3, Duration::from_millis(50))
            .get(format!("{}/asset.png", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn rate_limit_gives_up_after_max_retries() {
        let server = rate_limited_server(u64::MAX, None).await;

        let response = rate_limit_client(2, Duration::from_secs(5))
            .get(format!("{}/asset.png", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 429);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn transient_retry_leaves_429_to_rate_limit() {
        let server = rate_limited_server(u64::MAX, None).await;
        let policy = ExponentialBackoff::builder()
            .retry_bounds(Duration::from_millis(1), Duration::from_millis(1))
            .build_with_max_retries(3);
        let client = ClientBuilder::new(reqwest::Client::new())
            .with(RetryTransientMiddleware::new_with_policy_and_strategy(
                policy,
                NoRateLimitRetryStrategy,
            ))
            .with(RateLimitMiddleware::new(
                1,
                Duration::from_millis(1),
                Duration::from_millis(1),
            ))
            .build();

        let response = client
            .get(format!("{}/asset.png", server.uri()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 429);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
