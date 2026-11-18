//! 真实登录态下的写操作复核工具（人工触发，默认只读）。
//!
//! 存在的理由：私信发送、投票提交、黑名单增删只有 `MockTransport` 覆盖，
//! 站点真实响应没验过。本工具用 `curl` 实现 [`Transport`]，因此 **调用的是
//! `ForumClient` 里那几条真实代码路径**（`login` / `send_sms` / `vote_poll` /
//! `set_blacklist`），而不是另写一套请求。
//!
//! 三种凭据来源，任选其一：
//!
//! ```text
//! --password-file <path>   首行用户名、次行密码（权限须 0600），走真实登录
//! --session <path>         内核会话快照 JSON（App 落盘的同一种格式）
//! --cookie "cdb_auth=...; cdb_sid=..."   直接注入 cookie
//! ```
//!
//! 不带凭据时默认进入自检模式：只做匿名 GET + 断网重试对照，不产生任何写操作。
//!
//! ```text
//! # 自检（无需登录，离线也能跑重试部分）
//! cargo run --example verify_writes --offline
//!
//! # 只读复核：身份、资料、黑名单名单、投票表单
//! cargo run --example verify_writes --offline -- --session ~/.cache/hipda/session.json
//!
//! # 真实写操作（必须显式 --yes）
//! cargo run --example verify_writes --offline -- --session ... --yes \
//!     --blacklist-user someone --pm-target 12345 --pm-text "自测，请忽略" \
//!     --vote-fid 2 --vote-tid 3057651 --vote-option 12
//! ```
//!
//! 输出只含方法、URL、状态码与解析后的站点消息，**不打印 cookie 与密码**。

#![forbid(unsafe_code)]

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

use forum_core::transport::{ForumClient, HttpRequest, HttpResponse, Method, Transport};
use forum_core::{parse_document, parse_poll, Config, Error, Result};

// ---------------------------------------------------------------- 异步驱动

/// 本工具的传输层是阻塞的（起 curl 进程），Future 第一次 poll 就 Ready，
/// 所以一个不挂起的 executor 足够，无需引入 tokio。
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

/// 一次真实请求的记录，用于逐步打印「这一步到底发了什么、回了什么」。
#[derive(Debug, Clone)]
struct Exchange {
    method: &'static str,
    url: String,
    status: u16,
    final_url: String,
    bytes: usize,
    /// 站点返回的原文摘录（GBK 解码后压平），写操作成败要以它为准。
    preview: String,
}

#[derive(Clone)]
struct CurlTransport {
    dir: PathBuf,
    seq: Arc<AtomicUsize>,
    trace: Arc<Mutex<Vec<Exchange>>>,
}

