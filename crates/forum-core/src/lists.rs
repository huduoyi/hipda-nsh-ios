//! 通用列表数据源。把 Java `SimpleListJob` 的 11 种 type 收敛为一个枚举，
//! URL 拼装、GBK 编码、DOM 规则选择都在内核完成，UI 只传一个 `ListSource`。
//!
//! 对应 `SimpleListJob.fetchSimpleList()` 与 `HiParser.parseSimpleList()`：
//! 主题型列表（搜索 / 收藏 / 关注 / 我的帖子 / 我的回复 / 最新帖子）走
//! `tbody tr th.subject a`；通知走 `ul.feed li`；私信与私信详情走 `li.s_clear`。

use crate::config::{fill, Endpoints, ENCODE_GBK};
use crate::encoding::{encode_value, percent_encode};
use crate::model::SimpleList;
use crate::parse::SimpleListKind;
use crate::transport::{ForumClient, Transport};
use crate::Result;

/// `FavoriteHelper.TYPE_FAVORITE` / `TYPE_ATTENTION`。
pub const ITEM_FAVORITES: &str = "favorites";
pub const ITEM_ATTENTION: &str = "attention";

/// 一个可翻页的列表数据源。`page` 从 1 开始；没有分页的源忽略该字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ListSource {
    #[default]
    Notifications,
    Search {
        query: String,
        author: String,
        fid: String,
        full_text: bool,
        /// Discuz 的 `srchfrom` 秒数：0 不限，86400 一天内。
        srchfrom: i64,
        page: i32,
    },
    /// 私信会话列表（`pm.php?filter=privatepm`）。
    Sms,
    /// 与某人的私信往来（`pm.php?daterange=5&uid=`）。
    SmsDetail { uid: String, name: String },
    Favorites { page: i32 },
    Attention { page: i32 },
    MyPosts { page: i32 },
    MyReplies { page: i32 },
    NewPosts { search_id: String, page: i32 },
    UserThreads { uid: String, search_id: String, page: i32 },
}

impl ListSource {
    pub fn title(&self) -> String {
        match self {
            Self::Notifications => "通知".into(),
            Self::Search { query, .. } => format!("搜索：{query}"),
            Self::Sms => "私信".into(),
            Self::SmsDetail { name, .. } if !name.is_empty() => format!("与 {name} 的私信"),
            Self::SmsDetail { .. } => "私信详情".into(),
            Self::Favorites { .. } => "我的收藏".into(),
            Self::Attention { .. } => "我的关注".into(),
            Self::MyPosts { .. } => "我的帖子".into(),
            Self::MyReplies { .. } => "我的回复".into(),
            Self::NewPosts { .. } => "最新帖子".into(),
            Self::UserThreads { .. } => "该用户的帖子".into(),
        }
    }

    pub fn kind(&self) -> SimpleListKind {
        match self {
            Self::Notifications => SimpleListKind::Notify,
            Self::Sms | Self::SmsDetail { .. } => SimpleListKind::Sms,
            _ => SimpleListKind::Threads,
        }
    }

    pub fn page(&self) -> i32 {
        match self {
            Self::Search { page, .. }
            | Self::Favorites { page }
            | Self::Attention { page }
            | Self::MyPosts { page }
            | Self::MyReplies { page }
            | Self::NewPosts { page, .. }
            | Self::UserThreads { page, .. } => *page,
            _ => 1,
        }
    }

    /// 返回同类型、指定页码的数据源。无分页的源原样返回。
    pub fn with_page(&self, page: i32) -> Self {
        let page = page.max(1);
        match self {
            Self::Search { query, author, fid, full_text, srchfrom, .. } => Self::Search {
                query: query.clone(), author: author.clone(), fid: fid.clone(),
                full_text: *full_text, srchfrom: *srchfrom, page,
            },
            Self::Favorites { .. } => Self::Favorites { page },
            Self::Attention { .. } => Self::Attention { page },
            Self::MyPosts { .. } => Self::MyPosts { page },
            Self::MyReplies { .. } => Self::MyReplies { page },
            Self::NewPosts { search_id, .. } => Self::NewPosts { search_id: search_id.clone(), page },
            Self::UserThreads { uid, search_id, .. } => Self::UserThreads {
                uid: uid.clone(), search_id: search_id.clone(), page,
            },
            other => other.clone(),
        }
    }

