//! 用户设置。
//!
//! 分组与条目照搬原 HiPDA·NG 客户端（`~/code/hipda` 的 `res/xml/pref_*.xml`），
//! 再按 0.2 版界面重做补齐：主题皮肤、字号字重、行距、头像三件套、
//! 交替行底色、已读淡化、分隔线深浅、搜索范围等。
//!
//! 存储走 `Storage`（原生落 app 数据目录、Web 落 localStorage），
//! 键名版本化；字段缺失时按默认值补，旧版本快照不会被判成损坏。

use forum_core::config::DEFAULT_FORUMS;
use nsh_core::storage::Storage;

/// 版本化键名：格式变更时换 key，旧数据自然失效。
pub const SETTINGS_KEY: &str = "hipda.settings.v1";

/// 正文字号档位。跨端没有字体文件概念，改成三档缩放。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextScale {
    Small,
    #[default]
    Normal,
    Large,
}

impl TextScale {
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "小",
            Self::Normal => "标准",
            Self::Large => "大",
        }
    }

    pub fn body_size(self) -> f32 {
        match self {
            Self::Small => 13.0,
            Self::Normal => 15.0,
            Self::Large => 17.0,
        }
    }

    pub fn line_height(self) -> f32 {
        self.body_size() * 1.67
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Normal => "normal",
            Self::Large => "large",
        }
    }

    fn from_key(raw: &str) -> Self {
        match raw {
            "small" => Self::Small,
            "large" => Self::Large,
            _ => Self::Normal,
        }
    }
}

/// 经典主题皮肤。明暗由系统决定，皮肤只切换强调色板 —— 正是 NSH
/// 的主题机制允许的范围：颜色来自 app，运行时改不了 nsh.json 的 appearance。
/// 五套都是论坛客户端里常见的经典配色，默认「经典蓝」沿用主题原色。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Skin {
    #[default]
    Classic,
    Forest,
    Amber,
    Violet,
    Graphite,
}

impl Skin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "经典蓝",
            Self::Forest => "松林绿",
            Self::Amber => "暖琥珀",
            Self::Violet => "紫罗兰",
            Self::Graphite => "石墨灰",
        }
    }

    /// `theme().color(key)` 的键。Classic 不查自定义键，直接用主题主色。
    pub fn color_key(self) -> &'static str {
        match self {
            Self::Classic => "primary",
            Self::Forest => "skin-forest",
            Self::Amber => "skin-amber",
            Self::Violet => "skin-violet",
            Self::Graphite => "skin-graphite",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Forest => "forest",
            Self::Amber => "amber",
            Self::Violet => "violet",
            Self::Graphite => "graphite",
        }
    }

    fn from_key(raw: &str) -> Self {
        match raw {
            "forest" => Self::Forest,
            "amber" => Self::Amber,
            "violet" => Self::Violet,
            "graphite" => Self::Graphite,
            _ => Self::Classic,
        }
    }
}

/// 帖子列表排版样式：大头像 / 小头像 / 无头像。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListStyle {
    #[default]
    LargeAvatar,
    SmallAvatar,
    NoAvatar,
}

impl ListStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::LargeAvatar => "大头像",
            Self::SmallAvatar => "小头像",
            Self::NoAvatar => "无头像",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::LargeAvatar => "large",
            Self::SmallAvatar => "small",
            Self::NoAvatar => "none",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "small" => Self::SmallAvatar,
            "none" => Self::NoAvatar,
            _ => Self::LargeAvatar,
        }
    }
}

/// 头像大小（大中小），与排版样式独立：无头像时该项不影响列表。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvatarSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl AvatarSize {
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "小",
            Self::Medium => "中",
            Self::Large => "大",
        }
    }
    pub fn pt(self) -> f32 {
        match self {
            Self::Small => 30.0,
            Self::Medium => 38.0,
            Self::Large => 46.0,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "small" => Self::Small,
            "large" => Self::Large,
            _ => Self::Medium,
        }
    }
}

/// 头像形状：方形（圆角矩形） / 圆形（原版默认）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvatarShape {
    #[default]
    Round,
    Square,
}

impl AvatarShape {
    pub fn label(self) -> &'static str {
        match self {
            Self::Round => "圆",
            Self::Square => "方",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Round => "round",
            Self::Square => "square",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "square" => Self::Square,
            _ => Self::Round,
        }
    }
}

/// 用户没上传头像时的占位方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DefaultAvatar {
    /// 首字 + 深色底（原版行为）。
    #[default]
    Initial,
    /// 名字哈希生成的像素块。
    Pixel,
    /// 首字 + 浅色底。
    InitialLight,
}