impl CurlTransport {
    fn new(dir: PathBuf) -> Self {
        // cookie 会落到临时请求头文件里，目录权限必须是 0700。
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        let _ = builder.create(&dir);
        Self {
            dir,
            seq: Arc::new(AtomicUsize::new(0)),
            trace: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn post_count(&self, from: usize) -> usize {
        self.trace
            .lock()
            .map(|t| {
                t[from..]
                    .iter()
                    .filter(|e| e.method == "POST")
                    .count()
            })
            .unwrap_or(0)
    }

    fn snapshot(&self) -> Vec<Exchange> {
        self.trace.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

/// curl 配置文件里的字符串值：转义反斜杠与双引号。
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Transport for CurlTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        let n = self.seq.fetch_add(1, Ordering::SeqCst);
        let body_path = self.dir.join(format!("{n}.body"));
        let header_path = self.dir.join(format!("{n}.hdr"));
        let config_path = self.dir.join(format!("{n}.curl"));

        let mut config = String::new();
        config.push_str(&format!("url = {}\n", quote(&request.url)));
        config.push_str("silent\nshow-error\nlocation\nmax-redirs = 5\n");
        config.push_str("connect-timeout = 15\nmax-time = 60\n");
        // 本机地址不走代理：故障注入要的是「真的连不上」，而不是代理的 502。
        config.push_str("noproxy = \"127.0.0.1,localhost\"\n");
        config.push_str(&format!("output = {}\n", quote(&body_path.to_string_lossy())));
        config.push_str(&format!(
            "dump-header = {}\n",
            quote(&header_path.to_string_lossy())
        ));
        config.push_str("write-out = \"%{http_code} %{url_effective}\"\n");
        if request.method == Method::Post {
            config.push_str("request = \"POST\"\n");
        }
        for (name, value) in &request.headers {
            config.push_str(&format!("header = {}\n", quote(&format!("{name}: {value}"))));
        }
        if !request.body.is_empty() {
            let data_path = self.dir.join(format!("{n}.post"));
            fs::write(&data_path, &request.body)
                .map_err(|e| Error::Io(format!("写请求体失败：{e}")))?;
            config.push_str(&format!(
                "data-binary = {}\n",
                quote(&format!("@{}", data_path.to_string_lossy()))
            ));
        }
        fs::write(&config_path, config).map_err(|e| Error::Io(format!("写 curl 配置失败：{e}")))?;

        let output = Command::new("curl")
            .arg("--config")
            .arg(&config_path)
            .output()
            .map_err(|e| Error::Http(format!("无法执行 curl：{e}")))?;

        let method = match request.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr
                .lines()
                .find(|l| l.contains("curl:"))
                .unwrap_or("curl 执行失败")
                .trim()
                .to_string();
            // 失败也要记账：重试次数靠这份 trace 统计，漏记会把「没重试」看错。
            if let Ok(mut trace) = self.trace.lock() {
                trace.push(Exchange {
                    method,
                    url: request.url.clone(),
                    status: 0,
                    final_url: String::new(),
                    bytes: 0,
                    preview: String::new(),
                });
            }
            return Err(Error::Http(format!("{method} {} 失败：{reason}", request.url)));
        }

        let meta = String::from_utf8_lossy(&output.stdout);
        let (status, final_url) = meta
            .split_whitespace()
            .collect::<Vec<_>>()
            .split_first()
            .map(|(code, rest)| {
                (
                    code.parse::<u16>().unwrap_or(0),
                    rest.join(" "),
                )
            })
            .unwrap_or((0, request.url.clone()));

        let raw_headers = fs::read(&header_path).unwrap_or_default();
        let headers = parse_headers(&raw_headers);
        let body = fs::read(&body_path).unwrap_or_default();
        let content_type = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let preview = preview_of(&body, content_type);

        if let Ok(mut trace) = self.trace.lock() {
            trace.push(Exchange {
                method,
                url: request.url.clone(),
                status,
                final_url: final_url.clone(),
                bytes: body.len(),
                preview,
            });
        }

        Ok(HttpResponse {
            status,
            headers,
            body,
            final_url,
        })
    }
}

/// `curl -D` 会把每次跳转的响应头依次写入同一个文件，取最后一段
/// （即最终响应），中间的 301/302 只作为痕迹保留。
fn parse_headers(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let blocks: Vec<&str> = text.split("\r\n\r\n").flat_map(|b| b.split("\n\n")).collect();
    let last = blocks
        .iter()
        .rev()
        .find(|b| b.trim_start().starts_with("HTTP/"))
        .copied()
        .unwrap_or("");
    let mut headers = Vec::new();
    for line in last.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if !name.is_empty() {
                headers.push((name.to_string(), value.trim().to_string()));
            }
        }
    }
    headers
}

/// 站点原文摘录：按响应头编码解码，去掉标签后压平，供人眼确认
/// 「站点到底说了成功还是失败」。内核返回的固定文案不能作为证据。
fn preview_of(body: &[u8], content_type: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let text = forum_core::encoding::decode_body(body, content_type, forum_core::ENCODE_GBK)
        .unwrap_or_default();
    let text = strip_blocks(&text, "<script", "</script>");
    let text = strip_blocks(&text, "<style", "</style>");
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let flat = out.replace("![CDATA[", " ").replace("]]", " ");
    truncate(&flat, 120)
}

