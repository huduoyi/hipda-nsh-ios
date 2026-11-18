//! 需要登录态的写操作：私信发送、黑名单增删、投票提交、用户资料。
//!
//! 与 `post.rs` 一样，**每次写操作前都重新 GET 一次页面拿 formhash**，
//! 不能复用旧值；POST 一律不重试，避免重复发送或重复投票。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::fill;
use crate::model::{Blacklist, UserInfo};
use crate::parse::{
    parse_blacklist, parse_document, parse_form_hash, parse_sms_result, parse_user_info,
};
use crate::transport::{ForumClient, Transport};
use crate::{Error, Result};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl<T: Transport> ForumClient<T> {
    /// 用户资料（`space.php?uid=`）。
    pub async fn user_info(&self, uid: &str) -> Result<UserInfo> {
        if uid.trim().is_empty() {
            return Err(Error::Protocol("缺少 uid".into()));
        }
        let resp = self.get(&format!("{}{}", self.ep.user_info, uid.trim())).await?;
        let info = parse_user_info(&parse_document(&resp.body), &self.config());
        if info.username.is_empty() && info.detail.is_empty() {
            return Err(Error::Protocol(
                "用户资料页未解析出内容，可能是登录态失效或站点改版".into(),
            ));
        }
        Ok(info)
    }

    /// 黑名单名单 + 同页 formhash。
    pub async fn blacklist(&self) -> Result<Blacklist> {
        let resp = self.get(&self.ep.view_black).await?;
        let list = parse_blacklist(&parse_document(&resp.body));
        if list.users.is_empty() && !list.empty_hint {
            return Err(Error::Protocol("黑名单页未解析出名单".into()));
        }
        Ok(list)
    }

    /// 加入 / 移出黑名单。先取同页 formhash，再 POST `formhash` + `user`。
    pub async fn set_blacklist(&self, username: &str, add: bool) -> Result<String> {
        let username = username.trim();
        if username.is_empty() {
            return Err(Error::Protocol("请输入用户名".into()));
        }
        let list = self.blacklist().await?;
        if list.form_hash.is_empty() {
            return Err(Error::Protocol("黑名单页缺少 formhash".into()));
        }
        let target = if add { &self.ep.add_black } else { &self.ep.del_black };
        let pairs = vec![
            ("formhash".into(), list.form_hash),
            ("user".into(), username.to_string()),
        ];
        let resp = self.post_form(target, &pairs).await?;
        let (ok, message) = parse_sms_result(&resp.body);
        if ok {
            Ok(format!(
                "已{}黑名单：{username}",
                if add { "加入" } else { "移出" }
            ))
        } else {
            Err(Error::Protocol(message))
        }
    }

    /// 发送私信。对应 `PostSmsAsyncTask`：先 GET `pm.php?daterange=1&uid=`
    /// 取 `input#formhash`，再 POST `handlekey=pmreply` 等字段。
    /// `uid` 为空时按用户名发送（额外带 `msgto`）。
    pub async fn send_sms(&self, uid: &str, username: &str, message: &str) -> Result<String> {
        if message.trim().is_empty() {
            return Err(Error::Protocol("私信内容不能为空".into()));
        }
        let (uid, username) = (uid.trim(), username.trim());
        if uid.is_empty() && username.is_empty() {
            return Err(Error::Protocol("缺少收件人".into()));
        }
        let prepare = format!("{}{}", self.ep.sms_prepare_post, uid);
        let resp = self.get(&prepare).await?;
        let form_hash = parse_form_hash(&parse_document(&resp.body));
        if form_hash.is_empty() {
            return Err(Error::Protocol("无法获取发送凭据（页面无 formhash）".into()));
        }
        let (target, mut pairs) = if !uid.is_empty() {
            (fill(&self.ep.sms_post_by_uid, &[("uid", uid)]), Vec::new())
        } else {
            (
                self.ep.sms_post_by_name.clone(),
                vec![("msgto".into(), username.to_string())],
            )
        };
        pairs.extend([
            ("formhash".into(), form_hash),
            ("lastdaterange".into(), now_ms().to_string()),
            ("handlekey".into(), "pmreply".into()),
            ("message".into(), message.to_string()),
        ]);
        let resp = self.post_form(&target, &pairs).await?;
        let (ok, text) = parse_sms_result(&resp.body);
        if ok {
            Ok("短消息发送成功".into())
        } else {
            Err(Error::Protocol(text))
        }
    }

    /// 提交投票。`PostHelper.MODE_VOTE_POLL`：URL 带 fid/tid，表单里
    /// `pollanswers[]` 按选项 id 排序后重复出现，其余字段与普通发帖一致。
    pub async fn vote_poll(
        &self,
        fid: i32,
        tid: &str,
        answers: &[String],
        form_hash: &str,
    ) -> Result<String> {
        if answers.is_empty() {
            return Err(Error::Protocol("请先选择投票选项".into()));
        }
        if form_hash.trim().is_empty() {
            return Err(Error::Protocol("投票表单缺少 formhash".into()));
        }
        let fid_str = fid.to_string();
        let target = fill(&self.ep.vote_poll, &[("fid", fid_str.as_str()), ("tid", tid)]);
        let mut pairs = vec![
            ("formhash".into(), form_hash.to_string()),
            ("posttime".into(), now_ms().to_string()),
            ("wysiwyg".into(), "0".into()),
            ("usesig".into(), "1".into()),
            ("message".into(), String::new()),
        ];
        let mut sorted = answers.to_vec();
        sorted.sort();
        for answer in sorted {
            pairs.push(("pollanswers[]".into(), answer));
        }
        let resp = self.post_form(&target, &pairs).await?;
        let (ok, text) = parse_sms_result(&resp.body);
        if ok {
            Ok("投票成功".into())
        } else {
            Err(Error::Protocol(text))
        }
    }
}
