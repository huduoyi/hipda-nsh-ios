//! 登录协议。字段与 `LoginHelper.doLogin()` 一致。

use md5::{Digest, Md5};

use crate::config::DEFAULT_FORUMS;
use crate::parse::{parse_current_user, parse_document, parse_error_message, parse_form_hash, parse_login_state};
use crate::transport::ForumClient;
use crate::transport::Transport;
use crate::{Error, Result};

/// Discuz 登录响应是 XML 片段，原客户端用两个硬编码中文串判定结果
/// （`res/values/hipda.xml:4-5`）。
pub const LOGIN_SUCCESS_MARK: &str = "欢迎您回来";
pub const LOGIN_FAIL_MARK: &str = "登录失败";
/// 30 天，对应登录表单「记住我的登录状态」。
pub const DEFAULT_COOKIE_TIME: &str = "2592000";

/// 复刻 `LoginHelper.processedPassword()`：密码先转义再 MD5，
/// 已是 32 位（MD5 hex）则原样提交，避免二次哈希。
///
/// 源码细节：Java 里 `.replace("'", "\'")` 两侧都是单引号字符，实际是 no-op；
/// 真正生效的是反斜杠与双引号两处转义。这里按实际行为复刻。
pub fn processed_password(pass: &str) -> String {
    if pass.is_empty() || pass.len() == 32 {
        return pass.to_string();
    }
    let s = pass.replace('\\', "\\\\").replace('"', "\\\"");
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    let sum = hasher.finalize();
    let mut out = String::with_capacity(32);
    for b in sum {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginResult {
    pub ok: bool,
    pub message: String,
    pub form_hash: String,
    pub username: String,
    pub uid: String,
    /// 网络/未知错误可重试；密码错误应中止，避免触发论坛锁定。
    pub retryable: bool,
}

impl<T: Transport> ForumClient<T> {
    /// 按 Discuz 协议登录。成功后会再请求一次版块页，
    /// 从 `#umenu` 补齐 uid 并验证 cookie 生效。
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        question_id: &str,
        answer: &str,
    ) -> Result<LoginResult> {
        let mut result = LoginResult {
            username: username.to_string(),
            ..Default::default()
        };
        let page = self.get(&self.ep.login_get_form_hash).await
            .map_err(|e| Error::Http(format!("获取登录页失败：{e}")))?;
        let doc = parse_document(&page.body);
        let form_hash = parse_form_hash(&doc);
        if form_hash.is_empty() {
            // 已有有效会话时 Discuz 会把 logging.php 重定向成「欢迎您回来，
            // 现在将转入登录前页面」的跳转页，页面里没有 formhash。旧实现把这
            // 段论坛提示当成登录失败，界面因此停在登录页不跳转（2026-11-13
            // 实机复现）。此时应判定为「已经登录」，不重复提交密码。
            if page.body.contains(LOGIN_SUCCESS_MARK) || parse_login_state(&doc) {
                result.ok = true;
                result.message = "已处于登录态".into();
                if self.refresh_identity().await.is_ok() {
                    let (name, uid) = self.identity();
                    if !name.is_empty() {
                        result.username = name;
                    }
                    result.uid = uid;
                } else {
                    result.username = username.to_string();
                    self.set_identity(username, "");
                }
                return Ok(result);
            }
            let msg = parse_error_message(&doc);
            result.message = if msg.is_empty() {
                "无法获取登录凭据（页面无 formhash）".into()
            } else {
                msg
            };
            return Ok(result);
        }
        result.form_hash = form_hash.clone();

        let pairs = vec![
            ("m_formhash".into(), form_hash),
            ("referer".into(), format!("{}index.php", self.cfg.base_url())),
            ("loginfield".into(), "username".into()),
            ("username".into(), username.to_string()),
            ("password".into(), processed_password(password)),
            ("questionid".into(), question_id.to_string()),
            ("answer".into(), answer.to_string()),
            ("cookietime".into(), DEFAULT_COOKIE_TIME.to_string()),
        ];
        let resp = self.post_form(&self.ep.login_submit, &pairs).await
            .map_err(|e| Error::Http(format!("提交登录失败：{e}")))?;

        if resp.body.contains(LOGIN_SUCCESS_MARK) {
            result.ok = true;
            result.message = "登录成功".into();
        } else if let Some(i) = resp.body.find(LOGIN_FAIL_MARK) {
            let tail = &resp.body[i..];
            let msg = tail.find('次').map(|j| &tail[..j + '次'.len_utf8()]).unwrap_or(tail);
            result.message = msg.trim().to_string();
        } else {
            result.message = "登录失败，未知错误".into();
            result.retryable = true;
        }
        if !result.ok {
            return Ok(result);
        }

        self.set_identity(username, "");
        if self.refresh_identity().await.is_ok() {
            result.uid = self.identity().1;
        }
        Ok(result)
    }

    /// 从当前登录态页面的 `#umenu` 读取用户名与 uid。
    pub async fn refresh_identity(&self) -> Result<()> {
        let fid = DEFAULT_FORUMS.first().copied().unwrap_or(2);
        let url = format!("{}{fid}", self.ep.thread_list);
        for _ in 0..3 {
            let resp = self.get(&url).await?;
            let doc = parse_document(&resp.body);
            if parse_login_state(&doc) {
                let (username, uid) = parse_current_user(&doc);
                self.set_identity(&username, &uid);
                return Ok(());
            }
            // 刚登录完成时，论坛可能先返回「欢迎您回来…现在将转入登录前页面」
            // 的跳转提示页而非版块页；GET 幂等，重试到拿到真实页面为止。
            if !resp.body.contains("现在将转入登录前页面") {
                break;
            }
        }
        Err(Error::Protocol("页面未显示登录态，cookie 可能已失效".into()))
    }
}