impl DefaultAvatar {
    pub fn label(self) -> &'static str {
        match self {
            Self::Initial => "首字",
            Self::Pixel => "像素块",
            Self::InitialLight => "首字浅色",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Pixel => "pixel",
            Self::InitialLight => "initial-light",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "pixel" => Self::Pixel,
            "initial-light" => Self::InitialLight,
            _ => Self::Initial,
        }
    }
}

/// 已读帖子的标题淡化程度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadFade {
    #[default]
    Light,
    Heavy,
    None,
}

impl ReadFade {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "轻度淡化",
            Self::Heavy => "重度淡化",
            Self::None => "不变",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Heavy => "heavy",
            Self::None => "none",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "heavy" => Self::Heavy,
            "none" => Self::None,
            _ => Self::Light,
        }
    }
}

/// 分隔线深浅。NSH 没有滑块，用四档点选实现同等的「划动调节」手感。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Divider {
    None,
    #[default]
    Light,
    Medium,
    Strong,
}

impl Divider {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Light => "淡",
            Self::Medium => "中",
            Self::Strong => "浓",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Strong => "strong",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "none" => Self::None,
            "medium" => Self::Medium,
            "strong" => Self::Strong,
            _ => Self::Light,
        }
    }
}

/// 标题字号档位（列表标题，不等于正文字号）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitleSize {
    Small,
    #[default]
    Medium,
    Large,
    ExtraLarge,
}

impl TitleSize {
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "小",
            Self::Medium => "中",
            Self::Large => "大",
            Self::ExtraLarge => "特大",
        }
    }
    pub fn pt(self) -> f32 {
        match self {
            Self::Small => 14.0,
            Self::Medium => 15.5,
            Self::Large => 17.0,
            Self::ExtraLarge => 19.0,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::ExtraLarge => "xl",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "small" => Self::Small,
            "large" => Self::Large,
            "xl" => Self::ExtraLarge,
            _ => Self::Medium,
        }
    }
}

/// 标题字重。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitleWeight {
    Regular,
    #[default]
    Medium,
    Semibold,
    Bold,
}

impl TitleWeight {
    pub fn label(self) -> &'static str {
        match self {
            Self::Regular => "常规",
            Self::Medium => "中等",
            Self::Semibold => "半粗",
            Self::Bold => "加粗",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Medium => "medium",
            Self::Semibold => "semibold",
            Self::Bold => "bold",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "regular" => Self::Regular,
            "semibold" => Self::Semibold,
            "bold" => Self::Bold,
            _ => Self::Medium,
        }
    }
}

/// 列表行距 / 间距档位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spacing {
    Tight,
    #[default]
    Normal,
    Relaxed,
    Loose,
}

impl Spacing {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tight => "紧凑",
            Self::Normal => "标准",
            Self::Relaxed => "宽松",
            Self::Loose => "很宽松",
        }
    }
    /// 列表行的最小高度（pt）。
    pub fn row_height(self) -> f32 {
        match self {
            Self::Tight => 58.0,
            Self::Normal => 68.0,
            Self::Relaxed => 80.0,
            Self::Loose => 94.0,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Tight => "tight",
            Self::Normal => "normal",
            Self::Relaxed => "relaxed",
            Self::Loose => "loose",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "tight" => Self::Tight,
            "relaxed" => Self::Relaxed,
            "loose" => Self::Loose,
            _ => Self::Normal,
        }
    }
}

/// 搜索时间范围，对应 Discuz 的 `srchfrom` 参数（秒）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchTime {
    Day,
    Week,
    Month,
    Year,
    #[default]
    All,
}

