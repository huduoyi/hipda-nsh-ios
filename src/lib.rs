//! `HiPDA·nsh 版` —— HiPDA 论坛的非官方客户端。
//!
//! 一套 Rust 组件跑在 NSH 支持的六个平台上（iOS / Android / macOS /
//! Windows / Linux / Web）。业务协议、GBK 编解码、Cookie 会话与 HTML 解析
//! 全部在 `forum-core` 里，本 crate 只负责：
//!
//! - 把 NSH 的 `fetch` 适配成 `forum_core::Transport`（见 `transport.rs`）；
//! - 用 signal 驱动界面，把内核模型映射成列表行；
//! - 按 `theme.json` 的设计令牌绘制，浅色 / 深色由系统外观自动切换。
//!
//! 4D4Y 是 Discuz! 7.2，没有官方 API；所有数据都来自 PHP 页面的 GBK HTML。
//! 因此本客户端不缓存密码、不伪造登录态，也不在界面里塞演示数据。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use forum_core::config::{forum_name, local_forum_index, DEFAULT_FORUMS, FID_DISCOVERY};
use forum_core::emoji::{install_default_aliases, SMILEYS, SMILEY_COUNT};
use forum_core::lists::ListSource;
use forum_core::model::{ForumEntry, ForumIndex, ForumSection, Post, SimpleItem, Thread};
use forum_core::post::{PostMode, PostRequest};
use forum_core::transport::ForumClient;
use forum_core::{
    image_host_allowed, parse_document, parse_form_hash, unlisted_image_hosts, Config,
};
use nsh_base::theme;
use nsh_core::element::{AutoFill, input};
use nsh_core::picker::FilePickRequest;
use nsh_core::prelude::*;

use settings::{
    AvatarShape, AvatarSize, DefaultAvatar, Divider, ListStyle, ReadFade, SearchTime, Settings,
    Skin, Spacing, TextScale, TitleSize, TitleWeight,
};

mod cache;
mod session;
mod settings;
mod transport;

pub use transport::NshTransport;

/// `nsh.json` 是网络权限的唯一事实来源：编译期嵌入，运行时解析一次。
/// 内核与 UI 都从这里取白名单，避免「代码里放行、manifest 里没声明」。
const NSH_MANIFEST: &str = include_str!("../nsh.json");

fn allowed_image_hosts() -> &'static [String] {
    static HOSTS: OnceLock<Vec<String>> = OnceLock::new();
    HOSTS.get_or_init(|| {
        let manifest: serde_json::Value = serde_json::from_str(NSH_MANIFEST).unwrap_or_default();
        manifest["capabilities"]["network"]["hosts"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// 过滤正文图片：只放行 `nsh.json` 已声明的域名，返回可显示 URL 与缺失域名。
/// 未声明的图片不发起请求（NSH 会拒绝），而是把域名交给界面提示维护。
fn filter_image_urls(urls: Vec<String>) -> (Vec<String>, Vec<String>) {
    let hosts = allowed_image_hosts();
    let allowed: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let visible = urls
        .iter()
        .filter(|url| image_host_allowed(url, &allowed))
        .cloned()
        .collect();
    let missing = unlisted_image_hosts(urls.iter().map(String::as_str), &allowed);
    (visible, missing)
}

/// 当前界面。组件只运行一次，切换靠 `dynamic` 重建区域。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Screen {
    /// 论坛首页：分区 + 版块索引，与站点 index.php 一致，游客也能看。
    #[default]
    Index,
    Login,
    Threads,
    Detail,
    Compose,
    Feed,
    Search,
    /// 我的空间：私信 / 收藏 / 关注 / 我的帖子 / 黑名单等入口。
    Hub,
    /// 用户资料。
    Profile,
    /// 黑名单管理。
    Blacklist,
    /// 写私信。
    SmsCompose,
    /// 设置：分组与条目对齐原 HiPDA·NG 客户端。
    Settings,
}

/// 底部四个主标签。与 `Screen` 分开：详情页 / 设置页等属于某个标签的
/// 下层界面，返回时底部仍高亮该标签。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Forum,
    Message,
    Search,
    Mine,
}

/// 主题列表的一行。只保留界面要显示的字段，避免把内核模型绑到视图上。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ThreadRow {
    tid: String,
    title: String,
    meta: String,
    badge: String,
    sticky: bool,
    #[serde(default)]
    author: String,
    #[serde(default)]
    avatar: String,
    #[serde(default)]
    time: String,
    #[serde(default)]
    replies: i64,
    #[serde(default)]
    is_new: bool,
    #[serde(default)]
    with_picture: bool,
}

impl ThreadRow {
    fn from_thread(t: &Thread) -> Self {
        // 置顶 / 新帖用文字标签，不只靠颜色表达状态（无障碍要求）。
        let mut badge = String::new();
        if t.sticky {
            badge.push_str("置顶 ");
        }
        if t.is_new {
            badge.push('新');
        }
        Self {
            tid: t.tid.clone(),
            title: if t.title.is_empty() { "(无标题)" } else { &t.title }.to_string(),
            meta: format!("{} · {} 回复 · {} 浏览", t.author, t.reply_count, t.view_count),
            badge: badge.trim_end().to_string(),
            sticky: t.sticky,
            author: t.author.clone(),
            avatar: t.avatar_url.clone(),
            time: if t.time_update.is_empty() { t.time_create.clone() } else { t.time_update.clone() },
            replies: t.reply_count,
            is_new: t.is_new,
            with_picture: t.with_picture,
        }
    }
}

/// 通知 / 搜索结果的一行。与 `ThreadRow` 分开，避免把两套模型绑在一起。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SimpleRow {
    tid: String,
    title: String,
    meta: String,
    badge: String,
    /// 私信 / 通知行需要作者 uid 才能打开会话或用户资料。
    uid: String,
    author: String,
}

impl SimpleRow {
    fn from_item(item: &SimpleItem) -> Self {
        let mut parts = Vec::new();
        for s in [&item.info, &item.author, &item.forum, &item.time] {
            if !s.is_empty() && s.as_str() != item.title.as_str() {
                parts.push(s.chars().take(60).collect::<String>());
            }
        }
        Self {
            tid: item.tid.clone(),
            title: if item.title.is_empty() {
                "(无标题)".to_string()
            } else {
                item.title.clone()
            },
            meta: parts.join(" · "),
            // 新状态用文字徽标，不只靠颜色表达。
            badge: if item.is_new { "新".to_string() } else { String::new() },
            uid: item.uid.clone(),
            author: item.author.clone(),
        }
    }
}

/// 投票选项的一行。`id` 是提交时的 `pollanswers[]` 值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PollOptionRow {
    id: String,
    text: String,
    rates: String,
}

/// 详情的一层楼。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PostRow {
    floor: i32,
    author: String,
    /// 作者 uid，头像缺失时用它兜底拼地址，也用于后续跳资料页。
    uid: String,
    avatar: String,
    time: String,
    body: String,
    /// 正文里的图片 URL；`open_thread` 会把相对路径拼成绝对路径，
    /// 并剔除非 `nsh.json` 白名单域名。
    images: Vec<String>,
    /// 块级正文：引用、列表、代码、图片按块渲染，不再糊成一坨文本。
    blocks: Vec<forum_core::model::Block>,
}

impl PostRow {
    fn from_post(p: &Post) -> Self {
        Self {
            floor: p.floor,
            author: p.author.clone(),
            uid: p.author_id.clone(),
            avatar: p.avatar_url.clone(),
            time: p.time_post.clone(),
            body: tidy_body(&p.copy_text()),
            blocks: p.blocks.clone(),
            // 一屏最多显示 3 张，避免长图帖把详情列表撑垮。
            images: p
                .contents
                .iter()
                .filter(|c| c.kind == forum_core::model::ContentKind::Image && !c.url.is_empty())
                .take(3)
                .map(|c| c.url.clone())
                .collect(),
        }
    }
}

/// 单个主题的浏览进度。持久化到 `Storage`，重开 App 也能回到上次位置。
/// `offset` 是滚动偏移（pt），`last_floor` 是上次看到的楼层，用来判断新回复。
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct ThreadProgress {
    offset: f32,
    last_floor: i32,
}

/// 缓存里放的是**界面行**而不是内核模型：行结构稳定、字段少，
/// 站点解析逻辑改了也不用动缓存格式。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedList {
    rows: Vec<ThreadRow>,
    max_page: i32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedIndex {
    sections: Vec<SectionRow>,
    status: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedDetail {
    posts: Vec<PostRow>,
    max_page: i32,
    title: String,
    fid: i32,
}

/// 论坛首页的一个版块行。把 `ForumEntry` 拍平成可直接显示的文案，
/// 避免把内核模型绑到视图上。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ForumRow {
    fid: i32,
    name: String,
    description: String,
    counts: String,
    today: String,
    last: String,
}

