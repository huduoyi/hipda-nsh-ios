//! 论坛表情短码、资源名与 BBCode。
//!
//! 原客户端 `emoji` module 有 110 张 PNG：21 个默认表情、16 个「酷猴」、
//! 24 个「葡萄人」，再加这 40 张的深色变体和 9 个 UI 图标。深色变体与
//! 基础图共用同一个短码，因此**独立表情是 61 个**。数据来自
//! `emoji/.../emoji/{Default,Monkey,Dumb,MonkeyDark,DumbDark}.java` 的
//! `EMOJIS` / `IMG_SRCS` 三组表。
//!
//! 论坛正文里的表情是 `<img src=".../images/smilies/<组>/<文件>.gif">`，
//! Java 端取 `SmiliesPattern` 之后的相对路径、把 `/` 换成 `_` 得到资源名。
//! 本模块沿用同一规则，把图片还原成可读文本，而不是留下 `:01:` 这种
//! 会把「酷猴 01」和「葡萄 01」混淆的短码。

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

fn aliases() -> &'static RwLock<HashMap<String, String>> {
    static ALIASES: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    ALIASES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 无闭合冒号的裸短码，必须按字面匹配（否则 `:)` 会被 `:key:` 扫描吞掉）。
const LITERAL_CODES: &[&str] = &[":(", ":)", ":D", ":o", ":P"];

/// `(论坛资源名, 显示文本, 论坛短码)`。
///
/// 默认组有明确语义，直接换成 Unicode 表情；酷猴 / 葡萄人只有编号、
/// 没有语义信息，用带编号的中文标注，保证可访问且彼此可区分。
pub static SMILEYS: &[(&str, &str, &str)] = &[
    // Default（21）
    ("default_smile", "😊", ":)"),
    ("default_sweat", "😅", ":sweat:"),
    ("default_huffy", "😠", ":huffy:"),
    ("default_cry", "😢", ":cry:"),
    ("default_titter", "😁", ":titter:"),
    ("default_handshake", "🤝", ":handshake:"),
    ("default_victory", "✌️", ":victory:"),
    ("default_curse", "🤬", ":curse:"),
    ("default_dizzy", "😵", ":dizzy:"),
    ("default_shutup", "🤐", ":shutup:"),
    ("default_funk", "😨", ":funk:"),
    ("default_loveliness", "😍", ":loveliness:"),
    ("default_sad", "😞", ":("),
    ("default_biggrin", "😃", ":D"),
    ("default_cool", "😎", ":cool:"),
    ("default_mad", "😡", ":mad:"),
    ("default_shocked", "😮", ":o"),
    ("default_tongue", "😜", ":P"),
    ("default_lol", "😂", ":lol:"),
    ("default_shy", "😳", ":shy:"),
    ("default_sleepy", "😴", ":sleepy:"),
    // Monkey / coolmonkey（16），短码编号 41–56
    ("coolmonkey_01", "[酷猴41]", "{:2_41:}"),
    ("coolmonkey_02", "[酷猴42]", "{:2_42:}"),
    ("coolmonkey_03", "[酷猴43]", "{:2_43:}"),
    ("coolmonkey_04", "[酷猴44]", "{:2_44:}"),
    ("coolmonkey_05", "[酷猴45]", "{:2_45:}"),
    ("coolmonkey_06", "[酷猴46]", "{:2_46:}"),
    ("coolmonkey_07", "[酷猴47]", "{:2_47:}"),
    ("coolmonkey_08", "[酷猴48]", "{:2_48:}"),
    ("coolmonkey_09", "[酷猴49]", "{:2_49:}"),
    ("coolmonkey_10", "[酷猴50]", "{:2_50:}"),
    ("coolmonkey_11", "[酷猴51]", "{:2_51:}"),
    ("coolmonkey_12", "[酷猴52]", "{:2_52:}"),
    ("coolmonkey_13", "[酷猴53]", "{:2_53:}"),
    ("coolmonkey_14", "[酷猴54]", "{:2_54:}"),
    ("coolmonkey_15", "[酷猴55]", "{:2_55:}"),
    ("coolmonkey_16", "[酷猴56]", "{:2_56:}"),
    // Dumb / grapeman（24），短码编号 57–80
    ("grapeman_01", "[葡萄57]", "{:3_57:}"),
    ("grapeman_02", "[葡萄58]", "{:3_58:}"),
    ("grapeman_03", "[葡萄59]", "{:3_59:}"),
    ("grapeman_04", "[葡萄60]", "{:3_60:}"),
    ("grapeman_05", "[葡萄61]", "{:3_61:}"),
    ("grapeman_06", "[葡萄62]", "{:3_62:}"),
    ("grapeman_07", "[葡萄63]", "{:3_63:}"),
    ("grapeman_08", "[葡萄64]", "{:3_64:}"),
    ("grapeman_09", "[葡萄65]", "{:3_65:}"),
    ("grapeman_10", "[葡萄66]", "{:3_66:}"),
    ("grapeman_11", "[葡萄67]", "{:3_67:}"),
    ("grapeman_12", "[葡萄68]", "{:3_68:}"),
    ("grapeman_13", "[葡萄69]", "{:3_69:}"),
    ("grapeman_14", "[葡萄70]", "{:3_70:}"),
    ("grapeman_15", "[葡萄71]", "{:3_71:}"),
    ("grapeman_16", "[葡萄72]", "{:3_72:}"),
    ("grapeman_17", "[葡萄73]", "{:3_73:}"),
    ("grapeman_18", "[葡萄74]", "{:3_74:}"),
    ("grapeman_19", "[葡萄75]", "{:3_75:}"),
    ("grapeman_20", "[葡萄76]", "{:3_76:}"),
    ("grapeman_21", "[葡萄77]", "{:3_77:}"),
    ("grapeman_22", "[葡萄78]", "{:3_78:}"),
    ("grapeman_23", "[葡萄79]", "{:3_79:}"),
    ("grapeman_24", "[葡萄80]", "{:3_80:}"),
];

