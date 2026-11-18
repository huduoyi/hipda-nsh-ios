//! 用户设置。
//!
//! 分组与条目照搬原 HiPDA·NG 客户端（`~/code/hipda` 的 `res/xml/pref_*.xml`），
//! 但**只保留在六端客户端里真正能生效的项**：Android 专属的手势返回、
//! 标题栏动态隐藏、浮动按钮、后台通知任务、屏幕方向在这里没有对应机制，
//! 图片缓存大小与清除也没有可用 API（缓存由 NSH 图片层自己 LRU 管理），
//! 这些一律不摆假开关 —— 一个点了没反应的开关比没有这个开关更糟。
//!
//! 存储走 `Storage`（原生落 app 数据目录、Web 落 localStorage），
//! 键名版本化；字段缺失时按默认值补，旧版本快照不会被判成损坏。

use forum_core::config::DEFAULT_FORUMS;
use nsh_core::storage::Storage;

/// 版本化键名：格式变更时换 key，旧数据自然失效。
pub const SETTINGS_KEY: &str = "hipda.settings.v1";

/// 正文字号档位。原版是「自定义字体」文件选择，跨端没有字体文件概念，
/// 改成三档缩放 —— 效果等价（改正文可读性），且不用处理字体授权。
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

    /// 正文字号（pt）。网页是 13px，标准档在本地屏上放大一档更好读。
    pub fn body_size(self) -> f32 {
        match self {
            Self::Small => 13.0,
            Self::Normal => 15.0,
            Self::Large => 17.0,
        }
    }

    /// 行高。保持约 1.67 倍，缩放时行距同步，不然大字号会挤成一片。
    pub fn line_height(self) -> f32 {
        self.body_size() * 1.67
    }

    fn key(self) -> &'static str {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub text_scale: TextScale,
    /// 楼层头头像。关掉可以少一轮图片请求，弱网下明显更快。
    pub show_avatar: bool,
    /// 圆形头像（原版默认开）。关掉是圆角方形，方便看清头像内容。
    pub circle_avatar: bool,
    /// 正文图片。
    pub show_images: bool,
    /// 发帖小尾巴（原版同样默认关）。
    pub tail_enabled: bool,
    pub tail_text: String,
    pub tail_url: String,
    /// 列表页顶部的默认版面。空表示用内置的三个默认版块。
    pub default_forums: Vec<i32>,
    /// 按发表时间排序（对应原客户端版块菜单里的「按发表排序」）。
    /// 关掉是 Discuz 默认的「按最后回复」。
    pub sort_by_dateline: bool,
    /// 显示置顶帖。关掉后置顶主题不进入列表，长置顶不再占满首屏。
    pub show_sticky: bool,
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

    /// 某版面是否被选作默认版面。
    pub fn has_tab(&self, fid: i32) -> bool {
        self.tabs().contains(&fid)
    }

    /// 切换默认版面。**至少保留一个**：全不选会让列表页没有入口，
    /// 只能在设置里再点回来，属于把用户锁在门外。
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

    /// 回帖正文 + 小尾巴。尾巴按原版语义是「一行分隔 + 文案 +（可选）链接」，
    /// 链接为空时只追加文案，避免留下一个空的 `[url]`。
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
        })
        .to_string()
    }

    /// 解析失败或字段缺失都退回默认值：设置坏了不该让 App 起不来。
    pub fn from_json(raw: &str) -> Self {
        let mut out = Self::default();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return out;
        };
        if let Some(s) = value.get("text_scale").and_then(|v| v.as_str()) {
            out.text_scale = TextScale::from_key(s);
        }
        if let Some(b) = value.get("show_avatar").and_then(|v| v.as_bool()) {
            out.show_avatar = b;
        }
        if let Some(b) = value.get("circle_avatar").and_then(|v| v.as_bool()) {
            out.circle_avatar = b;
        }
        if let Some(b) = value.get("show_images").and_then(|v| v.as_bool()) {
            out.show_images = b;
        }
        if let Some(b) = value.get("tail_enabled").and_then(|v| v.as_bool()) {
            out.tail_enabled = b;
        }
        if let Some(s) = value.get("tail_text").and_then(|v| v.as_str()) {
            out.tail_text = s.to_string();
        }
        if let Some(s) = value.get("tail_url").and_then(|v| v.as_str()) {
            out.tail_url = s.to_string();
        }
        if let Some(b) = value.get("sort_by_dateline").and_then(|v| v.as_bool()) {
            out.sort_by_dateline = b;
        }
        if let Some(b) = value.get("show_sticky").and_then(|v| v.as_bool()) {
            out.show_sticky = b;
        }
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
        };
        let back = Settings::from_json(&s.to_json());
        assert_eq!(back, s);
        // 空串 / 坏 JSON 一律退回默认，不让设置把 App 卡死。
        assert_eq!(Settings::from_json(""), Settings::default());
        assert_eq!(Settings::from_json("{oops"), Settings::default());
        s.default_forums.clear();
        assert!(Settings::from_json("{}").default_forums.is_empty());
    }

    #[test]
    fn tabs_fall_back_to_builtin_and_keep_at_least_one() {
        let mut s = Settings::default();
        assert_eq!(s.tabs(), DEFAULT_FORUMS.to_vec());

        s.default_forums = vec![6, 6, 0, -1];
        assert_eq!(s.tabs(), vec![6], "去重且过滤非正数");

        // 只剩一个时不允许再取消，否则列表页没有入口。
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
}