    /// 搜索结果翻页必须用 `searchid` 重放，否则站点会重新搜索并返回第 1 页。
    pub fn with_search_id(&self, search_id: &str) -> Self {
        match self {
            Self::NewPosts { page, .. } => Self::NewPosts {
                search_id: search_id.to_string(), page: *page,
            },
            Self::UserThreads { uid, page, .. } => Self::UserThreads {
                uid: uid.clone(), search_id: search_id.to_string(), page: *page,
            },
            Self::Search { query, author, fid, full_text, srchfrom, page } => Self::Search {
                query: query.clone(), author: author.clone(), fid: fid.clone(),
                full_text: *full_text, srchfrom: *srchfrom, page: *page,
            },
            other => other.clone(),
        }
    }

    /// 生成请求 URL。中文关键词必须按 GBK 字节百分号编码（站点是 Discuz! 7.2）。
    pub fn url(&self, ep: &Endpoints) -> String {
        let page = self.page();
        let mut url = match self {
            Self::Notifications => ep.thread_notify.clone(),
            Self::Sms => ep.sms.clone(),
            Self::SmsDetail { uid, .. } => format!("{}{uid}", ep.sms_detail),
            Self::Search { query, author, fid, full_text, srchfrom, .. } => {
                let q = gbk(query);
                let a = gbk(author);
                let from = srchfrom.to_string();
                fill(
                    &ep.search,
                    &[
                        ("srchtype", if *full_text { "fulltext" } else { "title" }),
                        ("srchtxt", q.as_str()),
                        ("srchuname", a.as_str()),
                        ("fid", fid.as_str()),
                        ("srchfrom", from.as_str()),
                    ],
                )
            }
            Self::Favorites { .. } => fill(&ep.favorites, &[("item", ITEM_FAVORITES)]),
            Self::Attention { .. } => fill(&ep.favorites, &[("item", ITEM_ATTENTION)]),
            Self::MyPosts { .. } => ep.my_post.clone(),
            Self::MyReplies { .. } => ep.my_reply.clone(),
            Self::NewPosts { search_id, .. } => {
                if search_id.is_empty() { ep.new_posts.clone() }
                else { fill(&ep.search_by_id, &[("searchid", search_id.as_str())]) }
            }
            Self::UserThreads { uid, search_id, .. } => {
                if search_id.is_empty() {
                    fill(&ep.search_user_thread, &[("srchuid", uid.as_str())])
                } else {
                    fill(&ep.search_by_id, &[("searchid", search_id.as_str())])
                }
            }
        };
        if page > 1 && self.has_paging() {
            url.push_str(&format!("&page={page}"));
        }
        url
    }

    pub fn has_paging(&self) -> bool {
        !matches!(self, Self::Notifications | Self::Sms | Self::SmsDetail { .. })
    }

    /// 除了公开搜索，其余入口都需要登录态；UI 据此给出引导而不是发无效请求。
    pub fn needs_login(&self) -> bool {
        !matches!(self, Self::Search { .. } | Self::NewPosts { .. })
    }
}

fn gbk(s: &str) -> String {
    percent_encode(&encode_value(s, ENCODE_GBK))
}

impl<T: Transport> ForumClient<T> {
    /// 一步完成「构造 URL → GET → GBK 解码 → 按 DOM 规则解析」。
    pub async fn list(&self, source: &ListSource) -> Result<SimpleList> {
        let url = source.url(&self.ep);
        self.simple_list(&url, source.kind()).await
    }
}