impl ForumRow {
    fn from_entry(e: &ForumEntry) -> Self {
        let counts = if e.threads.is_empty() {
            String::new()
        } else {
            format!("{} 主题 / {} 帖", e.threads, e.posts)
        };
        let today = if e.today.is_empty() {
            String::new()
        } else {
            format!("今日 {}", e.today)
        };
        let mut last = String::new();
        if !e.last_title.is_empty() {
            last.push_str(&format!("最后：{}", e.last_title));
            if !e.last_author.is_empty() {
                last.push_str(&format!(" · {}", e.last_author));
            }
            if !e.last_time.is_empty() {
                last.push_str(&format!(" · {}", e.last_time));
            }
        }
        Self {
            fid: e.fid,
            name: if e.name.is_empty() { forum_name(e.fid).to_string() } else { e.name.clone() },
            description: e.description.clone(),
            counts,
            today,
            last,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SectionRow {
    name: String,
    forums: Vec<ForumRow>,
}

impl SectionRow {
    fn from_section(s: &ForumSection) -> Self {
        Self {
            name: s.name.clone(),
            forums: s.forums.iter().map(ForumRow::from_entry).collect(),
        }
    }
}

fn index_rows(index: &ForumIndex) -> Vec<SectionRow> {
    index.sections.iter().map(SectionRow::from_section).collect()
}

/// 界面状态。`Signal` 是 Copy 句柄，`Arc` 让网络客户端能被多个任务共享。
#[derive(Clone)]
struct Ui {
    client: Arc<ForumClient<NshTransport>>,
    screen: Signal<Screen>,
    logged_in: Signal<bool>,
    username: Signal<String>,
    password: Signal<String>,
    /// 「保持登录」：勾选时登录成功会落会话快照，下次启动自动恢复。
    /// 存的是登录凭据（cookie），**不存密码**。
    remember: Signal<bool>,
    /// 用户是否已经交互过：校验错误只在交互后出现，避免加载即报错。
    touched: Signal<bool>,
    status: Signal<String>,
    busy: Signal<bool>,
    /// 论坛首页的分区数据；启动与退出登录后都会重新拉取。
    sections: Signal<Vec<SectionRow>>,
    /// 首页副标题：分区数、版块数与数据来源（站点 / 本地兜底）。
    index_status: Signal<String>,
    rows: Signal<Vec<ThreadRow>>,
    posts: Signal<Vec<PostRow>>,
    /// 打开详情前的界面，供「返回」精确回退（可能来自版块列表或搜索结果）。
    origin: Signal<Screen>,
    fid: Signal<i32>,
    tid: Signal<String>,
    title: Signal<String>,
    page: Signal<i32>,
    max_page: Signal<i32>,
    /// 主题里出现的外链图片域名缺白名单时的提示（空串表示无缺口）。
    image_host_hint: Signal<String>,
    /// 正文图片的宽高比（高/宽），由图片加载完成后回填，用来把占位框
    /// 调成真实比例，避免固定高度导致大片留白或裁切。
    image_ratio: Signal<BTreeMap<String, f32>>,
    /// 详情页当前页码 / 总页数。Discuz 每页 50 层，长帖必须能往下翻。
    detail_page: Signal<i32>,
    detail_max: Signal<i32>,
    /// 详情页滚动容器的元素句柄：程序化跳页 / 页首 / 页尾 / 恢复位置都靠它。
    detail_scroll: Signal<ElementRef>,
    /// 楼层号 → 该楼层标题行的元素句柄，用于「跳到指定楼层」精确 reveal。
    /// 用普通 Rc<RefCell> 而非 Signal：楼层在渲染闭包里注册句柄，不进响应式，
    /// 避免「渲染期写 signal」触发 effect 重入。
    detail_floor_refs: Rc<RefCell<BTreeMap<i32, ElementRef>>>,
    /// 当前滚动偏移（pt），on_scroll 持续写入，用于「返回原位置」。
    detail_offset: Signal<f32>,
    /// 跳转工具：目标楼层 / 目标页输入框内容。
    detail_jump_floor: Signal<String>,
    detail_jump_page: Signal<String>,
    /// 跳走前保存的偏移；Some 时显示「返回原位置」。
    detail_return_offset: Signal<Option<f32>>,
    /// 每个主题上次的浏览进度（偏移 + 看到的最大楼层），按 tid 记录。
    detail_progress: Signal<BTreeMap<String, ThreadProgress>>,
    /// 待跳转的楼层：视图重建出该楼层后由 effect 执行 reveal 并清空。
    detail_pending_floor: Signal<Option<i32>>,
    /// 待恢复的浏览偏移：视图就绪后由 effect 执行 scroll_to 并清空。
    detail_restore_offset: Signal<Option<f32>>,
    editor: Signal<String>,
    feed_rows: Signal<Vec<SimpleRow>>,
    feed_title: Signal<String>,
    /// 当前通用列表的数据源，翻页与刷新都基于它重建 URL。
    feed_source: Signal<ListSource>,
    feed_page: Signal<i32>,
    feed_max_page: Signal<i32>,
    /// 搜索结果翻页必须带 searchid，否则站点会重新搜索回第 1 页。
    feed_search_id: Signal<String>,
    query: Signal<String>,
    /// 搜索页的来源（首页 / 版块列表 / 我的空间），「取消」要回到原处。
    search_origin: Signal<Screen>,
    /// 用户资料：拍平成可直接显示的文本行。
    profile_title: Signal<String>,
    profile_rows: Signal<Vec<String>>,
    profile_uid: Signal<String>,
    profile_name: Signal<String>,
    blacklist_rows: Signal<Vec<String>>,
    blacklist_input: Signal<String>,
    sms_to_uid: Signal<String>,
    sms_to_name: Signal<String>,
    sms_body: Signal<String>,
    /// 回帖页的表情选择器开关。
    emoji_open: Signal<bool>,
    /// 主题详情里的投票表单。
    poll_title: Signal<String>,
    poll_footer: Signal<String>,
    poll_hash: Signal<String>,
    poll_multi: Signal<bool>,
    poll_max: Signal<i32>,
    poll_rows: Signal<Vec<PollOptionRow>>,
    poll_selected: Signal<Vec<String>>,
    /// 用户设置。改动即时写回 `Storage`，界面直接读 signal，无需重启。
    settings: Signal<Settings>,
    /// 进入设置页前的界面，「返回」要精确回退。
    settings_origin: Signal<Screen>,
    /// 设置内的子页（菜单 / 五个分类）。
    settings_page: Signal<SettingsPage>,
    /// 版块列表右上角「⋯」菜单是否展开。
    menu_open: Signal<bool>,
    /// 发帖模式：回复 / 新主题。同一个编辑器，靠它切换字段与提交接口。
    compose_mode: Signal<PostMode>,
    /// 新主题标题（回复时不用）。
    compose_subject: Signal<String>,
    /// 尾巴开关的界面镜像。输入框所在的分组只依赖这三个 signal，
    /// 不依赖 `settings`：否则每敲一个字都会重建输入框、丢焦点。
    tail_open: Signal<bool>,
    tail_text: Signal<String>,
    tail_url: Signal<String>,
    /// 0.2 界面重做：底部当前主标签。
    tab: Signal<Tab>,
    /// 顶栏头像菜单展开（登录 / 个人主页 / 账号管理）。
    avatar_menu: Signal<bool>,
    /// 顶栏版块切换菜单展开。
    forum_menu: Signal<bool>,
    /// 搜索页参数：标题 / 全文、版块、时间。改变后即时生效到下一次搜索。
    search_full_text: Signal<bool>,
    search_fid: Signal<String>,
    search_time: Signal<SearchTime>,
    /// 本次会话内已打开的帖子 tid，用于「已读淡化」。重启后清空可接受：
    /// 淡化只是视觉提示，不影响数据。
    read_tids: Signal<BTreeSet<String>>,
}

impl Ui {
    fn new() -> Self {
        let client = Arc::new(ForumClient::new(Config::default(), NshTransport));
        Self {
            client,
            screen: signal(Screen::Index),
            logged_in: signal(false),
            username: signal(session::remembered_username()),
            remember: signal(session::remember()),
            password: signal(String::new()),
            touched: signal(false),
            status: signal("未登录也可浏览论坛首页；受限版块、通知与回帖需要登录。".into()),
            busy: signal(false),
            sections: signal(Vec::new()),
            index_status: signal("正在加载论坛首页…".into()),
            rows: signal(Vec::new()),
            posts: signal(Vec::new()),
            origin: signal(Screen::Index),
            fid: signal(FID_DISCOVERY),
            tid: signal(String::new()),
            title: signal(String::new()),
            page: signal(1),
            max_page: signal(1),
            image_host_hint: signal(String::new()),
            image_ratio: signal(BTreeMap::new()),
            detail_page: signal(1),
            detail_max: signal(1),
            // ElementRef 不实现 PartialEq，用 signal_with(Never)：句柄只用来发命令，
            // 写入不该触发重建，正好符合需求。
            detail_scroll: signal_with(element_ref(), SignalEquals::Never),
            detail_floor_refs: Rc::new(RefCell::new(BTreeMap::new())),
            detail_offset: signal(0.0),
            detail_jump_floor: signal(String::new()),
            detail_jump_page: signal(String::new()),
            detail_return_offset: signal(None),
            detail_progress: signal(BTreeMap::new()),
            detail_pending_floor: signal(None),
            detail_restore_offset: signal(None),
            editor: signal(String::new()),
            feed_rows: signal(Vec::new()),
            feed_title: signal("通知".into()),
            feed_source: signal(ListSource::Notifications),
            feed_page: signal(1),
            feed_max_page: signal(1),
            feed_search_id: signal(String::new()),
            query: signal(String::new()),
            search_origin: signal(Screen::Index),
            profile_title: signal("用户资料".into()),
            profile_rows: signal(Vec::new()),
            profile_uid: signal(String::new()),
            profile_name: signal(String::new()),
            blacklist_rows: signal(Vec::new()),
            blacklist_input: signal(String::new()),
            sms_to_uid: signal(String::new()),
            sms_to_name: signal(String::new()),
            sms_body: signal(String::new()),
            emoji_open: signal(false),
            poll_title: signal(String::new()),
            poll_footer: signal(String::new()),
            poll_hash: signal(String::new()),
            poll_multi: signal(false),
            poll_max: signal(1),
            poll_rows: signal(Vec::new()),
            poll_selected: signal(Vec::new()),
            settings: signal(Settings::load()),
            settings_origin: signal(Screen::Index),
            settings_page: signal(SettingsPage::Menu),
            menu_open: signal(false),
            compose_mode: signal(PostMode::Reply),
            compose_subject: signal(String::new()),
            tail_open: signal(false),
            tail_text: signal(String::new()),
            tail_url: signal(String::new()),
            tab: signal(Tab::Forum),
            avatar_menu: signal(false),
            forum_menu: signal(false),
            search_full_text: signal(false),
            search_fid: signal("0".into()),
            search_time: signal(SearchTime::All),
            read_tids: signal(BTreeSet::new()),
        }
    }

    fn note(&self, message: impl Into<String>) {
        self.status.set(message.into());
    }

    /// 错误信息同时做屏幕阅读器播报，视觉与无障碍状态同步。
    fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        announce_for_accessibility(message.clone());
        self.status.set(message);
        self.busy.set(false);
    }

    /// 论坛的「您还未登录」门页不是异常，而是站点自身的浏览限制：
    /// 给可操作提示即可，不要当成错误打断首页浏览。
    fn report_error(&self, error: forum_core::Error, context: &str) {
        let text = error.to_string();
        if text.contains("未登录") {
            self.note(format!("{context} 需要登录后浏览，点「登录」继续"));
        } else {
            self.fail(text);
        }
    }

    /// 加载论坛首页。**不需要登录**：站点 `index.php` 对游客开放部分分区，
    /// 登录态会多出「4D4Y」「生活版区」。站点不可达时用本地 21 个版块表
    /// 兜底，界面明确标注数据来源。
    fn load_index(self) {
        // 命中缓存就先画出来：首页是所有入口的起点，「打开即见内容」
        // 比「打开再转圈」的体感差距最大。
        let mut had_cache = false;
        if let Some((cached, age)) = cache::get::<CachedIndex>(&cache::index_key()) {
            self.sections.set(cached.sections);
            self.index_status.set(cached.status);
            had_cache = true;
            self.note(format!("显示的是{}的缓存，正在更新…", cache::humanize_age(age)));
        }
        self.busy.set(!had_cache);
        spawn(async move {
            let index = match self.client.forum_index().await {
                Ok(index) if !index.is_empty() => index,
                // WAF 挑战、断网或站点改版：不弹错误，退回本地目录。
                Ok(_) | Err(_) => local_forum_index(),
            };
            let (sections, forums) = (index.sections.len(), index.forum_count());
            self.index_status.set(if index.offline {
                format!("站点首页暂不可达 · 已显示本地版块目录（{forums} 个版块）")
            } else {
                format!(
                    "{sections} 个分区 · {forums} 个版块 · {}",
                    if index.logged_in { "登录态" } else { "游客可浏览" }
                )
            });
            let sections = index_rows(&index);
            let status = self.index_status.get();
            self.sections.set(sections.clone());
            // 只在真实数据上写缓存：`offline` 的本地兜底也写，但换个键不合适，
            // 直接跳过 —— 站点恢复后用户会拿到新数据。
            if !index.offline {
                cache::put(
                    &cache::index_key(),
                    &CachedIndex {
                        sections,
                        status,
                    },
                );
            }
            self.busy.set(false);

            // 平台层的 cookie jar 会自己带上 `cdb_auth`，所以站点可能已经认得
            // 这个客户端，而本机 `Storage` 里没有快照（换机、清缓存、旧版本未存）。
            // 这时界面会出现「未登录」却能读到登录态分区 —— 以站点为准补一份会话，
            // 否则写操作入口会误判成游客。
            self.sync_login_state(index.logged_in);
        });
    }

    /// 让界面登录态与站点实际表现保持一致。
    ///
    /// 两个来源必须对齐：`index.logged_in`（站点 `#umenu`）与本机会话。
    /// 2026-11-14 实测确认：**NSH 的 `fetch` 不会自动带上平台 cookie jar**，
    /// 所以客户端能读到登录态内容，只可能因为 `ForumClient` 自己持有了
    /// `cdb_auth`（来自 Storage 快照）。据此：
    ///
    /// - 站点是登录态而本机没有 → 补上界面状态（快照在，只是没置位）；
    /// - 站点是游客而本机是登录态 → cookie 已失效，按未登录处理并清快照，
    ///   否则写操作入口会误判成可写。
    fn sync_login_state(self, site_logged_in: bool) {
        if site_logged_in == self.logged_in.get() {
            return;
        }
        if site_logged_in {
            let (name, _) = self.client.identity();
            self.logged_in.set(true);
            if !name.is_empty() {
                self.username.set(name);
            }
        } else {
            // 只降级界面状态，**不删快照**：站点会因 WAF、限流或网络抖动
            // 偶发返回游客首页，此时删掉快照等于把用户登出，代价远大于
            // 一次界面误判。会话是否真的失效，交给写操作被站点拒绝或
            // 下次启动 `resume()` 的协议错误分支去判定。
            self.logged_in.set(false);
            self.username.set(String::new());
            self.note("站点当前未识别登录态，可稍后重试或重新登录。");
        }
    }

    fn login(self, username: String, password: String) {
        self.touched.set(true);
        let problem = validate_login(&username, &password);
        if !problem.is_empty() {
            self.fail(problem);
            return;
        }
        // 有效提交后才防重复请求；校验不通过时按钮保持可用。
        if self.busy.get() {
            return;
        }
        self.busy.set(true);
        self.note("正在登录…");
        spawn(async move {
            match self.client.login(&username, &password, "0", "").await {
                Ok(result) if result.ok => {
                    self.logged_in.set(true);
                    self.username.set(result.username.clone());
                    // 明文密码不驻留在 signal 里，也不写进任何持久化存储。
                    self.password.set(String::new());
                    self.touched.set(false);
                    session::set_remembered_username(&result.username);
                    let remember = self.remember.get();
                    self.note(if !self.client.is_logged_in() {
                        format!("{}（本机未保存会话）", result.message)
                    } else if !remember {
                        format!("登录成功：{} · 未勾选保持登录，下次需重新登录", result.username)
                    } else {
                        match session::save(&self.client) {
                            Ok(()) => format!("登录成功：{} · 已在本机记住登录态", result.username),
                            Err(error) => format!("登录成功：{} · 会话保存失败：{error}", result.username),
                        }
                    });
                    // 登录后固定进入 Discovery 第 1 页，而不是停留在登录页或
                    // 沿用上次翻到的版块 —— 与原客户端的「首页」一致。
                    let (screen, fid, page) = post_login_target();
                    self.fid.set(fid);
                    self.page.set(page);
                    self.screen.set(screen);
                    self.clone().load_threads(fid, page);
                }
                Ok(result) => self.fail(if result.message.is_empty() {
                    "登录失败，请检查账号与密码".to_string()
                } else {
                    result.message
                }),
                Err(error) => self.fail(error.to_string()),
            }
            self.busy.set(false);
        });
    }

    /// 启动时用已保存的会话恢复登录态。
    ///
    /// 先发一次 GET 让内核校验 cookie 在论坛侧是否仍有效（`cdb_auth`
    /// 会过期）。无论校验结果如何都停论坛首页：登录态只影响能看到的分区
    /// 与可用操作，不决定首屏是哪个界面。
    fn resume(self) {
        self.busy.set(true);
        self.note("正在校验本机保存的登录态…");
        spawn(async move {
            match self.client.refresh_identity().await {
                Ok(()) => {
                    let (username, _) = self.client.identity();
                    eprintln!("[resume] ok user={username}");
                    self.username.set(username);
                    self.logged_in.set(true);
                    // 顺手刷新快照：既更新时间戳，也补偿上一次可能没落盘的写入。
                    let _ = session::save(&self.client);
                    self.note("登录态有效，已恢复本机会话。");
                }
                // 网络不可达不能推断 cookie 失效：保留快照，别把用户踢下线。
                Err(error) if matches!(&error, forum_core::Error::Http(_)) => {
                    self.logged_in.set(true);
                    self.note(format!("暂时无法连接论坛（{error}），已保留本机登录态"));
                }
                Err(error) => {
                    eprintln!("[resume] err: {error}");
                    // **不删快照**。站点会因 WAF、限流或模板抖动偶发返回游客页，
                    // 把这种情况当"会话失效"直接清掉，就等于随机登出用户 ——
                    // 2026-11-15 一天内被这个分支清了两次。真正的失效由写操作
                    // 被站点拒绝、或用户主动退出登录来收尾。
                    self.logged_in.set(false);
                    self.username.set(String::new());
                    self.note(format!(
                        "站点未识别登录态（{error}）；会话仍保存在本机，可稍后重试或重新登录"
                    ));
                }
            }
            self.busy.set(false);
            // 登录态下首页会多出分区，必须重新拉一次。
            self.clone().load_index();
        });
    }

    /// 打开设置页。尾巴输入框用界面镜像承接编辑，先同步一次，
    /// 否则输入框会显示上一次会话残留的值。
    fn open_settings(self) {
        let current = self.screen.get();
        if current != Screen::Settings {
            self.settings_origin.set(current);
        }
        let settings = self.settings.get();
        self.tail_open.set(settings.tail_enabled);
        self.tail_text.set(settings.tail_text.clone());
        self.tail_url.set(settings.tail_url.clone());
        self.settings_page.set(SettingsPage::Menu);
        self.screen.set(Screen::Settings);
    }

    /// 修改设置：先落 signal（界面立即重绘），再写 Storage。
    ///
    /// 设置项都是「读多写少」的小值，同步写不需要防抖；写失败也不回滚 ——
    /// 配额满时用户至少还能用完这次会话，下次启动退回默认值。
    fn apply_settings(self, change: impl FnOnce(&mut Settings)) {
        let mut next = self.settings.get();
        change(&mut next);
        next.save();
        self.settings.set(next);
    }

    /// 退出登录：清本地快照与草稿，回到游客也能看的论坛首页。
    fn logout(self) {
        session::logout(&self.client);
        // 缓存里可能有登录态才看得到的版块与帖子，退出后不能留给游客。
        cache::clear();
        self.logged_in.set(false);
        self.username.set(String::new());
        self.password.set(String::new());
        self.rows.set(Vec::new());
        self.posts.set(Vec::new());
        self.editor.set(String::new());
        self.tid.set(String::new());
        self.feed_rows.set(Vec::new());
        self.query.set(String::new());
        self.touched.set(false);
        self.screen.set(Screen::Index);
        self.note("已退出登录，本机保存的会话与草稿已清除。");
        self.clone().load_index();
    }

    /// 加载版块主题列表。**不预先检查登录态**：站点对游客开放技术版区
    /// （2026-11-13 实测 fid=7 可读 75 条主题），受限版块才返回门页，
    /// 由 `report_error` 给出「需要登录」的可操作提示。
    fn load_threads(self, fid: i32, page: i32) {
        let by_dateline = self.settings.get().sort_by_dateline;
        let key = cache::list_key(fid, page, by_dateline);

        // 命中就先把这一页画出来（可能是几分钟前的），再后台刷新。
        // 返回上一页、来回翻页都是瞬时的，这是「顺滑」的主要来源。
        let cached = cache::get::<CachedList>(&key);
        if let Some((cached, age)) = &cached {
            self.rows.set(cached.rows.clone());
            self.max_page.set(cached.max_page.max(1));
            self.fid.set(fid);
            self.page.set(page);
            self.screen.set(Screen::Threads);
            self.note(format!(
                "{} · 显示{}的缓存，正在更新…",
                forum_name(fid),
                cache::humanize_age(*age)
            ));
        } else {
            self.note(format!("正在加载 {} 第 {} 页…", forum_name(fid), page));
        }
        self.busy.set(cached.is_none());

        spawn(async move {
            match self.client.thread_list(fid, page, by_dateline).await {
                Ok(list) => {
                    let rows = self.thread_rows_from(&list);
                    let max_page = list.max_page.max(1);
                    self.rows.set(rows.clone());
                    self.max_page.set(max_page);
                    self.fid.set(fid);
                    self.page.set(page);
                    self.screen.set(Screen::Threads);
                    cache::put(
                        &key,
                        &CachedList {
                            rows,
                            max_page,
                        },
                    );
                    self.note(format!(
                        "{}：{} 条主题，第 {}/{} 页",
                        forum_name(fid),
                        list.threads.len(),
                        page,
                        max_page
                    ));
                    // 预取下一页：用户往下翻时那一屏已经在本地了。
                    self.clone().prefetch_list(fid, page + 1, max_page);
                }
                Err(error) => {
                    // 有缓存就继续用，不要把已经画出来的列表换成错误页。
                    if cached.is_some() {
                        self.note(format!(
                            "更新失败（{error}），继续显示本机缓存"
                        ));
                    } else {
                        self.report_error(error, forum_name(fid));
                    }
                }
            }
            self.busy.set(false);
        });
    }

    /// 三个入口都能进搜索；记住来源，「取消」回原屏而不是硬跳版块列表。
    fn open_search(&self) {
        self.search_origin.set(self.screen.get());
        self.screen.set(Screen::Search);
    }

    /// 写操作统一守卫：游客只给可操作提示，不发注定失败的请求。
    fn require_login(&self) -> bool {
        if self.logged_in.get() {
            return true;
        }
        self.note("该操作需要登录后使用，点「登录」继续");
        self.screen.set(Screen::Login);
        false
    }

    /// 加载任一通用列表（通知 / 搜索 / 私信 / 收藏 / 关注 / 我的帖子 …）。
    /// URL 规则、GBK 编码与 DOM 选择全部由内核 `ListSource` 决定。
    fn load_list(self, source: ListSource) {
        if let ListSource::Search { query, .. } = &source
            && query.trim().is_empty()
        {
            self.fail("请输入搜索关键词");
            return;
        }
        if source.needs_login() && !self.require_login() {
            return;
        }
        let title = source.title();
        // 私信详情页的「写私信」要复用当前会话对象。
        if let ListSource::SmsDetail { uid, name } = &source {
            self.sms_to_uid.set(uid.clone());
            self.sms_to_name.set(name.clone());
        }
        self.feed_source.set(source.clone());
        self.feed_page.set(source.page());
        self.busy.set(true);
        self.note(format!("正在加载{title}…"));
        spawn(async move {
            match self.client.list(&source).await {
                Ok(list) => {
                    let n = list.items.len();
                    self.feed_rows
                        .set(list.items.iter().map(SimpleRow::from_item).collect());
                    self.feed_title.set(title.clone());
                    self.feed_max_page.set(list.max_page.max(1));
                    // 搜索类列表翻页要回放 searchid，否则站点会重新搜索回第 1 页。
                    if !list.search_id.is_empty() {
                        self.feed_search_id.set(list.search_id.clone());
                        self.feed_source.set(source.with_search_id(&list.search_id));
                    }
                    self.screen.set(Screen::Feed);
                    self.note(format!(
                        "{title}：{n} 条 · 第 {}/{} 页",
                        source.page(),
                        list.max_page.max(1)
                    ));
                }
                Err(error) => self.report_error(error, title.as_str()),
            }
            self.busy.set(false);
        });
    }

    fn flip_feed_page(self, delta: i32) {
        let page = (self.feed_page.get() + delta).clamp(1, self.feed_max_page.get().max(1));
        if page == self.feed_page.get() {
            return;
        }
        let mut source = self.feed_source.get().with_page(page);
        let sid = self.feed_search_id.get();
        if !sid.is_empty() {
            source = source.with_search_id(&sid);
        }
        self.clone().load_list(source);
    }

    fn open_profile(self, uid: String, name: String) {
        if uid.trim().is_empty() {
            self.note("该条目没有可用的 uid，无法打开用户资料");
            return;
        }
        self.busy.set(true);
        self.note("正在加载用户资料…");
        spawn(async move {
            match self.client.user_info(&uid).await {
                Ok(info) => {
                    let display = if info.username.is_empty() { name } else { info.username };
                    self.profile_title.set(format!("用户资料 · {display}"));
                    self.profile_uid
                        .set(if info.uid.is_empty() { uid } else { info.uid });
                    self.profile_name.set(display);
                    let mut rows = Vec::new();
                    if info.online {
                        rows.push("● 当前在线".to_string());
                    }
                    rows.extend(
                        info.detail
                            .lines()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                    );
                    self.profile_rows.set(rows);
                    self.screen.set(Screen::Profile);
                    self.note(format!("{} 的资料已加载", self.profile_name.get()));
                }
                Err(error) => self.report_error(error, "用户资料"),
            }
            self.busy.set(false);
        });
    }

    fn load_blacklist(self) {
        if !self.require_login() {
            return;
        }
        self.busy.set(true);
        spawn(async move {
            match self.client.blacklist().await {
                Ok(list) => {
                    let n = list.users.len();
                    self.blacklist_rows.set(list.users);
                    self.screen.set(Screen::Blacklist);
                    self.note(format!("黑名单：{n} 人"));
                }
                Err(error) => self.report_error(error, "黑名单"),
            }
            self.busy.set(false);
        });
    }

    fn set_blacklist(self, name: String, add: bool) {
        if name.trim().is_empty() {
            self.fail("请输入用户名");
            return;
        }
        if !self.require_login() {
            return;
        }
        self.busy.set(true);
        spawn(async move {
            match self.client.set_blacklist(&name, add).await {
                Ok(message) => {
                    self.blacklist_input.set(String::new());
                    self.note(message);
                    self.clone().load_blacklist();
                }
                Err(error) => self.report_error(error, "黑名单"),
            }
            self.busy.set(false);
        });
    }

    fn open_sms_compose(self, uid: String, name: String) {
        if !self.require_login() {
            return;
        }
        self.sms_to_uid.set(uid);
        self.sms_to_name.set(name);
        self.sms_body.set(String::new());
        self.screen.set(Screen::SmsCompose);
        self.note("写私信：内容不会保存为草稿");
    }

    fn send_sms(self) {
        let body = self.sms_body.get();
        if body.trim().is_empty() {
            self.fail("私信内容不能为空");
            return;
        }
        let uid = self.sms_to_uid.get();
        let name = self.sms_to_name.get();
        self.busy.set(true);
        spawn(async move {
            match self.client.send_sms(&uid, &name, &body).await {
                Ok(message) => {
                    self.sms_body.set(String::new());
                    self.note(message);
                    self.screen.set(Screen::Feed);
                }
                Err(error) => self.report_error(error, "发送私信"),
            }
            self.busy.set(false);
        });
    }

    fn toggle_poll_option(self, id: String) {
        let mut selected = self.poll_selected.get();
        if let Some(i) = selected.iter().position(|x| x == &id) {
            selected.remove(i);
        } else {
            let max = self.poll_max.get().max(1) as usize;
            if !self.poll_multi.get() {
                selected.clear();
            }
            if selected.len() >= max {
                self.note(format!("该投票最多可选 {max} 项"));
                return;
            }
            selected.push(id);
        }
        self.poll_selected.set(selected);
    }

    fn submit_vote(self) {
        if !self.require_login() {
            return;
        }
        let answers = self.poll_selected.get();
        let hash = self.poll_hash.get();
        let tid = self.tid.get();
        let fid = self.fid.get();
        self.busy.set(true);
        spawn(async move {
            match self.client.vote_poll(fid, &tid, &answers, &hash).await {
                Ok(message) => {
                    self.note(message);
                    self.poll_rows.set(Vec::new());
                    self.poll_selected.set(Vec::new());
                }
                Err(error) => self.report_error(error, "投票"),
            }
            self.busy.set(false);
        });
    }

    /// 表情选择器：把论坛短码插入回帖内容，和草稿一起保存。
    fn insert_emoji(self, code: String) {
        // 回帖写草稿，私信写独立的正文 signal，两个编辑页共用一个选择器。
        let sms = self.screen.get() == Screen::SmsCompose;
        let mut text = if sms { self.sms_body.get() } else { self.editor.get() };
        if !text.is_empty() && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(&code);
        if sms {
            self.sms_body.set(text);
        } else {
            self.editor.set(text.clone());
            session::save_draft(&text);
        }
        self.emoji_open.set(false);
        self.note(format!("已插入表情 {code}"));
    }

    /// 把内核楼层模型转成界面行：头像补全、图片拼绝对地址并按白名单过滤。
    /// `open_thread`（第 1 页）与 `load_more_posts`（后续页）共用，避免两处
    /// 各写一份导致图片过滤规则漂移。
    fn thread_rows(&self, posts: &[Post]) -> (Vec<PostRow>, BTreeSet<String>) {
        let mut unlisted: BTreeSet<String> = BTreeSet::new();
        let rows = posts
            .iter()
            .map(|p| {
                let mut row = PostRow::from_post(p);
                // 头像地址由内核按 uid 拼（`config.avatar_url`），
                // 个别楼层页面没给 uid 时才回退到作者名首字占位。
                if row.avatar.is_empty() && !row.uid.is_empty() {
                    row.avatar = self.client.config().avatar_url(&row.uid);
                }
                row.avatar = self.client.forum_url(&row.avatar);
                row.images = row
                    .images
                    .iter()
                    .map(|url| self.client.forum_url(url))
                    .collect();
                // NSH「未列出即拒绝」：不在 nsh.json 里的域名不发起请求，
                // 只把域名暴露给界面，避免静默丢图难以排查。
                let (visible, hosts) = filter_image_urls(row.images.clone());
                row.images = visible.clone();
                unlisted.extend(hosts);
                // 块里的图片同样要拼绝对地址：块级渲染直接用 block 里的 URL，
                // 这里不改就会去请求相对路径。
                row.blocks = row
                    .blocks
                    .into_iter()
                    .map(|b| self.abs_block_urls(b))
                    .collect();
                row
            })
            .collect();
        (rows, unlisted)
    }

    /// 把块里的相对图片地址补成绝对地址；引用块递归处理。
    fn abs_block_urls(&self, block: forum_core::model::Block) -> forum_core::model::Block {
        use forum_core::model::Block;
        match block {
            Block::Image { url, thumb_url } => Block::Image {
                url: self.client.forum_url(&url),
                thumb_url: if thumb_url.is_empty() {
                    String::new()
                } else {
                    self.client.forum_url(&thumb_url)
                },
            },
            Block::Quote {
                author,
                time,
                blocks,
            } => Block::Quote {
                author,
                time,
                blocks: blocks
                    .into_iter()
                    .map(|b| self.abs_block_urls(b))
                    .collect(),
            },
            other => other,
        }
    }

    /// 内核主题列表 → 界面行。置顶过滤只在这一处做，首屏 / 翻页 / 预取
    /// 三条路径口径一致，不会出现「首页有置顶、翻页没置顶」。
    fn thread_rows_from(&self, list: &forum_core::model::ThreadList) -> Vec<ThreadRow> {
        let show_sticky = self.settings.get().show_sticky;
        list.threads
            .iter()
            .filter(|t| show_sticky || !t.sticky)
            .map(ThreadRow::from_thread)
            .collect()
    }

    /// 后台预取某一页主题列表，只写缓存不动界面。
    /// 失败就静默放弃 —— 预取是锦上添花，不能给用户弹错误。
    fn prefetch_list(self, fid: i32, page: i32, max_page: i32) {
        if page < 1 || page > max_page {
            return;
        }
        let by_dateline = self.settings.get().sort_by_dateline;
        let key = cache::list_key(fid, page, by_dateline);
        if cache::contains(&key) {
            return;
        }
        spawn(async move {
            if let Ok(list) = self.client.thread_list(fid, page, by_dateline).await {
                cache::put(
                    &key,
                    &CachedList {
                        rows: self.thread_rows_from(&list),
                        max_page: list.max_page.max(1),
                    },
                );
            }
        });
    }

    /// 加载下一页楼层并**追加**到列表。不替换现有内容，避免打断当前阅读位置
    /// —— 详情页的滚动容器没有「回到顶部」能力，替换式翻页会让人失去上下文。
    fn load_more_posts(self) {
        if self.busy.get() {
            return;
        }
        let tid = self.tid.get();
        let page = self.detail_page.get() + 1;
        let max = self.detail_max.get();
        if tid.is_empty() || page > max {
            return;
        }
        self.busy.set(true);
        self.note(format!("正在加载第 {page} / {max} 页…"));
        spawn(async move {
            match self.client.thread_detail(&tid, page).await {
                Ok(detail) if detail.has_content() => {
                    let (mut rows, unlisted) = self.thread_rows(&detail.posts);
                    self.posts.update(|list| list.append(&mut rows));
                    self.detail_page.set(page);
                    if !unlisted.is_empty() {
                        let hosts: Vec<String> = unlisted.into_iter().collect();
                        self.image_host_hint.update(|hint| {
                            if hint.is_empty() {
                                *hint = format!(
                                    "有 {} 个外链图片域名未加入 nsh.json：{}（图片暂不加载，按 README 流程补白名单）",
                                    hosts.len(),
                                    hosts.join("、")
                                );
                            }
                        });
                    }
                    let total = self.posts.with(|p| p.len());
                    self.note(format!("第 {page} / {max} 页已加入，共 {total} 层楼"));
                }
                Ok(_) => self.note(format!("第 {page} 页没有内容，可能已到末尾")),
                Err(error) => self.note(format!("第 {page} 页加载失败：{error}")),
            }
            self.busy.set(false);
        });
    }

    /// 把当前详情页的浏览进度写入内存与 Storage，供下次进入恢复。
    /// 只在详情页、且有 tid 时写，避免污染其它界面。
    fn save_detail_progress(&self) {
        let tid = self.tid.get();
        if tid.is_empty() || self.screen.get() != Screen::Detail {
            return;
        }
        let last_floor = self.posts.with(|p| p.last().map(|r| r.floor).unwrap_or(0));
        let progress = ThreadProgress {
            offset: self.detail_offset.get().max(0.0),
            last_floor,
        };
        self.detail_progress.update(|m| {
            m.insert(tid.clone(), progress);
        });
        cache::put_progress(&tid, &progress);
    }

    /// 记录跳转前的位置，便于「跳到某楼层后一键回到原处」。
    fn detail_mark_return(&self) {
        self.detail_return_offset.set(Some(self.detail_offset.get()));
    }

    /// 回到上次跳转前的位置。
    fn detail_return_pos(&self) {
        if let Some(offset) = self.detail_return_offset.get() {
            self.detail_scroll
                .get()
                .scroll_to(point(px(0.0), px(offset)), true);
            self.detail_return_offset.set(None);
        }
    }

    /// 跳到页首。
    fn detail_scroll_top(&self) {
        self.detail_mark_return();
        self.detail_scroll
            .get()
            .scroll_to(point(px(0.0), px(0.0)), true);
    }

    /// 跳到页尾（当前已加载内容的最底部）。
    fn detail_scroll_bottom(&self) {
        self.detail_mark_return();
        self.detail_scroll.get().scroll_to_end(true);
    }

    /// 跳到指定楼层：楼层已在列表里就直接 reveal；未加载则先把缺失的页
    /// 连续加载进来，再由 effect 在楼层元素出现后 reveal。
    fn detail_goto_floor(&self, floor: i32) {
        if floor < 1 {
            self.note("楼层号要大于 0");
            return;
        }
        self.detail_mark_return();
        let loaded = self.posts.with(|p| p.iter().any(|r| r.floor == floor));
        if loaded {
            self.detail_pending_floor.set(Some(floor));
            return;
        }
        // 楼层不在已加载范围：连续加载到覆盖该楼层的页。Discuz 每页 50 层。
        let max_floor = self.posts.with(|p| p.last().map(|r| r.floor).unwrap_or(0));
        if floor <= max_floor {
            self.note(format!("没有第 {floor} 楼（最高 {max_floor} 楼）"));
            return;
        }
        self.detail_pending_floor.set(Some(floor));
        self.clone().load_pages_for_floor(floor);
    }

    /// 连续加载后续页，直到列表里出现目标楼层或到达末页。
    fn load_pages_for_floor(self, target_floor: i32) {
        if self.busy.get() {
            return;
        }
        let tid = self.tid.get();
        if tid.is_empty() {
            return;
        }
        self.busy.set(true);
        self.note(format!("正在翻页以定位第 {target_floor} 楼…"));
        spawn(async move {
            loop {
                let page = self.detail_page.get() + 1;
                let max = self.detail_max.get();
                if page > max {
                    self.note(format!("已到末页，没有第 {target_floor} 楼"));
                    break;
                }
                match self.client.thread_detail(&tid, page).await {
                    Ok(detail) if detail.has_content() => {
                        let (mut rows, _) = self.thread_rows(&detail.posts);
                        let reached = rows.iter().any(|r| r.floor >= target_floor);
                        self.posts.update(|list| list.append(&mut rows));
                        self.detail_page.set(page);
                        if reached {
                            self.note(format!("已翻到第 {page} 页，定位第 {target_floor} 楼"));
                            break;
                        }
                    }
                    _ => {
                        self.note(format!("第 {page} 页加载失败或为空，停止翻页"));
                        break;
                    }
                }
            }
            self.busy.set(false);
        });
    }

    /// 跳到指定页：连续加载到该页并定位到该页首层（Discuz 每页 50 层）。
    fn detail_goto_page(&self, page: i32) {
        if page < 1 {
            self.note("页码要大于 0");
            return;
        }
        let max = self.detail_max.get();
        if page > max {
            self.note(format!("本主题只有 {max} 页"));
            return;
        }
        let first_floor = (page - 1) * 50 + 1;
        self.note(format!("跳到第 {page} 页…"));
        self.detail_goto_floor(first_floor);
    }

    /// 跳到新回复：进度里记录的最后楼层之后的第一层楼。
    fn detail_goto_new_reply(&self) {
        let tid = self.tid.get();
        let last = self
            .detail_progress
            .get()
            .get(&tid)
            .map(|p| p.last_floor)
            .unwrap_or(0);
        if last <= 0 {
            self.note("还没有上次浏览记录，从头开始看即可");
            return;
        }
        let next = last + 1;
        self.note(format!("跳到第 {next} 楼（上次看到 {last} 楼）"));
        self.detail_goto_floor(next);
    }

    fn open_thread(self, tid: String, title: String) {
        // 记录来源：可能从版块列表进入，也可能从通知 / 搜索结果进入。
        self.origin.set(self.screen.get());
        // 已读淡化：本次会话打开过的帖子加入已读集合。
        if !tid.is_empty() {
            self.read_tids.update(|set| {
                set.insert(tid.clone());
            });
        }

        // 浏览位置：换主题要清掉上一主题的跳转状态，再读入本主题的进度。
        // 进度来自内存表或 Storage（跨会话），由 detail_view 的 effect 在楼层
        // 渲染出来后执行 scroll_to，避免内容还没布局就跳导致位置不准。
        self.detail_offset.set(0.0);
        self.detail_return_offset.set(None);
        self.detail_pending_floor.set(None);
        self.detail_floor_refs.borrow_mut().clear();
        let progress = self
            .detail_progress
            .get()
            .get(&tid)
            .copied()
            .or_else(|| cache::get_progress::<ThreadProgress>(&tid));
        if let Some(p) = progress {
            self.detail_progress.update(|m| {
                m.insert(tid.clone(), p);
            });
            if p.offset > 1.0 {
                self.detail_restore_offset.set(Some(p.offset));
            }
        } else {
            self.detail_restore_offset.set(None);
        }

        // 先放缓存：点开帖子立刻有内容，网络请求在后台跑。
        // 楼层结构（引用/图片/代码块）都缓存在 `posts` 里，恢复后排版一致。
        let key = cache::detail_key(&tid, 1);
        let cached = cache::get::<CachedDetail>(&key);
        if let Some((cached, age)) = &cached {
            self.posts.set(cached.posts.clone());
            self.detail_page.set(1);
            self.detail_max.set(cached.max_page.max(1));
            if !cached.title.is_empty() {
                self.title.set(cached.title.clone());
            }
            if cached.fid > 0 {
                self.fid.set(cached.fid);
            }
            self.tid.set(tid.clone());
            self.screen.set(Screen::Detail);
            self.note(format!(
                "显示{}的缓存，正在更新…",
                cache::humanize_age(*age)
            ));
        } else {
            self.note("正在打开主题…");
        }
        self.busy.set(cached.is_none());

        spawn(async move {
            match self.client.thread_detail(&tid, 1).await {
                Ok(detail) if detail.has_content() => {
                    let (rows, unlisted) = self.thread_rows(&detail.posts);
                    self.posts.set(rows);
                    self.detail_page.set(detail.page.max(1));
                    self.detail_max.set(detail.max_page.max(1));
                    self.image_host_hint.set(if unlisted.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "有 {} 个外链图片域名未加入 nsh.json：{}（图片暂不加载，按 README 流程补白名单）",
                            unlisted.len(),
                            unlisted.iter().cloned().collect::<Vec<_>>().join("、")
                        )
                    });
                    self.title.set(if detail.title.is_empty() { title } else { detail.title });
                    self.tid.set(tid);
                    if detail.fid > 0 {
                        self.fid.set(detail.fid);
                    }
                    // 投票表单在主题级，只在第 1 页出现；翻到别的主题要清空。
                    if let Some(poll) = &detail.poll {
                        self.poll_title.set(poll.title.clone());
                        self.poll_footer.set(poll.footer.clone());
                        self.poll_hash.set(poll.form_hash.clone());
                        self.poll_multi.set(poll.max_answer > 1);
                        self.poll_max.set(poll.max_answer.max(1));
                        self.poll_rows.set(
                            poll.options
                                .iter()
                                .map(|o| PollOptionRow {
                                    id: o.id.clone(),
                                    text: o.text.clone(),
                                    rates: o.rates.clone(),
                                })
                                .collect(),
                        );
                    } else {
                        self.poll_title.set(String::new());
                        self.poll_rows.set(Vec::new());
                    }
                    self.poll_selected.set(Vec::new());
                    self.screen.set(Screen::Detail);
                    let title_now = self.title.get();
                    let fid_now = self.fid.get();
                    cache::put(
                        &key,
                        &CachedDetail {
                            posts: self.posts.get(),
                            max_page: detail.max_page.max(1),
                            title: title_now,
                            fid: fid_now,
                        },
                    );
                    self.note(format!("已加载 {} 层楼", detail.posts.len()));
                }
                Ok(_) => self.fail("页面未解析出楼层，可能需要登录或站点已改版"),
                Err(error) => {
                    // 缓存已经画出来了就继续用，别把它换成错误页。
                    if cached.is_some() {
                        self.note(format!("更新失败（{error}），继续显示本机缓存"));
                    } else {
                        self.report_error(error, "该主题");
                    }
                }
            }
            self.busy.set(false);
        });
    }

