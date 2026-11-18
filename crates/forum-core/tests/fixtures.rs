//! 用 2026-11-13 的真实页面样本验证 Rust 内核与 Go 参考实现行为等价。
//!
//! 样本来源：
//! - `forumdisplay_fid2.html`：curl 抓的「未登录」提示页，原始 GBK 字节
//! - `logged_forumdisplay.html` / `logged_viewthread.html` / `thread_row_sample.html`：
//!   ego-browser 登录态 DOM，UTF-8
//! - `guest_index.html`：curl 带 cdb_sid 抓的游客论坛首页，原始 GBK 字节
//! - `logged_index.html`：ego-browser 登录态论坛首页 DOM，UTF-8

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use forum_core::config::{
    fill, forum_name, BS_TYPES, DEFAULT_FORUMS, DEFAULT_FORUM_SERVER, DEFAULT_IMAGE_HOST,
    ENCODE_GBK, ENCODE_UTF8, FID_BUY_SELL, FORUMS, SECURITY_QUESTIONS,
};
use forum_core::emoji::{bbcode_image, register_emoji_aliases};
use forum_core::encoding::{decode_body, encode_form};
use forum_core::parse::{parse_simple_list, SimpleListKind};
use forum_core::post::{append_tail, build_post_values, post_target_url, PostMode, PostRequest};
use forum_core::transport::{ForumClient, HttpRequest, HttpResponse, Transport};
use forum_core::*;

const GATE_GBK: &[u8] = include_bytes!("../testdata/forumdisplay_fid2.html");
const LOGGED_LIST: &str = include_str!("../testdata/logged_forumdisplay.html");
const LOGGED_DETAIL: &str = include_str!("../testdata/logged_viewthread.html");
const ROW_SAMPLE: &str = include_str!("../testdata/thread_row_sample.html");
const GUEST_INDEX_GBK: &[u8] = include_bytes!("../testdata/guest_index.html");
const LOGGED_INDEX: &str = include_str!("../testdata/logged_index.html");

/// 无依赖的最小执行器：MockTransport 的 future 一次 poll 即 Ready。
fn block_on<F: Future>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

// ---------------------------------------------------------------- 编码

#[test]
fn gbk_decode_of_real_page() {
    let body = decode_body(GATE_GBK, "text/html; charset=GBK", ENCODE_GBK).unwrap();
    assert!(body.contains("您还未登录，无权访问该版块"), "解码结果异常：{}", &body[..body.len().min(200)]);
    assert!(body.contains("Discuz! 7.2"), "未识别 Discuz! 7.2 元数据");
}

#[test]
fn encode_form_uses_gbk_bytes() {
    let pairs = vec![
        ("message".to_string(), "中文".to_string()),
        ("formhash".to_string(), "abc".to_string()),
    ];
    let gbk = encode_form(&pairs, ENCODE_GBK);
    // 「中文」GBK = D6 D0 CE C4
    assert!(gbk.contains("%D6%D0%CE%C4"), "中文未按 GBK 编码：{gbk}");
    assert!(gbk.contains("formhash=abc"));
    assert!(gbk.starts_with("formhash="), "key 应稳定排序：{gbk}");
    let utf8 = encode_form(&pairs, ENCODE_UTF8);
    assert!(utf8.contains("%E4%B8%AD%E6%96%87"), "UTF-8 编码异常：{utf8}");
}

// ---------------------------------------------------------------- 登录门页

#[test]
fn login_gate_page() {
    let body = decode_body(GATE_GBK, "text/html; charset=GBK", ENCODE_GBK).unwrap();
    let doc = parse_document(&body);
    assert_eq!(parse_form_hash(&doc), "01dc6a5d");
    assert!(!parse_login_state(&doc), "未登录提示页被误判为已登录");
    assert!(parse_error_message(&doc).contains("您还未登录"));
    let q = parse_security_questions(&doc);
    assert_eq!(q.len(), 8, "安全提问应 8 项");
    assert_eq!(q[1].text, "母亲的名字");
    assert_eq!(q[7].text, "驾驶执照的最后四位数字");
}

// ---------------------------------------------------------------- 登录态列表

#[test]
fn logged_forum_display() {
    let doc = parse_document(LOGGED_LIST);
    assert_eq!(parse_current_user(&doc), ("quejuwen".to_string(), "377314".to_string()));
    assert!(parse_login_state(&doc), "登录态页面被判为未登录");
    assert_eq!(parse_form_hash(&doc), "552aa0e8");

    let list = parse_thread_list(&doc, &Config::default(), 1);
    assert_eq!(list.threads.len(), 75, "主题数应为 75");
    assert_eq!(list.uid, "377314");
    let first = &list.threads[0];
    assert_eq!(first.tid, "3057651");
    assert!(first.title.contains("hipda已迁移到新域名4d4y"), "标题：{}", first.title);
    assert_eq!(first.author, "4d4y");
    assert_eq!(first.author_id, "29");
    assert!(first.avatar_url.contains("000/00/00/29_avatar_middle.jpg"), "头像：{}", first.avatar_url);

    let types = parse_thread_types(&doc);
    assert_eq!(types.len(), 15, "线上分类数应为 15，源码硬编码的 9 项已过时");
    assert_eq!(BS_TYPES.len(), 9);
    assert!(parse_max_page(&doc) > 1, "分页最大值应大于 1");
}

