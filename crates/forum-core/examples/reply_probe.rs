//! 回帖链路探针：验证「回帖请求的参数对不对」，并且**不制造内容**。
//!
//! 做法：用故意过短的正文（少于站点要求的 5 个字）走一次真实提交。
//!
//! - 拿到站点的字数错误提示 → URL、`replysubmit`、formhash、GBK 编码全部正确；
//! - 拿到「发帖结果未知 / 没反应」 → 参数仍然不对。
//!
//! 正文不合法，站点不会落库，所以不会在论坛上留下测试帖。
//!
//! ```text
//! cargo run -p forum-core --example reply_probe --offline -- \
//!     --tid 3476991 --cookie-file ~/.cache/hipda/cookies.txt
//! ```

#![forbid(unsafe_code)]

use std::future::Future;
use std::path::PathBuf;
use std::process::Command;
use std::task::{Context, Poll, Waker};

use forum_core::post::{post_target_url, PostMode, PostRequest};
use forum_core::transport::{ForumClient, HttpRequest, HttpResponse, Method, Transport};
use forum_core::{parse_document, parse_form_hash, Config, Error};

// ---------------------------------------------------------------- 极简执行器

fn block_on<F: Future>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

// ---------------------------------------------------------------- curl 传输

/// 走真实网络。请求体先落临时文件再用 `--data-binary @file`，
/// 避免 GBK 编码在命令行里被破坏。
struct CurlTransport;

impl Transport for CurlTransport {
    async fn send(&self, request: HttpRequest) -> forum_core::Result<HttpResponse> {
        let dir = std::env::temp_dir();
        let tag = format!("reply-probe-{}", std::process::id());
        let body_path = dir.join(format!("{tag}.body"));
        let post_path = dir.join(format!("{tag}.post"));
        let head_path = dir.join(format!("{tag}.hdr"));

        let mut config = String::new();
        config.push_str(&format!("url = \"{}\"\n", request.url));
        config.push_str("silent\nshow-error\nlocation\nmax-redirs = 5\n");
        config.push_str("connect-timeout = 15\nmax-time = 60\n");
        config.push_str(&format!("output = \"{}\"\n", body_path.display()));
        config.push_str(&format!("dump-header = \"{}\"\n", head_path.display()));
        config.push_str("write-out = \"%{http_code} %{url_effective}\"\n");
        if request.method == Method::Post {
            config.push_str("request = \"POST\"\n");
        }
        for (name, value) in &request.headers {
            config.push_str(&format!("header = \"{name}: {value}\"\n"));
        }
        if !request.body.is_empty() {
            std::fs::write(&post_path, &request.body)
                .map_err(|e| Error::Io(format!("写请求体失败：{e}")))?;
            config.push_str(&format!("data-binary = \"@{}\"\n", post_path.display()));
        }
        let config_path = dir.join(format!("{tag}.curl"));
        std::fs::write(&config_path, config)
            .map_err(|e| Error::Io(format!("写 curl 配置失败：{e}")))?;

        let output = Command::new("curl")
            .arg("--config")
            .arg(&config_path)
            .output()
            .map_err(|e| Error::Http(format!("无法执行 curl：{e}")))?;
        let meta = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut parts = meta.split_whitespace();
        let status: u16 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let final_url = parts.next().unwrap_or(&request.url).to_string();
        let body = std::fs::read(&body_path).unwrap_or_default();

        for path in [body_path, post_path, head_path, config_path] {
            let _ = std::fs::remove_file(path);
        }
        Ok(HttpResponse {
            status,
            headers: Vec::new(),
            body,
            final_url,
        })
    }
}

// ---------------------------------------------------------------- 主流程

fn main() {
    let mut tid = String::new();
    let mut cookie_file: Option<PathBuf> = None;
    let mut message = "短".to_string();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tid" => tid = args.next().unwrap_or_default(),
            "--cookie-file" => cookie_file = args.next().map(PathBuf::from),
            "--message" => message = args.next().unwrap_or_default(),
            "--help" | "-h" => {
                println!(
                    "用法：reply_probe --tid <主题> --cookie-file <cookie 文件> [--message <正文>]"
                );
                return;
            }
            other => {
                eprintln!("未知参数 {other}");
                return;
            }
        }
    }
    if tid.is_empty() {
        eprintln!("缺少 --tid");
        return;
    }

    let client = ForumClient::new(Config::default(), CurlTransport);
    if let Some(path) = cookie_file {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            eprintln!("读 cookie 失败：{}", path.display());
            return;
        };
        let cookies: Vec<(String, String)> = raw
            .split(';')
            .filter_map(|pair| {
                let (k, v) = pair.trim().split_once('=')?;
                Some((k.trim().to_string(), v.trim().to_string()))
            })
            .collect();
        let snapshot = serde_json::json!({
            "username": "", "uid": "", "cookies": cookies, "saved_at": 0
        })
        .to_string();
        match client.restore_session(&snapshot) {
            Ok(true) => println!("已注入 {} 个 cookie", cookies.len()),
            _ => {
                eprintln!("cookie 注入失败（缺 cdb_auth？）");
                return;
            }
        }
    }

    let url = format!("{}{tid}&page=1", client.endpoints().detail_list);
    let page = match block_on(client.get(&url)) {
        Ok(page) => page,
        Err(e) => {
            eprintln!("取详情页失败：{e}");
            return;
        }
    };
    let hash = parse_form_hash(&parse_document(&page.body));
    if hash.is_empty() {
        eprintln!("没取到 formhash —— 登录态可能已失效");
        return;
    }
    println!("formhash：已取到");

    let request = PostRequest {
        mode: PostMode::Reply,
        tid: tid.clone(),
        form_hash: hash,
        message: message.clone(),
        ..Default::default()
    };
    println!(
        "提交地址：{}",
        post_target_url(client.endpoints(), &request).unwrap_or_default()
    );
    println!(
        "正文长度：{} 字（站点要求 ≥ 5，故意用短文本探路）",
        message.trim().chars().count()
    );

    match block_on(client.submit_post(&request)) {
        Ok(outcome) => {
            println!("ok={} 跳转={}", outcome.ok, outcome.final_url);
            println!("站点文案：{}", outcome.message);
            if outcome.ok {
                println!("⚠ 这次真的发出去了（正文合法），请自行确认是否保留");
            } else {
                println!("✓ 没有产生帖子，且拿到了站点的拒绝原因 —— 请求参数已正确");
            }
        }
        Err(e) => println!("提交失败：{e}"),
    }
}
