//! NSH `fetch` → `forum_core::Transport` 适配层。
//!
//! 官方 NSH 要求跨平台代码使用引擎的网络栈（原生端走平台栈、Web 端走浏览器
//! `fetch`），而不是 `reqwest` / 标准库。这一层把 `forum-core` 的平台无关请求
//! 翻译成 `nsh_core::platform::HttpRequest`，再把原始字节交还内核：
//!
//! - **不能用 `HttpResponse::text()`**：4D4Y 返回 GBK，`text()` 只做 UTF-8
//!   lossy 转换，会把中文变成 U+FFFD。`ForumClient::send` 会用
//!   `forum_core::encoding::decode_body` 按 Content-Type 解码。
//! - **保留 `HttpHead.url`**：发帖成功后 Discuz 会 302 到 `viewthread.php`，
//!   内核靠最终 URL 判定结果。
//! - **保留 `Set-Cookie` 原始头**：`cdb_auth` 登录态由内核自行维护，
//!   六端行为一致，不依赖平台各自的 cookie jar。

use forum_core::transport::{HttpRequest, HttpResponse, Method, Transport};
use forum_core::{Error, Result};
use nsh_core::platform::HttpRequest as NshRequest;
use nsh_core::prelude::fetch;

/// 零大小的传输实现：所有状态都在 `ForumClient` 里，本类型可任意复制。
#[derive(Debug, Clone, Copy, Default)]
pub struct NshTransport;

impl Transport for NshTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        let method = match request.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let mut req = NshRequest::new(method, request.url);
        for (name, value) in &request.headers {
            req = req.header(name.clone(), value.clone());
        }
        if !request.body.is_empty() {
            req = req.body(request.body);
        }

        // `Fetch` 是引擎任务里的 Future，必须在组件 `spawn` 的任务中 await。
        let response = fetch(req).await.map_err(Error::Http)?;
        Ok(HttpResponse {
            status: response.status(),
            headers: response.head.headers,
            body: response.body,
            final_url: response.head.url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_stateless_and_shareable() {
        fn assert_send_sync<T: Send + Sync + Clone + Copy>() {}
        assert_send_sync::<NshTransport>();
    }
}