    /// 发表新主题。与回帖同一套守卫，但要多两件事：
    /// 取 `post.php?action=newthread` 的 formhash，以及带上标题与 fid。
    fn send_new_thread(self, subject: String, message: String) {
        if !self.logged_in.get() {
            self.fail("请先登录后再发帖");
            return;
        }
        if subject.trim().is_empty() {
            self.fail("标题不能为空");
            return;
        }
        if message.trim().is_empty() {
            self.fail("内容不能为空");
            return;
        }
        let fid = self.fid.get();
        if fid <= 0 {
            self.fail("请先进入一个版块再发帖");
            return;
        }
        if self.busy.get() {
            return;
        }
        let subject = subject.trim().to_string();
        self.busy.set(true);
        self.note("正在提交新主题…");
        spawn(async move {
            let message = self.settings.get().with_tail(&message);
            // 新帖表单在 post.php，formhash 与板块页不同，必须单独取一次。
            let url = format!("{}{fid}", self.client.endpoints().new_thread);
            let result = async {
                let page = self.client.get(&url).await?;
                let hash = parse_form_hash(&parse_document(&page.body));
                if hash.is_empty() {
                    return Err(forum_core::Error::Protocol(
                        "未取到发帖 formhash，登录态可能已失效".into(),
                    ));
                }
                let request = PostRequest {
                    mode: PostMode::New,
                    fid: fid.to_string(),
                    form_hash: hash,
                    subject: subject.clone(),
                    message,
                    ..Default::default()
                };
                self.client.submit_post(&request).await
            }
            .await;
            match result {
                Ok(outcome) if outcome.ok => {
                    self.editor.set(String::new());
                    self.compose_subject.set(String::new());
                    session::clear_draft();
                    self.note("主题已发布");
                    if outcome.tid.is_empty() {
                        self.clone().load_threads(fid, 1);
                    } else {
                        self.screen.set(Screen::Detail);
                        self.clone().open_thread(outcome.tid, subject);
                    }
                }
                Ok(outcome) => self.fail(if outcome.message.is_empty() {
                    "新主题未被论坛接受".to_string()
                } else {
                    outcome.message
                }),
                Err(error) => self.fail(error.to_string()),
            }
            self.busy.set(false);
        });
    }

    fn send_reply(self, message: String) {
        if !self.logged_in.get() {
            self.fail("请先登录后再回帖");
            return;
        }
        // 与网页端同一套规则（`postminchars = 5`，按字符计）：
        // 字数不够直接提示，不去打一趟注定被站点驳回的请求。
        if let Err(reason) = forum_core::post::validate_message(&message) {
            self.fail(reason);
            return;
        }
        let tid = self.tid.get();
        if tid.is_empty() {
            self.fail("缺少主题编号");
            return;
        }
        let fid = self.fid.get();
        if self.busy.get() {
            return;
        }
        self.busy.set(true);
        self.note("正在提交回帖…");
        spawn(async move {
            // 小尾巴在提交前拼：编辑器里看到的是正文本身，尾巴不占草稿，
            // 关掉开关后下次回帖自然就不带了。
            let message = self.settings.get().with_tail(&message);
            // Discuz 的 formhash 只存在于详情页 HTML，内核的详情模型不含它，
            // 因此提交前重新取一次页面并解析（GET，不产生副作用）。
            let url = format!("{}{}&page=1", self.client.endpoints().detail_list, tid);
            let result = async {
                let page = self.client.get(&url).await?;
                let hash = parse_form_hash(&parse_document(&page.body));
                if hash.is_empty() {
                    return Err(forum_core::Error::Protocol("未取到 formhash，登录态可能已失效".into()));
                }
                let request = PostRequest {
                    mode: PostMode::Reply,
                    tid: tid.clone(),
                    // fid 要带给站点：网页端快速回复表单里也有这一项，
                    // 缺它时部分版块会走到「无权限」分支。
                    fid: fid.to_string(),
                    form_hash: hash,
                    message,
                    ..Default::default()
                };
                self.client.submit_post(&request).await
            }
            .await;
            match result {
                Ok(outcome) if outcome.ok => {
                    self.editor.set(String::new());
                    session::clear_draft();
                    self.screen.set(Screen::Detail);
                    self.note("回帖已发布");
                    self.clone().open_thread(tid, self.title.get());
                }
                Ok(outcome) => self.fail(if outcome.message.is_empty() {
                    "回帖未被论坛接受".to_string()
                } else {
                    outcome.message
                }),
                Err(error) => self.fail(error.to_string()),
            }
            self.busy.set(false);
        });
    }

    /// 选一张本地图片上传到论坛，并把 `[attachimg]` BBCode 插入回帖正文。
    ///
    /// 上传前重新 GET 详情页取最新 formhash（与回帖同一约束）。
    /// 图片字节来自系统 picker，不落盘、不缓存。
    fn attach_image(self) {
        if !self.logged_in.get() {
            self.fail("请先登录后再上传图片");
            return;
        }
        if self.busy.get() {
            return;
        }
        self.busy.set(true);
        self.note("请选择一张图片…");
        spawn(async move {
            let files = pick_files(FilePickRequest::new(false, 1)).await;
            let Some(file) = files.into_iter().next() else {
                self.note("已取消选择图片");
                self.busy.set(false);
                return;
            };
            if !file.mime.starts_with("image/") {
                self.fail("所选文件不是图片");
                self.busy.set(false);
                return;
            }
            let result = async {
                let tid = self.tid.get();
                if tid.is_empty() {
                    return Err(forum_core::Error::Protocol("缺少主题编号".into()));
                }
                let url = format!("{}{}&page=1", self.client.endpoints().detail_list, tid);
                let page = self.client.get(&url).await?;
                let hash = parse_form_hash(&parse_document(&page.body));
                if hash.is_empty() {
                    return Err(forum_core::Error::Protocol(
                        "未取到 formhash，登录态可能已失效".into(),
                    ));
                }
                self.client
                    .upload_image(&hash, &file.name, file.data.bytes(), &file.mime)
                    .await
            }
            .await;
            match result {
                Ok(bbcode) => {
                    let mut text = self.editor.get();
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&bbcode);
                    self.editor.set(text.clone());
                    session::save_draft(&text);
                    self.note("图片已上传，BBCode 已插入正文");
                }
                Err(error) => self.fail(error.to_string()),
            }
            self.busy.set(false);
        });
    }
}

/// 登录成功后的落点：Discovery 第 1 页。抽成纯函数便于回归，
/// 防止以后又退回「停在登录页」或「沿用上次翻到的版块」。
fn post_login_target() -> (Screen, i32, i32) {
    (Screen::Threads, FID_DISCOVERY, 1)
}

/// 搜索入口统一构造：`fid=0` 表示不限版块，中文关键词由内核按 GBK 编码。
/// 标题 / 全文、版块与时间范围由界面参数传入（设置页会预填默认值）。
fn search_source(query: &str, full_text: bool, fid: String, time: SearchTime) -> ListSource {
    ListSource::Search {
        query: query.trim().to_string(),
        author: String::new(),
        fid,
        full_text,
        srchfrom: time.seconds(),
        page: 1,
    }
}

/// 只在用户交互后才调用：返回第一条需要修正的字段说明。
fn validate_login(username: &str, password: &str) -> String {
    if username.trim().is_empty() {
        "请输入用户名".to_string()
    } else if password.is_empty() {
        "请输入密码".to_string()
    } else {
        String::new()
    }
}

/// 当前应该显示的字段级校验错误（未交互时为空）。
fn field_error(ui: &Ui) -> String {
    if !ui.touched.get() {
        return String::new();
    }
    validate_login(&ui.username.get(), &ui.password.get())
}

// ---------------------------------------------------------------- 图标
//
// 统一 24×24 viewBox、2px 描边、圆头圆角；颜色不写死在 SVG 里，而是用
// `tint_color` 跟随主题前景色，深浅色共用一套资源。

const ICON_BACK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg>"##;
const ICON_HOME: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 11.5 12 4l9 7.5"/><path d="M6 10.5V20h12v-9.5"/></svg>"##;
const ICON_REFRESH: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.5 12a8.5 8.5 0 1 1-2.5-6"/><path d="M20.5 4v5h-5"/></svg>"##;
const ICON_REPLY: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 12H5"/><path d="M11 6l-6 6 6 6"/></svg>"##;
const ICON_NEXT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>"##;
const ICON_MAIL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5.5" width="18" height="13" rx="2"/><path d="M4 7.5l8 5.5 8-5.5"/></svg>"##;
const ICON_BELL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 8a6 6 0 1 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9"/><path d="M10.3 21a2 2 0 0 0 3.4 0"/></svg>"##;
const ICON_SEARCH: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.6-3.6"/></svg>"##;
const ICON_PERSON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"/><path d="M4 21c0-4 3.6-7 8-7s8 3 8 7"/></svg>"##;
const ICON_LOGOUT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3"/><path d="M10 16l4-4-4-4"/><path d="M14 12H4"/></svg>"##;
const ICON_GEAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3.2"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1A1.7 1.7 0 0 0 4.6 8a1.7 1.7 0 0 0-.3-1.9l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.9.3H9a1.7 1.7 0 0 0 1-1.5V2a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.9-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.9V8a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>"##;
const ICON_MORE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#000" stroke="none"><circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/></svg>"##;
const ICON_CHECK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7"/></svg>"##;
const ICON_EDIT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4z"/></svg>"##;
const ICON_CHEVRON_DOWN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>"##;

/// 图标：尺寸即视觉尺寸（图标应略小于同行文字，视觉上才不抢眼）。
fn icon_secondary(markup: &'static str, size: f32) -> impl IntoElement {
    svg(nsh_core::element::ImageSrc::svg_markup(markup))
        .w(px(size))
        .h(px(size))
        .tint_color(move || theme().secondary_foreground)
}

fn icon_on_primary(markup: &'static str, size: f32) -> impl IntoElement {
    svg(nsh_core::element::ImageSrc::svg_markup(markup))
        .w(px(size))
        .h(px(size))
        .tint_color(move || theme().primary_foreground)
}

/// 主按钮。桌面端鼠标精度高，视觉高度收到 34pt（原 44pt 一眼就"笨重"），
/// 圆角 8、字号 13，图标与文字间距 6pt（约 0.45em，图标不抢文字的视线）。
fn primary_button(label: String, on_press: impl Fn() + 'static) -> impl IntoElement {
    primary_with(None, label, on_press)
}

fn primary_button_icon(
    markup: &'static str,
    label: String,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    primary_with(Some(markup), label, on_press)
}

fn primary_with(
    markup: Option<&'static str>,
    label: String,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .h(px(34.0))
        .px(px(12.0))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(8.0))
        .bg(move || theme().primary)
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(dynamic(move || match markup {
            Some(m) => vec![icon_on_primary(m, 15.0).into_element()],
            None => Vec::new(),
        }))
        .child(
            text(move || label.clone())
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().primary_foreground),
        )
}

/// 次要按钮，比主按钮再低一档（30pt），用于导航与轻量操作。
fn ghost_button(label: String, on_press: impl Fn() + 'static) -> impl IntoElement {
    ghost_with(None, label, on_press)
}

fn ghost_button_icon(
    markup: &'static str,
    label: String,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    ghost_with(Some(markup), label, on_press)
}

fn ghost_with(
    markup: Option<&'static str>,
    label: String,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .h(px(30.0))
        .px(px(10.0))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(5.0))
        .rounded(px(7.0))
        .bg(move || theme().secondary)
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(dynamic(move || match markup {
            Some(m) => vec![icon_secondary(m, 14.0).into_element()],
            None => Vec::new(),
        }))
        .child(
            text(move || label.clone())
                .text_size(px(12.5))
                .text_color(move || theme().secondary_foreground),
        )
}

fn field(
    placeholder: &'static str,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
    on_submit: impl Fn(String) + 'static,
    fill: AutoFill,
    masked: bool,
) -> impl IntoElement {
    input()
        .placeholder(placeholder)
        .value(value)
        .on_change(move |v: &str| on_change(v.to_string()))
        .on_submit(move |v: &str| on_submit(v.to_string()))
        .auto_fill(fill)
        .auto_correct(false)
        .auto_capitalize(AutoCapitalize::None)
        .masked(masked)
        .min_h(px(44.0))
        .px_4()
        .py_2()
        .rounded(px(10.0))
        // 卡片色与页面底色接近，只靠底色看不出这是个输入框；
        // 加一层 1pt 描边，登录页才不像「两行裸文字」。
        .border_1()
        .border_color(move || theme().muted)
        .bg(move || theme().card)
        .text_color(move || theme().card_foreground)
}

/// 论坛首页：与站点 `index.php` 一致，按分区列出版块。
/// 游客也能打开，点版块时才由站点决定是否放行。
fn index_view(ui: Ui) -> impl IntoElement {
    let for_subtitle = ui.clone();
    let for_sections = ui.clone();
    let for_refresh = ui.clone();
    let for_notifications = ui.clone();
    let for_search = ui.clone();
    let for_hub = ui.clone();

    view()
        .flex_1()
        .gap(px(16.0))
        .px(px(16.0))
        .py(px(16.0))
        // 登录 / 退出已上移到顶部工具条，这里只留页面标题。
        .child(text("论坛首页").text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD))
        .child(
            text(move || for_subtitle.index_status.get())
                .text_size(px(12.0))
                .text_color(move || theme().muted_foreground),
        )
        // 论坛首页也要能直达个人功能，不必先钻进某个版块再返回。
        .child(
            view()
                .flex_row()
                .gap_2()
                // 不再等宽撑满：三个入口按内容宽度排成一行胶囊，视觉重量轻得多。
                .child(ghost_button_icon(ICON_BELL, "通知".into(), move || {
                    for_notifications
                        .clone()
                        .load_list(ListSource::Notifications)
                }))
                .child(ghost_button_icon(ICON_SEARCH, "搜索".into(), move || {
                    for_search.open_search()
                }))
                .child(ghost_button_icon(ICON_PERSON, "我的空间".into(), move || {
                    for_hub.screen.set(Screen::Hub)
                })),
        )
        .child(scroll().flex_1().child(
            view().w_full().gap(px(20.0)).child(dynamic(move || {
                let sections = for_sections.sections.get();
                if sections.is_empty() {
                    return vec![text("正在加载论坛首页…")
                        .text_size(px(14.0))
                        .text_color(move || theme().muted_foreground)
                        .into_element()];
                }
                sections
                    .iter()
                    .map(|section| {
                        let name = section.name.clone();
                        let forums = section.forums.clone();
                        let ui = for_sections.clone();
                        view()
                            .gap(px(8.0))
                            .child(
                                // 分区名是「分组标签」而不是标题：降字号、用次级色，
                                // 让视线先落在版块名上，层级才立得住。
                                text(move || name.clone())
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(move || theme().muted_foreground),
                            )
                            .child(
                                // 一个分区一个容器、行间 1pt 细线：替代「一版块一张卡」，
                                // 一屏的色块从十几个降到三四个，视觉噪音小得多。
                                view()
                                    .rounded(px(12.0))
                                    .bg(move || theme().card)
                                    .child(dynamic(move || {
                                        let mut items: Vec<AnyElement> =
                                            Vec::with_capacity(forums.len() * 2);
                                        for (index, f) in forums.iter().enumerate() {
                                            if index > 0 {
                                                items.push(
                                                    view()
                                                        .h(px(1.0))
                                                        .w_full()
                                                        .bg(move || theme().muted)
                                                        .into_element(),
                                                );
                                            }
                                            let ui = ui.clone();
                                            let fid = f.fid;
                                            let fname = f.name.clone();
                                            let desc = f.description.clone();
                                            let counts = f.counts.clone();
                                            let today = f.today.clone();
                                            let last = f.last.clone();
                                            items.push(
                                                view()
                                                    .gap(px(4.0))
                                                    .px(px(16.0))
                                                    .py(px(12.0))
                                                    .active(|s| s.opacity(0.6))
                                                    .on_press(move || {
                                                        ui.clone().load_threads(fid, 1)
                                                    })
                                                    .child(
                                                        view()
                                                            .flex_row()
                                                            .items_center()
                                                            .justify_between()
                                                            .child(
                                                                text(move || fname.clone())
                                                                    .text_size(px(15.0))
                                                                    .font_weight(FontWeight::MEDIUM),
                                                            )
                                                            .child(
                                                                text(move || today.clone())
                                                                    .text_size(px(11.5))
                                                                    .text_color(move || {
                                                                        theme()
                                                                            .color("unread")
                                                                            .unwrap_or(
                                                                                theme().primary,
                                                                            )
                                                                    }),
                                                            ),
                                                    )
                                                    .child(
                                                        text(move || desc.clone())
                                                            .text_size(px(12.0))
                                                            .text_color(move || {
                                                                theme().muted_foreground
                                                            }),
                                                    )
                                                    .child(
                                                        view()
                                                            .flex_row()
                                                            .gap(px(10.0))
                                                            .child(
                                                                text(move || counts.clone())
                                                                    .text_size(px(11.0))
                                                                    .text_color(move || {
                                                                        theme().muted_foreground
                                                                    }),
                                                            )
                                                            .child(
                                                                text(move || last.clone())
                                                                    .text_size(px(11.0))
                                                                    .text_color(move || {
                                                                        theme().muted_foreground
                                                                    }),
                                                            ),
                                                    )
                                                    .into_element(),
                                            );
                                        }
                                        items
                                    })),
                            )
                            .into_element()
                            .into_element()
                    })
                    .collect::<Vec<AnyElement>>()
            })),
        ))
        // 整行按钮被 stretch 撑满，视觉上像条横幅；改成右对齐的小按钮。
        .child(
            view()
                .flex_row()
                .justify_end()
                .child(ghost_button_icon(ICON_REFRESH, "刷新".into(), move || {
                    for_refresh.clone().load_index()
                })),
        )
}