/// 去掉 `<script>…</script>` 这类整块内容，避免预览里全是脚本。
/// 只按字面量切分，不引入 HTML 解析器。
fn strip_blocks(text: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------- 参数

#[derive(Default)]
struct Args {
    cookie: Option<String>,
    /// 从文件读 cookie（权限 0600）。比 `--cookie` 好在不会进 argv / ps 输出。
    cookie_file: Option<PathBuf>,
    session: Option<PathBuf>,
    password_file: Option<PathBuf>,
    save_session: Option<PathBuf>,
    uid: Option<String>,
    blacklist_user: Option<String>,
    pm_target: Option<String>,
    pm_text: Option<String>,
    vote_fid: Option<i32>,
    vote_tid: Option<String>,
    vote_options: Vec<String>,
    yes: bool,
    keep: bool,
    offline_retry: bool,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args::default();
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            let mut value = || {
                it.next()
                    .unwrap_or_else(|| fail(&format!("{arg} 缺少取值")))
            };
            match arg.as_str() {
                "--cookie" => a.cookie = Some(value()),
                "--cookie-file" => a.cookie_file = Some(PathBuf::from(value())),
                "--session" => a.session = Some(PathBuf::from(value())),
                "--password-file" => a.password_file = Some(PathBuf::from(value())),
                "--save-session" => a.save_session = Some(PathBuf::from(value())),
                "--uid" => a.uid = Some(value()),
                "--blacklist-user" => a.blacklist_user = Some(value()),
                "--pm-target" => a.pm_target = Some(value()),
                "--pm-text" => a.pm_text = Some(value()),
                "--vote-fid" => a.vote_fid = Some(value().parse().unwrap_or(0)),
                "--vote-tid" => a.vote_tid = Some(value()),
                "--vote-option" => a.vote_options.push(value()),
                "--yes" => a.yes = true,
                "--keep" => a.keep = true,
                "--offline-retry" => a.offline_retry = true,
                "--help" | "-h" => {
                    println!("{}", HELP);
                    std::process::exit(0);
                }
                other => fail(&format!("未知参数 {other}（--help 看用法）")),
            }
        }
        a
    }
}

const HELP: &str = "\
用法：cargo run --example verify_writes -- [选项]

凭据（三选一，缺省则只做匿名自检）：
  --cookie <raw>            cdb_auth=...; cdb_sid=...
  --cookie-file <path>      从 0600 文件读同一串（不进 argv）
  --session <path>          内核会话快照 JSON
  --password-file <path>    首行用户名、次行密码，权限须 0600
  --save-session <path>     登录成功后把快照写到该文件

目标：
  --uid <uid>               资料页目标（默认用当前登录用户）
  --blacklist-user <name>   黑名单增删复核的对象（会先加后删）
  --pm-target <uid>         私信收件人 uid
  --pm-text <text>          私信正文
  --vote-fid <fid> --vote-tid <tid> --vote-option <id>（可重复）

开关：
  --yes                     真正提交写操作；缺省只预检不提交
  --offline-retry           只跑「GET 重试 3 次 / POST 只发 1 次」对照（离线可跑）
  --keep                    保留临时目录（排查 curl 原文时用）
";

fn fail(message: &str) -> ! {
    eprintln!("✗ {message}");
    std::process::exit(2)
}

// ---------------------------------------------------------------- 步骤记录

struct Step {
    name: &'static str,
    ok: bool,
    detail: String,
    posts: usize,
    exchanges: Vec<Exchange>,
}

struct Runner {
    steps: Vec<Step>,
    transport: CurlTransport,
}

impl Runner {
    fn new(transport: CurlTransport) -> Self {
        Self {
            steps: Vec::new(),
            transport,
        }
    }

    /// 跑一段内核调用，记录期间发出的请求与 POST 次数。
    fn run(
        &mut self,
        name: &'static str,
        body: impl FnOnce() -> Result<(bool, String)>,
    ) -> bool {
        let from = self.transport.snapshot().len();
        let outcome = body();
        let exchanges = self.transport.snapshot()[from..].to_vec();
        let posts = self.transport.post_count(from);
        let (ok, detail) = match outcome {
            Ok((ok, detail)) => (ok, detail),
            Err(e) => (false, e.to_string()),
        };
        println!("[{:02}] {name}", self.steps.len() + 1);
        for e in &exchanges {
            let tail = if e.final_url.is_empty() || e.final_url == e.url {
                String::new()
            } else {
                format!(" → {}", e.final_url)
            };
            let status = if e.status == 0 {
                "无响应".to_string()
            } else {
                format!("HTTP {}", e.status)
            };
            println!("     {} {}  {}{}  {} B", e.method, e.url, status, tail, e.bytes);
        }
        if exchanges.is_empty() {
            println!("     （未发出请求）");
        }
        if let Some(last) = exchanges.last() {
            if !last.preview.is_empty() {
                println!("     站点原文：{}", last.preview);
            }
        }
        println!("     {} {detail}", if ok { "✓" } else { "✗" });
        if posts > 0 {
            println!("     POST 次数：{posts}");
        }
        self.steps.push(Step {
            name,
            ok,
            detail,
            posts,
            exchanges,
        });
        ok
    }
}

