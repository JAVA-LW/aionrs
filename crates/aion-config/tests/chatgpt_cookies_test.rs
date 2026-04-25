use aion_config::chatgpt_cookies::ChatGptCloudflareCookieStore;
use reqwest::cookie::CookieStore;
use reqwest::header::HeaderValue;
use url::Url;

#[test]
fn cloudflare_store_keeps_only_chatgpt_cloudflare_cookies() {
    let store = ChatGptCloudflareCookieStore::default();
    let url = Url::parse("https://chatgpt.com/backend-api/codex/responses").unwrap();
    let set_cookies = [
        HeaderValue::from_static("__cf_bm=cf-bm; Path=/; Secure; HttpOnly"),
        HeaderValue::from_static("_cfuvid=cfuvid; Path=/; Secure; HttpOnly"),
        HeaderValue::from_static("cf_chl_rc_m=challenge; Path=/; Secure; HttpOnly"),
        HeaderValue::from_static("__Secure-next-auth.session-token=session; Path=/; Secure"),
        HeaderValue::from_static("oai-auth-token=auth; Path=/; Secure"),
    ];

    let mut iter = set_cookies.iter();
    store.set_cookies(&mut iter, &url);

    let cookie_header = store.cookies(&url).unwrap();
    let cookie_header = cookie_header.to_str().unwrap();
    assert!(cookie_header.contains("__cf_bm=cf-bm"));
    assert!(cookie_header.contains("_cfuvid=cfuvid"));
    assert!(cookie_header.contains("cf_chl_rc_m=challenge"));
    assert!(!cookie_header.contains("__Secure-next-auth.session-token"));
    assert!(!cookie_header.contains("oai-auth-token"));
}

#[test]
fn cloudflare_store_rejects_non_chatgpt_or_non_https_urls() {
    let store = ChatGptCloudflareCookieStore::default();
    let chatgpt_http = Url::parse("http://chatgpt.com/backend-api/codex/responses").unwrap();
    let other_https = Url::parse("https://example.com/backend-api/codex/responses").unwrap();
    let set_cookies = [HeaderValue::from_static(
        "__cf_bm=cf-bm; Path=/; Secure; HttpOnly",
    )];

    let mut iter = set_cookies.iter();
    store.set_cookies(&mut iter, &chatgpt_http);
    assert!(store.cookies(&chatgpt_http).is_none());

    let mut iter = set_cookies.iter();
    store.set_cookies(&mut iter, &other_https);
    assert!(store.cookies(&other_https).is_none());
}
