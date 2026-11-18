//! 会话与草稿的本地持久化。
//!
//! 用 NSH 的 `nsh_core::storage::Storage`：原生端落到 app 数据目录、
//! Web 端落到 `localStorage`，六端同一套代码。它不经过 capabilities
//! 的网络/文件系统授权（这是 app 自有的键值数据），且读写是同步的。
//!
//! **只保存内核的会话快照**（cookie + 用户名 / uid），绝不保存明文密码。
//! 快照由 `ForumClient::session_snapshot` 生成、由 `restore_session` 校验，
//! 本模块不解释 cookie 内容。

use forum_core::transport::ForumClient;

use crate::transport::NshTransport;
use nsh_core::storage::Storage;

type Client = ForumClient<NshTransport>;

/// 版本化键名：格式变更时换 key，旧数据自然失效，不会误解析。
pub const SESSION_KEY: &str = "hipda.session.v1";
pub const DRAFT_KEY: &str = "hipda.draft.v1";
/// 记住的用户名：只用于登录页预填，不含任何凭据。
pub const USERNAME_KEY: &str = "hipda.username.v1";
/// 是否保持登录（默认开）。关掉后不落会话快照，下次必须重新输入。
pub const REMEMBER_KEY: &str = "hipda.remember.v1";

/// 启动时恢复会话。返回 `(用户名, uid)`；无快照、快照损坏或不含
/// `cdb_auth` 时返回 `None`，并顺手清掉损坏的键。
pub fn restore(client: &Client) -> Option<(String, String)> {
    let raw = Storage::get_item(SESSION_KEY)?;
    match client.restore_session(&raw) {
        Ok(true) => Some(client.identity()),
        Ok(false) => None,
        Err(_) => {
            // 快照损坏或被手工改坏：丢弃，不让用户卡在登录页之外。
            Storage::remove_item(SESSION_KEY);
            None
        }
    }
}

/// 登录页预填的用户名。只存名字，不存密码。
pub fn remembered_username() -> String {
    Storage::get_item(USERNAME_KEY).unwrap_or_default()
}

pub fn set_remembered_username(name: &str) {
    let name = name.trim();
    if name.is_empty() {
        Storage::remove_item(USERNAME_KEY);
    } else {
        let _ = Storage::set_item(USERNAME_KEY, name);
    }
}

/// 是否保持登录。默认开：用户要的是「下次打开还在登录态」。
pub fn remember() -> bool {
    Storage::get_item(REMEMBER_KEY).is_none_or(|v| v != "0")
}

/// 切换「保持登录」。关掉时顺手删掉已存的会话，避免下次仍自动登录。
pub fn set_remember(on: bool) {
    let _ = Storage::set_item(REMEMBER_KEY, if on { "1" } else { "0" });
    if !on {
        Storage::remove_item(SESSION_KEY);
    }
}

/// 登录成功后保存会话。存储配额不足时返回错误，但不影响本次登录。
pub fn save(client: &Client) -> Result<(), String> {
    let json = client.session_snapshot().map_err(|e| e.to_string())?;
    Storage::set_item(SESSION_KEY, json).map_err(|e| e.to_string())
}

/// 退出登录：同时清掉会话、草稿与内核内存里的 cookie。
pub fn logout(client: &Client) {
    Storage::remove_item(SESSION_KEY);
    Storage::remove_item(DRAFT_KEY);
    client.logout();
}

/// 回帖草稿。编辑时写入，发布成功后清除。
pub fn draft() -> String {
    Storage::get_item(DRAFT_KEY).unwrap_or_default()
}

pub fn save_draft(text: &str) {
    if text.is_empty() {
        Storage::remove_item(DRAFT_KEY);
    } else {
        let _ = Storage::set_item(DRAFT_KEY, text);
    }
}

pub fn clear_draft() {
    Storage::remove_item(DRAFT_KEY);
}
