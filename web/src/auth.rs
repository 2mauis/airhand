//! 可选的访问密码（`AIRHAND_WEB_PASSWORD`）。
//!
//! 用浏览器自带的 HTTP Basic 登录框（用户名随意），不需要单独的登录页。登录成功后再发一个 Cookie：
//! 并不是所有浏览器都会在 WebSocket 握手时带上 Basic 凭据，Cookie 则一定会带。
//! Cookie 的值由密码推导而来，所以服务重启后不用重新登录；改密码后旧的 Cookie 立即失效。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::http::{App, Conn};

const COOKIE_NAME: &str = "airhand_auth";

pub struct Auth {
    password: String,
    token: String,
}

impl Auth {
    pub fn new(password: String) -> Self {
        let token = hex(&Sha256::digest(
            [b"airhand-web-session\0".as_slice(), password.as_bytes()].concat(),
        ));
        Self { password, token }
    }

    /// 请求带着有效的 Cookie
    fn cookie_ok(&self, headers: &HeaderMap) -> bool {
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|kv| kv.trim().strip_prefix(COOKIE_NAME)?.strip_prefix('='))
            .any(|v| same(v, &self.token))
    }

    /// `None`：没有提供密码；`Some(ok)`：提供了，是否正确
    fn basic(&self, headers: &HeaderMap) -> Option<bool> {
        let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
        let (scheme, encoded) = value.trim().split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        let decoded = String::from_utf8_lossy(&decoded);
        // 用户名随意，只看密码
        let password = decoded.split_once(':').map_or("", |(_, p)| p);
        Some(same(password, &self.password))
    }

    fn set_cookie(&self, secure: bool) -> HeaderValue {
        let secure = if secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Strict{secure}",
            self.token
        ))
        .expect("valid cookie")
    }
}

/// 中间件：没有设置密码时直接放行
pub async fn check(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let Some(auth) = &app.auth else {
        return next.run(req).await;
    };
    if auth.cookie_ok(req.headers()) {
        return next.run(req).await;
    }
    match auth.basic(req.headers()) {
        Some(true) => {
            let secure = req.extensions().get::<Conn>().is_some_and(|c| c.tls);
            let mut res = next.run(req).await;
            res.headers_mut()
                .append(SET_COOKIE, auth.set_cookie(secure));
            res
        }
        Some(false) => {
            // 密码错误：放慢一点，让暴力猜测更难
            tokio::time::sleep(Duration::from_millis(500)).await;
            unauthorized()
        }
        None => unauthorized(),
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            WWW_AUTHENTICATE,
            HeaderValue::from_static(r#"Basic realm="Airhand", charset="UTF-8""#),
        )],
        "Airhand: password required",
    )
        .into_response()
}

/// 比较时间与内容无关（先各自取哈希，再逐字节比较）
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (Sha256::digest(a.as_bytes()), Sha256::digest(b.as_bytes()));
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(axum::http::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn basic_and_cookie() {
        let auth = Auth::new("s3cret:x".into());
        let basic = |user_pass: &str| {
            let enc = base64::engine::general_purpose::STANDARD.encode(user_pass);
            auth.basic(&headers(&[(AUTHORIZATION, &format!("Basic {enc}"))]))
        };
        assert_eq!(basic("anyone:s3cret:x"), Some(true));
        assert_eq!(basic(":s3cret:x"), Some(true));
        assert_eq!(basic("anyone:wrong"), Some(false));
        assert_eq!(basic("no-colon"), Some(false));
        assert_eq!(auth.basic(&headers(&[(AUTHORIZATION, "Bearer x")])), None);
        assert_eq!(auth.basic(&HeaderMap::new()), None);

        let cookie = auth.set_cookie(true);
        let cookie = cookie.to_str().unwrap();
        assert!(cookie.ends_with("; Secure"), "{cookie}");
        let pair = cookie.split(';').next().unwrap();
        assert!(auth.cookie_ok(&headers(&[(COOKIE, &format!("a=b; {pair}; c=d"))])));
        assert!(!auth.cookie_ok(&headers(&[(COOKIE, "airhand_auth=nope")])));
        assert!(!auth.cookie_ok(&HeaderMap::new()));
        // 换了密码，旧 Cookie 失效
        assert!(!Auth::new("other".into()).cookie_ok(&headers(&[(COOKIE, pair)])));
    }
}