#[test]
fn single_thread_row_sample() {
    // 单独一行 tbody 必须放进 table，HTML5 树构建器才会保留 tbody 节点。
    let wrapped = format!("<html><body><table>{ROW_SAMPLE}</table></body></html>");
    let doc = parse_document(&wrapped);
    let list = parse_thread_list(&doc, &Config::default(), 1);
    assert_eq!(list.threads.len(), 1, "单行样本应解析出 1 条主题");
    assert_eq!(list.threads[0].tid, "3057651");
}

// ---------------------------------------------------------------- 登录态详情

#[test]
fn logged_view_thread() {
    let doc = parse_document(LOGGED_DETAIL);
    let d = parse_thread_detail(&doc, &Config::default(), "3057651", 1);
    assert!(d.title.contains("hipda已迁移到新域名4d4y"), "标题：{}", d.title);
    assert_eq!(d.posts.len(), 1, "楼层数应为 1");
    assert!(d.has_content());
    let p = &d.posts[0];
    assert_eq!(p.post_id, "64690536");
    assert_eq!(p.floor, 1);
    assert_eq!(p.author, "4d4y");
    assert_eq!(p.author_id, "29");
    assert!(p.time_post.contains("2022-6-13 22:53"), "时间：{}", p.time_post);
    assert!(p.copy_text().contains("由此带来第三方客户端暂时无法用"), "正文：{}", p.copy_text());
}

// ---------------------------------------------------------------- 协议与配置

#[test]
fn avatar_url() {
    let cfg = Config::default();
    assert_eq!(
        cfg.avatar_url("723379"),
        format!("{DEFAULT_IMAGE_HOST}/forum/uc_server/data/avatar/000/72/33/79_avatar_middle.jpg")
    );
    assert!(cfg.avatar_url("29").ends_with("000/00/00/29_avatar_middle.jpg"));
    for bad in ["", "1234567890", "abc"] {
        assert_eq!(cfg.avatar_url(bad), "", "非法 uid {bad} 应返回空串");
    }
}

#[test]
fn processed_password() {
    let md5hex = "098f6bcd4621d373cade4e832627b4f6";
    assert_eq!(forum_core::processed_password(md5hex), md5hex, "已哈希密码不应二次处理");
    assert_eq!(forum_core::processed_password("abc").len(), 32);
    // 反斜杠与双引号先转义再哈希
    assert_eq!(forum_core::processed_password(r#"a\b"c"#).len(), 32);
    assert_ne!(
        forum_core::processed_password(r#"a\b"c"#),
        forum_core::processed_password("abc"),
        "转义分支应生效"
    );
}

#[test]
fn upload_response() {
    assert_eq!(parse_upload_response("DISCUZUPLOAD|0|1721652|1").unwrap(), "1721652");
    for bad in ["DISCUZUPLOAD|0|0|1", "DISCUZUPLOAD|1|1721652|1", "上传失败"] {
        assert!(parse_upload_response(bad).is_err(), "错误响应未被拒绝：{bad}");
    }
}

#[test]
fn endpoints_and_forums() {
    let ep = Endpoints::new(&Config::default());
    assert_eq!(ep.as_map().len(), 37, "端点表字段数（新增 forum_index）");
    assert!(ep.thread_list.starts_with(&format!("{DEFAULT_FORUM_SERVER}/forum/forumdisplay.php?fid=")));
    assert!(!fill(&ep.redirect_to_post, &[("pid", "1"), ("tid", "2")]).contains('{'));
    assert_eq!(FORUMS.len(), 21, "HiUtils.FORUMS 实际 21 项");
    assert_eq!(forum_name(FID_BUY_SELL), "Buy & Sell");
    assert_eq!(DEFAULT_FORUMS.len(), 3);
    assert_eq!(SECURITY_QUESTIONS.len(), 8);
    assert_eq!(query_param("viewthread.php?tid=1&amp;page=3#p", "page"), "3");
}

#[test]
fn emoji_and_bbcode() {
    let mut m = HashMap::new();
    m.insert("smile".to_string(), "😄".to_string());
    register_emoji_aliases(m);
    assert_eq!(forum_core::emoji::replace_emoji("hi :smile: :unknown:"), "hi 😄 :unknown:");
    assert_eq!(bbcode_image("1721652"), "[attachimg]1721652[/attachimg]");
}

#[test]
fn post_request_building() {
    let ep = Endpoints::new(&Config::default());
    let req = PostRequest {
        mode: PostMode::Reply,
        tid: "3057651".into(),
        form_hash: "deadbeef".into(),
        message: "中文回复".into(),
        attachments: vec!["1721652".into()],
        use_signature: true,
        ..Default::default()
    };
    // 回帖 URL 必须带 `replysubmit=yes`：Discuz 只有见到这个参数才处理提交，
    // 否则只把编辑页原样返回，客户端会误判成「发帖结果未知」。
    assert_eq!(
        post_target_url(&ep, &req).unwrap(),
        format!("{DEFAULT_FORUM_SERVER}/forum/post.php?action=reply&tid=3057651&replysubmit=yes&infloat=yes&handlekey=fastpost")
    );
    let v = build_post_values(&req, 1_700_000_000_000);
    let get = |k: &str| v.iter().find(|(n, _)| n == k).map(|(_, x)| x.clone());
    assert_eq!(get("formhash").as_deref(), Some("deadbeef"));
    assert_eq!(get("posttime").as_deref(), Some("1700000000000"));
    assert_eq!(get("usesig").as_deref(), Some("1"));
    assert_eq!(get("message").as_deref(), Some("中文回复"));
    assert_eq!(get("attachnew[][description]").as_deref(), Some("1721652"));
    assert_eq!(append_tail("正文\n", "-- 来自 HiPDA·nsh", true), "正文\n\n-- 来自 HiPDA·nsh");
    assert_eq!(append_tail("正文", "尾巴", false), "正文");
    assert!(post_target_url(&ep, &PostRequest { mode: PostMode::Reply, ..Default::default() }).is_err());
}

// ---------------------------------------------------------------- Transport / 会话

struct MockTransport {
    replies: Mutex<VecDeque<HttpResponse>>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl MockTransport {
    fn new(replies: Vec<HttpResponse>) -> Self {
        Self { replies: Mutex::new(replies.into()), seen: Mutex::new(Vec::new()) }
    }
}

impl Transport for MockTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req.clone());
        let mut q = self.replies.lock().unwrap();
        Ok(q.pop_front().unwrap_or_else(|| HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html; charset=GBK".into())],
            body: b"<html><body>ok</body></html>".to_vec(),
            final_url: String::new(),
        }))
    }
}