// ---------------------------------------------------------------- 主体

fn main() {
    let args = Args::parse();
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let dir = std::env::temp_dir().join(format!("hipda-verify-{started}-{}", std::process::id()));
    let transport = CurlTransport::new(dir.clone());
    let client = ForumClient::new(Config::default(), transport.clone());
    let mut runner = Runner::new(transport.clone());

    if args.offline_retry {
        offline_retry(&client, &mut runner);
        summarize(&runner, &args);
        return;
    }

    if args.cookie.is_none()
        && args.cookie_file.is_none()
        && args.session.is_none()
        && args.password_file.is_none()
    {
        println!("未提供凭据 → 匿名自检（不含任何写操作）\n");
        anonymous(&client, &mut runner);
        offline_retry(&client, &mut runner);
        summarize(&runner, &args);
        cleanup(&dir, &args);
        return;
    }

    // 1. 建立登录态
    let logged_in = acquire_session(&client, &args, &mut runner);
    if !logged_in {
        println!();
        summarize(&runner, &args);
        cleanup(&dir, &args);
        std::process::exit(1);
    }

    // 2. 只读复核：身份、资料、黑名单名单、投票表单
    readonly(&client, &args, &mut runner);

    // 3. 写操作（必须 --yes）
    if !args.yes {
        println!("\n未传 --yes：只做预检，不提交任何写操作。");
    } else {
        println!("\n--yes 已确认，开始真实写操作：");
        write_ops(&client, &args, &mut runner);
    }

    summarize(&runner, &args);
    cleanup(&dir, &args);
}

fn acquire_session(client: &ForumClient<CurlTransport>, args: &Args, runner: &mut Runner) -> bool {
    if let Some(path) = &args.session {
        return runner.run("载入会话快照", || {
            let raw = fs::read_to_string(path)
                .map_err(|e| Error::Io(format!("读取 {} 失败：{e}", path.display())))?;
            if client.restore_session(&raw)? {
                let (name, uid) = client.identity();
                Ok((true, format!("快照可用：{name} (uid {uid})")))
            } else {
                Ok((false, "快照里没有有效的 cdb_auth".into()))
            }
        });
    }

    if args.cookie.is_some() || args.cookie_file.is_some() {
        return runner.run("注入 cookie", || {
            let raw = match (&args.cookie, &args.cookie_file) {
                (Some(raw), _) => raw.clone(),
                (None, Some(path)) => fs::read_to_string(path)
                    .map_err(|e| Error::Io(format!("读取 cookie 文件失败：{e}")))?,
                _ => String::new(),
            };
            let cookies: Vec<(String, String)> = raw
                .split(';')
                .filter_map(|pair| {
                    let (k, v) = pair.trim().split_once('=')?;
                    Some((k.trim().to_string(), v.trim().to_string()))
                })
                .collect();
            let has_auth = cookies.iter().any(|(k, v)| k == "cdb_auth" && !v.is_empty());
            if !has_auth {
                return Ok((false, "cookie 里缺少 cdb_auth，不算登录态".into()));
            }
            let snapshot = serde_json::json!({
                "username": "", "uid": "", "cookies": cookies, "saved_at": 0
            })
            .to_string();
            let ok = client.restore_session(&snapshot)?;
            Ok((ok, format!("注入 {} 个 cookie", cookies.len())))
        });
    }

    let path = match &args.password_file {
        Some(p) => p.clone(),
        None => return false,
    };
    runner.run("账号密码登录", || {
        let text = fs::read_to_string(&path)
            .map_err(|e| Error::Io(format!("读取凭据文件失败：{e}")))?;
        let mut lines = text.lines();
        let username = lines.next().unwrap_or("").trim().to_string();
        let password = lines.next().unwrap_or("").trim().to_string();
        if username.is_empty() || password.is_empty() {
            return Ok((false, "凭据文件应为「首行用户名、次行密码」".into()));
        }
        let result = block_on(client.login(&username, &password, "", ""))?;
        if result.ok {
            if let Some(save) = &args.save_session {
                let _ = client.save_session(save);
            }
            Ok((
                true,
                format!("{}：{} (uid {})", result.message, result.username, result.uid),
            ))
        } else {
            Ok((false, result.message))
        }
    })
}

