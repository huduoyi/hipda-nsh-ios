//! 站点常量、配置与端点表。对应 Java 的 `HiUtils.java`。

use std::collections::BTreeMap;

use crate::model::{ForumEntry, ForumIndex, ForumSection};

/// 论坛主站。2026-11-13 实测根路径被 WAF 挡 403，但具体 PHP 端点可用。
pub const DEFAULT_FORUM_SERVER: &str = "https://www.4d4y.com";
/// 图片 CDN，可由 `GET /config.php` 的 `CDN` 键刷新。
pub const DEFAULT_IMAGE_HOST: &str = "https://img02.4d4y.com";
pub const COOKIE_DOMAIN: &str = "4d4y.com";
pub const AVATAR_PATH: &str = "uc_server/data/avatar/";
pub const AVATAR_SUFFIX: &str = "_avatar_middle.jpg";
pub const SMILES_PATTERN: &str = "4d4y.com/forum/images/smilies/";
/// Discuz 判定登录态的 cookie 名（`OkHttpHelper.hasAuthCookie`）。
pub const LOGIN_COOKIE: &str = "cdb_auth";

/// 客户端发布帖 tid，用于默认小尾巴。
pub const CLIENT_TID: u32 = 1_579_403;
/// 单图上传上限（`HiUtils.DEFAULT_MAX_UPLOAD_FILE_SIZE`）。
pub const DEFAULT_MAX_UPLOAD_FILE_SIZE: usize = 8 * 1024 * 1024;
/// 每页楼层数默认值（`HiSettingsHelper.getMaxPostsInPage()`）。
pub const DEFAULT_POSTS_PER_PAGE: u32 = 50;
/// 主题列表每页最少条数（`ThreadListFragment.MIN_TREADS_IN_PAGE`）。
pub const MIN_THREADS_IN_PAGE: u32 = 10;

pub const ENCODE_GBK: &str = "GBK";
pub const ENCODE_UTF8: &str = "UTF-8";
pub const CONTENT_TYPE_FORM: &str = "application/x-www-form-urlencoded";

/// 客户端配置。所有域名与编码都可覆盖，便于换站、本地 mock 或域名再次迁移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub forum_server: String,
    pub image_host: String,
    pub encode: String,
    pub user_agent: String,
    pub app_version: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            forum_server: DEFAULT_FORUM_SERVER.into(),
            image_host: DEFAULT_IMAGE_HOST.into(),
            encode: ENCODE_GBK.into(),
            user_agent: String::new(),
            app_version: "0.1.0".into(),
        }
    }
}

impl Config {
    /// 补齐默认项并去掉域名尾斜杠。
    pub fn with_defaults(mut self) -> Self {
        let d = Config::default();
        if self.forum_server.is_empty() {
            self.forum_server = d.forum_server;
        }
        if self.image_host.is_empty() {
            self.image_host = d.image_host;
        }
        if self.encode.is_empty() {
            self.encode = d.encode;
        }
        if self.app_version.is_empty() {
            self.app_version = d.app_version;
        }
        self.forum_server = self.forum_server.trim_end_matches('/').to_string();
        self.image_host = self.image_host.trim_end_matches('/').to_string();
        if self.user_agent.is_empty() {
            self.user_agent = format!("hipda-nsh {}", self.app_version);
        }
        self
    }

    pub fn base_url(&self) -> String { format!("{}/forum/", self.forum_server) }
    pub fn image_base_url(&self) -> String { format!("{}/forum/", self.image_host) }
    pub fn avatar_base_url(&self) -> String { format!("{}{}", self.image_base_url(), AVATAR_PATH) }
    pub fn config_url(&self) -> String { format!("{}/config.php", self.forum_server) }

    /// 复刻 `HiUtils.getAvatarUrlByUid`：uid 左补 0 至 9 位，按 3/2/2/2 分段。
    /// 例：`723379 -> 000/72/33/79_avatar_middle.jpg`。
    /// 空值、超 9 位、非数字 uid 返回空串（与源码一致）。
    pub fn avatar_url(&self, uid: &str) -> String {
        if uid.is_empty() || uid.len() > 9 || !uid.bytes().all(|b| b.is_ascii_digit()) {
            return String::new();
        }
        let padded = format!("{}{}", "0".repeat(9 - uid.len()), uid);
        format!(
            "{}{}/{}/{}/{}{}",
            self.avatar_base_url(),
            &padded[0..3],
            &padded[3..5],
            &padded[5..7],
            &padded[7..9],
            AVATAR_SUFFIX
        )
    }
}