fn html_reply(body: &str, extra_headers: Vec<(&str, &str)>) -> HttpResponse {
    // 测试夹具里的字符串是 UTF-8；真实 GBK 解码另有专门用例。
    let mut headers = vec![("Content-Type".to_string(), "text/html; charset=UTF-8".to_string())];
    headers.extend(extra_headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    HttpResponse { status: 200, headers, body: body.as_bytes().to_vec(), final_url: String::new() }
}

#[test]
fn login_cookie_cdn_and_session() {
    let replies = vec![
        // 1. GET 登录页
        html_reply(r#"<html><body><input name="formhash" value="deadbeef"></body></html>"#, vec![]),
        // 2. POST 登录（Discuz 在响应里下登录态 cookie）
        html_reply("欢迎您回来，quejuwen", vec![("Set-Cookie", "cdb_auth=abc123; path=/; domain=4d4y.com")]),
        // 3. GET 版块页补用户身份
        html_reply(
            r#"<html><body><div id="umenu"><cite><a href="space.php?uid=377314">quejuwen</a></cite></div></body></html>"#,
            vec![],
        ),
        // 4. GET /config.php
        HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: br#"{"CDN":"https://img02.4d4y.com"}"#.to_vec(),
            final_url: String::new(),
        },
    ];
    let mock = MockTransport::new(replies);
    let client = ForumClient::new(Config::default(), mock);

    let result = block_on(client.login("quejuwen", "secret", "0", "")).unwrap();
    assert!(result.ok, "登录应成功：{result:?}");
    assert_eq!(result.form_hash, "deadbeef");
    assert_eq!(result.uid, "377314");
    assert!(client.is_logged_in(), "cdb_auth cookie 未生效");

    let cdn = block_on(client.refresh_cdn()).unwrap();
    assert_eq!(cdn, "https://img02.4d4y.com");
    assert_eq!(client.config().image_host, "https://img02.4d4y.com");

    // 再发起一次 GET，确认 Cookie 已注入后续请求。
    let _ = block_on(client.get("https://www.4d4y.com/forum/notice.php"));
    let dir = std::env::temp_dir().join(format!("hipda-nsh-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.json");
    client.save_session(&path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("secret"), "会话文件不得包含明文密码");
    assert!(raw.contains("cdb_auth"));

    let client2 = ForumClient::new(Config::default(), MockTransport::new(vec![]));
    assert!(client2.load_session(&path).unwrap(), "会话恢复后应处于登录态");
    assert_eq!(client2.identity(), ("quejuwen".to_string(), "377314".to_string()));
    client2.logout();
    assert!(!client2.is_logged_in());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn transport_injects_cookie_and_gbk_form() {
    let mock = MockTransport::new(vec![html_reply("ok", vec![("Set-Cookie", "cdb_auth=xyz; path=/")])]);
    let client = ForumClient::new(Config::default(), mock);
    let _ = block_on(client.get("https://www.4d4y.com/forum/notice.php")).unwrap();
    let _ = block_on(client.post_form(
        "https://www.4d4y.com/forum/post.php?action=reply&tid=1",
        &[("message".into(), "中文".into())],
    )).unwrap();
    let seen = client.transport.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].header("Cookie"), Some("cdb_auth=xyz"));
    let body = String::from_utf8_lossy(&seen[1].body);
    assert!(body.contains("%D6%D0%CE%C4"), "POST 正文应按 GBK 编码：{body}");
    assert_eq!(seen[1].method, forum_core::Method::Post);
}

/// 平台层（NSH `Storage` / Web `localStorage`）只拿字符串，不碰文件系统；
/// 这条用例覆盖「快照 → 恢复」以及损坏数据的拒绝逻辑。
#[test]
fn session_snapshot_roundtrips_through_platform_kv() {
    let mock = MockTransport::new(vec![html_reply(
        "ok",
        vec![("Set-Cookie", "cdb_auth=abc123; path=/; domain=4d4y.com")],
    )]);
    let client = ForumClient::new(Config::default(), mock);
    let _ = block_on(client.get("https://www.4d4y.com/forum/notice.php")).unwrap();
    client.set_identity("quejuwen", "377314");

    let json = client.session_snapshot().unwrap();
    assert!(!json.contains("secret"), "快照不得含明文密码");
    assert!(json.contains("cdb_auth"));

    let restored = ForumClient::new(Config::default(), MockTransport::new(vec![]));
    assert!(restored.restore_session(&json).unwrap());
    assert_eq!(restored.identity(), ("quejuwen".to_string(), "377314".to_string()));
    assert!(restored.is_logged_in());

    // 空串、纯空白、无登录 cookie 的快照都不算登录态，
    // 避免退出登录后残留的空对象让界面误显示「已登录」。
    let blank = ForumClient::new(Config::default(), MockTransport::new(vec![]));
    assert!(!blank.restore_session("").unwrap());
    assert!(!blank.restore_session("   ").unwrap());
    assert!(!blank
        .restore_session(r#"{"username":"x","uid":"","cookies":[],"saved_at":0}"#)
        .unwrap());
    assert!(!blank.is_logged_in());
    assert!(blank.restore_session("not json").is_err());
}

/// 搜索 / 收藏 / 我的帖子这类主题表格页：与主题列表不同，`tbody`
/// 没有 `normalthread_` id，且分页链接带 `searchid`。
#[test]
fn simple_list_parses_thread_like_search_results() {
    let html = r#"<html><body>
      <div class="pages_btns"><div class="pages"><a href="search.php?searchid=42&amp;page=2">2</a></div></div>
      <input name="formhash" value="deadbeef">
      <table><tbody>
        <tr>
          <th class="subject"><a href="viewthread.php?tid=3057651&amp;extra=x">搜索到的主题</a></th>
          <td class="author"><cite><a href="space.php?uid=377314">quejuwen</a></cite><em>2026-11-13</em></td>
          <td class="forum">Discovery</td><td class="lastpost">最后回复</td>
        </tr>
      </tbody></table>
    </body></html>"#;
    let doc = parse_document(html);
    let list = parse_simple_list(&doc, &Config::default(), SimpleListKind::Threads);
    assert_eq!(list.items.len(), 1, "{list:?}");
    let item = &list.items[0];
    assert_eq!(item.tid, "3057651");
    assert_eq!(item.title, "搜索到的主题");
    assert_eq!(item.author, "quejuwen");
    assert_eq!(item.uid, "377314");
    assert_eq!(item.forum, "Discovery");
    assert_eq!(item.time, "2026-11-13");
    assert_eq!(item.info, "最后回复");
    assert_eq!(list.search_id, "42");
    assert_eq!(list.max_page, 2);
    assert_eq!(list.form_hash, "deadbeef");
}

#[test]
fn simple_list_parses_notification_feed_and_sms() {
    let notify = r#"<html><body><ul class="feed"><li><div class="f_reply">
        <a href="space.php?uid=377314">quejuwen</a> 回复了
        <a href="viewthread.php?tid=3057651">你的主题</a>
        <img src="images/default/newpm.gif">
      </div></li></ul></body></html>"#;
    let list = parse_simple_list(
        &parse_document(notify),
        &Config::default(),
        SimpleListKind::Notify,
    );
    assert_eq!(list.items.len(), 1, "{list:?}");
    assert_eq!(list.items[0].tid, "3057651");
    assert_eq!(list.items[0].title, "你的主题");
    assert_eq!(list.items[0].author, "quejuwen");
    assert!(list.items[0].is_new);
    assert!(list.items[0].info.contains("回复了"));

    let sms = r#"<html><body><li class="s_clear">
        <p class="cite"><cite>alice</cite> 2026-11-13 10:00 <img src="newpm.gif"></p>
        <a href="space.php?uid=42"><img class="avatar" src="noavatar_small.gif"></a>
        <div class="summary">你好，这是一条私信内容</div>
      </li></body></html>"#;
    let list = parse_simple_list(
        &parse_document(sms),
        &Config::default(),
        SimpleListKind::Sms,
    );
    assert_eq!(list.items.len(), 1, "{list:?}");
    assert_eq!(list.items[0].author, "alice");
    assert_eq!(list.items[0].uid, "42");
    assert_eq!(list.items[0].info, "你好，这是一条私信内容");
    assert_eq!(list.items[0].time, "2026-11-13 10:00");
    assert!(list.items[0].is_new);
}

#[test]
fn error_message_strips_discuz_redirect_script() {
    let html = r#"<html><body><div class="alert_info">
      <h4>4D4Y 提示信息</h4>
      <p>欢迎您回来，quejuwen，现在将转入登录前页面。</p>
      <script>setTimeout("window.location.href ='index.php';", 1000);</script>
      <p>如果您的浏览器没有自动跳转，请点击此链接</p>
    </div></body></html>"#;
    let message = parse_error_message(&parse_document(html));
    assert!(message.contains("欢迎您回来"));
    assert!(!message.contains("setTimeout"), "脚本文字不该进状态栏：{message}");
    assert!(!message.contains("window.location"), "脚本文字不该进状态栏：{message}");
    assert!(message.chars().count() <= 120);
}

#[test]
fn search_url_encodes_chinese_in_gbk() {
    let client = ForumClient::new(Config::default(), MockTransport::new(vec![]));
    let url = client.search_url("中文", "", "2", false, 2);
    assert!(url.contains("srchtxt=%D6%D0%CE%C4"), "中文未按 GBK 编码：{url}");
    assert!(url.contains("srchtype=title"));
    assert!(url.contains("&page=2"));
    let full = client.search_url("中文", "quejuwen", "2", true, 1);
    assert!(full.contains("srchtype=fulltext"));
    assert!(!full.contains("&page="));
}

// ---------------------------------------------------------------- 论坛首页

/// 游客首页（GBK 原始字节）只开放「技术版区」8 个版块，
/// 每个版块要能解析出主题数 / 帖数 / 最后发表，供首页直接展示。
#[test]
fn guest_forum_index_parses_sections_and_last_posts() {
    let body = decode_body(GUEST_INDEX_GBK, "text/html; charset=GBK", ENCODE_GBK).unwrap();
    let index = parse_forum_index(&parse_document(&body));
    assert!(!index.logged_in, "游客首页不应显示登录态");
    assert!(!index.offline);
    assert_eq!(index.sections.len(), 1, "{index:?}");
    let tech = &index.sections[0];
    assert_eq!(tech.name, "技术版区");
    assert_eq!(tech.gid, 36);
    assert_eq!(tech.forums.len(), 8, "游客可见 8 个技术版块：{tech:?}");
    let geek = &tech.forums[0];
    assert_eq!(geek.fid, 7);
    assert_eq!(geek.name, "Geek Talks · 奇客怪谈");
    assert!(geek.description.contains("Bluetooth"), "版块描述未解析：{geek:?}");
    assert_eq!(geek.threads, "40473");
    assert_eq!(geek.posts, "552015");
    assert_eq!(geek.last_title, "测试有2只梨app");
    assert_eq!(geek.last_tid, "3473850");
    assert_eq!(geek.last_author, "城南花开");
    assert_eq!(geek.last_time, "2026-11-10 17:36");
    // 只有部分版块带「今日: N」。
    assert!(tech.forums.iter().any(|f| f.fid == 12 && f.today == "1"), "{tech:?}");
}

/// 登录态首页暴露更多分区（4D4Y / 技术版区 / 生活版区），
/// Discovery 在「生活版区」里 —— 这是首页必须动态解析的直接证据。
#[test]
fn logged_in_forum_index_has_more_sections() {
    let index = parse_forum_index(&parse_document(LOGGED_INDEX));
    assert!(index.logged_in);
    assert_eq!(index.sections.len(), 3, "{index:?}");
    assert_eq!(index.forum_count(), 14);
    assert_eq!(index.sections[0].name, "4D4Y");
    assert_eq!(index.sections[0].forums[0].fid, FID_BUY_SELL);
    let life = index.sections.iter().find(|s| s.name == "生活版区").unwrap();
    assert!(life.forums.iter().any(|f| f.fid == 2 && f.name == "Discovery"));
}

#[test]
fn local_forum_index_is_marked_as_offline_fallback() {
    let index = forum_core::config::local_forum_index();
    assert!(index.offline);
    assert_eq!(index.forum_count(), 21);
    assert_eq!(index.sections[0].forums[0].fid, 2);
    assert!(!index.is_empty());
}

/// `index.php` 无 cdb_sid 会被 WAF 挡 403；客户端先预热拿 sid 再取首页，
/// 且预热 cookie 要注入后续请求。
#[test]
fn forum_index_warms_sid_before_loading_homepage() {
    let page = r#"<html><body><div class="mainbox list"><h3><a href="index.php?gid=36">技术版区</a></h3>
      <table id="category_36"><tbody id="forum7"><tr><th><div class="left">
        <h2><a href="forumdisplay.php?fid=7">Geek Talks</a></h2><p>描述</p>
      </div></th><td class="forumnums"><em>1</em> / 2</td>
      <td class="forumlast"><p><a href="redirect.php?tid=99&amp;goto=lastpost">最后主题</a></p>
      <cite>alice - 2026-11-13 10:00</cite></td></tr></tbody></table></div></body></html>"#;
    let replies = vec![
        html_reply("<html><body>预热</body></html>", vec![("Set-Cookie", "cdb_sid=u8dmnf; path=/")]),
        html_reply(page, vec![]),
    ];
    let mock = MockTransport::new(replies);
    let client = ForumClient::new(Config::default(), mock);
    let index = block_on(client.forum_index()).unwrap();
    assert_eq!(index.forum_count(), 1);
    assert_eq!(index.sections[0].forums[0].fid, 7);
    let seen = client.transport.seen.lock().unwrap();
    assert!(seen.len() >= 2);
    assert!(seen[0].url.contains("logging.php?action=login"), "首次应预热 sid：{}", seen[0].url);
    assert!(seen[1].url.ends_with("index.php"));
    assert_eq!(seen[1].header("Cookie"), Some("cdb_sid=u8dmnf"));
}

/// 已有会话时登录门页会变成「欢迎您回来」跳转页且没有 formhash，
/// 旧实现把它当登录失败、界面停在登录页。现在应直接判定为已登录。
#[test]
fn login_with_existing_session_is_not_reported_as_failure() {
    let replies = vec![
        html_reply(
            r#"<html><body><div class="alert_info"><p>欢迎您回来，quejuwen，现在将转入登录前页面。</p></div></body></html>"#,
            vec![("Set-Cookie", "cdb_auth=abc123; path=/")],
        ),
        html_reply(
            r#"<html><body><div id="umenu"><cite><a href="space.php?uid=377314">quejuwen</a></cite></div></body></html>"#,
            vec![],
        ),
    ];
    let client = ForumClient::new(Config::default(), MockTransport::new(replies));
    let result = block_on(client.login("quejuwen", "secret", "0", "")).unwrap();
    assert!(result.ok, "已登录态不应被判为失败：{result:?}");
    assert_eq!(result.uid, "377314");
    assert_eq!(result.username, "quejuwen");
    assert!(client.is_logged_in());
}

// ---------------------------------------------------------------- 列表数据源 / 写操作 / 投票

/// Java `SimpleListJob.fetchSimpleList()` 的 11 种 type 收敛到 `ListSource`，
/// URL 与翻页参数必须逐项对齐，否则「我的回复」「关注」等入口会打到错页面。
#[test]
fn list_sources_build_urls_and_dom_kinds() {
    let ep = Endpoints::new(&Config::default().with_defaults());
    let b = "https://www.4d4y.com/forum/";
    assert_eq!(ListSource::Notifications.url(&ep), format!("{b}notice.php"));
    assert_eq!(ListSource::Sms.url(&ep), format!("{b}pm.php?filter=privatepm"));
    assert_eq!(
        ListSource::SmsDetail { uid: "42".into(), name: "alice".into() }.url(&ep),
        format!("{b}pm.php?daterange=5&uid=42")
    );
    assert_eq!(
        ListSource::Favorites { page: 1 }.url(&ep),
        format!("{b}my.php?item=favorites&type=thread")
    );
    assert_eq!(
        ListSource::Attention { page: 2 }.url(&ep),
        format!("{b}my.php?item=attention&type=thread&page=2")
    );
    assert_eq!(ListSource::MyPosts { page: 1 }.url(&ep), format!("{b}my.php?item=threads"));
    assert_eq!(
        ListSource::MyReplies { page: 3 }.url(&ep),
        format!("{b}my.php?item=posts&page=3")
    );
    assert_eq!(
        ListSource::NewPosts { search_id: String::new(), page: 1 }.url(&ep),
        format!("{b}search.php?srchfrom=86400&searchsubmit=yes")
    );
    assert!(ListSource::NewPosts { search_id: "77".into(), page: 2 }
        .url(&ep).contains("searchid=77"));
    assert!(ListSource::UserThreads {
        uid: "377314".into(), search_id: String::new(), page: 1
    }.url(&ep).contains("srchuid=377314"));

    // 中文搜索词必须按 GBK 字节编码；翻页要接 &page。
    let url = ListSource::Search {
        query: "中文".into(), author: String::new(), fid: "0".into(),
        full_text: false, page: 2,
    }.url(&ep);
    assert!(url.contains("srchtxt=%D6%D0%CE%C4"), "{url}");
    assert!(url.ends_with("&page=2"), "{url}");

    assert_eq!(ListSource::Notifications.kind(), SimpleListKind::Notify);
    assert_eq!(ListSource::Sms.kind(), SimpleListKind::Sms);
    assert_eq!(ListSource::Favorites { page: 1 }.kind(), SimpleListKind::Threads);
    assert_eq!(ListSource::MyPosts { page: 1 }.with_page(4).page(), 4);
    assert!(!ListSource::Sms.has_paging());
    assert!(ListSource::Favorites { page: 1 }.has_paging());
    assert!(ListSource::Sms.needs_login());
    assert_eq!(ListSource::SmsDetail { uid: "42".into(), name: "alice".into() }.title(), "与 alice 的私信");
}

const POLL_HTML: &str = r##"<html><body>
<form id="poll">
  <input name="formhash" value="abc123">
  <div class="pollinfo">多选投票: 最多可选 2 项 <a href="#">查看结果</a></div>
  <p class="polltimer">还剩 3 天</p>
  <div class="pollchart"><table><tbody>
    <tr><td class="selector"><input type="checkbox" value="1"></td>
        <td class="polloption"><label>选项 A</label></td></tr>
    <tr><td colspan="2"><em>10 票 (50%)</em></td></tr>
    <tr><td class="selector"><input type="checkbox" value="2"></td>
        <td class="polloption"><label>选项 B</label></td></tr>
    <tr><td colspan="2"><em>10 票 (50%)</em></td></tr>
    <tr><td colspan="2">共 20 人参与 <button type="submit">投票</button></td></tr>
  </tbody></table></div>
</form></body></html>"##;

#[test]
fn poll_form_parses_options_rates_and_multi_choice() {
    let poll = parse_poll(&parse_document(POLL_HTML)).expect("应解析出投票表单");
    assert!(poll.title.contains("多选投票"));
    assert!(poll.title.contains("还剩 3 天"));
    assert_eq!(poll.max_answer, 2);
    assert_eq!(poll.form_hash, "abc123");
    assert_eq!(poll.options.len(), 2);
    assert_eq!(poll.options[0].id, "1");
    assert_eq!(poll.options[0].text, "选项 A");
    assert!(poll.options[0].rates.contains("10 票"), "{:?}", poll.options[0]);
    assert_eq!(poll.options[1].id, "2");
    assert!(!poll.footer.contains("投票"), "按钮文字应被剔除：{}", poll.footer);
    // 投票挂在第 1 页的主题级，翻页不应重复出现。
    let d1 = parse_thread_detail(&parse_document(POLL_HTML), &Config::default(), "9", 1);
    let d2 = parse_thread_detail(&parse_document(POLL_HTML), &Config::default(), "9", 2);
    assert!(d1.poll.is_some());
    assert!(d2.poll.is_none());
}

#[test]
fn user_info_and_blacklist_pages_parse() {
    let profile = r#"<html><body>
      <div id="umenu"><a href="logging.php?action=logout&amp;formhash=hash77">退出</a></div>
      <div class="side"><div class="profile_side"><div class="avatar">
        <img src="/forum/uc_server/avatar.png"></div></div></div>
      <div id="profilecontent"><div class="itemtitle">
        <h1>alice</h1><img src="images/online.gif">
        <ul><li>alice (UID: 42)</li></ul>
      </div></div>
      <div class="main"><div class="s_clear"><ul class="commonlist">
        <li>注册日期 2020-01-01</li><li>积分 100</li>
      </ul></div></div>
      <h3 class="blocktitle">个人资料</h3><h3 class="blocktitle">统计信息</h3>
    </body></html>"#;
    let info = parse_user_info(&parse_document(profile), &Config::default());
    assert_eq!(info.username, "alice");
    assert_eq!(info.uid, "42");
    assert!(info.online);
    assert_eq!(info.form_hash, "hash77");
    assert!(info.detail.contains("注册日期 2020-01-01"));
    assert!(info.avatar_url.starts_with("https://www.4d4y.com/forum/"));

    let black = r#"<html><body><div class="blacklist"><ul class="commonlist">
      <li><a href="space.php?username=bob&amp;uid=2">bob</a></li>
      <li><a href="space.php?username=alice&amp;uid=42">alice</a></li>
      <li><a href="space.php?username=bob&amp;uid=2">bob</a></li>
      </ul></div><input name="formhash" value="blackhash"></body></html>"#;
    let list = parse_blacklist(&parse_document(black));
    assert_eq!(list.users, vec!["bob", "alice"]);
    assert_eq!(list.form_hash, "blackhash");
    assert!(!list.empty_hint);
}

#[test]
fn ajax_results_and_external_image_hosts() {
    assert!(parse_sms_result(r#"<div class="summary">ok</div>"#).0);
    assert!(parse_sms_result("<root>投票成功，感谢参与<script>x</script></root>").0);
    assert!(!parse_sms_result("<root>您还未登录</root>").0);
    assert!(!parse_sms_result(r#"<div class="alert_error">请勿重复投票</div>"#).0);

    let html = r#"<html><body>
      <img src="https://img02.4d4y.com/a.png">
      <img src="https://cdn.example.com/b.png">
      <img src="https://cdn.example.com/c.png">
      <img src="https://sub.example.org/d.png">
      <img src="images/smilies/default/smile.gif">
      <img src="//pic.4d4y.com/e.png">
    </body></html>"#;
    let allowed = ["*.4d4y.com", "img02.4d4y.com", "example.com"];
    let hosts = external_image_hosts(&parse_document(html), &allowed);
    assert_eq!(hosts, vec!["sub.example.org".to_string()], "{hosts:?}");
}

/// 写操作必须「先重取 formhash 再 POST」，且 POST 表单字段与 Java 对齐。
#[test]
fn sms_and_vote_posts_refresh_formhash_and_use_gbk() {
    let sms_replies = vec![
        html_reply(r#"<input id="formhash" name="formhash" value="smshash">"#, vec![]),
        html_reply(r#"<div class="summary">发送成功</div>"#, vec![]),
    ];
    let client = ForumClient::new(Config::default(), MockTransport::new(sms_replies));
    let result = block_on(client.send_sms("42", "alice", "中文私信")).unwrap();
    assert_eq!(result, "短消息发送成功");
    let seen = client.transport.seen.lock().unwrap();
    assert!(seen[0].url.contains("daterange=1&uid=42"), "{}", seen[0].url);
    assert!(seen[1].url.contains("action=send") && seen[1].url.contains("uid=42"), "{}", seen[1].url);
    let body = String::from_utf8_lossy(&seen[1].body);
    assert!(body.contains("formhash=smshash"), "{body}");
    assert!(body.contains("handlekey=pmreply"), "{body}");
    assert!(body.contains("%D6%D0%CE%C4"), "中文应按 GBK 编码：{body}");
    drop(seen);

    let vote_replies = vec![html_reply("<root>投票成功，感谢参与</root>", vec![])];
    let client2 = ForumClient::new(Config::default(), MockTransport::new(vote_replies));
    let result2 = block_on(client2.vote_poll(2, "3057651", &["2".into(), "1".into()], "pollhash")).unwrap();
    assert_eq!(result2, "投票成功");
    let seen2 = client2.transport.seen.lock().unwrap();
    assert!(seen2[0].url.contains("fid=2") && seen2[0].url.contains("tid=3057651"), "{}", seen2[0].url);
    let body2 = String::from_utf8_lossy(&seen2[0].body);
    assert!(body2.contains("formhash=pollhash"), "{body2}");
    // 选项 id 排序后依次出现。
    let a = body2.find("pollanswers%5B%5D=1").unwrap_or(usize::MAX);
    let b = body2.find("pollanswers%5B%5D=2").unwrap_or(usize::MAX);
    assert!(a < b, "pollanswers 应排序：{body2}");
}

// ---------------------------------------------------------------- 外链图片域名白名单

/// 域名策略：大小写 / 端口 / 协议相对 URL / 通配符 / 相对路径。
#[test]
fn image_host_policy_handles_wildcards_and_relative_urls() {
    assert_eq!(
        image_host("https://IMG02.4D4Y.COM:443/a.png").as_deref(),
        Some("img02.4d4y.com")
    );
    assert_eq!(
        image_host("//pic.example.com/a.png").as_deref(),
        Some("pic.example.com")
    );
    assert_eq!(image_host("attachments/a.jpg"), None);

    let allowed = ["*.4d4y.com", "cdn.example.com"];
    assert!(image_host_allowed("https://pic.4d4y.com/a.png", &allowed));
    assert!(image_host_allowed("attachments/a.jpg", &allowed));
    assert!(!image_host_allowed("https://evil.example.net/a.png", &allowed));
    assert_eq!(
        unlisted_image_hosts(
            [
                "https://evil.example.net/a.png",
                "https://evil.example.net/b.png",
                "https://cdn.example.com/c.png",
                "/forum/attachments/d.jpg",
            ],
            &allowed,
        ),
        vec!["evil.example.net".to_string()]
    );
}

/// `nsh.json` 是网络权限的唯一事实来源。新增真实样本后如果引入了新的
/// 外链图片域名，此测试会失败并列出域名，提示按 README 流程补白名单。
#[test]
fn sample_pages_have_no_unlisted_image_hosts() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../nsh.json")).expect("nsh.json 应可解析");
    let declared: Vec<String> = manifest["capabilities"]["network"]["hosts"]
        .as_array()
        .expect("nsh.json 应声明 network.hosts")
        .iter()
        .map(|v| v.as_str().expect("host 应为字符串").to_string())
        .collect();
    let allowed: Vec<&str> = declared.iter().map(String::as_str).collect();
    assert!(allowed.contains(&"*.4d4y.com"), "通配符规则不可删：{allowed:?}");

    let samples: Vec<(&str, String)> = vec![
        (
            "forumdisplay_fid2.html",
            decode_body(GATE_GBK, "text/html; charset=GBK", ENCODE_GBK).unwrap(),
        ),
        ("logged_forumdisplay.html", LOGGED_LIST.to_string()),
        ("logged_viewthread.html", LOGGED_DETAIL.to_string()),
        ("thread_row_sample.html", ROW_SAMPLE.to_string()),
        (
            "guest_index.html",
            decode_body(GUEST_INDEX_GBK, "text/html; charset=GBK", ENCODE_GBK).unwrap(),
        ),
        ("logged_index.html", LOGGED_INDEX.to_string()),
    ];
    for (name, body) in samples {
        let missing = external_image_hosts(&parse_document(&body), &allowed);
        assert!(
            missing.is_empty(),
            "{name} 出现未加入 nsh.json 的图片域名：{missing:?}"
        );
    }
}