fn login_view(ui: Ui) -> impl IntoElement {
    let for_user = ui.clone();
    let for_pass = ui.clone();
    let for_button = ui.clone();
    let for_submit = ui.clone();
    let for_error = ui.clone();
    let for_home = ui.clone();
    let for_remember = ui.clone();

    view()
        .flex_1()
        .justify_center()
        .gap_4()
        .px_5()
        .child(text("登录 HiPDA 论坛").text_size(px(22.0)).font_weight(FontWeight::BOLD))
        .child(
            text("密码不会保存；登录状态只存在本机，下次启动自动恢复，可随时退出登录。客户端不会代替你做任何自动化操作。")
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(field(
            "用户名",
            move || for_user.username.get(),
            {
                let ui = ui.clone();
                move |v| ui.username.set(v)
            },
            {
                let ui = for_submit.clone();
                move |_| {
                    let ui = ui.clone();
                    ui.clone().login(ui.username.get(), ui.password.get());
                }
            },
            AutoFill::Username,
            false,
        ))
        .child(field(
            "密码",
            move || for_pass.password.get(),
            {
                let ui = ui.clone();
                move |v| ui.password.set(v)
            },
            {
                let ui = for_submit.clone();
                move |_| {
                    let ui = ui.clone();
                    ui.clone().login(ui.username.get(), ui.password.get());
                }
            },
            AutoFill::Password,
            true,
        ))
        // 校验反馈：只在交互后出现，且同时播报给屏幕阅读器。
        .child(dynamic(move || {
            let message = field_error(&for_error);
            if message.is_empty() {
                None
            } else {
                Some(
                    text(move || format!("⚠ {message}"))
                        .text_size(px(13.0))
                        .text_color(move || theme().destructive)
                        .into_element(),
                )
            }
        }))
        // 「保持登录」：勾选才落会话快照。说明写清楚存的是凭据不是密码，
        // 免得上一次「密码不会保存」的承诺和这个开关互相打脸。
        .child(dynamic(move || {
            let ui = for_remember.clone();
            let on = ui.remember.get();
            let toggle = ui.clone();
            vec![
                view()
                    .flex_row()
                    // 与标题首行对齐，而不是与两行文字的整体中心对齐 ——
                    // 勾选框挂在第二行旁边会像是属于说明文字。
                    .items_start()
                    .gap(px(10.0))
                    .active(|s| s.opacity(0.7))
                    .on_press(move || {
                        let ui = toggle.clone();
                        let next = !ui.remember.get();
                        session::set_remember(next);
                        ui.remember.set(next);
                        if !next {
                            ui.note("已关闭保持登录：本次退出后需要重新登录");
                        }
                    })
                    .child(view().pt(px(1.0)).child(check_mark(on)))
                    .child(
                        view()
                            .flex_1()
                            .gap(px(2.0))
                            .child(
                                text("保持登录")
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(
                                text("只保存登录凭据，不保存密码；下次打开免登录")
                                    .text_size(px(11.5))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    )
                    .into_element(),
            ]
        }))
        .child(primary_button("登录".into(), move || {
            let ui = for_button.clone();
            ui.clone().login(ui.username.get(), ui.password.get());
        }))
        // 「返回首页」放在登录按钮下方：主操作在上，退路在下，
        // 它也不该抢走第一个焦点。
        .child(ghost_button_icon(ICON_HOME, "返回首页".into(), move || {
            for_home.screen.set(Screen::Index)
        }))
}

fn threads_view(ui: Ui) -> impl IntoElement {
    let for_forums = ui.clone();
    let for_rows = ui.clone();
    let for_empty = ui.clone();
    let for_count = ui.clone();
    let for_keys = ui.clone();
    let for_hub = ui.clone();
    let for_prev = ui.clone();
    let for_next = ui.clone();
    let for_status = ui.clone();
    let for_notifications = ui.clone();
    let for_search = ui.clone();
    let for_menu = ui.clone();
    let for_overlay = ui.clone();

    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        // 版块菜单：发表新帖 / 排序 / 置顶 / 站内地址（对齐原版三点菜单）。
        // 首页 / 登录入口已上移到三段顶栏，这里只保留三点菜单。
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .child({
                    let ui = for_menu.clone();
                    ghost_button_icon(ICON_MORE, "菜单".into(), move || {
                        let ui = ui.clone();
                        let next = !ui.menu_open.get();
                        ui.menu_open.set(next);
                    })
                }),
        )
        // 版块切换：三个默认版块，触控目标 ≥44pt。
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(dynamic(move || {
                    // 版块标签来自设置里的「默认版面」；字体名优先站点返回的名字，
                    // 站点不可达时用内置表兜底，保证标签不会变成数字。
                    let current = for_forums.fid.get();
                    for_forums
                        .settings
                        .get()
                        .tabs()
                        .into_iter()
                        .map(|fid| {
                            let label = for_forums
                                .sections
                                .with(|sections| {
                                    sections
                                        .iter()
                                        .flat_map(|s| s.forums.iter())
                                        .find(|f| f.fid == fid)
                                        .map(|f| f.name.clone())
                                })
                                .filter(|name| !name.is_empty())
                                .unwrap_or_else(|| forum_name(fid).to_string());
                            let ui = for_forums.clone();
                            let active = fid == current;
                            let button = ghost_button(label, move || ui.clone().load_threads(fid, 1));
                            // 当前版块用主色标出来，否则几个标签长得一模一样，
                            // 翻两页就不知道自己在哪个版了。
                            if active {
                                // 主色描边用「容器当底色 + 1.5pt 内边距」实现：
                                // ghost_button 返回的是 impl IntoElement，不能直接改边框样式。
                                view()
                                    .p(px(1.5))
                                    .rounded(px(8.5))
                                    .bg(move || theme().primary)
                                    .child(button)
                                    .into_element()
                            } else {
                                button.into_element()
                            }
                        })
                        .collect::<Vec<AnyElement>>()
                })),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(text(move || format!("第 {}/{} 页", ui.page.get(), ui.max_page.get()))
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground))
                .child(
                    view()
                        .flex_row()
                        .gap_2()
                        .child(ghost_button("上一页".into(), move || {
                            let ui = for_prev.clone();
                            let page = (ui.page.get() - 1).max(1);
                            ui.clone().load_threads(ui.fid.get(), page);
                        }))
                        .child(ghost_button("下一页".into(), move || {
                            let ui = for_next.clone();
                            let page = (ui.page.get() + 1).min(ui.max_page.get().max(1));
                            ui.clone().load_threads(ui.fid.get(), page);
                        })),
                ),
        )
        .child(dynamic(move || {
            if for_empty.rows.with(|r| r.is_empty()) {
                vec![text("暂无主题，点击上方版块或「刷新」加载")
                    .text_size(px(14.0))
                    .text_color(move || theme().muted_foreground)
                    .into_element()]
            } else {
                Vec::new()
            }
        }))
        // 虚拟化列表：只创建视口附近的行，翻到 1000 页也不会一次性建几千个节点。
        // 行内容必须在绑定里读 `at.index()`，行被回收复用时才会跟着换数据。
        .child(
            virtual_list(
                move || for_count.rows.with(|r| r.len()),
                move |at| {
                    let ui = for_rows.clone();
                    let ui_title = ui.clone();
                    let ui_meta = ui.clone();
                    let ui_badge = ui.clone();
                    let ui_bg = ui.clone();
                    let ui_avatar = ui.clone();
                    let ui_tint = ui.clone();
                    let ui_dot = ui.clone();
                    let settings = ui.settings.get();
                    let row_height = settings.spacing.row_height();
                    let title_size = settings.title_size.pt();
                    let title_weight = title_font_weight(settings.title_weight);
                    let alternating = settings.alternating_rows;
                    let read_fade = settings.read_fade;
                    let accent_key = settings.skin.color_key();
                    let index = at.index();
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .min_h(px(row_height))
                        .rounded(px(10.0))
                        .bg(move || {
                            let sticky = ui_bg
                                .rows
                                .with(|r| r.get(index).is_some_and(|x| x.sticky));
                            if sticky {
                                theme().color("quote").unwrap_or(theme().card)
                            } else if alternating && index % 2 == 1 {
                                theme().color("row-alt").unwrap_or(theme().card)
                            } else {
                                theme().card
                            }
                        })
                        .active(|s| s.opacity(0.6))
                        .on_press({
                            let ui = ui.clone();
                            move || {
                                let row = ui.rows.with(|r| r.get(index).cloned());
                                if let Some(row) = row {
                                    ui.clone().open_thread(row.tid, row.title);
                                }
                            }
                        })
                        // 头像
                        .child(dynamic(move || {
                            let row = ui_avatar.rows.with(|r| r.get(index).cloned());
                            match row {
                                Some(row) => vec![list_avatar(
                                    ui_avatar.clone(),
                                    row.avatar,
                                    row.author,
                                )
                                .into_element()],
                                None => Vec::new(),
                            }
                        }))
                        .child(
                            view()
                                .flex_1()
                                .gap_1()
                                .child(
                                    text(move || {
                                        ui_title.rows.with(|r| {
                                            r.get(index)
                                                .map(|x| x.title.clone())
                                                .unwrap_or_default()
                                        })
                                    })
                                    .text_size(px(title_size))
                                    .font_weight(title_weight)
                                    .text_color(move || {
                                        let read = ui_tint.read_tids.with(|set| {
                                            ui_tint.rows.with(|r| {
                                                r.get(index)
                                                    .map(|x| set.contains(&x.tid))
                                                    .unwrap_or(false)
                                            })
                                        });
                                        if !read {
                                            theme().foreground
                                        } else {
                                            match read_fade {
                                                ReadFade::None => theme().foreground,
                                                ReadFade::Light => theme().muted_foreground,
                                                ReadFade::Heavy => theme().color("floor")
                                                    .unwrap_or(theme().muted_foreground),
                                            }
                                        }
                                    }),
                                )
                                .child(
                                    text(move || {
                                        ui_meta.rows.with(|r| {
                                            r.get(index)
                                                .map(|x| x.meta.clone())
                                                .unwrap_or_default()
                                        })
                                    })
                                    .text_size(px(12.0))
                                    .text_color(move || theme().muted_foreground),
                                ),
                        )
                        // 未读圆点 + 新回复胶囊 + 图片标记
                        .child(dynamic(move || {
                            let row = ui_dot.rows.with(|r| r.get(index).cloned());
                            let Some(row) = row else { return Vec::new() };
                            let mut out: Vec<AnyElement> = Vec::new();
                            if settings.unread_dot && row.is_new {
                                out.push(
                                    view()
                                        .w(px(8.0))
                                        .h(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(move || theme().color(accent_key)
                                            .unwrap_or_else(|| theme().primary))
                                        .into_element(),
                                );
                            }
                            if settings.new_reply_mark && row.replies > 0 {
                                out.push(
                                    text(format!("{}", row.replies))
                                        .text_size(px(12.0))
                                        .text_color(move || theme().muted_foreground)
                                        .into_element(),
                                );
                            }
                            if row.with_picture {
                                out.push(
                                    text("图")
                                        .text_size(px(10.0))
                                        .text_color(move || theme().muted_foreground)
                                        .into_element(),
                                );
                            }
                            out
                        }))
                        .child(
                            text(move || {
                                ui_badge.rows.with(|r| {
                                    r.get(index)
                                        .map(|x| x.badge.clone())
                                        .unwrap_or_default()
                                })
                            })
                            .text_size(px(12.0))
                            .text_color(move || theme().color("unread").unwrap_or(theme().primary)),
                        )
                },
            )
            .keys(move || {
                for_keys
                    .rows
                    .with(|r| r.iter().map(|x| x.tid.clone()).collect::<Vec<_>>())
            })
            .estimated_item_size(px(64.0))
            .flex_1(),
        )
        // 与首页一致：入口按内容宽度排成胶囊，刷新右对齐；「退出登录」只在
        // 顶部工具条出现一次，这里不再重复画一个。
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .gap_2()
                        .child(ghost_button_icon(ICON_BELL, "通知".into(), move || {
                            for_notifications
                                .clone()
                                .load_list(ListSource::Notifications)
                        }))
                        .child(ghost_button_icon(ICON_SEARCH, "搜索".into(), move || {
                            for_search.open_search();
                        }))
                        .child(ghost_button_icon(ICON_PERSON, "我的空间".into(), move || {
                            for_hub.screen.set(Screen::Hub);
                        })),
                )
                .child(ghost_button_icon(ICON_REFRESH, "刷新".into(), move || {
                    let ui = for_status.clone();
                    ui.clone().load_threads(ui.fid.get(), ui.page.get());
                })),
        )
        // 「⋯」菜单：对齐原版版块页的三点菜单。用 overlay 而不是弹窗，
        // 面板贴在按钮下方，点空白处关闭。
        .child(dynamic(move || {
            let ui = for_overlay.clone();
            if !ui.menu_open.get() {
                return Vec::new();
            }
            let settings = ui.settings.get();
            let fid = ui.fid.get();

            let close = ui.clone();
            let new_post = ui.clone();
            let sort_ui = ui.clone();
            let sticky_ui = ui.clone();
            let link_ui = ui.clone();

            let items = vec![
                menu_item("发表新帖", None, move || {
                    let ui = new_post.clone();
                    ui.menu_open.set(false);
                    if !ui.logged_in.get() {
                        ui.clone().require_login();
                        return;
                    }
                    ui.compose_mode.set(PostMode::New);
                    ui.compose_subject.set(String::new());
                    ui.editor.set(String::new());
                    ui.screen.set(Screen::Compose);
                }),
                menu_item("按发表排序", Some(settings.sort_by_dateline), move || {
                    let ui = sort_ui.clone();
                    let next = !ui.settings.get().sort_by_dateline;
                    ui.menu_open.set(false);
                    ui.clone().apply_settings(|s| s.sort_by_dateline = next);
                    // 排序是站点侧参数，改完必须重新取第 1 页。
                    ui.clone().load_threads(ui.fid.get(), 1);
                }),
                menu_item("显示置顶帖", Some(settings.show_sticky), move || {
                    let ui = sticky_ui.clone();
                    let next = !ui.settings.get().show_sticky;
                    ui.menu_open.set(false);
                    ui.clone().apply_settings(|s| s.show_sticky = next);
                    ui.clone().load_threads(ui.fid.get(), 1);
                }),
                menu_item("打开站内地址", None, move || {
                    let ui = link_ui.clone();
                    ui.menu_open.set(false);
                    // 不弹浏览器（没有 openUrl 授权），复制链接更可控。
                    let url = format!("{}{fid}", ui.client.endpoints().thread_list);
                    set_clipboard(url.clone());
                    ui.note(format!("已复制站内地址：{url}"));
                }),
            ];

            vec![
                overlay()
                    .absolute()
                    .inset_0()
                    .child(
                        // 透明遮罩：点击菜单外任何地方关闭。
                        view()
                            .absolute()
                            .inset_0()
                            .on_press(move || close.menu_open.set(false)),
                    )
                    .child(
                        view()
                            .absolute()
                            .inset_0()
                            .items_end()
                            .justify_start()
                            .pt(px(56.0))
                            .pr(px(16.0))
                            .child(
                                view()
                                    .w(px(196.0))
                                    .py(px(6.0))
                                    .rounded(px(10.0))
                                    .border_1()
                                    .border_color(move || theme().muted)
                                    // 卡片与页面同为浅底，只靠阴影区分浮层，
                                    // 少了它菜单会和列表糊在一起。
                                    .shadow_lg()
                                    .bg(move || theme().card)
                                    .children(items),
                            ),
                    )
                    .into_element(),
            ]
        }))
}

