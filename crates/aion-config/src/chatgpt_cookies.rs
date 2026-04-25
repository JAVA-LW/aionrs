use std::sync::{Arc, LazyLock};

use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::HeaderValue;
use url::Url;

static CHATGPT_CLOUDFLARE_COOKIE_STORE: LazyLock<Arc<ChatGptCloudflareCookieStore>> =
    LazyLock::new(|| Arc::new(ChatGptCloudflareCookieStore::default()));

#[derive(Debug, Default)]
pub struct ChatGptCloudflareCookieStore {
    inner: Jar,
}

impl CookieStore for ChatGptCloudflareCookieStore {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        if !is_chatgpt_https_url(url) {
            return;
        }

        let accepted = cookie_headers
            .filter(|header| {
                header
                    .to_str()
                    .ok()
                    .and_then(set_cookie_name)
                    .is_some_and(is_cloudflare_cookie_name)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut accepted = accepted.iter();
        self.inner.set_cookies(&mut accepted, url);
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        if !is_chatgpt_https_url(url) {
            return None;
        }

        let cookies = self.inner.cookies(url)?;
        let cookies = cookies.to_str().ok()?;
        let filtered = cookies
            .split(';')
            .map(str::trim)
            .filter(|cookie| {
                cookie
                    .split_once('=')
                    .map(|(name, _)| is_cloudflare_cookie_name(name.trim()))
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        if filtered.is_empty() {
            return None;
        }

        HeaderValue::from_str(&filtered.join("; ")).ok()
    }
}

pub fn chatgpt_cloudflare_cookie_store() -> Arc<ChatGptCloudflareCookieStore> {
    Arc::clone(&CHATGPT_CLOUDFLARE_COOKIE_STORE)
}

fn set_cookie_name(header: &str) -> Option<&str> {
    header
        .split_once(';')
        .map(|(first, _)| first)
        .unwrap_or(header)
        .split_once('=')
        .map(|(name, _)| name.trim())
        .filter(|name| !name.is_empty())
}

fn is_cloudflare_cookie_name(name: &str) -> bool {
    matches!(
        name,
        "__cf_bm" | "_cfuvid" | "cf_clearance" | "__cflb" | "cf_ob_info" | "cf_use_ob"
    ) || name.starts_with("cf_chl_")
}

fn is_chatgpt_https_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }

    let Some(host) = url.host_str() else {
        return false;
    };

    matches!(
        host,
        "chatgpt.com" | "chat.openai.com" | "chatgpt-staging.com"
    ) || host.ends_with(".chatgpt.com")
        || host.ends_with(".chatgpt-staging.com")
}
