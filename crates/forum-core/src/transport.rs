//! 平台无关的网络抽象与论坛客户端。
//!
//! 关键设计：本 crate **不依赖任何 HTTP 库**。原生端可实现 [`Transport`] 走
//! reqwest，Web 端用 NSH 提供的 `fetch`（官方文档明确要求跨平台代码使用
//! NSH 的网络栈，而不是标准库 `net/http` / `reqwest`）。
//! Cookie 会话由本 crate 维护，因此登录态在六端行为一致。

use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, Endpoints, LOGIN_COOKIE};
use crate::encoding::decode_body;
use crate::model::{ForumIndex, Session};
use crate::parse::{parse_document, parse_forum_index};
use crate::{Error, Result, CONTENT_TYPE_FORM};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Method {
    #[default]
    Get,
    Post,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub method: Method,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// 发帖成功后 Discuz 会 302 到 `viewthread.php`，判定成功必须看它。
    pub final_url: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 平台方实现的网络传输。`async fn` 让 Web 端可以直接 await NSH 的 `fetch`。
/// 本 trait 只作泛型约束、不用 `dyn`，故无需为 Future 附加 auto trait 边界。
#[allow(async_fn_in_trait)]
pub trait Transport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse>;
}

/// 解码后的统一响应，屏蔽 GBK / UTF-8 差异。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub body: String,
    pub status: u16,
    pub final_url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct State {
    cookies: Vec<(String, String)>,
    username: String,
    uid: String,
}

/// 有登录态的论坛客户端。
pub struct ForumClient<T: Transport> {
    pub(crate) cfg: Config,
    pub(crate) ep: Endpoints,
    pub transport: T,
    pub(crate) image_host: RwLock<String>,
    state: Mutex<State>,
    pub(crate) get_retry: usize,
}

impl<T: Transport> ForumClient<T> {
    pub fn new(cfg: Config, transport: T) -> Self {
        let cfg = cfg.with_defaults();
        let ep = Endpoints::new(&cfg);
        let image_host = cfg.image_host.clone();
        Self {
            cfg,
            ep,
            transport,
            image_host: RwLock::new(image_host),
            state: Mutex::new(State::default()),
            // 对应 OkHttpHelper 的 RetryIntercepter；POST 不重试，避免重复发帖。
            get_retry: 3,
        }
    }

    /// 当前配置（含运行时刷新的 CDN 域名）。
    pub fn config(&self) -> Config {
        let mut c = self.cfg.clone();
        if let Ok(host) = self.image_host.read() {
            c.image_host = host.clone();
        }
        c
    }

    pub fn endpoints(&self) -> &Endpoints { &self.ep }

    /// 把页面里的相对 href 拼成绝对 URL。
    pub fn forum_url(&self, reference: &str) -> String {
        if reference.starts_with("http://") || reference.starts_with("https://") {
            return reference.to_string();
        }
        let base = self.cfg.base_url();
        if let Some(rest) = reference.strip_prefix('/') {
            return format!("{}{rest}", self.cfg.forum_server);
        }
        format!("{base}{reference}")
    }