/// 独立表情数量（深色变体与 UI 图标不计）。
pub const SMILEY_COUNT: usize = 61;

/// 注入 `:shortcode:` → 显示文本映射。
pub fn register_emoji_aliases(map: HashMap<String, String>) {
    if let Ok(mut guard) = aliases().write() {
        guard.extend(map);
    }
}

/// 资源名 → 显示文本；深色变体归一到基础资源名。
pub fn smiley_display(resource: &str) -> Option<&'static str> {
    let base = resource.strip_suffix("_dark").unwrap_or(resource);
    SMILEYS
        .iter()
        .find(|(name, _, _)| *name == base)
        .map(|(_, display, _)| *display)
}

/// 把论坛 smilies 图片 URL 还原成可读文本。
///
/// `.../images/smilies/default/smile.gif` → `😊`；
/// `.../images/smilies/coolmonkey/01.gif` → `[酷猴41]`。
/// 非 smilies URL 或未知资源返回 `None`，由调用方回退成短码。
pub fn smiley_from_url(src: &str) -> Option<String> {
    // 不带前导斜杠，完整 URL 与页面里的相对 src 都能命中。
    const MARKER: &str = "images/smilies/";
    let start = src.find(MARKER)? + MARKER.len();
    let tail = &src[start..];
    let end = tail.rfind('.').unwrap_or(tail.len());
    let resource = tail[..end].replace('/', "_");
    smiley_display(&resource).map(str::to_string)
}

/// 把论坛短码转成 `replace_emoji` 用的注册表 key。
fn code_key(code: &str) -> Option<String> {
    if let Some(inner) = code.strip_prefix("{:").and_then(|s| s.strip_suffix(":}")) {
        return Some(inner.to_string());
    }
    if code.len() > 2 && code.starts_with(':') && code.ends_with(':') {
        return Some(code[1..code.len() - 1].to_string());
    }
    if LITERAL_CODES.contains(&code) {
        return Some(code.to_string());
    }
    None
}

/// 默认别名表：资源名、默认组文件名、论坛短码都指向同一显示文本。
pub fn default_aliases() -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(SMILEYS.len() * 3);
    for (resource, display, code) in SMILEYS {
        map.insert((*resource).to_string(), (*display).to_string());
        // 默认组的文件名（去掉 `default_` 前缀）也是常见短码。
        // 酷猴 / 葡萄人的文件名只是 `01`，两组会撞车，故意不注册。
        if let Some(base) = resource.strip_prefix("default_") {
            map.insert(base.to_string(), (*display).to_string());
        }
        if let Some(key) = code_key(code) {
            map.insert(key, (*display).to_string());
        }
    }
    map
}

