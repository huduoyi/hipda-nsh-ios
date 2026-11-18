//! 发帖 / 回帖 / 收藏 / 图片上传。对应 `PostHelper`、`UploadImgHelper`、`FavoriteHelper`。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{fill, query_param, Endpoints};
use crate::emoji::bbcode_image;
use crate::parse::{
    parse_document, parse_error_message, parse_simple_list, parse_upload_response, SimpleListKind,
};
use crate::transport::{ForumClient, Transport};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PostMode {
    #[default]
    Reply,
    New,
    Edit,
    NewPoll,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostRequest {
    pub mode: PostMode,
    pub fid: String,
    pub tid: String,
    /// 编辑时必填。
    pub pid: String,
    pub form_hash: String,
    pub subject: String,
    pub message: String,
    pub type_id: String,
    /// 上传成功后拿到的图片 ID。
    pub attachments: Vec<String>,
    pub delete_attachments: Vec<String>,
    pub poll_answers: Vec<String>,
    pub attention_add: bool,
    pub notice_author: bool,
    pub notice_auth_msg: String,
    pub use_signature: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostResult {
    pub ok: bool,
    pub message: String,
    pub final_url: String,
    pub tid: String,
}

/// 回帖正文的最小字数，与网页端 `var postminchars = parseInt('5')` 一致。
pub const MIN_POST_CHARS: usize = 5;

/// 回帖正文校验。
///
/// 网页端用 JS 的 `str.length` 计数（中文算 1 个字符），所以这里也按**字符**
/// 而不是字节算 —— 否则「你好啊」会被本地拦下，而站点其实接受。
/// 在提交前拦一道，用户不用等一次注定失败的往返才知道字数不够。
pub fn validate_message(message: &str) -> std::result::Result<(), String> {
    let count = message.trim().chars().count();
    if count == 0 {
        return Err("回帖内容不能为空".into());
    }
    if count < MIN_POST_CHARS {
        return Err(format!(
            "回帖至少 {MIN_POST_CHARS} 个字，当前 {count} 个字"
        ));
    }
    Ok(())
}

/// 构造表单字段，逐条对应 `PostHelper.java:146-190`。
/// `posttime` 由调用方传入（毫秒），使函数可测试且请求可复现。
pub fn build_post_values(req: &PostRequest, posttime_ms: i64) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vec![
        ("formhash".into(), req.form_hash.clone()),
        ("posttime".into(), posttime_ms.to_string()),
        ("wysiwyg".into(), "0".into()),
        ("message".into(), req.message.clone()),
    ];
    if req.use_signature {
        v.push(("usesig".into(), "1".into()));
    }
    for a in &req.delete_attachments {
        v.push(("attachdel[]".into(), a.clone()));
    }
    for a in &req.attachments {
        v.push(("attachnew[][description]".into(), a.clone()));
    }
    if !req.subject.is_empty() {
        v.push(("subject".into(), req.subject.clone()));
    }
    if !req.type_id.is_empty() {
        v.push(("typeid".into(), req.type_id.clone()));
    }
    for ans in &req.poll_answers {
        v.push(("pollanswers[]".into(), ans.clone()));
    }
    if req.attention_add {
        v.push(("attention_add".into(), "1".into()));
    }
    if req.notice_author {
        v.push(("noticeauthor".into(), "1".into()));
        v.push(("noticeauthormsg".into(), req.notice_auth_msg.clone()));
        v.push(("noticetrimstr".into(), String::new()));
    }
    v
}

pub fn post_target_url(ep: &Endpoints, req: &PostRequest) -> Result<String> {
    match req.mode {
        PostMode::Reply => {
            if req.tid.is_empty() { return Err(Error::Protocol("回帖缺少 tid".into())); }
            // Discuz 的 `post.php` 只有带上 `replysubmit=yes` 才**处理**提交；
            // 不带的 POST 只会把编辑页原样返回（HTTP 200、不跳转），旧实现因此
            // 永远拿不到 viewthread 跳转、被判成「发帖结果未知」—— 也就是
            // 用户看到的「回帖功能不能用」。`fid` 一并带上，与网页端表单一致。
            let fid = if req.fid.is_empty() {
                String::new()
            } else {
                format!("&fid={}", req.fid)
            };
            Ok(format!(
                "{}{}{}&replysubmit=yes&infloat=yes&handlekey=fastpost",
                ep.reply, req.tid, fid
            ))
        }
        PostMode::New => {
            if req.fid.is_empty() { return Err(Error::Protocol("发新帖缺少 fid".into())); }
            Ok(format!("{}{}", ep.new_thread, req.fid))
        }
        PostMode::NewPoll => {
            if req.fid.is_empty() { return Err(Error::Protocol("发投票缺少 fid".into())); }
            Ok(format!("{}{}&special=1", ep.new_thread, req.fid))
        }
        PostMode::Edit => {
            if req.pid.is_empty() { return Err(Error::Protocol("编辑缺少 pid".into())); }
            Ok(format!("{}&pid={}", ep.edit, req.pid))
        }
    }
}

/// 在原客户端尾部追加客户端标识（`Utils.addTail`）。
pub fn append_tail(message: &str, tail: &str, enabled: bool) -> String {
    if !enabled || tail.is_empty() {
        return message.to_string();
    }
    format!("{}\n\n{tail}", message.trim_end_matches('\n'))
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

impl<T: Transport> ForumClient<T> {
    /// 提交发帖/回帖。Discuz 成功后 302 到 `viewthread.php`，
    /// 失败则返回含 `alert_error` 的提示页，因此用 FinalURL + 错误页双重判定。
    pub async fn submit_post(&self, req: &PostRequest) -> Result<PostResult> {
        let target = post_target_url(&self.ep, req)?;
        if req.form_hash.is_empty() {
            return Err(Error::Protocol("缺少 formhash，请先 GET 发帖页".into()));
        }
        let resp = self.post_form(&target, &build_post_values(req, now_ms())).await?;
        let doc = parse_document(&resp.body);
        let mut result = PostResult {
            final_url: resp.final_url.clone(),
            message: parse_error_message(&doc),
            ..Default::default()
        };
        result.ok = resp.final_url.contains("viewthread.php") && result.message.is_empty();
        if !result.ok && result.message.is_empty() {
            result.message = "发帖结果未知，请检查帖子列表确认".into();
        }
        if result.ok {
            result.tid = query_param(&resp.final_url, "tid");
            if result.tid.is_empty() {
                result.tid = req.tid.clone();
            }
        }
        Ok(result)
    }

    /// 收藏 / 关注 / 取消（`FavoriteHelper.java:261-277`）。
    pub async fn favorite(&self, form_hash: &str, tid: &str, item: &str, add: bool) -> Result<()> {
        let kv = [("item", item), ("tid", tid)];
        let template = if add { &self.ep.favorite_add } else { &self.ep.favorite_remove };
        let target = fill(template, &kv);
        let pairs = vec![
            ("formhash".into(), form_hash.to_string()),
            (
                if item == "favorites" { "favsubmit" } else { "attentionsubmit" }.into(),
                "true".into(),
            ),
        ];
        let resp = self.post_form(&target, &pairs).await?;
        if resp.body.contains("失败") || resp.body.contains("error") {
            return Err(Error::Protocol(format!("收藏操作失败：{}", &resp.body[..resp.body.len().min(200)])));
        }
        Ok(())
    }

    /// 上传图片并返回 `[attachimg]ID[/attachimg]` BBCode。
    /// 站点响应格式：`DISCUZUPLOAD|0|<id>|1`。
    pub async fn upload_image(
        &self,
        form_hash: &str,
        filename: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<String> {
        const BOUNDARY: &str = "hipda-nsh-upload-boundary";
        let (_, uid) = self.identity();
        let mut buf: Vec<u8> = Vec::with_capacity(data.len() + 512);
        let field = |buf: &mut Vec<u8>, name: &str, value: &str| {
            buf.extend_from_slice(
                format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
            );
        };
        field(&mut buf, "uid", &uid);
        field(&mut buf, "hash", form_hash);
        field(&mut buf, "Filename", filename);
        buf.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"Filedata\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
            ).as_bytes(),
        );
        buf.extend_from_slice(data);
        buf.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

        let resp = self.post_multipart(&self.ep.upload_image, BOUNDARY, &buf).await?;
        let id = parse_upload_response(&resp.body)?;
        Ok(bbcode_image(&id))
    }

    /// 拉取主题列表（第 `page` 页）。游客可读的版块不要求登录，
    /// 是否放行完全由站点返回的页面决定（2026-11-13 实测 fid=7 可读）。
    /// 版块主题列表。`by_dateline` 对应原客户端的「按发表排序」菜单项：
    /// Discuz 默认按最后回复时间排，`orderby=dateline` 才是按发帖时间。
    pub async fn thread_list(
        &self,
        fid: i32,
        page: i32,
        by_dateline: bool,
    ) -> Result<crate::model::ThreadList> {
        let order = if by_dateline { "&orderby=dateline" } else { "" };
        let url = format!("{}{fid}&page={page}{order}", self.ep.thread_list);
        let resp = self.get(&url).await?;
        let doc = parse_document(&resp.body);
        guest_gate(self, &doc)?;
        Ok(crate::parse::parse_thread_list(&doc, &self.config(), page))
    }

    /// 拉取帖子详情（第 `page` 页）。技术版区主题游客同样可读。
    pub async fn thread_detail(&self, tid: &str, page: i32) -> Result<crate::model::ThreadDetail> {
        let url = format!("{}{tid}&page={page}", self.ep.detail_list);
        let resp = self.get(&url).await?;
        let doc = parse_document(&resp.body);
        guest_gate(self, &doc)?;
        Ok(crate::parse::parse_thread_detail(&doc, &self.config(), tid, page))
    }

    /// 拉取通用列表页（通知 / 私信 / 搜索 / 收藏 / 我的帖子 …）。
    /// URL 由调用方给出，内核只负责 GBK 解码与按 `kind` 选择解析规则。
    pub async fn simple_list(
        &self,
        url: &str,
        kind: SimpleListKind,
    ) -> Result<crate::model::SimpleList> {
        let resp = self.get(url).await?;
        let doc = parse_document(&resp.body);
        guest_gate(self, &doc)?;
        Ok(parse_simple_list(&doc, &self.config(), kind))
    }

    /// 通知列表（`notice.php` → `ul.feed li`）。
    pub async fn notifications(&self) -> Result<crate::model::SimpleList> {
        self.simple_list(&self.ep.thread_notify, SimpleListKind::Notify)
            .await
    }

    /// 构造搜索 URL。论坛是 GBK，关键词与作者名必须按 GBK 字节百分号编码，
    /// 直接拼 UTF-8 会让中文搜索词变成乱码（与 POST 表单同一约束）。
    pub fn search_url(
        &self,
        query: &str,
        author: &str,
        fid: &str,
        full_text: bool,
        page: i32,
    ) -> String {
        let enc = |s: &str| {
            crate::encoding::percent_encode(&crate::encoding::encode_value(s, crate::ENCODE_GBK))
        };
        let q = enc(query);
        let a = enc(author);
        let mut url = fill(
            &self.ep.search,
            &[
                ("srchtype", if full_text { "fulltext" } else { "title" }),
                ("srchtxt", q.as_str()),
                ("srchuname", a.as_str()),
                ("fid", fid),
            ],
        );
        if page > 1 {
            url.push_str(&format!("&page={page}"));
        }
        url
    }
}

/// 游客访问受限版块 / 主题时，Discuz 会返回带 `div.alert_info` 的
/// 「您还未登录」提示页。只在「未登录 + 明确说未登录」时转成可展示错误，
/// 避免把「搜索无结果」之类的普通提示也当成门禁，也避免把登录成功后的
/// 「欢迎您回来」跳转页误判为失败。
fn guest_gate<T: Transport>(client: &ForumClient<T>, doc: &crate::parse::Document) -> Result<()> {
    if client.is_logged_in() {
        return Ok(());
    }
    let message = parse_error_message(doc);
    if message.contains("未登录") {
        return Err(Error::Protocol(message));
    }
    Ok(())
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn message_needs_five_characters() {
        assert!(validate_message("").is_err());
        assert!(validate_message("   ").is_err(), "全空白也算空");
        assert!(validate_message("五个字啊啊").is_ok(), "5 个汉字应通过");
        assert!(validate_message("四字啊").is_err(), "4 个字要被拦下");
        // 中文按字符算，不按字节：3 个汉字在站点侧（PHP strlen）其实够 5 字节，
        // 这里按网页端 JS 规则算 3 个字，同样拦下，口径与浏览器一致。
        assert!(validate_message("三个字").is_err());
        assert!(validate_message("abcde").is_ok());
        assert!(validate_message("  abcde  ").is_ok(), "首尾空白不计数");
    }

    #[test]
    fn reply_url_carries_submit_flag() {
        let ep = Endpoints::new(&crate::Config::default());
        let req = PostRequest {
            mode: PostMode::Reply,
            tid: "42".into(),
            fid: "7".into(),
            ..Default::default()
        };
        let url = post_target_url(&ep, &req).unwrap();
        assert!(url.contains("tid=42"), "{url}");
        assert!(url.contains("fid=7"), "{url}");
        assert!(url.contains("replysubmit=yes"), "少了这个参数站点不会处理提交：{url}");
    }
}
