//! 数据模型。对应 Java 的 24 个 bean，收敛为少数结构化类型。

/// 主题列表中的一行（`ThreadBean`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thread {
    pub tid: String,
    pub title: String,
    pub title_color: String,
    pub sticky: bool,
    pub poll: bool,
    pub thread_type: String,
    pub author: String,
    pub author_id: String,
    pub avatar_url: String,
    pub time_create: String,
    pub time_update: String,
    pub last_post_by: String,
    pub reply_count: i64,
    pub view_count: i64,
    pub with_picture: bool,
    pub is_new: bool,
    pub max_page: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadList {
    /// 当前登录用户 uid，来自 `#umenu cite a`。
    pub uid: String,
    pub threads: Vec<Thread>,
    pub page: i32,
    pub max_page: i32,
}

/// 论坛首页（`index.php`）的一个版块。Discuz! 7.2 老模板：
/// `table[id^=category_] tbody[id^=forum]`。游客与登录态可见的分区不同，
/// 因此首页必须动态解析，不能用客户端的本地版块表代替。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForumEntry {
    pub fid: i32,
    pub name: String,
    pub description: String,
    /// `td.forumnums` 的「主题数 / 帖数」，保留原样文本便于直接展示。
    pub threads: String,
    pub posts: String,
    /// 今日新帖数（仅部分版块有 `<em>(今日: N)</em>`）。
    pub today: String,
    /// 最后发表的主题标题。
    pub last_title: String,
    pub last_tid: String,
    pub last_author: String,
    pub last_time: String,
}

/// 论坛首页的一个分区（如「技术版区」「生活版区」）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForumSection {
    pub gid: i32,
    pub name: String,
    pub forums: Vec<ForumEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForumIndex {
    pub sections: Vec<ForumSection>,
    /// 页面 `div#umenu` 是否显示登录态。游客首页只开放部分分区。
    pub logged_in: bool,
    /// 站点首页不可达时由本地版块表兜底，界面据此提示数据来源。
    pub offline: bool,
}

impl ForumIndex {
    pub fn forum_count(&self) -> usize {
        self.sections.iter().map(|s| s.forums.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.forum_count() == 0
    }
}

/// 楼层正文节点类型。原客户端用 ~10 个 `ContentAbs` 子类，这里收敛为一个枚举。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContentKind {
    #[default]
    Text,
    Link,
    Image,
    Attach,
    Quote,
    GotoFloor,
    Email,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    pub kind: ContentKind,
    pub text: String,
    pub url: String,
    pub thumb_url: String,
    pub author: String,
    pub time: String,
    pub post_id: String,
    pub tid: String,
    pub floor: i32,
    pub file_size: i64,
}

/// 一层楼（`DetailBean`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Post {
    pub post_id: String,
    /// 楼层号在删帖后会错位，**不能当主键**；主键用 `post_id`。
    pub floor: i32,
    pub author: String,
    pub author_id: String,
    pub avatar_url: String,
    pub time_post: String,
    pub warned: bool,
    pub signature: String,
    pub contents: Vec<Content>,
    /// 块级正文，界面按块渲染；`contents` 继续供复制/搜索使用。
    pub blocks: Vec<Block>,
    pub poll: Option<Poll>,
}