/// 正文图片：先按 4:3 占位，加载完成后按真实宽高比重排。
///
/// 旧实现固定 `h(180)`：论坛里 434×580 的长图会被压成一条；更糟的是模板
/// 占位符（1×1 的 `none.gif`/`attachimg.gif`）也被拉成 180 高，整层看着
/// 就是一块空白 —— 表现为「帖子里的图片不显示」。
/// 正文排版整理：
///
/// - 去掉 `[图片]` 占位：图片已由 `post_image` 单独渲染，正文里再留一个
///   占位符只会显示成一行没意义的文字；
/// - 连续空行压成一个：Discuz 正文靠 `<br>` 排版，原文常有三四个空行，
///   在客户端里会撑出一大块空白。
fn tidy_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank = 0;
    for line in raw.lines() {
        let cleaned = line.replace("[图片]", "");
        let cleaned = cleaned.trim_end();
        if cleaned.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(cleaned);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// 列表头像（0.2 重做）：按排版样式 / 头像大小 / 形状 / 默认头像三件套渲染。
/// 无头像样式返回空节点；地址为空时按「首字 / 像素块 / 首字浅色」兜底。
fn list_avatar(ui: Ui, src: String, name: String) -> AnyElement {
    let settings = ui.settings.get();
    if settings.list_style == ListStyle::NoAvatar {
        return view().into_element();
    }
    let size = match settings.list_style {
        ListStyle::SmallAvatar => 34.0,
        _ => settings.avatar_size.pt(),
    };
    let radius = match settings.avatar_shape {
        AvatarShape::Round => size / 2.0,
        AvatarShape::Square => 9.0,
    };
    let base = view()
        .w(px(size))
        .h(px(size))
        .rounded(px(radius))
        .overflow_hidden()
        .bg(move || theme().muted);
    if !src.trim().is_empty() {
        return base
            .child(image(src).w_full().h_full().object_fit(nsh_core::ResizeMode::Cover))
            .into_element();
    }
    match settings.default_avatar {
        DefaultAvatar::Pixel => {
            // 名字哈希成 3×3 像素块，同一个人每次都是同一张「脸」。
            let mut seed: u32 = 2166136261;
            for b in name.bytes() {
                seed ^= b as u32;
                seed = seed.wrapping_mul(16777619);
            }
            let mut rows: Vec<AnyElement> = Vec::new();
            for r in 0..3usize {
                let mut cells: Vec<AnyElement> = Vec::new();
                for c in 0..3usize {
                    let on = ((seed >> (r * 3 + c)) & 1) == 1;
                    let key = settings.skin.color_key();
                    cells.push(
                        view()
                            .flex_1()
                            .bg(move || {
                                if on {
                                    theme().color(key).unwrap_or_else(|| theme().primary)
                                } else {
                                    theme().card
                                }
                            })
                            .into_element(),
                    );
                }
                rows.push(view().flex_row().flex_1().children(cells).into_element());
            }
            base.child(view().w_full().h_full().children(rows))
                .into_element()
        }
        style => {
            let light = style == DefaultAvatar::InitialLight;
            let first: String = name.chars().take(1).collect();
            let first = if first.is_empty() { "?".to_string() } else { first };
            let key = settings.skin.color_key();
            base.items_center()
                .justify_center()
                .bg(move || {
                    if light {
                        theme().color(key).unwrap_or_else(|| theme().primary)
                    } else {
                        theme().muted
                    }
                })
                .child(
                    text(move || first.clone())
                        .text_size(px(size * 0.42))
                        .text_color(move || theme().muted_foreground),
                )
                .into_element()
        }
    }
}

/// 标题字重档位 → NSH 字重常量。
fn title_font_weight(w: TitleWeight) -> FontWeight {
    match w {
        TitleWeight::Regular => FontWeight::NORMAL,
        TitleWeight::Medium => FontWeight::MEDIUM,
        TitleWeight::Semibold => FontWeight::SEMIBOLD,
        TitleWeight::Bold => FontWeight::BOLD,
    }
}

/// 楼层头头像：36pt。圆形是原版默认，设置里可以改成圆角方形；
/// 地址缺失或加载失败时保留底色 + 作者首字，不让楼层头塌掉。
fn post_avatar(ui: Ui, src: String, name: String) -> AnyElement {
    let for_shape = ui.clone();
    let base = view()
        .w(px(36.0))
        .h(px(36.0))
        .rounded(move || {
            if for_shape.settings.get().circle_avatar {
                px(18.0)
            } else {
                px(10.0)
            }
        })
        .overflow_hidden()
        .bg(move || theme().muted);
    if src.trim().is_empty() {
        let first = name.chars().next().map(|c| c.to_string()).unwrap_or_default();
        base.items_center()
            .justify_center()
            .child(
                text(move || first.clone())
                    .text_size(px(14.0))
                    .text_color(move || theme().muted_foreground),
            )
            .into_element()
    } else {
        base.child(
            image(src)
                .w_full()
                .h_full()
                .object_fit(nsh_core::ResizeMode::Cover),
        )
        .into_element()
    }
}

fn post_image(ui: Ui, src: String) -> AnyElement {
    let key = src.clone();
    let width_key = key.clone();
    let for_ratio = ui.clone();
    let for_width = ui.clone();
    let for_load = ui.clone();
    let load_src = src.clone();
    view()
        // 竖图（手机截图、相机竖拍）限宽居中，横图铺满。论坛原图 434×580
        // 这类占多数，拉满整宽会在两侧留出大片灰边，看着像「图没加载出来」。
        .w(move || {
            let ratio = for_width
                .image_ratio
                .with(|m| m.get(&width_key).copied())
                .unwrap_or(0.75);
            if ratio > 1.15 {
                relative(0.62)
            } else {
                relative(1.0)
            }
        })
        .aspect_ratio(move || {
            for_ratio
                .image_ratio
                .with(|m| m.get(&key).copied())
                .unwrap_or(0.75)
        })
        .rounded(px(10.0))
        .child(
            image(src)
                .w_full()
                .h_full()
                .object_fit(nsh_core::ResizeMode::Contain)
                .on_load(
                    move |size: &nsh_core::geometry::Size<nsh_core::geometry::DevicePixels>| {
                        let w = size.width.0 as f32;
                        let h = size.height.0 as f32;
                        if w > 0.0 && h > 0.0 {
                            let ratio = h / w;
                            for_load.image_ratio.update(|m| {
                                m.insert(load_src.clone(), ratio);
                            });
                        }
                    },
                ),
        )
        .into_element()
}

/// 按块渲染正文：段落 / 引用 / 列表 / 代码 / 图片 / 附件 / 分隔线。
///
/// 改块级渲染的原因：单段纯文本会把「引用」「列表」「代码」全部拍平，
/// 长帖读起来像一堵墙。块级结构由内核 `parse_blocks` 产出。
fn render_blocks(ui: Ui, blocks: &[forum_core::model::Block]) -> Vec<AnyElement> {
    let settings = ui.settings.get();
    let size = settings.text_scale.body_size();
    let line = settings.text_scale.line_height();
    let show_images = settings.show_images;
    blocks
        .iter()
        .map(|block| render_block(&ui, block, size, line, show_images))
        .collect()
}

fn render_block(
    ui: &Ui,
    block: &forum_core::model::Block,
    size: f32,
    line: f32,
    show_images: bool,
) -> AnyElement {
    use forum_core::model::Block;
    match block {
        Block::Paragraph(text_value) => {
            let body = text_value.clone();
            text(move || body.clone())
                .text_size(px(size))
                .line_height(px(line))
                .text_color(move || theme().card_foreground)
                .into_element()
        }
        Block::Quote {
            author,
            time,
            blocks,
        } => {
            // 有署名才画标题行：匿名引用（`<blockquote>` 缩进块）本来就没有
            // 「谁、什么时候」，硬写一个「引用」反而像漏了内容。
            let header = match (author.is_empty(), time.is_empty()) {
                (false, false) => format!("引用 {author} · {time}"),
                (false, true) => format!("引用 {author}"),
                (true, false) => format!("引用 · {time}"),
                (true, true) => String::new(),
            };
            let inner = render_blocks(ui.clone(), blocks);
            let mut container = view()
                .gap(px(6.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(move || theme().muted);
            if !header.is_empty() {
                container = container.child(
                    text(move || header.clone())
                        .text_size(px(11.5))
                        .text_color(move || theme().muted_foreground),
                );
            }
            container.children(inner).into_element()
        }
        Block::List { ordered, items } => {
            let ordered = *ordered;
            let mut rows: Vec<AnyElement> = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let marker = if ordered {
                    format!("{}.", index + 1)
                } else {
                    "•".to_string()
                };
                let body = item.clone();
                rows.push(
                    view()
                        .flex_row()
                        .gap(px(6.0))
                        .child(
                            text(move || marker.clone())
                                .text_size(px(size))
                                .line_height(px(line))
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text(move || body.clone())
                                .text_size(px(size))
                                .line_height(px(line))
                                .text_color(move || theme().card_foreground),
                        )
                        .into_element(),
                );
            }
            view().gap(px(2.0)).children(rows).into_element()
        }
        Block::Code(code) => {
            let body = code.clone();
            view()
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(move || theme().muted)
                .child(
                    text(move || body.clone())
                        .text_size(px((size - 2.0).max(11.0)))
                        .line_height(px(line - 3.0))
                        .text_color(move || theme().card_foreground),
                )
                .into_element()
        }
        Block::Image { url, .. } => {
            if show_images {
                post_image(ui.clone(), url.clone())
            } else {
                text("本层有图片，已按设置在「阅读与图片」中隐藏")
                    .text_size(px(11.5))
                    .text_color(move || theme().muted_foreground)
                    .into_element()
            }
        }
        Block::Attach { name, url, size } => {
            let label = if name.is_empty() {
                url.clone()
            } else {
                name.clone()
            };
            let size_hint = *size;
            text(move || {
                if size_hint > 0 {
                    format!("附件：{label}（{size_hint} 字节）")
                } else {
                    format!("附件：{label}")
                }
            })
            .text_size(px(12.5))
            .text_color(move || theme().primary)
            .into_element()
        }
        Block::Heading(text_value) => {
            let body = text_value.clone();
            text(move || body.clone())
                .text_size(px(size + 4.0))
                .line_height(px(line + 6.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().card_foreground)
                .into_element()
        }
        Block::Align { center, text: value } => {
            let body = value.clone();
            let center = *center;
            let container = if center {
                view().w_full().items_center()
            } else {
                view().w_full().items_end()
            };
            container
                .child(
                    text(move || body.clone())
                        .text_size(px(size))
                        .line_height(px(line))
                        .text_color(move || theme().card_foreground),
                )
                .into_element()
        }
        Block::Link { text: label, url } => {
            let label = label.clone();
            let target = url.clone();
            let ui = ui.clone();
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .active(|s| s.opacity(0.6))
                .on_press(move || {
                    let ui = ui.clone();
                    let target = target.clone();
                    // 交给系统打开：能力声明在 nsh.json 的 `openUrl`，
                    // 只放行 http/https，其它 scheme 会被拒绝。
                    spawn(async move {
                        let opened = open_url(target.clone()).await;
                        if !opened {
                            ui.note(format!("没能打开这个链接：{target}"));
                        }
                    });
                })
                .child(
                    text(move || label.clone())
                        .text_size(px(size))
                        .line_height(px(line))
                        .text_color(move || theme().primary)
                        // 下划线是最直观的「这行可以点」的提示，
                        // 只靠颜色在深色主题下不够明显。
                        .underline(),
                )
                .into_element()
        }
        Block::Rule => view()
            .h(px(1.0))
            .w_full()
            .bg(move || theme().muted)
            .into_element(),
    }
}

/// 详情页的翻页 / 定位工具条。放在楼层滚动区上方，常驻可见。
fn detail_nav_bar(ui: Ui) -> impl IntoElement {
    let prev = ui.clone();
    let next = ui.clone();
    let top = ui.clone();
    let bottom = ui.clone();
    let back = ui.clone();
    let new_reply = ui.clone();
    let jump_page = ui.clone();
    let jump_floor = ui.clone();
    let page_go = ui.clone();
    let floor_go = ui.clone();
    let for_page_val = ui.clone();
    let for_floor_val = ui.clone();
    let page_change = ui.clone();
    let floor_change = ui.clone();
    let page_pages = ui.clone();

    view()
        .flex_col()
        .gap(px(8.0))
        .px_3()
        .py_2()
        .rounded(px(12.0))
        .bg(move || theme().card)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                // 上一页
                .child(ghost_button("◀ 上一页".into(), move || {
                    let p = prev.detail_page.get() - 1;
                    if p >= 1 {
                        prev.detail_goto_page(p);
                    } else {
                        prev.note("已经是第 1 页");
                    }
                }))
                .child(
                    text(move || {
                        format!(
                            "第 {}/{} 页",
                            page_pages.detail_page.get(),
                            page_pages.detail_max.get()
                        )
                    })
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground),
                )
                // 下一页
                .child(ghost_button("下一页 ▶".into(), move || {
                    let p = next.detail_page.get() + 1;
                    if p <= next.detail_max.get() {
                        next.detail_goto_page(p);
                    } else {
                        next.note("已经是最后一页");
                    }
                }))
                // 页码输入 + 跳转
                .child(
                    field(
                        "页",
                        move || for_page_val.detail_jump_page.get(),
                        move |v| page_change.detail_jump_page.set(v),
                        move |v| {
                            if let Ok(p) = v.trim().parse::<i32>() {
                                jump_page.detail_goto_page(p);
                            }
                        },
                        AutoFill::Off,
                        false,
                    )
                    .w(px(52.0))
                    .min_h(px(34.0)),
                )
                .child(ghost_button("跳页".into(), move || {
                    let v = page_go.detail_jump_page.get();
                    if let Ok(p) = v.trim().parse::<i32>() {
                        page_go.detail_goto_page(p);
                    } else {
                        page_go.note("请输入页码数字");
                    }
                })),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(ghost_button("页首".into(), move || top.detail_scroll_top()))
                .child(ghost_button("页尾".into(), move || bottom.detail_scroll_bottom()))
                // 楼层输入 + 跳转
                .child(
                    field(
                        "楼层",
                        move || for_floor_val.detail_jump_floor.get(),
                        move |v| floor_change.detail_jump_floor.set(v),
                        move |v| {
                            if let Ok(f) = v.trim().parse::<i32>() {
                                jump_floor.detail_goto_floor(f);
                            }
                        },
                        AutoFill::Off,
                        false,
                    )
                    .w(px(60.0))
                    .min_h(px(34.0)),
                )
                .child(ghost_button("跳楼".into(), move || {
                    let v = floor_go.detail_jump_floor.get();
                    if let Ok(f) = v.trim().parse::<i32>() {
                        floor_go.detail_goto_floor(f);
                    } else {
                        floor_go.note("请输入楼层数字");
                    }
                }))
                // 返回原位置：仅跳转过后出现
                .child(dynamic(move || {
                    if back.detail_return_offset.get().is_some() {
                        vec![ghost_button("↩ 原位置".into(), {
                            let ui = back.clone();
                            move || ui.detail_return_pos()
                        })
                        .into_element()]
                    } else {
                        Vec::new()
                    }
                }))
                // 新回复：仅当本次主题有上次进度时出现
                .child(dynamic(move || {
                    let tid = new_reply.tid.get();
                    let has = new_reply
                        .detail_progress
                        .get()
                        .get(&tid)
                        .map(|p| p.last_floor > 0)
                        .unwrap_or(false);
                    if has {
                        vec![ghost_button("新回复 →".into(), {
                            let ui = new_reply.clone();
                            move || ui.detail_goto_new_reply()
                        })
                        .into_element()]
                    } else {
                        Vec::new()
                    }
                })),
        )
}

fn detail_view(ui: Ui) -> impl IntoElement {
    let for_back = ui.clone();
    let for_home = ui.clone();
    let for_compose = ui.clone();
    let for_posts = ui.clone();
    let for_poll = ui.clone();
    let for_hint = ui.clone();
    let for_pager = ui.clone();
    let for_scroll = ui.clone();
    let for_effect = ui.clone();
    let for_tools = ui.clone();

    // 楼层渲染 / 进度恢复的时序协调：
    // - pending_floor：楼层元素出现后 reveal 过去（可能刚连续翻页加载进来）；
    // - restore_offset：进入帖子、楼层渲染出来后滚回上次位置。
    // 两个动作只执行一次，做完就清空，避免滚动时反复触发。
    effect(move || {
        let floor = for_effect.detail_pending_floor.get();
        let posts = for_effect.posts.get();
        if let Some(f) = floor {
            if posts.iter().any(|p| p.floor == f) {
                let found = for_effect.detail_floor_refs.borrow().get(&f).copied();
                if let Some(r) = found {
                    r.reveal(RevealOptions::default().vertical(ScrollAlignment::Start));
                    for_effect.detail_pending_floor.set(None);
                }
            }
        }
        if let Some(offset) = for_effect.detail_restore_offset.get()
            && !posts.is_empty()
        {
            for_effect
                .detail_scroll
                .get()
                .scroll_to(point(px(0.0), px(offset)), false);
            for_effect.detail_restore_offset.set(None);
        }
    });

    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ghost_button_icon(ICON_BACK, "返回".into(), move || {
                    let screen = for_back.origin.get();
                    for_back.screen.set(screen)
                }))
                .child(ghost_button_icon(ICON_HOME, "首页".into(), move || {
                    for_home.screen.set(Screen::Index)
                }))
                .child(text(move || ui.title.get()).text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).flex_1()),
        )
        // 跳转工具条：连续翻页 / 跳页 / 跳楼层 / 页首 / 页尾 / 返回原处 / 新回复。
        .child(detail_nav_bar(for_tools.clone()))
        .child(
            scroll()
                .flex_1()
                .element_ref(&ui.detail_scroll.get())
                .on_scroll(move |event: &ScrollEvent| {
                    let y = event.offset.y.0.max(0.0);
                    for_scroll.detail_offset.set(y);
                })
                .on_scroll_end({
                    let ui = ui.clone();
                    move |_| {
                        ui.save_detail_progress();
                    }
                })
                .child(
                // 注意：这里不能给 `flex_1()`。滚动容器的内容高度必须由子节点撑开；
                // 一旦让内容视图也 flex_1，它就等于视口高度，超出一屏的部分
                // （长正文、正文图片、楼层尾部）会被直接裁掉，且滚不到。
                view()
                    .w_full()
                    .gap_3()
                    .child(dynamic(move || {
                        if for_poll.poll_rows.with(|r| r.is_empty()) {
                            Vec::new()
                        } else {
                            vec![poll_card(for_poll.clone()).into_element()]
                        }
                    }))
                    // 白名单缺口要可见，而不是静默丢图。
                    .child(dynamic(move || {
                        let hint = for_hint.image_host_hint.get();
                        if hint.is_empty() {
                            Vec::new()
                        } else {
                            vec![text(move || hint.clone())
                                .text_size(px(12.0))
                                .text_color(move || theme().destructive)
                                .into_element()]
                        }
                    }))
                    .child(dynamic(move || {
                        for_posts
                            .posts
                            .get()
                            .iter()
                            .map(|post| {
                                let author = post.author.clone();
                                let time = post.time.clone();
                                let body = post.body.clone();
                                let blocks = post.blocks.clone();
                                let floor = post.floor;
                                let avatar = post.avatar.clone();
                                let name_for_avatar = author.clone();
                                let for_avatar = for_posts.clone();
                                let for_body_size = for_posts.clone();
                                let avatar_for_row = avatar.clone();
                                let name_for_row = name_for_avatar.clone();
                                // 每层楼一个句柄，注册到 map 供「跳到指定楼层」reveal 用。
                                let floor_ref = element_ref();
                                for_posts
                                    .detail_floor_refs
                                    .borrow_mut()
                                    .insert(floor, floor_ref);
                                view()
                                    .rounded(px(14.0))
                                    .bg(move || theme().card)
                                    .element_ref(&floor_ref)
                                    // 楼层头吸顶：滚过一层楼时，作者与楼层信息一直可见，
                                    // 不用往上翻才知道自己在看谁的哪一楼。背景必须不透明，
                                    // 否则正文会从底部透出来。
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_3()
                                            .px_4()
                                            .py_3()
                                            .bg(move || theme().card)
                                            .sticky()
                                            .top(px(0.0))
                                            // 头像可关：弱网下少一轮图片请求，楼层头退回纯文字。
                                            .child(dynamic(move || {
                                                let ui = for_avatar.clone();
                                                if !ui.settings.get().show_avatar {
                                                    return Vec::new();
                                                }
                                                vec![post_avatar(
                                                    ui,
                                                    avatar_for_row.clone(),
                                                    name_for_row.clone(),
                                                )]
                                            }))
                                            .child(
                                                view()
                                                    .flex_1()
                                                    .gap(px(2.0))
                                                    .child(
                                                        text(move || author.clone())
                                                            .text_size(px(14.0))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(move || theme().card_foreground),
                                                    )
                                                    .child(
                                                        text(move || time.clone())
                                                            .text_size(px(11.0))
                                                            .text_color(move || theme().muted_foreground),
                                                    ),
                                            )
                                            .child(
                                                text(move || format!("{floor} 楼"))
                                                    .text_size(px(11.0))
                                                    .px_2()
                                                    .py(px(1.0))
                                                    .rounded(px(6.0))
                                                    .bg(move || theme().muted)
                                                    .text_color(move || theme().muted_foreground),
                                            ),
                                    )
                                    .child(
                                        view()
                                            .gap_3()
                                            .px_4()
                                            .pb_4()
                                            // 正文：按块渲染。老版本把整层楼的 HTML 拍成一段纯文本，
                                            // 引用、列表、代码全糊在一起；现在段落/引用/列表/代码
                                            // 各自成型，字号与行高仍跟设置里的档位走。
                                            .child(dynamic(move || {
                                                let ui = for_body_size.clone();
                                                if blocks.is_empty() {
                                                    // 解析没产出块时（例如站点改版）退回纯文本，
                                                    // 至少内容还在。
                                                    let plain = body.clone();
                                                    return vec![
                                                        text(move || plain.clone())
                                                            .text_size(move || {
                                                                px(ui.settings
                                                                    .get()
                                                                    .text_scale
                                                                    .body_size())
                                                            })
                                                            .line_height(move || {
                                                                px(ui.settings
                                                                    .get()
                                                                    .text_scale
                                                                    .line_height())
                                                            })
                                                            .text_color(move || {
                                                                theme().card_foreground
                                                            })
                                                            .into_element(),
                                                    ];
                                                }
                                                render_blocks(ui, &blocks)
                                            }))

                                    )
                                    .into_element()
                            })
                            .collect::<Vec<AnyElement>>()
                    })),
            ),
        )
        // 分页：Discuz 每页 50 层，长帖不翻页就读不到后面。追加式加载，
        // 不替换已有楼层，避免打断当前阅读位置。
        .child(dynamic(move || {
            let ui = for_pager.clone();
            let page = ui.detail_page.get();
            let max = ui.detail_max.get();
            if max <= 1 {
                return Vec::new();
            }
            vec![
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        text(move || format!("第 {page} / {max} 页"))
                            .text_size(px(12.0))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(if page < max {
                        let ui = ui.clone();
                        ghost_button_icon(ICON_NEXT, "加载下一页".into(), move || ui.clone().load_more_posts())
                            .into_element()
                    } else {
                        text("已到最后一页")
                            .text_size(px(12.0))
                            .text_color(move || theme().muted_foreground)
                            .into_element()
                    })
                    .into_element(),
            ]
        }))
        .child(dynamic(move || {
            let ui = for_compose.clone();
            if ui.logged_in.get() {
                view()
                    .flex_row()
                    .justify_end()
                    .child(primary_button_icon(ICON_REPLY, "回复此主题".into(), move || {
                        // 切走编辑器时未发完的内容仍在本机草稿里，回来继续写。
                        ui.editor.set(session::draft());
                        ui.screen.set(Screen::Compose);
                    }))
                    .into_element()
            } else {
                // 游客能读开放版块的正文，但写操作必须登录。
                view()
                    .flex_row()
                    .justify_end()
                    .child(ghost_button("登录后回复".into(), move || {
                        ui.screen.set(Screen::Login)
                    }))
                    .into_element()
            }
        }))
}

fn compose_view(ui: Ui) -> impl IntoElement {
    let for_cancel = ui.clone();
    let for_home = ui.clone();
    let for_input = ui.clone();
    let for_attach = ui.clone();
    let for_emoji = ui.clone();
    let for_panel = ui.clone();
    let for_title = ui.clone();
    let for_subject = ui.clone();
    let for_send_mode = ui.clone();

    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ghost_button("取消".into(), move || {
                    // 新主题是从版块列表进来的，取消回列表；回复回详情。
                    let ui = for_cancel.clone();
                    if ui.compose_mode.get() == PostMode::New {
                        ui.screen.set(Screen::Threads);
                    } else {
                        ui.screen.set(Screen::Detail);
                    }
                }))
                .child(ghost_button_icon(ICON_HOME, "首页".into(), move || {
                    for_home.screen.set(Screen::Index)
                }))
                .child(
                    text(move || {
                        if for_title.compose_mode.get() == PostMode::New {
                            "发表新帖"
                        } else {
                            "回复此主题"
                        }
                    })
                    .text_size(px(15.0))
                    .font_weight(FontWeight::SEMIBOLD),
                ),
        )
        // 新主题需要标题；回复不需要，所以只在新建模式下出现。
        .child(dynamic(move || {
            let ui = for_subject.clone();
            if ui.compose_mode.get() != PostMode::New {
                return Vec::new();
            }
            let typed = ui.clone();
            vec![
                input()
                    .placeholder("主题标题")
                    .value(move || typed.compose_subject.get())
                    .on_change({
                        let ui = ui.clone();
                        move |v: &str| ui.compose_subject.set(v.to_string())
                    })
                    .auto_correct(false)
                    .min_h(px(44.0))
                    .p_3()
                    .rounded(px(10.0))
                    .bg(move || theme().card),
            ]
        }))
        .child(
            input()
                .placeholder("写下你的回复…（至少 5 个字）")
                .multiline(true)
                .value(move || for_input.editor.get())
                .on_change({
                    let ui = ui.clone();
                    move |v: &str| {
                        ui.editor.set(v.to_string());
                        // 短文本，Storage 同步写内存、后端异步落盘；相同值会短路。
                        session::save_draft(v);
                    }
                })
                .auto_correct(true)
                .min_h(px(160.0))
                .p_4()
                .rounded(px(12.0))
                .bg(move || theme().card)
                .text_color(move || theme().card_foreground),
        )
        .child(dynamic(move || {
            let ui = for_send_mode.clone();
            if ui.compose_mode.get() == PostMode::New {
                vec![
                    primary_button("发布主题".into(), move || {
                        let ui = ui.clone();
                        ui.clone()
                            .send_new_thread(ui.compose_subject.get(), ui.editor.get());
                    })
                    .into_element(),
                ]
            } else {
                vec![
                    primary_button("发布回复".into(), move || {
                        let ui = ui.clone();
                        ui.clone().send_reply(ui.editor.get());
                    })
                    .into_element(),
                ]
            }
        }))
        .child(ghost_button("插入图片".into(), move || {
            for_attach.clone().attach_image();
        }))
        .child(ghost_button("插入表情".into(), move || {
            let ui = for_emoji.clone();
            ui.emoji_open.set(!ui.emoji_open.get());
        }))
        .child(dynamic(move || {
            if for_panel.emoji_open.get() {
                vec![emoji_picker(for_panel.clone()).into_element()]
            } else {
                Vec::new()
            }
        }))
}

fn feed_view(ui: Ui) -> impl IntoElement {
    let for_home = ui.clone();
    let for_hub = ui.clone();
    let for_rows = ui.clone();
    let for_empty = ui.clone();
    let for_count = ui.clone();
    let for_keys = ui.clone();
    let for_paging = ui.clone();
    let for_prev = ui.clone();
    let for_next = ui.clone();
    let for_actions = ui.clone();

    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ghost_button_icon(ICON_HOME, "首页".into(), move || {
                    for_home.screen.set(Screen::Index)
                }))
                .child(ghost_button("我的空间".into(), move || {
                    for_hub.screen.set(Screen::Hub)
                }))
                .child(
                    text(move || ui.feed_title.get())
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1(),
                ),
        )
        .child(dynamic(move || {
            if !for_paging.feed_source.with(|s| s.has_paging()) {
                return Vec::new();
            }
            let page = for_paging.clone();
            let prev = for_prev.clone();
            let next = for_next.clone();
            vec![view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(view().flex_1().child(
                    text(move || {
                        format!(
                            "第 {}/{} 页",
                            page.feed_page.get(),
                            page.feed_max_page.get().max(1)
                        )
                    })
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground),
                ))
                .child(ghost_button("上一页".into(), move || {
                    prev.clone().flip_feed_page(-1)
                }))
                .child(ghost_button("下一页".into(), move || {
                    next.clone().flip_feed_page(1)
                }))
                .into_element()]
        }))
        .child(dynamic(move || {
            if for_empty.feed_rows.with(|r| r.is_empty()) {
                vec![text("暂无内容，可返回后重新加载")
                    .text_size(px(14.0))
                    .text_color(move || theme().muted_foreground)
                    .into_element()]
            } else {
                Vec::new()
            }
        }))
        .child(
            virtual_list(
                move || for_count.feed_rows.with(|r| r.len()),
                move |at| {
                    let ui = for_rows.clone();
                    let ui_title = ui.clone();
                    let ui_meta = ui.clone();
                    let ui_badge = ui.clone();
                    view()
                        .gap_1()
                        .px_4()
                        .py_3()
                        .min_h(px(56.0))
                        .rounded(px(12.0))
                        .bg(move || theme().card)
                        .active(|s| s.opacity(0.6))
                        .on_press({
                            let ui = ui.clone();
                            move || {
                                let row = ui.feed_rows.with(|r| r.get(at.index()).cloned());
                                let Some(row) = row else { return };
                                let source = ui.feed_source.get();
                                match source {
                                    ListSource::Sms if !row.uid.is_empty() => ui.clone().load_list(
                                        ListSource::SmsDetail {
                                            uid: row.uid,
                                            name: row.author,
                                        },
                                    ),
                                    ListSource::SmsDetail { .. } if !row.uid.is_empty() => {
                                        ui.clone().open_profile(row.uid, row.author)
                                    }
                                    _ if !row.tid.is_empty() => {
                                        ui.clone().open_thread(row.tid, row.title)
                                    }
                                    _ => ui.note("该条目没有可打开的链接"),
                                }
                            }
                        })
                        .child(
                            text(move || {
                                ui_title.feed_rows.with(|r| {
                                    r.get(at.index())
                                        .map(|x| x.title.clone())
                                        .unwrap_or_default()
                                })
                            })
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(
                            text(move || {
                                ui_meta.feed_rows.with(|r| {
                                    r.get(at.index())
                                        .map(|x| x.meta.clone())
                                        .unwrap_or_default()
                                })
                            })
                            .text_size(px(12.0))
                            .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text(move || {
                                ui_badge.feed_rows.with(|r| {
                                    r.get(at.index())
                                        .map(|x| x.badge.clone())
                                        .unwrap_or_default()
                                })
                            })
                            .text_size(px(12.0))
                            .text_color(move || {
                                theme().color("unread").unwrap_or(theme().primary)
                            }),
                        )
                },
            )
            .keys(move || {
                for_keys.feed_rows.with(|r| {
                    r.iter()
                        .enumerate()
                        .map(|(i, x)| format!("{i}-{}", x.tid))
                        .collect::<Vec<_>>()
                })
            })
            .estimated_item_size(px(72.0))
            .flex_1(),
        )
        .child(dynamic(move || {
            let refresh_ui = for_actions.clone();
            let mut items = vec![view().flex_1().child(ghost_button_icon(ICON_REFRESH, "刷新".into(), move || {
                let ui = refresh_ui.clone();
                let mut source = ui.feed_source.get().with_page(ui.feed_page.get());
                let sid = ui.feed_search_id.get();
                if !sid.is_empty() {
                    source = source.with_search_id(&sid);
                }
                ui.clone().load_list(source);
            }))
            .into_element()];
            if matches!(for_actions.feed_source.get(), ListSource::SmsDetail { .. }) {
                let sms_ui = for_actions.clone();
                items.push(
                    view()
                        .flex_1()
                        .child(ghost_button_icon(ICON_MAIL, "写私信".into(), move || {
                            sms_ui.clone().open_sms_compose(
                                sms_ui.sms_to_uid.get(),
                                sms_ui.sms_to_name.get(),
                            );
                        }))
                        .into_element(),
                );
            }
            items
        }))
}