impl SearchTime {
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "一天内",
            Self::Week => "一周内",
            Self::Month => "一月内",
            Self::Year => "一年内",
            Self::All => "时间不限",
        }
    }
    /// `srchfrom` 秒数，0 表示不限。
    pub fn seconds(self) -> i64 {
        match self {
            Self::Day => 86400,
            Self::Week => 604800,
            Self::Month => 2592000,
            Self::Year => 31536000,
            Self::All => 0,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
            Self::All => "all",
        }
    }
    fn from_key(raw: &str) -> Self {
        match raw {
            "day" => Self::Day,
            "week" => Self::Week,
            "month" => Self::Month,
            "year" => Self::Year,
            _ => Self::All,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub text_scale: TextScale,
    /// 楼层头头像。关掉可以少一轮图片请求，弱网下明显更快。
    pub show_avatar: bool,
    /// 圆形头像（原版默认开）。由 `avatar_shape` 接管，保留字段兼容旧快照。
    pub circle_avatar: bool,
    /// 正文图片。
    pub show_images: bool,
    /// 发帖小尾巴（原版同样默认关）。
    pub tail_enabled: bool,
    pub tail_text: String,
    pub tail_url: String,
    /// 列表页顶部的默认版面。空表示用内置的三个默认版块。
    pub default_forums: Vec<i32>,
    /// 按发表时间排序。关掉是 Discuz 默认的「按最后回复」。
    pub sort_by_dateline: bool,
    /// 显示置顶帖。
    pub show_sticky: bool,

    // ---- 0.2 界面重做新增 ----
    /// 主题皮肤（强调色板）。明暗仍跟随系统。
    pub skin: Skin,
    /// 帖子列表排版样式。
    pub list_style: ListStyle,
    pub avatar_size: AvatarSize,
    pub avatar_shape: AvatarShape,
    pub default_avatar: DefaultAvatar,
    pub read_fade: ReadFade,
    pub divider: Divider,
    pub title_size: TitleSize,
    pub title_weight: TitleWeight,
    pub spacing: Spacing,
    /// 文字大小跟随系统。开启时列表/正文用系统尺寸（此处以标准档近似），
    /// 关闭时可按 `text_scale` 单独微调。
    pub follow_system_font: bool,
    /// 列表交替行底色。
    pub alternating_rows: bool,
    /// 新回复标记开关（浅色胶囊）。
    pub new_reply_mark: bool,
    /// 未读帖子显示圆点。
    pub unread_dot: bool,
    /// 显示最后回复人。
    pub show_last_reply: bool,
    /// 搜索默认：标题 / 全文。
    pub search_full_text: bool,
    /// 搜索默认版块 fid，"0" 为全部。
    pub search_fid: String,
    /// 搜索默认时间范围。
    pub search_time: SearchTime,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            text_scale: TextScale::Normal,
            show_avatar: true,
            circle_avatar: true,
            show_images: true,
            tail_enabled: false,
            tail_text: String::new(),
            tail_url: String::new(),
            default_forums: Vec::new(),
            sort_by_dateline: false,
            show_sticky: true,
            skin: Skin::Classic,
            list_style: ListStyle::LargeAvatar,
            avatar_size: AvatarSize::Medium,
            avatar_shape: AvatarShape::Round,
            default_avatar: DefaultAvatar::Initial,
            read_fade: ReadFade::Light,
            divider: Divider::Light,
            title_size: TitleSize::Medium,
            title_weight: TitleWeight::Medium,
            spacing: Spacing::Normal,
            follow_system_font: true,
            alternating_rows: true,
            new_reply_mark: true,
            unread_dot: false,
            show_last_reply: true,
            search_full_text: false,
            search_fid: "0".into(),
            search_time: SearchTime::All,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        match Storage::get_item(SETTINGS_KEY) {
            Some(raw) => Self::from_json(&raw),
            None => Self::default(),
        }
    }

    pub fn save(&self) {
        let _ = Storage::set_item(SETTINGS_KEY, self.to_json());
    }

    /// 列表页要显示的版面：去重、过滤非正数；没选过就用内置默认。
    pub fn tabs(&self) -> Vec<i32> {
        let mut picked: Vec<i32> = Vec::new();
        for fid in &self.default_forums {
            if *fid > 0 && !picked.contains(fid) {
                picked.push(*fid);
            }
        }
        if picked.is_empty() {
            DEFAULT_FORUMS.to_vec()
        } else {
            picked
        }
    }

    pub fn has_tab(&self, fid: i32) -> bool {
        self.tabs().contains(&fid)
    }

    /// 切换默认版面。至少保留一个。
    pub fn toggle_tab(&mut self, fid: i32) {
        let mut next: Vec<i32> = self.tabs();
        if let Some(pos) = next.iter().position(|x| *x == fid) {
            if next.len() > 1 {
                next.remove(pos);
            }
        } else {
            next.push(fid);
        }
        self.default_forums = next;
    }

    pub fn with_tail(&self, body: &str) -> String {
        if !self.tail_enabled {
            return body.to_string();
        }
        let text = self.tail_text.trim();
        let url = self.tail_url.trim();
        if text.is_empty() && url.is_empty() {
            return body.to_string();
        }
        let tail = match (text.is_empty(), url.is_empty()) {
            (false, false) => format!("{text} {url}"),
            (false, true) => text.to_string(),
            (true, false) => url.to_string(),
            (true, true) => String::new(),
        };
        format!("{}\n\n—— {tail}", body.trim_end())
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "text_scale": self.text_scale.key(),
            "show_avatar": self.show_avatar,
            "circle_avatar": self.circle_avatar,
            "show_images": self.show_images,
            "tail_enabled": self.tail_enabled,
            "tail_text": self.tail_text,
            "tail_url": self.tail_url,
            "default_forums": self.default_forums,
            "sort_by_dateline": self.sort_by_dateline,
            "show_sticky": self.show_sticky,
            "skin": self.skin.key(),
            "list_style": self.list_style.key(),
            "avatar_size": self.avatar_size.key(),
            "avatar_shape": self.avatar_shape.key(),
            "default_avatar": self.default_avatar.key(),
            "read_fade": self.read_fade.key(),
            "divider": self.divider.key(),
            "title_size": self.title_size.key(),
            "title_weight": self.title_weight.key(),
            "spacing": self.spacing.key(),
            "follow_system_font": self.follow_system_font,
            "alternating_rows": self.alternating_rows,
            "new_reply_mark": self.new_reply_mark,
            "unread_dot": self.unread_dot,
            "show_last_reply": self.show_last_reply,
            "search_full_text": self.search_full_text,
            "search_fid": self.search_fid,
            "search_time": self.search_time.key(),
        })
        .to_string()
    }

    /// 解析失败或字段缺失都退回默认值：设置坏了不该让 App 起不来。
    pub fn from_json(raw: &str) -> Self {
        let mut out = Self::default();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return out;
        };
        macro_rules! str_field {
            ($key:literal, $field:ident, $parse:expr) => {
                if let Some(s) = value.get($key).and_then(|v| v.as_str()) {
                    out.$field = $parse(s);
                }
            };
        }
        macro_rules! bool_field {
            ($key:literal, $field:ident) => {
                if let Some(b) = value.get($key).and_then(|v| v.as_bool()) {
                    out.$field = b;
                }
            };
        }
        str_field!("text_scale", text_scale, TextScale::from_key);
        bool_field!("show_avatar", show_avatar);
        bool_field!("circle_avatar", circle_avatar);
        bool_field!("show_images", show_images);
        bool_field!("tail_enabled", tail_enabled);
        bool_field!("sort_by_dateline", sort_by_dateline);
        bool_field!("show_sticky", show_sticky);
        bool_field!("follow_system_font", follow_system_font);
        bool_field!("alternating_rows", alternating_rows);
        bool_field!("new_reply_mark", new_reply_mark);
        bool_field!("unread_dot", unread_dot);
        bool_field!("show_last_reply", show_last_reply);
        bool_field!("search_full_text", search_full_text);
        str_field!("tail_text", tail_text, |s: &str| s.to_string());
        str_field!("tail_url", tail_url, |s: &str| s.to_string());
        str_field!("skin", skin, Skin::from_key);
        str_field!("list_style", list_style, ListStyle::from_key);
        str_field!("avatar_size", avatar_size, AvatarSize::from_key);
        str_field!("avatar_shape", avatar_shape, AvatarShape::from_key);
        str_field!("default_avatar", default_avatar, DefaultAvatar::from_key);
        str_field!("read_fade", read_fade, ReadFade::from_key);
        str_field!("divider", divider, Divider::from_key);
        str_field!("title_size", title_size, TitleSize::from_key);
        str_field!("title_weight", title_weight, TitleWeight::from_key);
        str_field!("spacing", spacing, Spacing::from_key);
        str_field!("search_fid", search_fid, |s: &str| s.to_string());
        str_field!("search_time", search_time, SearchTime::from_key);
        if let Some(list) = value.get("default_forums").and_then(|v| v.as_array()) {
            out.default_forums = list
                .iter()
                .filter_map(|v| v.as_i64())
                .filter_map(|v| i32::try_from(v).ok())
                .collect();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_keeps_every_field() {
        let mut s = Settings {
            text_scale: TextScale::Large,
            show_avatar: false,
            circle_avatar: false,
            show_images: false,
            tail_enabled: true,
            tail_text: "来自 HiPDA·nsh".into(),
            tail_url: "https://example.com".into(),
            default_forums: vec![6, 7],
            sort_by_dateline: true,
            show_sticky: false,
            skin: Skin::Violet,
            list_style: ListStyle::SmallAvatar,
            avatar_size: AvatarSize::Large,
            avatar_shape: AvatarShape::Square,
            default_avatar: DefaultAvatar::Pixel,
            read_fade: ReadFade::Heavy,
            divider: Divider::Strong,
            title_size: TitleSize::ExtraLarge,
            title_weight: TitleWeight::Bold,
            spacing: Spacing::Relaxed,
            follow_system_font: false,
            alternating_rows: false,
            new_reply_mark: false,
            unread_dot: true,
            show_last_reply: false,
            search_full_text: true,
            search_fid: "2".into(),
            search_time: SearchTime::Week,
        };
        let back = Settings::from_json(&s.to_json());
        assert_eq!(back, s);
        assert_eq!(Settings::from_json(""), Settings::default());
        assert_eq!(Settings::from_json("{oops"), Settings::default());
        s.default_forums.clear();
        assert!(Settings::from_json("{}").default_forums.is_empty());
    }

    #[test]
    fn old_snapshot_gets_new_field_defaults() {
        // 只含旧字段的快照，新增字段必须回默认值而不是崩掉。
        let old = r#"{"text_scale":"large","show_avatar":true,"default_forums":[2]}"#;
        let s = Settings::from_json(old);
        assert_eq!(s.text_scale, TextScale::Large);
        assert_eq!(s.skin, Skin::Classic);
        assert_eq!(s.list_style, ListStyle::LargeAvatar);
        assert_eq!(s.search_fid, "0");
        assert!(s.alternating_rows);
    }

    #[test]
    fn tabs_fall_back_to_builtin_and_keep_at_least_one() {
        let mut s = Settings::default();
        assert_eq!(s.tabs(), DEFAULT_FORUMS.to_vec());

        s.default_forums = vec![6, 6, 0, -1];
        assert_eq!(s.tabs(), vec![6], "去重且过滤非正数");

        s.toggle_tab(6);
        assert_eq!(s.tabs(), vec![6]);
        s.toggle_tab(7);
        assert_eq!(s.tabs(), vec![6, 7]);
        s.toggle_tab(6);
        assert_eq!(s.tabs(), vec![7]);
    }

    #[test]
    fn tail_only_appends_when_enabled_and_configured() {
        let mut s = Settings::default();
        assert_eq!(s.with_tail("正文"), "正文");

        s.tail_enabled = true;
        assert_eq!(s.with_tail("正文"), "正文", "尾巴为空时不追加空行");

        s.tail_text = "来自 HiPDA·nsh".into();
        assert_eq!(s.with_tail("正文"), "正文\n\n—— 来自 HiPDA·nsh");

        s.tail_url = "https://example.com".into();
        assert_eq!(
            s.with_tail("正文\n"),
            "正文\n\n—— 来自 HiPDA·nsh https://example.com"
        );
    }

    #[test]
    fn text_scale_keeps_line_height_proportional() {
        for scale in [TextScale::Small, TextScale::Normal, TextScale::Large] {
            let ratio = scale.line_height() / scale.body_size();
            assert!((ratio - 1.67).abs() < 0.01, "{scale:?} 行高比例跑偏");
        }
    }

    #[test]
    fn enums_roundtrip_all_variants() {
        fn check<T: Copy + PartialEq + std::fmt::Debug>(
            values: &[T],
            key: impl Fn(T) -> &'static str,
            parse: impl Fn(&str) -> T,
        ) {
            for &v in values {
                assert_eq!(parse(key(v)), v, "{v:?} 枚举往返失败");
            }
        }
        check(&[Skin::Classic, Skin::Forest, Skin::Amber, Skin::Violet, Skin::Graphite], Skin::key, Skin::from_key);
        check(&[ListStyle::LargeAvatar, ListStyle::SmallAvatar, ListStyle::NoAvatar], ListStyle::key, ListStyle::from_key);
        check(&[AvatarSize::Small, AvatarSize::Medium, AvatarSize::Large], AvatarSize::key, AvatarSize::from_key);
        check(&[AvatarShape::Round, AvatarShape::Square], AvatarShape::key, AvatarShape::from_key);
        check(&[DefaultAvatar::Initial, DefaultAvatar::Pixel, DefaultAvatar::InitialLight], DefaultAvatar::key, DefaultAvatar::from_key);
        check(&[ReadFade::Light, ReadFade::Heavy, ReadFade::None], ReadFade::key, ReadFade::from_key);
        check(&[Divider::None, Divider::Light, Divider::Medium, Divider::Strong], Divider::key, Divider::from_key);
        check(&[TitleSize::Small, TitleSize::Medium, TitleSize::Large, TitleSize::ExtraLarge], TitleSize::key, TitleSize::from_key);
        check(&[TitleWeight::Regular, TitleWeight::Medium, TitleWeight::Semibold, TitleWeight::Bold], TitleWeight::key, TitleWeight::from_key);
        check(&[Spacing::Tight, Spacing::Normal, Spacing::Relaxed, Spacing::Loose], Spacing::key, Spacing::from_key);
        check(&[SearchTime::Day, SearchTime::Week, SearchTime::Month, SearchTime::Year, SearchTime::All], SearchTime::key, SearchTime::from_key);
    }
}