/// `HiUtils.updateBaseUrls()` 生成的全部 30 个 URL 模板。
/// 占位符 `{xxx}` 由 [`fill`] 替换，带尾等号的是拼接型 URL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub thread_list: String,
    pub detail_list: String,
    pub reply: String,
    pub edit: String,
    pub new_thread: String,
    pub vote_poll: String,
    pub my_reply: String,
    pub my_post: String,
    pub last_page: String,
    pub redirect_to_post: String,
    pub goto_post: String,
    pub sms: String,
    pub sms_detail: String,
    pub sms_prepare_post: String,
    pub sms_post_by_uid: String,
    pub sms_post_by_name: String,
    pub thread_notify: String,
    pub check_sms: String,
    pub new_sms: String,
    pub clear_sms: String,
    pub upload_image: String,
    pub search: String,
    pub search_user_thread: String,
    pub search_by_id: String,
    pub new_posts: String,
    pub favorites: String,
    pub favorite_add: String,
    pub favorite_remove: String,
    pub favorite_delete: String,
    pub user_info: String,
    pub user_warning: String,
    pub add_black: String,
    pub del_black: String,
    pub view_black: String,
    pub login_get_form_hash: String,
    pub login_submit: String,
    /// 论坛首页（`index.php`）。游客带 `cdb_sid` 后也能访问，但可见分区更少。
    pub forum_index: String,
}

impl Endpoints {
    pub fn new(c: &Config) -> Self {
        let b = c.base_url();
        Self {
            thread_list: format!("{b}forumdisplay.php?fid="),
            detail_list: format!("{b}viewthread.php?tid="),
            reply: format!("{b}post.php?action=reply&tid="),
            edit: format!("{b}post.php?action=edit"),
            new_thread: format!("{b}post.php?action=newthread&fid="),
            vote_poll: format!("{b}misc.php?action=votepoll&fid={{fid}}&tid={{tid}}&pollsubmit=yes&quickforward=yes&inajax=1"),
            my_reply: format!("{b}my.php?item=posts"),
            my_post: format!("{b}my.php?item=threads"),
            last_page: format!("{b}redirect.php?goto=lastpost&from=fastpost&tid="),
            redirect_to_post: format!("{b}redirect.php?goto=findpost&pid={{pid}}&ptid={{tid}}"),
            goto_post: format!("{b}gotopost.php?pid={{pid}}"),
            sms: format!("{b}pm.php?filter=privatepm"),
            sms_detail: format!("{b}pm.php?daterange=5&uid="),
            sms_prepare_post: format!("{b}pm.php?daterange=1&uid="),
            sms_post_by_uid: format!("{b}pm.php?action=send&pmsubmit=yes&infloat=yes&inajax=1&uid={{uid}}"),
            sms_post_by_name: format!("{b}pm.php?action=send&pmsubmit=yes&infloat=yes&inajax=1"),
            thread_notify: format!("{b}notice.php"),
            check_sms: format!("{b}pm.php?checknewpm"),
            new_sms: format!("{b}pm.php?filter=newpm"),
            clear_sms: format!("{b}pm.php?action=del&uid={{uid}}&filter=privatepm"),
            upload_image: format!("{b}misc.php?action=swfupload&operation=upload&simple=1&type=image"),
            search: format!("{b}search.php?srchtype={{srchtype}}&srchtxt={{srchtxt}}&searchsubmit=true&st=on&srchuname={{srchuname}}&srchfilter=all&srchfrom=0&before=&orderby=lastpost&ascdesc=desc&srchfid%5B0%5D={{fid}}"),
            search_user_thread: format!("{b}search.php?srchfid=all&srchfrom=0&searchsubmit=yes&srchuid={{srchuid}}"),
            search_by_id: format!("{b}search.php?searchid={{searchid}}&orderby=lastpost&ascdesc=desc&searchsubmit=yes"),
            new_posts: format!("{b}search.php?srchfrom=86400&searchsubmit=yes"),
            favorites: format!("{b}my.php?item={{item}}&type=thread"),
            favorite_add: format!("{b}my.php?item={{item}}&action=add&inajax=1&ajaxtarget=favorite_msg&tid={{tid}}"),
            favorite_remove: format!("{b}my.php?item={{item}}&action=remove&inajax=1&ajaxtarget=favorite_msg&tid={{tid}}"),
            favorite_delete: format!("{b}my.php?item={{item}}&type=thread"),
            user_info: format!("{b}space.php?uid="),
            user_warning: format!("{b}misc.php?action=viewwarning&tid={{tid}}&uid={{uid}}"),
            add_black: format!("{b}pm.php?action=addblack"),
            del_black: format!("{b}pm.php?action=delblack"),
            view_black: format!("{b}pm.php?action=viewblack"),
            login_get_form_hash: format!("{b}logging.php?action=login"),
            login_submit: format!("{b}logging.php?action=login&loginsubmit=yes&inajax=1"),
            forum_index: format!("{b}index.php"),
        }
    }

    /// 供测试与调试：全部端点名 → URL。
    pub fn as_map(&self) -> BTreeMap<&'static str, &str> {
        let mut m = BTreeMap::new();
        macro_rules! put { ($($f:ident),+ $(,)?) => { $( m.insert(stringify!($f), self.$f.as_str()); )+ } }
        put!(
            thread_list, detail_list, reply, edit, new_thread, vote_poll, my_reply, my_post,
            last_page, redirect_to_post, goto_post, sms, sms_detail, sms_prepare_post,
            sms_post_by_uid, sms_post_by_name, thread_notify, check_sms, new_sms, clear_sms,
            upload_image, search, search_user_thread, search_by_id, new_posts, favorites,
            favorite_add, favorite_remove, favorite_delete, user_info, user_warning,
            add_black, del_black, view_black, login_get_form_hash, login_submit, forum_index,
        );
        m
    }
}