fn search_view(ui: Ui) -> impl IntoElement {
    let for_input = ui.clone();
    let for_submit = ui.clone();
    let for_go = ui.clone();
    let for_scope = ui.clone();
    let for_fid = ui.clone();
    let for_time = ui.clone();

    // 搜索参数初值：进页面时从设置同步一次，之后用户可在页内改。
    // （Tab 切换时也会同步，这里再兜一次，防止直达搜索页参数为空。）
    if ui.search_fid.get().is_empty() {
        ui.search_fid.set(ui.settings.get().search_fid.clone());
    }

    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            text("搜索论坛")
                .text_size(px(20.0))
                .font_weight(FontWeight::BOLD),
        )
        .child(
            input()
                .placeholder("搜索帖子标题")
                .value(move || for_input.query.get())
                .on_change({
                    let ui = ui.clone();
                    move |v: &str| ui.query.set(v.to_string())
                })
                .on_submit({
                    let ui = for_submit.clone();
                    move |v: &str| {
                        let full = ui.search_full_text.get();
                        let fid = ui.search_fid.get();
                        let time = ui.search_time.get();
                        ui.clone().load_list(search_source(v, full, fid, time));
                    }
                })
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .min_h(px(44.0))
                .px_4()
                .py_2()
                .rounded(px(10.0))
                .bg(move || theme().card)
                .text_color(move || theme().card_foreground),
        )
        // 标题 / 全文
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(dynamic(move || {
                    let ui = for_scope.clone();
                    let full = ui.search_full_text.get();
                    vec![
                        filter_chip("标题", !full, {
                            let ui = ui.clone();
                            move || ui.search_full_text.set(false)
                        }),
                        filter_chip("全文", full, {
                            let ui = ui.clone();
                            move || ui.search_full_text.set(true)
                        }),
                    ]
                })),
        )
        // 版块选择：全部 + 默认版面，横向可换行
        .child(dynamic(move || {
            let ui = for_fid.clone();
            let current = ui.search_fid.get();
            let accent_key = ui.settings.get().skin.color_key();
            let mut row = view().flex_row().flex_wrap().gap_2();
            let (label, fid, on) = ("全部版块", "0".to_string(), current == "0");
            row = row.child(filter_chip_color(label, on, accent_key, {
                let ui = ui.clone();
                let fid = fid.clone();
                move || ui.search_fid.set(fid.clone())
            }));
            for f in ui.settings.get().tabs() {
                let name = ui
                    .sections
                    .with(|sections| {
                        sections
                            .iter()
                            .flat_map(|s| s.forums.iter())
                            .find(|x| x.fid == f)
                            .map(|x| x.name.clone())
                    })
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| forum_name(f).to_string());
                let fid_str = f.to_string();
                let on = current == fid_str;
                let key = accent_key;
                row = row.child(filter_chip_color(&name, on, key, {
                    let ui = ui.clone();
                    let fid = fid_str.clone();
                    move || ui.search_fid.set(fid.clone())
                }));
            }
            vec![row.into_element()]
        }))
        // 时间范围
        .child(dynamic(move || {
            let ui = for_time.clone();
            let current = ui.search_time.get();
            let accent_key = ui.settings.get().skin.color_key();
            let mut row = view().flex_row().flex_wrap().gap_2();
            for time in [
                SearchTime::All,
                SearchTime::Day,
                SearchTime::Week,
                SearchTime::Month,
                SearchTime::Year,
            ] {
                let on = current == time;
                let key = accent_key;
                row = row.child(filter_chip_color(time.label(), on, key, {
                    let ui = ui.clone();
                    move || ui.search_time.set(time)
                }));
            }
            vec![row.into_element()]
        }))
        .child(primary_button("搜索".into(), move || {
            let ui = for_go.clone();
            let query = ui.query.get();
            let full = ui.search_full_text.get();
            let fid = ui.search_fid.get();
            let time = ui.search_time.get();
            ui.clone().load_list(search_source(&query, full, fid, time));
        }))
        // 空态占位：与参考图一致，未搜索时给明确引导。
        .child(
            dynamic(move || {
                let ui = for_go.clone();
                if ui.feed_rows.with(|r| r.is_empty()) {
                    vec![
                        view()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .child(
                                text("搜索论坛\n按标题或全文查找帖子，可按版块与时间筛选")
                                    .text_size(px(14.0))
                                    .text_color(move || theme().muted_foreground),
                            )
                            .into_element(),
                    ]
                } else {
                    Vec::new()
                }
            }),
        )
}

/// 通用筛选胶囊（默认主色高亮）。
fn filter_chip(label: &str, on: bool, on_press: impl Fn() + 'static) -> AnyElement {
    let key = Skin::Classic.color_key();
    filter_chip_color(label, on, key, on_press)
}

/// 可指定强调色键的筛选胶囊。
fn filter_chip_color(
    label: &str,
    on: bool,
    accent_key: &'static str,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let label = label.to_string();
    view()
        .px(px(12.0))
        .py(px(7.0))
        .rounded(px(16.0))
        .border_1()
        .border_color(move || {
            if on {
                theme().color(accent_key).unwrap_or_else(|| theme().primary)
            } else {
                theme().muted
            }
        })
        .bg(move || if on { theme().card } else { theme().muted })
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(
            text(move || label.clone())
                .text_size(px(12.5))
                .text_color(move || {
                    if on {
                        theme().color(accent_key).unwrap_or_else(|| theme().primary)
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .into_element()
}

/// 崩溃留痕：panic 默认只打到 stderr，开发宿主被关掉后就没处看了。
/// 这里追加写到 `Storage`（app 自有键值，不依赖文件权限）与 `/tmp`，
/// 下次启动还能读到，便于定位「闪退」到底发生在哪一步。
fn install_panic_log() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_default();
            let line = format!("[panic] {info} @ {location}");
            eprintln!("{line}");
            nsh_core::storage::Storage::set_item("hipda.panic.v1", line.clone()).ok();
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/hipda-panic.log")
            {
                use std::io::Write;
                let _ = writeln!(file, "{line}");
            }
            default_hook(info);
        }));
    });
}

/// 应用的根组件，`nsh.json` 的 `"root": "App"` 挂载它。
#[native_module("hipda-nsh/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// 发布新主题：游客先引导登录，已登录打开编辑器。顶栏发布按钮与版块菜单共用。
fn new_post(ui: Ui) {
    if !ui.logged_in.get() {
        ui.clone().require_login();
        return;
    }
    ui.menu_open.set(false);
    ui.avatar_menu.set(false);
    ui.compose_mode.set(PostMode::New);
    ui.compose_subject.set(String::new());
    ui.editor.set(String::new());
    ui.tab.set(Tab::Forum);
    ui.screen.set(Screen::Compose);
}

/// 是否显示三段顶栏与底部四 Tab。详情 / 设置 / 登录等是主 Tab 的下层界面，
/// 它们各自有返回栏，不再叠一层全局导航。
fn is_main_screen(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::Threads | Screen::Feed | Screen::Search | Screen::Hub | Screen::Index
    )
}

/// 三段式顶栏：左头像（账号菜单）｜中版块名（切换菜单）｜右发布。
fn top_bar(ui: Ui) -> AnyElement {
    let for_avatar = ui.clone();
    let for_forum = ui.clone();
    let for_publish = ui.clone();
    let for_menu_overlay = ui.clone();
    let for_forum_overlay = ui.clone();

    // 版块名：优先站点返回的名字，兜底内置表。
    let current_fid = ui.fid.get();
    let forum_label = ui
        .sections
        .with(|sections| {
            sections
                .iter()
                .flat_map(|s| s.forums.iter())
                .find(|f| f.fid == current_fid)
                .map(|f| f.name.clone())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| forum_name(current_fid).to_string());

    let mut root = view()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(16.0))
        .py(px(10.0))
        .gap(px(12.0));

    // ---- 左：头像按钮 ----
    root = root.child(
        view()
            .w(px(34.0))
            .h(px(34.0))
            .rounded(px(17.0))
            .items_center()
            .justify_center()
            .active(|s| s.opacity(0.7))
            .on_press({
                let ui = for_avatar.clone();
                move || {
                    let next = !ui.avatar_menu.get();
                    ui.avatar_menu.set(next);
                    ui.forum_menu.set(false);
                }
            })
            .bg(move || theme().primary)
            .child(
                text(move || {
                    let name = for_avatar.username.get();
                    let c = name.chars().next().unwrap_or('H');
                    c.to_string()
                })
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().primary_foreground),
            ),
    );

    // ---- 中：版块名 + 下拉箭头 ----
    root = root.child(
        view()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .active(|s| s.opacity(0.7))
            .on_press({
                let ui = for_forum.clone();
                move || {
                    let next = !ui.forum_menu.get();
                    ui.forum_menu.set(next);
                    ui.avatar_menu.set(false);
                }
            })
            .child(
                text(move || forum_label.clone())
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(icon_secondary(ICON_CHEVRON_DOWN, 16.0)),
    );

    // ---- 右：发布 ----
    root = root.child(
        view()
            .w(px(34.0))
            .h(px(34.0))
            .items_center()
            .justify_center()
            .active(|s| s.opacity(0.6))
            .on_press(move || new_post(for_publish.clone()))
            .child(icon_secondary(ICON_EDIT, 21.0)),
    );

    // 头像菜单浮层（用 dynamic 包住，展开时才建节点）。
    let avatar_ui = for_menu_overlay.clone();
    let avatar_sheet = dynamic(move || {
        if !avatar_ui.avatar_menu.get() {
            return Vec::new();
        }
        let ui = avatar_ui.clone();
        let mut items: Vec<AnyElement> = Vec::new();
        let logged = ui.logged_in.get();
        if logged {
            // 个人主页：需要 uid，未取到时给提示而不是空跳转。
            let profile_ui = ui.clone();
            items.push(menu_item("个人主页", None, move || {
                let ui = profile_ui.clone();
                ui.avatar_menu.set(false);
                let uid = ui.profile_uid.get();
                let name = ui.username.get();
                if uid.trim().is_empty() {
                    ui.note("暂时无法取得你的 uid，请稍后重试");
                } else {
                    ui.clone().open_profile(uid, name);
                }
            }));
            // 切换 / 添加账号：本客户端单会话，等价于退出后用另一账号登录。
            let switch_ui = ui.clone();
            items.push(menu_item("切换账号", None, move || {
                let ui = switch_ui.clone();
                ui.avatar_menu.set(false);
                ui.clone().logout();
                ui.screen.set(Screen::Login);
            }));
            let logout_ui = ui.clone();
            items.push(menu_item("退出登录", None, move || {
                logout_ui.clone().logout()
            }));
        } else {
            let login_ui = ui.clone();
            items.push(menu_item("登录账号", None, move || {
                let ui = login_ui.clone();
                ui.avatar_menu.set(false);
                ui.screen.set(Screen::Login);
            }));
        }
        let settings_ui = ui.clone();
        items.push(menu_item("设置", None, move || {
            settings_ui.clone().open_settings()
        }));
        vec![
            overlay()
                .absolute()
                .inset_0()
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .on_press(move || ui.avatar_menu.set(false)),
                )
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .items_start()
                        .justify_start()
                        .pt(px(52.0))
                        .pl(px(12.0))
                        .child(
                            view()
                                .w(px(180.0))
                                .py(px(6.0))
                                .rounded(px(10.0))
                                .border_1()
                                .border_color(move || theme().muted)
                                .shadow_lg()
                                .bg(move || theme().card)
                                .children(items),
                        ),
                )
                .into_element(),
        ]
    });

    // 版块切换菜单：列出默认版面，点一个即加载该版。
    let forum_ui = for_forum_overlay.clone();
    let forum_sheet = dynamic(move || {
        if !forum_ui.forum_menu.get() {
            return Vec::new();
        }
        let ui = forum_ui.clone();
        let current = ui.fid.get();
        let mut items: Vec<AnyElement> = Vec::new();
        for fid in ui.settings.get().tabs() {
            let label = ui
                .sections
                .with(|sections| {
                    sections
                        .iter()
                        .flat_map(|s| s.forums.iter())
                        .find(|f| f.fid == fid)
                        .map(|f| f.name.clone())
                })
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| forum_name(fid).to_string());
            let ui2 = ui.clone();
            items.push(menu_item_s(label, Some(fid == current), move || {
                ui2.forum_menu.set(false);
                ui2.clone().load_threads(fid, 1);
            }));
        }
        vec![
            overlay()
                .absolute()
                .inset_0()
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .on_press(move || ui.forum_menu.set(false)),
                )
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .items_start()
                        .justify_center()
                        .pt(px(52.0))
                        .child(
                            view()
                                .w(px(200.0))
                                .py(px(6.0))
                                .rounded(px(10.0))
                                .border_1()
                                .border_color(move || theme().muted)
                                .shadow_lg()
                                .bg(move || theme().card)
                                .children(items),
                        ),
                )
                .into_element(),
        ]
    });

    root.child(avatar_sheet).child(forum_sheet).into_element()
}

/// 底部四标签：论坛 / 消息 / 搜索 / 我的。点击同类标签回到该 Tab 首页并刷新。
fn bottom_tabs(ui: Ui) -> AnyElement {
    let active = ui.tab.get();
    let key = ui.settings.get().skin.color_key();

    let item = |tab: Tab, label: &'static str, icon: &'static str| -> AnyElement {
        let ui = ui.clone();
        let on = active == tab;
        let key = key;
        view()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .py(px(8.0))
            .active(|s| s.opacity(0.6))
            .on_press(move || {
                let ui = ui.clone();
                ui.tab.set(tab);
                match tab {
                    Tab::Forum => {
                        // 点论坛 = 刷新当前版块第 1 页；首次进入用默认版块。
                        let fid = ui.fid.get();
                        ui.clone().load_threads(fid, 1);
                    }
                    Tab::Message => {
                        ui.clone().load_list(ListSource::Notifications);
                    }
                    Tab::Search => {
                        ui.search_full_text.set(ui.settings.get().search_full_text);
                        ui.search_fid.set(ui.settings.get().search_fid.clone());
                        ui.search_time.set(ui.settings.get().search_time);
                        ui.open_search();
                    }
                    Tab::Mine => {
                        ui.screen.set(Screen::Hub);
                    }
                }
            })
            .child(
                svg(nsh_core::element::ImageSrc::svg_markup(icon))
                    .w(px(22.0))
                    .h(px(22.0))
                    .tint_color(move || {
                        if on {
                            theme().color(key).unwrap_or_else(|| theme().primary)
                        } else {
                            theme().muted_foreground
                        }
                    }),
            )
            .child(
                text(label)
                    .text_size(px(10.5))
                    .text_color(move || {
                        if on {
                            theme().color(key).unwrap_or_else(|| theme().primary)
                        } else {
                            theme().muted_foreground
                        }
                    }),
            )
            .into_element()
    };

    view()
        .flex_row()
        .items_center()
        .py(px(4.0))
        .border_1()
        .border_color(move || theme().muted)
        .bg(move || theme().card)
        .child(item(Tab::Forum, "论坛", ICON_HOME))
        .child(item(Tab::Message, "消息", ICON_MAIL))
        .child(item(Tab::Search, "搜索", ICON_SEARCH))
        .child(item(Tab::Mine, "我的", ICON_PERSON))
}

/// HiPDA·nsh 版 —— HiPDA 论坛的非官方客户端。
#[component]
pub fn App() -> impl IntoElement {
    // 61 个论坛表情的默认映射：标题 / 正文里的短码在解析时就被替换。
    // `register_emoji_aliases` 是覆盖式写入，重复挂载也幂等。
    install_default_aliases();
    install_panic_log();
    let ui = Ui::new();

    // 首屏恒为论坛首页：登录态只改变可见分区与可用操作，不决定进哪个界面。
    // 有本机会话时异步校验 cookie 并重载首页（登录态会多出分区），
    // 没有会话也照常加载游客首页。
    if let Some((username, _)) = session::restore(&ui.client) {
        eprintln!("[boot] 命中快照 user={username}");
        ui.logged_in.set(true);
        ui.username.set(username);
        ui.clone().resume();
    } else {
        eprintln!("[boot] 无快照");
        ui.clone().load_index();
    }
    // 开发期截图 / 冒烟用：`HIPDA_DEBUG_SCREEN=settings nsh run` 直接落在设置页。
    // 正常启动不设这个变量，行为不变。
    if let Ok(screen) = std::env::var("HIPDA_DEBUG_SCREEN") {
        match screen.trim() {
            "settings" => ui.clone().open_settings(),
            "settings-forum" => {
                ui.clone().open_settings();
                ui.settings_page.set(SettingsPage::Forum);
            }
            "settings-ui" => {
                ui.clone().open_settings();
                ui.settings_page.set(SettingsPage::Ui);
            }
            "settings-notify" => {
                ui.clone().open_settings();
                ui.settings_page.set(SettingsPage::Notify);
            }
            "settings-network" => {
                ui.clone().open_settings();
                ui.settings_page.set(SettingsPage::Network);
            }
            "settings-other" => {
                ui.clone().open_settings();
                ui.settings_page.set(SettingsPage::Other);
            }
            // 版块页 + 展开的三点菜单，用于截图核对。
            "threads-menu" => {
                ui.clone().load_threads(2, 1);
                ui.menu_open.set(true);
            }
            _ => {}
        }
    }
    // 开发期截图 / 冒烟用：`HIPDA_DEBUG_TID=448060 nsh run` 直接落在某个主题上。
    // 正常启动不设这个变量，行为不变。
    if let Ok(tid) = std::env::var("HIPDA_DEBUG_TID")
        && !tid.trim().is_empty()
    {
        ui.clone().open_thread(tid.trim().to_string(), String::new());
    }
    let for_body = ui.clone();
    let for_footer = ui.clone();
    let for_chrome = ui.clone();

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(
            view()
                .flex_1()
                .p_safe_area()
                // 主 Tab 页：三段顶栏 + 底部四 Tab；下层页面（详情 / 设置等）
                // 由各自顶栏负责，这里只留内容，避免两层导航叠加。
                .child(dynamic(move || {
                    let ui = for_chrome.clone();
                    if is_main_screen(ui.screen.get()) {
                        vec![top_bar(ui.clone()).into_element()]
                    } else {
                        vec![view().h(px(0.0)).into_element()]
                    }
                }))
                .child(
                    view()
                        .flex_1()
                        .child(dynamic(move || match for_body.screen.get() {
                            Screen::Index => index_view(for_body.clone()).into_element(),
                            Screen::Login => login_view(for_body.clone()).into_element(),
                            Screen::Threads => threads_view(for_body.clone()).into_element(),
                            Screen::Detail => detail_view(for_body.clone()).into_element(),
                            Screen::Compose => compose_view(for_body.clone()).into_element(),
                            Screen::Feed => feed_view(for_body.clone()).into_element(),
                            Screen::Search => search_view(for_body.clone()).into_element(),
                            Screen::Hub => hub_view(for_body.clone()).into_element(),
                            Screen::Profile => profile_view(for_body.clone()).into_element(),
                            Screen::Blacklist => blacklist_view(for_body.clone()).into_element(),
                            Screen::SmsCompose => sms_compose_view(for_body.clone()).into_element(),
                            Screen::Settings => settings_view(for_body.clone()).into_element(),
                        })),
                )
                // 底部四 Tab：只在主 Tab 页显示。
                .child(dynamic(move || {
                    let ui = for_chrome.clone();
                    if is_main_screen(ui.screen.get()) {
                        vec![bottom_tabs(ui.clone()).into_element()]
                    } else {
                        Vec::new()
                    }
                }))
                .child(
                    view()
                        .px(px(16.0))
                        .py(px(5.0))
                        .child(text(move || {
                            let busy = if for_footer.busy.get() { " · 加载中" } else { "" };
                            format!("{}{busy}", for_footer.status.get())
                        })
                        .text_size(px(10.5))
                        .text_color(move || theme().muted_foreground)),
                ),
        )
}
// ---------------------------------------------------------------- 我的空间 / 资料 / 黑名单 / 私信 / 表情 / 投票

const HUB_COUNT: usize = 9;

/// 我的空间入口表。`ListSource` 带 String 字段，不能做 const，用函数按索引构造。
fn hub_entry(i: usize) -> (&'static str, &'static str, Option<ListSource>, bool) {
    match i {
        0 => ("通知", "回复与系统提醒", Some(ListSource::Notifications), false),
        1 => ("私信", "会话列表，点进可看往来", Some(ListSource::Sms), false),
        2 => ("我的帖子", "我发起的主题", Some(ListSource::MyPosts { page: 1 }), false),
        3 => ("我的回复", "我参与过的主题", Some(ListSource::MyReplies { page: 1 }), false),
        4 => ("我的收藏", "收藏的主题", Some(ListSource::Favorites { page: 1 }), false),
        5 => ("我的关注", "关注的主题", Some(ListSource::Attention { page: 1 }), false),
        6 => ("最新帖子", "24 小时内的新帖", Some(ListSource::NewPosts {
            search_id: String::new(), page: 1,
        }), false),
        7 => ("黑名单", "查看 / 添加 / 移除", None, true),
        8 => ("搜索主题", "按标题搜索，支持中文", None, false),
        _ => ("", "", None, false),
    }
}

fn hub_open(ui: Ui, i: usize) {
    let (_, _, source, blacklist) = hub_entry(i);
    if blacklist {
        ui.clone().load_blacklist();
    } else if i == 8 {
        ui.open_search();
    } else if let Some(source) = source {
        ui.load_list(source);
    }
}