fn anonymous(client: &ForumClient<CurlTransport>, runner: &mut Runner) {
    runner.run("论坛首页（游客 + cdb_sid 预热）", || {
        let index = block_on(client.forum_index())?;
        Ok((
            !index.sections.is_empty(),
            format!("{} 个分区 / {} 个版块", index.sections.len(), index.forum_count()),
        ))
    });
    runner.run("登录页 formhash", || {
        let resp = block_on(client.get(&client.endpoints().login_get_form_hash))?;
        let hash = forum_core::parse_form_hash(&parse_document(&resp.body));
        Ok((
            !hash.is_empty(),
            if hash.is_empty() {
                "页面没有 formhash（可能被 WAF 挡了）".to_string()
            } else {
                format!("HTTP {}，formhash 长度 {}", resp.status, hash.len())
            },
        ))
    });
}

fn readonly(client: &ForumClient<CurlTransport>, args: &Args, runner: &mut Runner) {
    runner.run("登录态校验（版块页 #umenu）", || {
        block_on(client.refresh_identity())?;
        let (name, uid) = client.identity();
        Ok((!name.is_empty(), format!("{name} (uid {uid})")))
    });

    let target = args.uid.clone().unwrap_or_else(|| client.identity().1);
    if !target.is_empty() {
        runner.run("用户资料 space.php", || {
            let info = block_on(client.user_info(&target))?;
            Ok((
                !info.username.is_empty(),
                format!("{}：{}", info.username, truncate(&info.detail, 60)),
            ))
        });
    }

    runner.run("黑名单名单 + 同页 formhash", || {
        let list = block_on(client.blacklist())?;
        Ok((
            true,
            format!(
                "{} 人{}{}",
                list.users.len(),
                if list.empty_hint { "（站点提示为空）" } else { "" },
                if list.form_hash.is_empty() {
                    "，缺 formhash（写操作会失败）"
                } else {
                    "，formhash 就绪"
                }
            ),
        ))
    });

    if let (Some(fid), Some(tid)) = (args.vote_fid, args.vote_tid.clone()) {
        runner.run("投票表单 form#poll", || {
            let url = format!("{}{tid}", client.endpoints().detail_list);
            let resp = block_on(client.get(&url))?;
            let poll = parse_poll(&parse_document(&resp.body));
            match poll {
                Some(p) => {
                    let detail = if p.options.is_empty() {
                        // 站点会在这里给出「您所在的用户组没有投票权限」等说明，
                        // 这与解析无关，照实报出来，别让人以为客户端解析坏了。
                        format!(
                            "「{}」没有可投选项：{}",
                            p.title,
                            if p.footer.is_empty() {
                                "站点未给出原因"
                            } else {
                                p.footer.as_str()
                            }
                        )
                    } else {
                        format!(
                            "「{}」{} 个选项，上限 {} 项{}",
                            p.title,
                            p.options.len(),
                            p.max_answer,
                            if p.form_hash.is_empty() {
                                "，缺 formhash"
                            } else {
                                ""
                            }
                        )
                    };
                    Ok((!p.form_hash.is_empty(), detail))
                }
                None => Ok((false, format!("tid {tid} 首楼没有投票表单"))),
            }
        });
        let _ = fid;
    }
}