/// 替换 URL 模板里的 `{xxx}` 占位符。
pub fn fill(template: &str, kv: &[(&str, &str)]) -> String {
    let mut s = template.to_string();
    for (k, v) in kv {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// 从 href 里取查询参数（兼容 `&amp;` 与结尾无 `&` 的情况）。
/// 对应 Go 版 `QueryParam`，Discuz 的 href 常带 HTML 实体，直接用 URL 解析会漏。
pub fn query_param(href: &str, key: &str) -> String {
    let href = href.replace("&amp;", "&");
    let needle = format!("{key}=");
    let Some(start) = href.find(&needle) else { return String::new() };
    let v = &href[start + needle.len()..];
    let end = v.find(['&', '#']).unwrap_or(v.len());
    v[..end].to_string()
}

/// 版块。原客户端硬编码在 `HiUtils.FORUMS`（不从 HTML 解析）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forum {
    pub id: i32,
    pub name: &'static str,
}

pub const FID_DISCOVERY: i32 = 2;
pub const FID_BUY_SELL: i32 = 6;

/// 21 个版块（源码数组实际 21 项，注释误写 23）。
pub const FORUMS: &[Forum] = &[
    Forum { id: FID_DISCOVERY, name: "Discovery" },
    Forum { id: FID_BUY_SELL, name: "Buy & Sell" },
    Forum { id: 7, name: "Geek Talks" },
    Forum { id: 59, name: "E-INK" },
    Forum { id: 12, name: "PalmOS" },
    Forum { id: 57, name: "疑似机器人" },
    Forum { id: 63, name: "已完成交易" },
    Forum { id: 62, name: "Joggler" },
    Forum { id: 5, name: "站务与公告" },
    Forum { id: 9, name: "Smartphone" },
    Forum { id: 56, name: "iPhone, iPod Touch，iPad" },
    Forum { id: 60, name: "Android, Chrome, & Google" },
    Forum { id: 14, name: "Windows Mobile，PocketPC，HPC" },
    Forum { id: 22, name: "麦客爱苹果" },
    Forum { id: 50, name: "DC,NB,MP3,Gadgets" },
    Forum { id: 24, name: "意欲蔓延" },
    Forum { id: 23, name: "随笔与个人文集" },
    Forum { id: 25, name: "吃喝玩乐" },
    Forum { id: 51, name: "La Femme" },
    Forum { id: 65, name: "改版建议" },
    Forum { id: 64, name: "只讨论2.0" },
];

/// 默认展示的三个版块（`HiUtils.DEFAULT_FORUMS`）。
pub const DEFAULT_FORUMS: &[i32] = &[FID_DISCOVERY, FID_BUY_SELL, 7];

pub fn forum_name(fid: i32) -> &'static str {
    FORUMS.iter().find(|f| f.id == fid).map(|f| f.name).unwrap_or("")
}

/// 站点首页不可达（WAF / 断网）时的兜底：用客户端自带的 21 个版块表
/// 渲染一屏「论坛首页」，保证未登录、离线时界面仍有可浏览内容。
/// `offline: true` 让 UI 明确标注数据来源，不伪造成站点实时数据。
pub fn local_forum_index() -> ForumIndex {
    ForumIndex {
        sections: vec![ForumSection {
            gid: 0,
            name: "论坛版块（本地目录）".to_string(),
            forums: FORUMS
                .iter()
                .map(|f| ForumEntry {
                    fid: f.id,
                    name: f.name.to_string(),
                    ..Default::default()
                })
                .collect(),
        }],
        logged_in: false,
        offline: true,
    }
}

/// Buy & Sell 版块的分类筛选（`HiUtils.BS_TYPES`）。
/// 注意：2026-11-13 线上已有 15 个主题分类，本表已过时，UI 应优先用
/// [`parse::parse_thread_types`] 动态解析。
pub const BS_TYPES: &[(&str, &str)] = &[
    ("全部", ""), ("手机", "1"), ("掌上电脑", "2"), ("笔记本电脑", "3"),
    ("无线产品", "4"), ("数码相机、摄像机", "5"), ("MP3随身听", "6"),
    ("各类配件", "7"), ("其他好玩的", "8"),
];

/// 安全提问 questionid 0..7（`strings_preference.xml`），2026-11-13 实测与站点一致。
pub const SECURITY_QUESTIONS: &[&str] = &[
    "安全提问", "母亲的名字", "爷爷的名字", "父亲出生的城市",
    "您其中一位老师的名字", "您个人计算机的型号", "您最喜欢的餐馆名称",
    "驾驶执照的最后四位数字",
];
