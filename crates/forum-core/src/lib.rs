//! forum-core —— HiPDA·nsh 版的平台无关业务内核。
//!
//! 4D4Y 跑的是 Discuz! 7.2，没有官方 JSON API，全部业务都是
//! 「GET/POST 一个 PHP 页面 → 解析 GBK HTML → 填充模型」。
//! 本 crate 只做协议、编解码与解析，不含任何平台/UI 依赖：
//!
//! - 网络通过 [`Transport`] trait 注入：原生端可用 reqwest，Web 端用 NSH 的 `fetch`。
//! - 因此本 crate 可编译到 `wasm32-unknown-unknown`，也能直接 `cargo test`。
//!
//! 所有规则来自 `~/code/hipda`（HiPDA·NG v5.0.05）源码，并用
//! 2026-11-13 的登录态真实页面校验过。
#![forbid(unsafe_code)]

pub mod actions;
pub mod auth;
pub mod config;
pub mod emoji;
pub mod encoding;
pub mod lists;
pub mod model;
pub mod parse;
pub mod post;
pub mod transport;

use std::fmt;

pub use auth::{processed_password, LoginResult};
pub use config::{
    query_param, Config, Endpoints, Forum, CONTENT_TYPE_FORM, ENCODE_GBK, FID_BUY_SELL,
    FID_DISCOVERY,
};
pub use lists::{
    ListSource, ITEM_ATTENTION, ITEM_FAVORITES,
};
pub use model::{
    Blacklist, Content, ContentKind, ForumEntry, ForumIndex, ForumSection, Notification, Poll,
    PollOption, Post, SearchQuery, Session, SimpleItem, SimpleList, Thread, ThreadDetail,
    ThreadList, UserInfo,
};
pub use parse::{
    external_image_hosts, image_host, image_host_allowed, parse_blacklist, parse_current_user,
    parse_document, parse_error_message, parse_form_hash, parse_forum_index, parse_login_state,
    parse_max_page, parse_notification, parse_poll, parse_security_questions, parse_sms_result,
    parse_thread_detail, parse_thread_list, parse_thread_types, parse_upload_response,
    parse_user_info, unlisted_image_hosts, Document, SecurityOption, SimpleListKind, ThreadType,
};
pub use transport::{ForumClient, HttpRequest, HttpResponse, Method, Response, Transport};

/// 内核错误。所有变体都带可展示的中文说明，便于 UI 直接呈现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 网络层错误（可重试）。
    Http(String),
    /// HTML 解析错误。
    Parse(String),
    /// 站点协议错误（formhash 缺失、上传被拒等）。
    Protocol(String),
    /// 本地存储错误。
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Http(s) => write!(f, "网络错误：{s}"),
            Error::Parse(s) => write!(f, "解析错误：{s}"),
            Error::Protocol(s) => write!(f, "协议错误：{s}"),
            Error::Io(s) => write!(f, "存储错误：{s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