fn hub_view(ui: Ui) -> impl IntoElement {
    let for_account = ui.clone();
    let for_fav = ui.clone();
    let for_later = ui.clone();
    let for_history = ui.clone();
    let for_posts = ui.clone();
    let for_replies = ui.clone();
    let for_draft = ui.clone();
    let for_follow = ui.clone();
    let for_settings = ui.clone();
    let for_login = ui.clone();
    let key = ui.settings.get().skin.color_key();

    scroll()
        .flex_1()
        .child(
            view()
                .flex_1()
                .gap(px(16.0))
                .child(text("我的").text_size(px(20.0)).font_weight(FontWeight::BOLD))
                // 账号卡片
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(16.0))
                        .py(px(14.0))
                        .rounded(px(12.0))
                        .bg(move || theme().card)
                        .child(
                            view()
                                .w(px(44.0))
                                .h(px(44.0))
                                .rounded(px(10.0))
                                .items_center()
                                .justify_center()
                                .bg(move || theme().color(key).unwrap_or_else(|| theme().primary))
                                .child(
                                    text("H")
                                        .text_size(px(20.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(move || theme().primary_foreground),
                                ),
                        )
                        .child(
                            view()
                                .flex_1()
                                .gap(px(3.0))
                                .child(dynamic(move || {
                                    let ui = for_account.clone();
                                    let name = ui.username.get();
                                    let title = if ui.logged_in.get() {
                                        if name.is_empty() { "已登录".to_string() } else { name }
                                    } else {
                                        "未登录".to_string()
                                    };
                                    vec![
                                        text(move || title.clone())
                                            .text_size(px(15.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .into_element(),
                                        text("HiPDA 论坛账号")
                                            .text_size(px(11.5))
                                            .text_color(move || theme().muted_foreground)
                                            .into_element(),
                                    ]
                                })),
                        )
                        .child(dynamic(move || {
                            let ui = for_login.clone();
                            if ui.logged_in.get() {
                                Vec::new()
                            } else {
                                vec![ghost_button("登录".into(), move || {
                                    for_login.screen.set(Screen::Login)
                                })
                                .into_element()]
                            }
                        })),
                )
                // 收藏 / 稍后读 / 浏览历史
                .child(settings_group(
                    String::new(),
                    vec![
                        nav_row("收藏".into(), "我收藏的主题".into(), move || {
                            for_fav.clone().load_list(ListSource::Favorites { page: 1 })
                        }),
                        nav_row("稍后读".into(), "关注的主题".into(), move || {
                            for_later.clone().load_list(ListSource::Attention { page: 1 })
                        }),
                        nav_row("浏览历史".into(), "本次打开过的主题".into(), move || {
                            let ui = for_history.clone();
                            let n = ui.read_tids.with(|s| s.len());
                            ui.note(format!("本次会话已浏览 {n} 个主题"));
                        }),
                    ],
                ))
                // 我的帖子 / 我的回复 / 草稿 / 关注的人
                .child(settings_group(
                    String::new(),
                    vec![
                        nav_row("我的帖子".into(), "我发起的主题".into(), move || {
                            for_posts.clone().load_list(ListSource::MyPosts { page: 1 })
                        }),
                        nav_row("我的回复".into(), "我参与过的主题".into(), move || {
                            for_replies.clone().load_list(ListSource::MyReplies { page: 1 })
                        }),
                        nav_row("草稿".into(), "未发出的回帖内容".into(), move || {
                            let ui = for_draft.clone();
                            let text = session::draft();
                            ui.editor.set(text.clone());
                            if text.trim().is_empty() {
                                ui.note("当前没有草稿");
                            } else {
                                ui.note("草稿已载入回帖编辑器");
                                ui.screen.set(Screen::Detail);
                            }
                        }),
                        nav_row("关注的人".into(), "我的关注列表".into(), move || {
                            for_follow.clone().load_list(ListSource::Attention { page: 1 })
                        }),
                    ],
                ))
                .child(settings_group(
                    String::new(),
                    vec![nav_row("设置".into(), String::new(), move || {
                        for_settings.clone().open_settings()
                    })],
                )),
        )
}

fn profile_view(ui: Ui) -> impl IntoElement {
    let for_home = ui.clone();
    let for_hub = ui.clone();
    let for_rows = ui.clone();
    let for_count = ui.clone();
    let for_sms = ui.clone();
    let for_threads = ui.clone();
    let for_black = ui.clone();
    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ghost_button_icon(ICON_HOME, "首页".into(), move || {
                    for_home.screen.set(Screen::Index)
                }))
                .child(ghost_button("我的空间".into(), move || {
                    for_hub.screen.set(Screen::Hub)
                }))
                .child(text(move || ui.profile_title.get())
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex_1()),
        )
        .child(
            virtual_list(
                move || for_count.profile_rows.with(|r| r.len()),
                move |at| {
                    let ui = for_rows.clone();
                    text(move || {
                        ui.profile_rows
                            .with(|r| r.get(at.index()).cloned().unwrap_or_default())
                    })
                    .text_size(px(13.0))
                    .text_color(move || theme().card_foreground)
                },
            )
            .estimated_item_size(px(24.0))
            .flex_1(),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(view().flex_1().child(ghost_button_icon(ICON_MAIL, "发私信".into(), move || {
                    let ui = for_sms.clone();
                    ui.clone().open_sms_compose(ui.profile_uid.get(), ui.profile_name.get());
                })))
                .child(view().flex_1().child(ghost_button("他的帖子".into(), move || {
                    let ui = for_threads.clone();
                    ui.clone().load_list(ListSource::UserThreads {
                        uid: ui.profile_uid.get(),
                        search_id: String::new(),
                        page: 1,
                    });
                })))
                .child(view().flex_1().child(ghost_button("加黑名单".into(), move || {
                    let ui = for_black.clone();
                    ui.clone().set_blacklist(ui.profile_name.get(), true);
                }))),
        )
}

fn blacklist_row(ui: Ui, at: Position) -> impl IntoElement {
    let label = ui.clone();
    let remove = ui.clone();
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .min_h(px(48.0))
        .rounded(px(10.0))
        .bg(move || theme().card)
        .child(
            text(move || {
                label.blacklist_rows
                    .with(|r| r.get(at.index()).cloned().unwrap_or_default())
            })
            .flex_1(),
        )
        .child(ghost_button("移除".into(), move || {
            let name = remove
                .blacklist_rows
                .with(|r| r.get(at.index()).cloned().unwrap_or_default());
            if !name.is_empty() {
                remove.clone().set_blacklist(name, false);
            }
        }))
}

// ---------------------------------------------------------------- 设置
//
// 结构对齐原 HiPDA·NG（`~/code/hipda` 的 `SettingActivity`）：顶层是
// 「客户端发布帖 / 关于 / 检查更新」三条信息 +「分类设置」五个入口，
// 每个入口进子页。子页里只放本客户端**真能生效**的项：Android 专属的
// 手势返回、标题栏隐藏、浮动按钮、后台通知任务、屏幕方向，以及没有 API
// 支撑的缓存清理，一律不摆假开关 —— 点了没反应的开关比没有更糟。

/// 设置内的子页。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsPage {
    #[default]
    Menu,
    Appearance,
    ThreadList,
    Reading,
    Forum,
    Ui,
    Notify,
    Network,
    Other,
}

impl SettingsPage {
    fn title(self) -> &'static str {
        match self {
            Self::Menu => "设置",
            Self::Appearance => "外观",
            Self::ThreadList => "帖子列表",
            Self::Reading => "阅读与回复",
            Self::Forum => "论坛选项",
            Self::Ui => "用户界面",
            Self::Notify => "消息通知",
            Self::Network => "图片与缓存",
            Self::Other => "其它",
        }
    }
}

/// 设置行：左侧标题 + 说明，右侧控件。整行可点，高度 ≥44pt。
fn setting_row(label: String, desc: String, trailing: AnyElement) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .px(px(16.0))
        .py(px(11.0))
        .child(
            view()
                .flex_1()
                .gap(px(3.0))
                .child(
                    text(move || label.clone())
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(dynamic(move || {
                    if desc.is_empty() {
                        Vec::new()
                    } else {
                        // 每次重绘都新建一份，避免把捕获的 String 移进内层闭包。
                        let line = desc.clone();
                        vec![
                            text(move || line.clone())
                                .text_size(px(11.5))
                                .text_color(move || theme().muted_foreground)
                                .into_element(),
                        ]
                    }
                })),
        )
        .child(trailing)
        .into_element()
}

/// 纯信息行：右侧不画控件，也不响应点击。
fn info_row(label: String, desc: String) -> AnyElement {
    setting_row(label, desc, view().into_element())
}

/// 开关：闭合时圆钮靠左，打开时靠右并换成主色。圆钮用 card 色，
/// 浅色下是白、深色下是卡片底色，两种主题都不用另配资源。
fn switch_toggle(on: bool) -> AnyElement {
    let track = view()
        .w(px(38.0))
        .h(px(22.0))
        .px(px(2.0))
        .rounded(px(11.0))
        // 横向 flex：view() 默认纵向，justify_end 在纵轴上不生效，
        // 圆钮会永远停在水平居中位置。
        .flex_row()
        .items_center()
        .bg(move || if on { theme().primary } else { theme().muted });
    let track = if on {
        track.justify_end()
    } else {
        track.justify_start()
    };
    track
        .child(
            view()
                .w(px(18.0))
                .h(px(18.0))
                .rounded(px(9.0))
                .bg(move || theme().card),
        )
        .into_element()
}

/// 可勾选项的勾选标记：选中是主色圆底 + 对勾，未选是空圆底。
fn check_mark(on: bool) -> AnyElement {
    let base = view()
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || if on { theme().primary } else { theme().muted });
    if on {
        base.child(icon_on_primary(ICON_CHECK, 12.0)).into_element()
    } else {
        base.into_element()
    }
}

/// 开关行：整行可点，右侧是开关。
fn toggle_row(on: bool, label: String, desc: String, on_press: impl Fn() + 'static) -> AnyElement {
    view()
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(setting_row(label, desc, switch_toggle(on)))
        .into_element()
}

/// 多选行：右侧是勾选标记。
fn check_row(on: bool, label: String, on_press: impl Fn() + 'static) -> AnyElement {
    view()
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(setting_row(label, String::new(), check_mark(on)))
        .into_element()
}

/// 跳转行：右侧一个箭头。
fn nav_row(label: String, desc: String, on_press: impl Fn() + 'static) -> AnyElement {
    view()
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(setting_row(
            label,
            desc,
            icon_secondary(ICON_NEXT, 16.0).into_element(),
        ))
        .into_element()
}

/// 分组：小标题 + 一个容器，行间 1pt 细线（与首页列表同一套视觉）。
/// `title` 为空时只画容器 —— 原版首组就没有标题。
fn settings_group(title: String, rows: Vec<AnyElement>) -> AnyElement {
    let mut body: Vec<AnyElement> = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            body.push(
                view()
                    .h(px(1.0))
                    .w_full()
                    .bg(move || theme().muted)
                    .into_element(),
            );
        }
        body.push(row);
    }
    let card = view()
        .rounded(px(12.0))
        .bg(move || theme().card)
        .children(body);
    if title.is_empty() {
        card.into_element()
    } else {
        view()
            .gap(px(8.0))
            .child(
                text(move || title.clone())
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || theme().muted_foreground),
            )
            .child(card)
            .into_element()
    }
}

/// 一段横向档位选择器，包成设置行（左标题说明 + 右档位胶囊）。
fn chip_picker_row(label: String, desc: String, chips: AnyElement) -> AnyElement {
    setting_row(label, desc, chips)
}

/// 「恢复默认」按钮行。接受一个闭包，由页面决定重置哪些字段。
fn restore_default_row(on_press: impl Fn() + 'static) -> AnyElement {
    view()
        .py(px(12.0))
        .items_center()
        .justify_center()
        .active(|s| s.opacity(0.6))
        .on_press(on_press)
        .child(
            text("恢复默认")
                .text_size(px(13.0))
                .text_color(move || theme().destructive),
        )
        .into_element()
}

/// 字号选择的一段。
fn scale_chip(
    current: settings::TextScale,
    mine: settings::TextScale,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let on = current == mine;
    view()
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .bg(move || if on { theme().card } else { theme().muted })
        .active(|s| s.opacity(0.7))
        .on_press(on_press)
        .child(
            text(move || mine.label())
                .text_size(px(12.0))
                .text_color(move || {
                    if on {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .into_element()
}


/// 版块菜单的一项。`checked` 为 `Some` 时右侧画勾选标记（排序 / 置顶这类开关项）。
fn menu_item(
    label: &'static str,
    checked: Option<bool>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(14.0))
        .py(px(10.0))
        .active(|s| s.opacity(0.6))
        .on_press(on_press)
        .child(text(label).text_size(px(13.5)))
        .child(dynamic(move || match checked {
            Some(true) => vec![icon_secondary(ICON_CHECK, 14.0).into_element()],
            _ => Vec::new(),
        }))
        .into_element()
}

/// 与 menu_item 相同，但标签是运行时字符串（版块名）。
fn menu_item_s(
    label: String,
    checked: Option<bool>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(14.0))
        .py(px(10.0))
        .active(|s| s.opacity(0.6))
        .on_press(on_press)
        .child(text(move || label.clone()).text_size(px(13.5)))
        .child(dynamic(move || match checked {
            Some(true) => vec![icon_secondary(ICON_CHECK, 14.0).into_element()],
            _ => Vec::new(),
        }))
        .into_element()
}

fn settings_view(ui: Ui) -> impl IntoElement {
    let for_back = ui.clone();
    let for_title = ui.clone();
    let for_body = ui.clone();

    view()
        .flex_1()
        .gap(px(16.0))
        .px(px(16.0))
        .py(px(16.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(ghost_button_icon(ICON_BACK, "返回".into(), move || {
                    // 子页先回菜单，菜单才回来源界面：原版也是这个层级，
                    // 从「其它」里按返回不该直接跳回帖子列表。
                    let ui = for_back.clone();
                    if ui.settings_page.get() == SettingsPage::Menu {
                        ui.screen.set(ui.settings_origin.get());
                    } else {
                        ui.settings_page.set(SettingsPage::Menu);
                    }
                }))
                .child(
                    text(move || for_title.settings_page.get().title())
                        .text_size(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD),
                ),
        )
        .child(dynamic(move || {
            let ui = for_body.clone();
            vec![match ui.settings_page.get() {
                SettingsPage::Menu => settings_menu(ui.clone()),
                SettingsPage::Appearance => settings_appearance_page(ui.clone()),
                SettingsPage::ThreadList => settings_thread_list_page(ui.clone()),
                SettingsPage::Reading => settings_reading_page(ui.clone()),
                SettingsPage::Forum => settings_forum_page(ui.clone()),
                SettingsPage::Ui => settings_ui_page(ui.clone()),
                SettingsPage::Notify => settings_notify_page(ui.clone()),
                SettingsPage::Network => settings_network_page(ui.clone()),
                SettingsPage::Other => settings_other_page(ui.clone()),
            }]
        }))
}

/// 顶层菜单：三条信息 + 分类设置五个入口。
fn settings_menu(ui: Ui) -> AnyElement {
    let for_appearance = ui.clone();
    let for_list = ui.clone();
    let for_reading = ui.clone();
    let for_forum = ui.clone();
    let for_ui = ui.clone();
    let for_notify = ui.clone();
    let for_network = ui.clone();
    let for_other = ui.clone();

    let s = ui.settings.get();
    let read_group = settings_group(
        "显示与阅读".into(),
        vec![
            nav_row("外观".into(), s.skin.label().into(), move || {
                for_appearance.settings_page.set(SettingsPage::Appearance)
            }),
            nav_row("帖子列表".into(), s.list_style.label().into(), move || {
                for_list.settings_page.set(SettingsPage::ThreadList)
            }),
            nav_row("阅读与回复".into(), "平铺 · 分页".into(), move || {
                for_reading.settings_page.set(SettingsPage::Reading)
            }),
        ],
    );

    let forum_group = settings_group(
        "版块与首页".into(),
        vec![
            nav_row("论坛选项".into(), format!("{} 个常用版块", s.tabs().len()), move || {
                for_forum.settings_page.set(SettingsPage::Forum)
            }),
            nav_row("用户界面".into(), "正文字号、头像".into(), move || {
                for_ui.settings_page.set(SettingsPage::Ui)
            }),
        ],
    );

    let general_group = settings_group(
        "通用".into(),
        vec![
            nav_row("消息通知".into(), "本客户端不常驻后台".into(), move || {
                for_notify.settings_page.set(SettingsPage::Notify)
            }),
            nav_row("图片与缓存".into(), "正文图片、缓存说明".into(), move || {
                for_network.settings_page.set(SettingsPage::Network)
            }),
            nav_row("其它".into(), "服务器、草稿、版本".into(), move || {
                for_other.settings_page.set(SettingsPage::Other)
            }),
        ],
    );

    scroll()
        .flex_1()
        .child(
            view()
                .w_full()
                .gap(px(20.0))
                .child(read_group)
                .child(forum_group)
                .child(general_group),
        )
        .into_element()
}

/// 论坛选项：默认版面（多选）+ 发帖小尾巴 + 黑名单。
fn settings_forum_page(ui: Ui) -> AnyElement {
    let for_forums = ui.clone();
    let for_tail = ui.clone();
    let for_tail_text = ui.clone();
    let for_tail_url = ui.clone();

    let forums = dynamic(move || {
        let ui = for_forums.clone();
        let settings = ui.settings.get();
        let mut forums: Vec<(i32, String)> = ui.sections.with(|sections| {
            sections
                .iter()
                .flat_map(|s| s.forums.iter().map(|f| (f.fid, f.name.clone())))
                .collect()
        });
        if forums.is_empty() {
            // 首页还没加载完或站点不可达：用内置默认版块兜底，
            // 否则这里会是一片空白，看着像设置页坏了。
            forums = DEFAULT_FORUMS
                .iter()
                .map(|fid| (*fid, forum_name(*fid).to_string()))
                .collect();
        }
        // 已选版面必须始终可见可取消：游客态站点只返回一部分版块，
        // 否则选过就再也点不掉。
        for fid in settings.tabs() {
            if !forums.iter().any(|(f, _)| *f == fid) {
                forums.push((fid, forum_name(fid).to_string()));
            }
        }
        let mut rows: Vec<AnyElement> = Vec::with_capacity(forums.len());
        for (fid, name) in forums {
            let pick = ui.clone();
            rows.push(check_row(settings.has_tab(fid), name, move || {
                pick.clone().apply_settings(move |s| s.toggle_tab(fid));
            }));
        }
        vec![settings_group("默认版面".into(), rows)]
    });

    let blacklist = {
        let nav = ui.clone();
        settings_group(
            String::new(),
            vec![nav_row(
                "黑名单".into(),
                "屏蔽名单的增删".into(),
                move || {
                    let ui = nav.clone();
                    ui.clone().load_blacklist();
                    ui.screen.set(Screen::Blacklist);
                },
            )],
        )
    };

    // 小尾巴：输入框只依赖 tail_* 三个 signal，打字不会重建输入框。
    let tail = dynamic(move || {
        let ui = for_tail.clone();
        let open = ui.tail_open.get();
        let toggle_ui = ui.clone();
        let mut rows: Vec<AnyElement> = vec![toggle_row(
            open,
            "发帖添加小尾巴".into(),
            "回帖时在正文末尾追加一行".into(),
            move || {
                let ui = toggle_ui.clone();
                let next = !ui.tail_open.get();
                ui.tail_open.set(next);
                ui.clone().apply_settings(|s| s.tail_enabled = next);
            },
        )];
        if open {
            let text_ui = ui.clone();
            rows.push(
                view()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .child(
                        input()
                            .placeholder("尾巴文字，例如：来自 HiPDA·nsh")
                            .value(move || for_tail_text.tail_text.get())
                            .on_change({
                                let ui = text_ui.clone();
                                move |v: &str| {
                                    ui.tail_text.set(v.to_string());
                                    ui.clone()
                                        .apply_settings(|s| s.tail_text = v.to_string());
                                }
                            })
                            .auto_correct(false)
                            .min_h(px(40.0))
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .bg(move || theme().muted),
                    )
                    .into_element(),
            );
            let url_ui = ui.clone();
            rows.push(
                view()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .child(
                        input()
                            .placeholder("尾巴链接（可留空）")
                            .value(move || for_tail_url.tail_url.get())
                            .on_change({
                                let ui = url_ui.clone();
                                move |v: &str| {
                                    ui.tail_url.set(v.to_string());
                                    ui.clone()
                                        .apply_settings(|s| s.tail_url = v.to_string());
                                }
                            })
                            .auto_correct(false)
                            .min_h(px(40.0))
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .bg(move || theme().muted),
                    )
                    .into_element(),
            );
        }
        vec![settings_group(String::new(), rows)]
    });

    scroll()
        .flex_1()
        .child(
            view()
                .w_full()
                .gap(px(20.0))
                .child(forums)
                .child(tail)
                .child(blacklist),
        )
        .into_element()
}

/// 用户界面：正文字号 + 头像。
/// 外观：主题皮肤 + 强调色 + 跟随系统字号 + 分隔线深浅。
/// 明暗模式由 NSH 的 nsh.json 决定（跟随系统），运行时切换不在 NSH 能力内，
/// 因此这里不摆假开关，而是用经典皮肤真实切换强调色。
fn settings_appearance_page(ui: Ui) -> AnyElement {
    let body = dynamic(move || {
        let ui = ui.clone();
        let s = ui.settings.get();
        let key = s.skin.color_key();
        let mut rows: Vec<AnyElement> = Vec::new();

        let mut chips = view().flex_row().flex_wrap().gap(px(6.0));
        for skin in [
            Skin::Classic,
            Skin::Forest,
            Skin::Amber,
            Skin::Violet,
            Skin::Graphite,
        ] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(skin.label(), s.skin == skin, key, move || {
                pick.clone().apply_settings(|x| x.skin = skin);
            }));
        }
        rows.push(chip_picker_row(
            "主题".into(),
            "五套经典配色，即时生效".into(),
            chips.into_element(),
        ));
        rows.push(info_row(
            "强调色".into(),
            "随主题自动变化（当前：".to_string() + s.skin.label() + "）",
        ));

        let font_ui = ui.clone();
        let follow = s.follow_system_font;
        rows.push(toggle_row(
            follow,
            "跟随系统文字大小".into(),
            "关闭后可用正文字号单独调节".into(),
            move || {
                font_ui
                    .clone()
                    .apply_settings(|x| x.follow_system_font = !follow);
            },
        ));

        let mut chips = view().flex_row().gap(px(6.0));
        for d in [Divider::None, Divider::Light, Divider::Medium, Divider::Strong] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(d.label(), s.divider == d, key, move || {
                pick.clone().apply_settings(|x| x.divider = d);
            }));
        }
        rows.push(chip_picker_row(
            "分隔线".into(),
            "列表与正文分隔线的深浅".into(),
            chips.into_element(),
        ));

        let group = settings_group(String::new(), rows);
        let reset_ui = ui.clone();
        vec![
            group,
            settings_group(
                String::new(),
                vec![restore_default_row(move || {
                    reset_ui.clone().apply_settings(|x| {
                        let d = Settings::default();
                        x.skin = d.skin;
                        x.follow_system_font = d.follow_system_font;
                        x.divider = d.divider;
                    });
                })],
            ),
        ]
    });
    scroll()
        .flex_1()
        .child(view().w_full().gap(px(20.0)).child(body))
        .into_element()
}