    pub fn set_identity(&self, username: &str, uid: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.username = username.to_string();
            s.uid = uid.to_string();
        }
    }

    pub fn identity(&self) -> (String, String) {
        self.state
            .lock()
            .map(|s| (s.username.clone(), s.uid.clone()))
            .unwrap_or_default()
    }

    /// 用 `cdb_auth` cookie 判定登录态（`OkHttpHelper.hasAuthCookie`）。
    pub fn is_logged_in(&self) -> bool {
        self.state.lock().map(|s| {
            s.cookies.iter().any(|(k, v)| k == LOGIN_COOKIE && !v.is_empty())
        }).unwrap_or(false)
    }

    /// 发起 GET 并解码响应。仅网络层错误重试，HTTP 4xx/5xx 不重试。
    pub async fn get(&self, url: &str) -> Result<Response> {
        let mut last = Error::Http(String::new());
        for _ in 0..self.get_retry.max(1) {
            let req = self.build_request(Method::Get, url, Vec::new());
            match self.send(req).await {
                Ok(resp) => return Ok(resp),
                Err(e @ Error::Http(_)) => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Http(format!("GET {url} 失败（重试 {} 次）：{last}", self.get_retry)))
    }

    /// 提交表单。表单值按站点编码（GBK）做百分号编码。
    pub async fn post_form(&self, url: &str, pairs: &[(String, String)]) -> Result<Response> {
        let body = crate::encoding::encode_form(pairs, &self.cfg.encode);
        let req = self.build_request(Method::Post, url, body.into_bytes());
        self.send(req).await
    }

    /// 提交 multipart（图片上传）。
    pub async fn post_multipart(
        &self,
        url: &str,
        boundary: &str,
        body: &[u8],
    ) -> Result<Response> {
        let mut req = self.build_request(Method::Post, url, body.to_vec());
        req.headers.push((
            "Content-Type".into(),
            format!("multipart/form-data; boundary={boundary}"),
        ));
        self.send(req).await
    }

    fn build_request(&self, method: Method, url: &str, body: Vec<u8>) -> HttpRequest {
        let mut headers = vec![
            ("User-Agent".into(), self.cfg.user_agent.clone()),
            ("Accept".into(), "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into()),
            ("Accept-Language".into(), "zh-CN,zh;q=0.9".into()),
        ];
        if method == Method::Post {
            headers.push(("Content-Type".into(), format!("{CONTENT_TYPE_FORM}; charset={}", self.cfg.encode)));
            headers.push(("Referer".into(), self.cfg.base_url()));
        }
        let cookie = self.state.lock().map(|s| {
            s.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
        }).unwrap_or_default();
        if !cookie.is_empty() {
            headers.push(("Cookie".into(), cookie));
        }
        HttpRequest { url: url.to_string(), method, headers, body }
    }

    async fn send(&self, mut req: HttpRequest) -> Result<Response> {
        // multipart 必须覆盖默认的 urlencoded Content-Type：同名头以后者为准，
        // 这里移除先写入的那个，避免平台层取到第一个。
        if req.header("Content-Type").is_some_and(|v| v.starts_with("multipart/")) {
            let mut seen = false;
            req.headers.retain(|(k, v)| {
                if k.eq_ignore_ascii_case("Content-Type") && v.starts_with(CONTENT_TYPE_FORM) && !seen {
                    seen = true;
                    false
                } else {
                    true
                }
            });
        }
        let resp = self.transport.send(req).await?;
        self.absorb_cookies(&resp);
        let content_type = resp.header("content-type").unwrap_or("");
        let body = decode_body(&resp.body, content_type, &self.cfg.encode)?;
        Ok(Response {
            body,
            status: resp.status,
            final_url: if resp.final_url.is_empty() { String::new() } else { resp.final_url },
            headers: resp.headers,
        })
    }

    fn absorb_cookies(&self, resp: &HttpResponse) {
        let mut updates = Vec::new();
        for (k, v) in &resp.headers {
            if !k.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            let first = v.split(';').next().unwrap_or("").trim();
            if let Some((name, value)) = first.split_once('=') {
                if !name.is_empty() {
                    updates.push((name.to_string(), value.to_string()));
                }
            }
        }
        if updates.is_empty() {
            return;
        }
        if let Ok(mut s) = self.state.lock() {
            for (name, value) in updates {
                if let Some(existing) = s.cookies.iter_mut().find(|(k, _)| *k == name) {
                    existing.1 = value;
                } else {
                    s.cookies.push((name, value));
                }
            }
        }
    }

    fn has_cookie(&self, name: &str) -> bool {
        self.state
            .lock()
            .map(|s| s.cookies.iter().any(|(k, v)| k == name && !v.is_empty()))
            .unwrap_or(false)
    }

    /// 拉取论坛首页 `index.php`（分区 + 版块索引），**不需要登录**。
    ///
    /// 站点前置了 WAF：不带 `cdb_sid` 直接 GET `index.php` 会拿到 403
    /// Cloudflare 挑战页（2026-11-13 实测）；先 GET 一次
    /// `logging.php?action=login` 让 Discuz 下发 `cdb_sid`（内核自动吸收
    /// `Set-Cookie`），再取首页即可拿到 GBK 的版块索引。GET 幂等，失败时
    /// 重新预热再试，最多三轮；仍失败由 UI 用本地版块表兜底。
    pub async fn forum_index(&self) -> Result<ForumIndex> {
        let mut last = Error::Http(String::new());
        for attempt in 0..3 {
            if attempt > 0 || !self.has_cookie("cdb_sid") {
                // 断网时不必再空转：直接返回让 UI 用本地版块表兜底。
                self.get(&self.ep.login_get_form_hash).await?;
            }
            match self.get(&self.ep.forum_index).await {
                Ok(resp) if resp.status == 200 => {
                    let index = parse_forum_index(&parse_document(&resp.body));
                    if !index.is_empty() {
                        return Ok(index);
                    }
                    last = Error::Protocol("论坛首页未解析出版块，可能被 WAF 拦截或站点改版".into());
                }
                Ok(resp) => last = Error::Http(format!("论坛首页返回 HTTP {}", resp.status)),
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// 从 `/config.php` 读取 CDN 地址并刷新图片域名。
    /// 2026-11-13 实测响应：`{"CDN": "https://img02.4d4y.com"}`。
    pub async fn refresh_cdn(&self) -> Result<String> {
        let resp = self.get(&self.cfg.config_url()).await?;
        let json: serde_json::Value = serde_json::from_str(&resp.body)
            .map_err(|e| Error::Protocol(format!("解析 /config.php 失败：{e}")))?;
        let cdn = json.get("CDN").and_then(|v| v.as_str()).unwrap_or("").trim_end_matches('/');
        if cdn.is_empty() {
            return Ok(self.config().image_host);
        }
        if let Ok(mut host) = self.image_host.write() {
            *host = cdn.to_string();
        }
        Ok(cdn.to_string())
    }

    /// 会话快照 JSON（不含明文密码），供平台层写入自己的 KV 存储。
    ///
    /// 六端通用：NSH `Storage` 在原生端落文件、Web 端落到 `localStorage`。
    /// 本方法只产出字符串，不碰文件系统，因此 `wasm32` 上同样可用。
    pub fn session_snapshot(&self) -> Result<String> {
        let (username, uid, cookies) = self
            .state
            .lock()
            .map(|s| (s.username.clone(), s.uid.clone(), s.cookies.clone()))
            .unwrap_or_default();
        let session = Session {
            username,
            uid,
            cookies,
            saved_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };
        serde_json::to_string(&session).map_err(|e| Error::Io(e.to_string()))
    }

    /// 从快照 JSON 恢复登录态；空串或缺少有效 `cdb_auth` 返回 `Ok(false)`。
    ///
    /// 只接受带登录 cookie 的快照，避免把「退出登录后的空状态」当成会话，
    /// 从而不会在启动时发一次注定失败的请求。
    pub fn restore_session(&self, raw: &str) -> Result<bool> {
        if raw.trim().is_empty() {
            return Ok(false);
        }
        let session: Session =
            serde_json::from_str(raw).map_err(|e| Error::Io(e.to_string()))?;
        if !session
            .cookies
            .iter()
            .any(|(k, v)| k == LOGIN_COOKIE && !v.is_empty())
        {
            return Ok(false);
        }
        if let Ok(mut s) = self.state.lock() {
            s.cookies = session.cookies;
            s.username = session.username;
            s.uid = session.uid;
        }
        Ok(self.is_logged_in())
    }

    /// 会话快照落盘（桌面端）。文件权限 0600，且不含明文密码。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_session(&self, path: &std::path::Path) -> Result<()> {
        use std::io::Write;
        let json = self.session_snapshot()?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(path).map_err(|e| Error::Io(e.to_string()))?;
        file.write_all(json.as_bytes())
            .map_err(|e| Error::Io(e.to_string()))
    }

    /// 从快照文件恢复登录态；文件不存在返回 `Ok(false)`。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_session(&self, path: &std::path::Path) -> Result<bool> {
        let raw = match std::fs::read_to_string(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(Error::Io(e.to_string())),
        };
        self.restore_session(&raw)
    }

    pub fn logout(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.cookies.clear();
            s.username.clear();
            s.uid.clear();
        }
    }
}