fn write_ops(client: &ForumClient<CurlTransport>, args: &Args, runner: &mut Runner) {
    // 黑名单：加 → 校验名单 → 删 → 校验名单恢复。隐私、可逆，先跑它。
    if let Some(user) = &args.blacklist_user {
        runner.run("黑名单 · 加入", || {
            let message = block_on(client.set_blacklist(user, true))?;
            let list = block_on(client.blacklist())?;
            let present = list.users.iter().any(|u| u == user);
            Ok((
                present,
                format!(
                    "{message}；复查名单{}",
                    if present { "已包含" } else { "未包含（写操作未生效）" }
                ),
            ))
        });
        runner.run("黑名单 · 移出", || {
            let message = block_on(client.set_blacklist(user, false))?;
            let list = block_on(client.blacklist())?;
            let present = list.users.iter().any(|u| u == user);
            Ok((
                !present,
                format!(
                    "{message}；复查名单{}",
                    if present { "仍包含（未生效）" } else { "已恢复" }
                ),
            ))
        });
    }

    if let (Some(uid), Some(text)) = (&args.pm_target, &args.pm_text) {
        runner.run("私信 · 发送", || {
            let message = block_on(client.send_sms(uid, "", text))?;
            Ok((true, message))
        });
    }

    if let (Some(fid), Some(tid)) = (args.vote_fid, args.vote_tid.clone()) {
        runner.run("投票 · 提交", || {
            let url = format!("{}{tid}", client.endpoints().detail_list);
            let resp = block_on(client.get(&url))?;
            let poll = parse_poll(&parse_document(&resp.body))
                .ok_or_else(|| Error::Protocol(format!("tid {tid} 没有投票表单")))?;
            let answers = if args.vote_options.is_empty() {
                // 未指定选项时只选第一个，避免误投多选。
                vec![poll
                    .options
                    .first()
                    .map(|o| o.id.clone())
                    .ok_or_else(|| Error::Protocol("投票没有选项".into()))?]
            } else {
                args.vote_options.clone()
            };
            for id in &answers {
                if !poll.options.iter().any(|o| &o.id == id) {
                    return Ok((false, format!("选项 {id} 不在该投票里")));
                }
            }
            let message = block_on(client.vote_poll(fid, tid.as_str(), &answers, &poll.form_hash))?;
            Ok((true, format!("{message}（选项 {}）", answers.join(","))))
        });
    }
}

/// 这条与站点无关，但正是「POST 不重试」的正面证据：
/// 同样的网络故障下，GET 重试 3 次，POST（含写操作）只发 1 次。
/// 指向本机一个必然拒绝连接的端口，不产生任何外部流量。
fn offline_retry(client: &ForumClient<CurlTransport>, runner: &mut Runner) {
    let dead = "http://127.0.0.1:1/hipda-retry-probe";

    runner.run("故障注入 · GET 应重试 3 次", || {
        let result = block_on(client.get(dead));
        Ok((
            result.is_err(),
            "连接被拒时应判网络错误".into(),
        ))
    });
    expect_last(runner, 3, 0, "GET 重试 3 次后放弃");

    runner.run("故障注入 · POST 只发 1 次", || {
        let result = block_on(client.post_form(dead, &[("probe".into(), "1".into())]));
        Ok((result.is_err(), "同一故障下 POST 应直接失败".into()))
    });
    expect_last(runner, 1, 1, "POST 只发 1 次，不重发");
}

/// 校验刚跑完那一步的请求数 / POST 数，不匹配就把这一步判失败。
fn expect_last(runner: &mut Runner, requests: usize, posts: usize, hint: &str) {
    let Some(step) = runner.steps.last_mut() else {
        return;
    };
    let actual_requests = step.exchanges.len();
    let actual_posts = step.posts;
    if actual_requests == requests && actual_posts == posts {
        step.detail = format!("{hint}（请求 {actual_requests} 次 / POST {actual_posts} 次）");
        return;
    }
    step.ok = false;
    step.detail = format!(
        "期望请求 {requests} 次 / POST {posts} 次，实际 {actual_requests} / {actual_posts}"
    );
}

fn summarize(runner: &Runner, args: &Args) {
    let failed = runner.steps.iter().filter(|s| !s.ok).count();
    let multipost: Vec<&str> = runner
        .steps
        .iter()
        .filter(|s| s.posts > 1)
        .map(|s| s.name)
        .collect();

    println!("\n================ 汇总 ================");
    for s in &runner.steps {
        println!(
            "{} {:<28} {}",
            if s.ok { "✓" } else { "✗" },
            s.name,
            truncate(&s.detail, 70)
        );
    }
    println!("--------------------------------------");
    println!(
        "{} 步，{} 通过，{} 失败",
        runner.steps.len(),
        runner.steps.len() - failed,
        failed
    );
    if multipost.is_empty() {
        println!("POST 次数：所有步骤均 ≤ 1（无重发）");
    } else {
        println!("⚠ 以下步骤 POST 超过 1 次：{multipost:?}");
    }
    if !args.yes && args.cookie.is_some() {
        println!("提示：加 --yes 才会提交写操作。");
    }
    if failed > 0 || !multipost.is_empty() {
        std::process::exit(1);
    }
}

fn cleanup(dir: &Path, args: &Args) {
    if args.keep {
        println!("\n临时请求原文保留在 {}", dir.display());
        return;
    }
    let _ = fs::remove_dir_all(dir);
}

fn truncate(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{cut}…")
}