/// 应用启动时调用一次，装入 61 个表情的默认映射。
pub fn install_default_aliases() {
    register_emoji_aliases(default_aliases());
}

fn is_key(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'))
}

/// 把三种短码形式转成显示文本；未注册的短码原样保留，与 Java
/// `EmojiParser.parseToUnicode` 的容错行为一致：
///
/// - `:smile:` / `:sweat:`
/// - `:) :D :o :P :(`（Discuz 的裸短码）
/// - `{:2_41:}`（酷猴 / 葡萄人的编号短码）
pub fn replace_emoji(s: &str) -> String {
    let Ok(guard) = aliases().read() else {
        return s.to_string();
    };
    if guard.is_empty() {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // A) `{:2_41:}`：`{` 和 `:` 都是单字节，切片边界安全。
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            if let Some(rel) = s[i + 2..].find(":}") {
                let end = i + 2 + rel;
                let key = &s[i + 2..end];
                if is_key(key) {
                    if let Some(v) = guard.get(key) {
                        out.push_str(v);
                        i = end + 2;
                        continue;
                    }
                }
            }
        }
        // B) 裸短码 `:) :D :o :P :(`，优先于 `:key:` 扫描。
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii() {
            let candidate = &s[i..i + 2];
            if LITERAL_CODES.contains(&candidate) {
                if let Some(v) = guard.get(candidate) {
                    out.push_str(v);
                    i += 2;
                    continue;
                }
            }
        }
        // C) `:smile:` 形式。
        if bytes[i] == b':' {
            if let Some(rel) = s[i + 1..].find(':') {
                let end = i + 1 + rel;
                let key = &s[i + 1..end];
                if is_key(key) {
                    if let Some(v) = guard.get(key) {
                        out.push_str(v);
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 上传成功后插入正文的图片 BBCode。
pub fn bbcode_image(attach_id: &str) -> String {
    format!("[attachimg]{attach_id}[/attachimg]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_all_distinct_emoticons() {
        assert_eq!(SMILEYS.len(), SMILEY_COUNT);
        let unique: std::collections::HashSet<&str> =
            SMILEYS.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(unique.len(), SMILEY_COUNT, "资源名不得重复");
    }

    #[test]
    fn smiley_urls_map_back_to_display_text() {
        assert_eq!(
            smiley_from_url("https://img02.4d4y.com/forum/images/smilies/default/smile.gif")
                .as_deref(),
            Some("😊")
        );
        assert_eq!(
            smiley_from_url("images/smilies/coolmonkey/01.gif").as_deref(),
            Some("[酷猴41]")
        );
        assert_eq!(
            smiley_from_url("images/smilies/grapeman/24.gif").as_deref(),
            Some("[葡萄80]")
        );
        // 深色变体与基础图同一表情。
        assert_eq!(smiley_display("coolmonkey_01_dark"), Some("[酷猴41]"));
        assert_eq!(smiley_from_url("images/attachicons/common.gif"), None);
    }

    #[test]
    fn default_aliases_cover_three_code_forms() {
        let map = default_aliases();
        assert_eq!(map.get("smile").map(String::as_str), Some("😊"));
        assert_eq!(map.get("default_smile").map(String::as_str), Some("😊"));
        assert_eq!(map.get("sweat").map(String::as_str), Some("😅"));
        assert_eq!(map.get(":)").map(String::as_str), Some("😊"));
        assert_eq!(map.get("2_41").map(String::as_str), Some("[酷猴41]"));
        assert_eq!(map.get("3_80").map(String::as_str), Some("[葡萄80]"));
        // 两组文件都叫 `01`，不能用它当短码。
        assert!(!map.contains_key("01"));
    }

    #[test]
    fn replace_emoji_handles_all_three_code_forms() {
        // 只注册本用例用到的键，避免依赖全局注册表的测试执行顺序。
        register_emoji_aliases(HashMap::from([
            ("sweat".to_string(), "S".to_string()),
            (":)".to_string(), "H".to_string()),
            ("2_41".to_string(), "M".to_string()),
        ]));
        assert_eq!(
            replace_emoji("hi :sweat: :) {:2_41:} :unknown:"),
            "hi S H M :unknown:"
        );
        // 裸短码优先于 `:key:` 扫描，且中文不会被切片打断。
        assert_eq!(replace_emoji("中文:)末尾"), "中文H末尾");
    }
}