impl Post {
    /// 可复制的纯文本（`DetailBean.Contents.getCopyText()`）。
    pub fn copy_text(&self) -> String {
        self.contents
            .iter()
            .map(|c| match c.kind {
                ContentKind::Text | ContentKind::Link | ContentKind::Quote
                | ContentKind::GotoFloor | ContentKind::Email => c.text.as_str(),
                ContentKind::Image => "[图片]",
                ContentKind::Attach => "[附件]",
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn images(&self) -> Vec<&Content> {
        self.contents.iter().filter(|c| c.kind == ContentKind::Image).collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Poll {
    pub title: String,
    pub footer: String,
    pub max_answer: i32,
    pub form_hash: String,
    pub options: Vec<PollOption>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PollOption {
    pub id: String,
    pub text: String,
    pub rates: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadDetail {
    pub tid: String,
    pub fid: i32,
    pub title: String,
    pub page: i32,
    pub max_page: i32,
    pub posts: Vec<Post>,
    /// 首楼的投票表单（`form#poll`），Discuz 把整个投票放在主题级而非楼层级。
    pub poll: Option<Poll>,
}

impl ThreadDetail {
    /// 标题是否为空（用于判断解析是否命中真实详情页）。
    pub fn has_content(&self) -> bool { !self.posts.is_empty() }
}

/// 正文的**块级**结构。
///
/// 原来的 `contents` 是一条扁平的内容流：引用被 `el.text()` 拍平成一行、
/// 列表与代码块也只剩文字，到了界面就是一大段糊在一起的文本。块级模型把
/// 「哪一段是引用、哪一段是列表、哪一块是代码」保留下来，界面才能给出
/// 引用竖条、列表符号、代码底色这些真正的排版。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Block {
    /// 普通段落。段内换行用 `\n` 保留（Discuz 靠 `<br>` 换行）。
    Paragraph(String),
    /// 引用：Discuz 的 `div.quote > blockquote`，头部带原作者与时间。
    Quote {
        author: String,
        time: String,
        blocks: Vec<Block>,
    },
    /// 列表，`items` 已经拍平成纯文本（嵌套项按缩进拼进同一项）。
    List { ordered: bool, items: Vec<String> },
    /// 代码块（`pre` / `code` / Discuz 的 `div.blockcode`）。
    Code(String),
    /// 标题：站点用 `<h1..h6>` 或 `<font size="5"><strong>` 表示。
    Heading(String),
    /// 居中 / 靠右的一段：Discuz 的 `<p align="center|right">`。
    Align { center: bool, text: String },
    /// 正文图片。
    Image { url: String, thumb_url: String },
    /// 非图片附件。
    Attach { name: String, url: String, size: i64 },
    /// 正文里**独占一行**的链接（裸 URL 或独立的一段链接）。
    /// 段落内的普通文字链接仍按文字渲染——句子中插一个可点区块会打断阅读。
    Link { text: String, url: String },
    /// 分隔线。
    Rule,
}

/// 收藏 / 搜索 / 我的帖子 / 私信 / 通知的统一列表行（`SimpleListItemBean`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleItem {
    pub tid: String,
    pub pid: String,
    pub title: String,
    pub info: String,
    pub forum: String,
    pub time: String,
    pub uid: String,
    pub author: String,
    pub avatar_url: String,
    pub is_new: bool,
    pub has_form_hash: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleList {
    pub items: Vec<SimpleItem>,
    pub search_id: String,
    pub max_page: i32,
    pub form_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserInfo {
    pub username: String,
    pub uid: String,
    pub avatar_url: String,
    pub online: bool,
    pub detail: String,
    pub form_hash: String,
}

/// 黑名单页（`pm.php?action=viewblack`）：用户名列表 + 页面 formhash，
/// 增删操作必须带上同页取到的 formhash。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blacklist {
    pub users: Vec<String>,
    pub form_hash: String,
    /// 页面明确显示「暂无数据」时为真，用于区分「空名单」与「解析失败」。
    pub empty_hint: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notification {
    pub sms_count: i32,
    pub thread_count: i32,
    pub username: String,
    pub uid: String,
    pub content: String,
}

impl Notification {
    pub fn has_new(&self) -> bool { self.sms_count > 0 || self.thread_count > 0 }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub query: String,
    pub author: String,
    pub uid: String,
    pub fid: String,
    pub search_id: String,
    pub full_text: bool,
}

/// 会话快照。**不保存明文密码**（原客户端在两处明文落盘，是安全缺陷）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub username: String,
    pub uid: String,
    pub cookies: Vec<(String, String)>,
    pub saved_at: i64,
}