/// 帖子列表：排版样式 / 头像三件套 / 已读淡化 / 显示选项。
fn settings_thread_list_page(ui: Ui) -> AnyElement {
    let body = dynamic(move || {
        let ui = ui.clone();
        let s = ui.settings.get();
        let key = s.skin.color_key();
        let mut rows: Vec<AnyElement> = Vec::new();

        let mut chips = view().flex_row().gap(px(6.0));
        for style in [ListStyle::LargeAvatar, ListStyle::SmallAvatar, ListStyle::NoAvatar] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(style.label(), s.list_style == style, key, move || {
                pick.clone().apply_settings(|x| x.list_style = style);
            }));
        }
        rows.push(chip_picker_row(
            "排版样式".into(),
            "列表行的头像排布".into(),
            chips.into_element(),
        ));

        let mut chips = view().flex_row().gap(px(6.0));
        for size in [AvatarSize::Small, AvatarSize::Medium, AvatarSize::Large] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(size.label(), s.avatar_size == size, key, move || {
                pick.clone().apply_settings(|x| x.avatar_size = size);
            }));
        }
        rows.push(chip_picker_row(
            "头像大小".into(),
            String::new(),
            chips.into_element(),
        ));

        let mut chips = view().flex_row().gap(px(6.0));
        for shape in [AvatarShape::Round, AvatarShape::Square] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(shape.label(), s.avatar_shape == shape, key, move || {
                pick.clone().apply_settings(|x| x.avatar_shape = shape);
            }));
        }
        rows.push(chip_picker_row(
            "头像形状".into(),
            String::new(),
            chips.into_element(),
        ));

        let mut chips = view().flex_row().flex_wrap().gap(px(6.0));
        for mode in [
            DefaultAvatar::Initial,
            DefaultAvatar::Pixel,
            DefaultAvatar::InitialLight,
        ] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(mode.label(), s.default_avatar == mode, key, move || {
                pick.clone().apply_settings(|x| x.default_avatar = mode);
            }));
        }
        rows.push(chip_picker_row(
            "默认头像".into(),
            "用户未上传头像时显示".into(),
            chips.into_element(),
        ));

        let mut chips = view().flex_row().flex_wrap().gap(px(6.0));
        for fade in [ReadFade::Light, ReadFade::Heavy, ReadFade::None] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(fade.label(), s.read_fade == fade, key, move || {
                pick.clone().apply_settings(|x| x.read_fade = fade);
            }));
        }
        rows.push(chip_picker_row(
            "已读帖子标题".into(),
            String::new(),
            chips.into_element(),
        ));

        let alt_ui = ui.clone();
        let alt = s.alternating_rows;
        rows.push(toggle_row(
            alt,
            "交替行底色".into(),
            "相邻帖子用不同底色区分".into(),
            move || {
                alt_ui
                    .clone()
                    .apply_settings(|x| x.alternating_rows = !alt);
            },
        ));
        let mark_ui = ui.clone();
        let mark = s.new_reply_mark;
        rows.push(toggle_row(
            mark,
            "新回复标记".into(),
            "显示回复数胶囊".into(),
            move || {
                mark_ui
                    .clone()
                    .apply_settings(|x| x.new_reply_mark = !mark);
            },
        ));
        let dot_ui = ui.clone();
        let dot = s.unread_dot;
        rows.push(toggle_row(
            dot,
            "未读帖子显示圆点".into(),
            String::new(),
            move || {
                dot_ui
                    .clone()
                    .apply_settings(|x| x.unread_dot = !dot);
            },
        ));
        let last_ui = ui.clone();
        let last = s.show_last_reply;
        rows.push(toggle_row(
            last,
            "显示最后回复人".into(),
            String::new(),
            move || {
                last_ui
                    .clone()
                    .apply_settings(|x| x.show_last_reply = !last);
            },
        ));

        let group = settings_group(String::new(), rows);
        let reset_ui = ui.clone();
        vec![
            group,
            settings_group(
                String::new(),
                vec![restore_default_row(move || {
                    reset_ui.clone().apply_settings(|x| {
                        let d = Settings::default();
                        x.list_style = d.list_style;
                        x.avatar_size = d.avatar_size;
                        x.avatar_shape = d.avatar_shape;
                        x.default_avatar = d.default_avatar;
                        x.read_fade = d.read_fade;
                        x.alternating_rows = d.alternating_rows;
                        x.new_reply_mark = d.new_reply_mark;
                        x.unread_dot = d.unread_dot;
                        x.show_last_reply = d.show_last_reply;
                    });
                })],
            ),
        ]
    });
    scroll()
        .flex_1()
        .child(view().w_full().gap(px(20.0)).child(body))
        .into_element()
}

/// 阅读与回复：标题字号 / 字重 / 间距 / 正文字号。
fn settings_reading_page(ui: Ui) -> AnyElement {
    let body = dynamic(move || {
        let ui = ui.clone();
        let s = ui.settings.get();
        let key = s.skin.color_key();
        let mut rows: Vec<AnyElement> = Vec::new();

        let mut chips = view().flex_row().gap(px(6.0));
        for size in [TitleSize::Small, TitleSize::Medium, TitleSize::Large, TitleSize::ExtraLarge] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(size.label(), s.title_size == size, key, move || {
                pick.clone().apply_settings(|x| x.title_size = size);
            }));
        }
        rows.push(chip_picker_row("字号".into(), "列表标题字号".into(), chips.into_element()));

        let mut chips = view().flex_row().gap(px(6.0));
        for w in [TitleWeight::Regular, TitleWeight::Medium, TitleWeight::Semibold, TitleWeight::Bold] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(w.label(), s.title_weight == w, key, move || {
                pick.clone().apply_settings(|x| x.title_weight = w);
            }));
        }
        rows.push(chip_picker_row("字重".into(), "列表标题粗细".into(), chips.into_element()));

        let mut chips = view().flex_row().flex_wrap().gap(px(6.0));
        for sp in [Spacing::Tight, Spacing::Normal, Spacing::Relaxed, Spacing::Loose] {
            let pick = ui.clone();
            chips = chips.child(filter_chip_color(sp.label(), s.spacing == sp, key, move || {
                pick.clone().apply_settings(|x| x.spacing = sp);
            }));
        }
        rows.push(chip_picker_row("间距".into(), "列表行距，可逐档调节".into(), chips.into_element()));

        let mut body_chips = view()
            .flex_row()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(move || theme().muted);
        for mine in [TextScale::Small, TextScale::Normal, TextScale::Large] {
            let pick = ui.clone();
            body_chips = body_chips.child(scale_chip(s.text_scale, mine, move || {
                pick.clone().apply_settings(|x| x.text_scale = mine);
            }));
        }
        rows.push(chip_picker_row(
            "正文字号".into(),
            "帖子正文大小".into(),
            body_chips.into_element(),
        ));

        let group = settings_group(String::new(), rows);
        let reset_ui = ui.clone();
        vec![
            group,
            settings_group(
                String::new(),
                vec![restore_default_row(move || {
                    reset_ui.clone().apply_settings(|x| {
                        let d = Settings::default();
                        x.title_size = d.title_size;
                        x.title_weight = d.title_weight;
                        x.spacing = d.spacing;
                        x.text_scale = d.text_scale;
                    });
                })],
            ),
        ]
    });
    scroll()
        .flex_1()
        .child(view().w_full().gap(px(20.0)).child(body))
        .into_element()
}

fn settings_ui_page(ui: Ui) -> AnyElement {
    let for_reading = ui.clone();
    let reading = dynamic(move || {
        let ui = for_reading.clone();
        let settings = ui.settings.get();
        let mut rows: Vec<AnyElement> = Vec::new();

        let mut chips = view()
            .flex_row()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(move || theme().muted);
        for mine in [
            settings::TextScale::Small,
            settings::TextScale::Normal,
            settings::TextScale::Large,
        ] {
            let pick = ui.clone();
            chips = chips.child(scale_chip(settings.text_scale, mine, move || {
                pick.clone().apply_settings(|s| s.text_scale = mine);
            }));
        }
        rows.push(setting_row(
            "正文字号".into(),
            "只影响帖子正文".into(),
            chips.into_element(),
        ));

        let avatar_ui = ui.clone();
        let show_avatar = settings.show_avatar;
        rows.push(toggle_row(
            show_avatar,
            "显示头像".into(),
            "关掉可少一轮图片请求".into(),
            move || {
                avatar_ui
                    .clone()
                    .apply_settings(|s| s.show_avatar = !show_avatar);
            },
        ));

        let circle_ui = ui.clone();
        let circle = settings.circle_avatar;
        rows.push(toggle_row(
            circle,
            "圆形头像".into(),
            "关掉为圆角方形".into(),
            move || {
                circle_ui
                    .clone()
                    .apply_settings(|s| s.circle_avatar = !circle);
            },
        ));

        vec![settings_group(String::new(), rows)]
    });

    scroll()
        .flex_1()
        .child(view().w_full().gap(px(20.0)).child(reading))
        .into_element()
}

/// 消息通知：本客户端不驻留后台，没有系统推送；站内消息走「通知」列表。
fn settings_notify_page(ui: Ui) -> AnyElement {
    let nav = ui.clone();
    let rows = vec![
        info_row(
            "后台通知".into(),
            "本客户端不常驻后台，不发送系统推送".into(),
        ),
        nav_row("站内通知".into(), "回复、私信在通知列表里看".into(), move || {
            nav.clone().load_list(ListSource::Notifications)
        }),
    ];
    scroll()
        .flex_1()
        .child(
            view()
                .w_full()
                .child(settings_group(String::new(), rows)),
        )
        .into_element()
}

/// 图片与缓存：正文图片开关 + 缓存说明（缓存由图片层 LRU 管理，无清理 API）。
fn settings_network_page(ui: Ui) -> AnyElement {
    let for_images = ui.clone();
    let images = dynamic(move || {
        let ui = for_images.clone();
        let show = ui.settings.get().show_images;
        let clear_ui = ui.clone();
        let rows = vec![
            toggle_row(
                show,
                "显示正文图片".into(),
                "关掉只显示图片数量".into(),
                move || {
                    ui.clone().apply_settings(|s| s.show_images = !show);
                },
            ),
            info_row(
                "图片缓存".into(),
                "由系统按最近使用自动回收，不提供手动清理".into(),
            ),
            nav_row(
                "清除浏览缓存".into(),
                format!("当前 {} 条（首页 / 列表 / 帖子）", cache::len()),
                move || {
                    cache::clear();
                    clear_ui.clone().note("已清除本机浏览缓存，下次打开会重新加载");
                },
            ),
        ];
        vec![settings_group(String::new(), rows)]
    });
    scroll()
        .flex_1()
        .child(view().w_full().child(images))
        .into_element()
}

/// 其它：服务器、草稿、版本与声明。
fn settings_other_page(ui: Ui) -> AnyElement {
    let draft_ui = ui.clone();
    let rows = vec![
        info_row(
            "论坛服务器".into(),
            "www.4d4y.com（暂不支持切换）".into(),
        ),
        nav_row(
            "清除本机草稿".into(),
            "只删未发送的回复草稿".into(),
            move || {
                let ui = draft_ui.clone();
                session::clear_draft();
                ui.editor.set(String::new());
                ui.note("本机草稿已清除");
            },
        ),
        info_row(
            "版本".into(),
            format!("{} · NSH + Rust", env!("CARGO_PKG_VERSION")),
        ),
        info_row(
            "声明".into(),
            "非官方客户端，与 4D4Y 官方无关".into(),
        ),
    ];
    scroll()
        .flex_1()
        .child(
            view()
                .w_full()
                .child(settings_group(String::new(), rows)),
        )
        .into_element()
}

fn blacklist_view(ui: Ui) -> impl IntoElement {
    let for_home = ui.clone();
    let for_input = ui.clone();
    let for_add = ui.clone();
    let for_count = ui.clone();
    let for_rows = ui.clone();
    let for_refresh = ui.clone();
    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ghost_button_icon(ICON_HOME, "首页".into(), move || {
                    for_home.screen.set(Screen::Index)
                }))
                .child(text("黑名单").text_size(px(20.0)).font_weight(FontWeight::BOLD).flex_1()),
        )
        .child(
            input()
                .placeholder("输入论坛用户名")
                .value(move || for_input.blacklist_input.get())
                .on_change({
                    let ui = ui.clone();
                    move |v: &str| ui.blacklist_input.set(v.to_string())
                })
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .min_h(px(44.0))
                .p_3()
                .rounded(px(10.0))
                .bg(move || theme().card),
        )
        .child(ghost_button("加入黑名单".into(), move || {
            let ui = for_add.clone();
            ui.clone().set_blacklist(ui.blacklist_input.get(), true);
        }))
        .child(
            virtual_list(
                move || for_count.blacklist_rows.with(|r| r.len()),
                move |at| blacklist_row(for_rows.clone(), at),
            )
            .keys(move || {
                for_count
                    .blacklist_rows
                    .with(|r| r.clone())
            })
            .estimated_item_size(px(52.0))
            .flex_1(),
        )
        .child(ghost_button("刷新名单".into(), move || {
            for_refresh.clone().load_blacklist();
        }))
}

fn sms_compose_view(ui: Ui) -> impl IntoElement {
    let for_home = ui.clone();
    let for_cancel = ui.clone();
    let for_input = ui.clone();
    let for_send = ui.clone();
    let for_emoji = ui.clone();
    let for_panel = ui.clone();
    view()
        .flex_1()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(ghost_button_icon(ICON_BACK, "返回".into(), move || {
                    for_cancel.screen.set(Screen::Feed)
                }))
                .child(ghost_button_icon(ICON_HOME, "首页".into(), move || {
                    for_home.screen.set(Screen::Index)
                }))
                .child(text(move || format!("收件人：{}", ui.sms_to_name.get()))
                    .text_size(px(14.0))
                    .flex_1()),
        )
        .child(
            input()
                .placeholder("写下私信内容…")
                .multiline(true)
                .value(move || for_input.sms_body.get())
                .on_change({
                    let ui = ui.clone();
                    move |v: &str| ui.sms_body.set(v.to_string())
                })
                .auto_correct(true)
                .min_h(px(160.0))
                .p_4()
                .rounded(px(12.0))
                .bg(move || theme().card),
        )
        .child(primary_button("发送私信".into(), move || {
            for_send.clone().send_sms();
        }))
        .child(ghost_button("插入表情".into(), move || {
            let ui = for_emoji.clone();
            ui.emoji_open.set(!ui.emoji_open.get());
        }))
        .child(dynamic(move || {
            if for_panel.emoji_open.get() {
                vec![emoji_picker(for_panel.clone()).into_element()]
            } else {
                Vec::new()
            }
        }))
}

/// 61 个论坛表情的选择器。用虚拟化列表避免一次创建 61 个节点；
/// 显示的是短码与占位字形，点击后把短码插入正文（不依赖外链图片域名）。
fn emoji_picker(ui: Ui) -> impl IntoElement {
    let for_rows = ui.clone();
    view()
        .h(px(180.0))
        .rounded(px(12.0))
        .bg(move || theme().muted)
        .child(
            virtual_list(SMILEY_COUNT, move |at| {
                let ui = for_rows.clone();
                view()
                    .min_h(px(40.0))
                    .px_3()
                    .py_2()
                    .rounded(px(8.0))
                    .bg(move || theme().card)
                    .active(|s| s.opacity(0.6))
                    .on_press(move || {
                        if let Some((_, _, code)) = SMILEYS.get(at.index()) {
                            ui.clone().insert_emoji((*code).to_string());
                        }
                    })
                    .child(
                        text(move || {
                            SMILEYS
                                .get(at.index())
                                .map(|(_, display, code)| format!("{display}   {code}"))
                                .unwrap_or_default()
                        })
                        .text_size(px(13.0)),
                    )
            })
            .estimated_item_size(px(40.0))
            .flex_1(),
        )
}

/// 主题首楼的投票卡片：选项可勾选，多选按 `max_answer` 限制。
fn poll_card(ui: Ui) -> impl IntoElement {
    let for_opts = ui.clone();
    let for_submit = ui.clone();
    view()
        .gap_2()
        .px_4()
        .py_3()
        .rounded(px(12.0))
        .bg(move || theme().card)
        .child(text(move || ui.poll_title.get()).font_weight(FontWeight::SEMIBOLD))
        .child(
            text(move || {
                if ui.poll_multi.get() {
                    format!("多选 · 最多可选 {} 项", ui.poll_max.get().max(1))
                } else {
                    "单选投票".to_string()
                }
            })
            .text_size(px(12.0))
            .text_color(move || theme().muted_foreground),
        )
        .child(dynamic(move || {
            for_opts
                .poll_rows
                .get()
                .iter()
                .map(|o| {
                    let id = o.id.clone();
                    let id_press = id.clone();
                    let label = o.text.clone();
                    let rates = o.rates.clone();
                    let ui_press = for_opts.clone();
                    let ui_text = for_opts.clone();
                    view()
                        .min_h(px(44.0))
                        .px_3()
                        .py_2()
                        .rounded(px(8.0))
                        .bg(move || theme().secondary)
                        .active(|s| s.opacity(0.6))
                        .on_press(move || {
                            ui_press.clone().toggle_poll_option(id_press.clone())
                        })
                        .child(
                            text(move || {
                                let checked = ui_text.poll_selected.get().contains(&id);
                                format!(
                                    "{} {} {}",
                                    if checked { "☑" } else { "☐" },
                                    label,
                                    rates
                                )
                            })
                            .text_size(px(13.0)),
                        )
                        .into_element()
                })
                .collect::<Vec<AnyElement>>()
        }))
        .child(text(move || ui.poll_footer.get())
            .text_size(px(11.0))
            .text_color(move || theme().muted_foreground))
        .child(primary_button("提交投票".into(), move || {
            for_submit.clone().submit_vote();
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_lands_on_discovery_first_page_not_login_screen() {
        let (screen, fid, page) = post_login_target();
        assert_eq!(screen, Screen::Threads);
        assert_eq!(forum_name(fid), "Discovery");
        assert_eq!(fid, FID_DISCOVERY);
        assert_eq!(page, 1);
        // 未登录时的首屏是论坛首页，两者不要混为一谈。
        assert_eq!(Screen::default(), Screen::Index);
    }

    #[test]
    fn validation_waits_for_interaction_and_reports_first_missing_field() {
        assert_eq!(validate_login("", ""), "请输入用户名");
        assert_eq!(validate_login("alice", ""), "请输入密码");
        assert_eq!(validate_login("alice", "secret"), "");
    }

    #[test]
    fn thread_rows_keep_state_in_text_not_only_color() {
        let thread = Thread {
            tid: "42".into(),
            title: "测试主题".into(),
            author: "alice".into(),
            reply_count: 3,
            view_count: 10,
            sticky: true,
            is_new: true,
            ..Default::default()
        };
        let row = ThreadRow::from_thread(&thread);
        assert_eq!(row.tid, "42");
        assert_eq!(row.badge, "置顶 新");
        assert!(row.meta.contains("3 回复"));
    }

    #[test]
    fn post_rows_use_copy_text() {
        let post = Post {
            floor: 7,
            author: "bob".into(),
            contents: vec![forum_core::model::Content {
                kind: forum_core::model::ContentKind::Text,
                text: "正文".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let row = PostRow::from_post(&post);
        assert_eq!(row.floor, 7);
        assert_eq!(row.body, "正文");
    }

    #[test]
    fn post_rows_collect_only_image_content() {
        let post = Post {
            contents: vec![
                forum_core::model::Content {
                    kind: forum_core::model::ContentKind::Image,
                    url: "attachments/a.jpg".into(),
                    ..Default::default()
                },
                forum_core::model::Content {
                    kind: forum_core::model::ContentKind::Text,
                    text: "文字".into(),
                    ..Default::default()
                },
                forum_core::model::Content {
                    kind: forum_core::model::ContentKind::Image,
                    url: "https://img02.4d4y.com/b.png".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let row = PostRow::from_post(&post);
        assert_eq!(row.images.len(), 2);
        assert_eq!(row.images[0], "attachments/a.jpg");
        assert_eq!(row.images[1], "https://img02.4d4y.com/b.png");
        assert!(row.body.contains("文字"));
    }

    #[test]
    fn default_screen_is_forum_index_not_login() {
        assert_eq!(Screen::default(), Screen::Index, "首屏必须是论坛首页");
    }

    #[test]
    fn index_rows_flatten_sections_without_losing_fid() {
        let index = ForumIndex {
            sections: vec![ForumSection {
                gid: 36,
                name: "技术版区".into(),
                forums: vec![ForumEntry {
                    fid: 7,
                    name: "Geek Talks".into(),
                    threads: "40473".into(),
                    posts: "552015".into(),
                    last_title: "测试有2只梨app".into(),
                    last_author: "城南花开".into(),
                    last_time: "2026-11-10 17:36".into(),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        let rows = index_rows(&index);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "技术版区");
        assert_eq!(rows[0].forums[0].fid, 7);
        assert!(rows[0].forums[0].counts.contains("40473"));
        assert!(rows[0].forums[0].last.contains("城南花开"));
    }

    #[test]
    fn offline_index_still_gives_guests_something_to_browse() {
        let rows = index_rows(&local_forum_index());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].forums.len(), 21);
        assert_eq!(rows[0].forums[0].fid, FID_DISCOVERY);
        assert!(rows[0].forums[0].counts.is_empty(), "本地兜底不应伪造统计数");
    }

    #[test]
    fn simple_rows_keep_new_state_in_text() {
        let item = SimpleItem {
            tid: "3057651".into(),
            title: "系统通知".into(),
            info: "版主移动了你的主题".into(),
            author: "quejuwen".into(),
            is_new: true,
            ..Default::default()
        };
        let row = SimpleRow::from_item(&item);
        assert_eq!(row.tid, "3057651");
        assert_eq!(row.badge, "新");
        assert!(row.meta.contains("quejuwen"));
        // 标题已在主行显示，不在 meta 里重复一遍。
        assert!(!row.meta.contains("系统通知"));
    }

    #[test]
    fn hub_entries_cover_all_personal_features() {
        let names: Vec<&str> = (0..HUB_COUNT).map(|i| hub_entry(i).0).collect();
        assert_eq!(
            names,
            vec![
                "通知", "私信", "我的帖子", "我的回复", "我的收藏", "我的关注",
                "最新帖子", "黑名单", "搜索主题",
            ]
        );
        // 除黑名单与搜索外，每个入口都带一个内核列表数据源。
        for i in 0..HUB_COUNT {
            if i == 7 || i == 8 {
                assert!(hub_entry(i).2.is_none());
            } else {
                assert!(hub_entry(i).2.is_some(), "{i} 缺少 ListSource");
            }
        }
        assert_eq!(
            hub_entry(2).2,
            Some(ListSource::MyPosts { page: 1 })
        );
    }

    #[test]
    fn image_whitelist_comes_from_nsh_manifest_and_blocks_unlisted_hosts() {
        let hosts = allowed_image_hosts();
        assert!(hosts.iter().any(|h| h == "*.4d4y.com"), "{hosts:?}");
        let urls = vec![
            "https://img02.4d4y.com/a.png".to_string(),
            "https://cdn.example.com/b.png".to_string(),
            "attachments/c.jpg".to_string(),
        ];
        let (visible, missing) = filter_image_urls(urls);
        assert_eq!(
            visible,
            vec![
                "https://img02.4d4y.com/a.png".to_string(),
                "attachments/c.jpg".to_string(),
            ]
        );
        assert_eq!(missing, vec!["cdn.example.com".to_string()]);
    }

    #[test]
    fn smiley_picker_covers_all_forum_shortcodes() {
        assert_eq!(SMILEY_COUNT, 61);
        assert_eq!(SMILEYS.len(), SMILEY_COUNT);
        // 短码不能退化成图片 basename（两组表情都叫 01）。
        assert!(SMILEYS.iter().all(|(_, _, code)| !code.is_empty()));
    }

    /// 正文整理：图片占位符去掉、连续空行压成一个。
    #[test]
    fn tidy_body_drops_image_placeholder_and_collapses_blank_lines() {
        let raw = "看看Newton能够做到些什么吧\n[图片]\n\n\n\n下一段\n";
        assert_eq!(tidy_body(raw), "看看Newton能够做到些什么吧\n\n下一段");
        // 行内占位符只删标记，不破坏同行文字。
        assert_eq!(tidy_body("图在此 [图片] 后面"), "图在此  后面");
    }
}
