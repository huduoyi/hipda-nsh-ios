//! 本地缓存：内存 + `Storage` 两层，语义是 **stale-while-revalidate**。
//!
//! 目标不是省流量，而是**感知速度**：命中就先把内容画出来（哪怕是几分钟前
//! 的），再在后台静默刷新 —— 用户不会再看到空白页和转圈，回到上一屏是瞬时的。
//!
//! 三条约束决定了这个模块的形状：
//!
//! 1. **存储只有 `Storage`**（原生落 app 数据目录、Web 落 localStorage），
//!    没有文件系统 API，所以缓存是一条条键值，不是文件；
//! 2. `Storage` 有配额（`StorageFull`），写失败绝不能影响浏览，只能退化成
//!    「这轮不缓存」；
//! 3. 站点内容会变，太久远的条目读出来是误导，因此有条目年龄上限。

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use nsh_core::storage::Storage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// 键前缀：带版本号，格式变更时换掉即可让旧缓存自然失效。
///
/// v2：解析层修了附件图（`zoom()` 里的原图）。v1 缓存的楼层里没有图片块，
/// 继续用会让人觉得「修了还是没图」，所以换版本，让旧条目自然作废。
/// v3：新增可点击的链接块；v2 缓存里链接还是纯文字。
const PREFIX: &str = "hipda.cache.v3.";

/// 最多保留多少条。列表页一条约 5 KB，详情页一条可达几十 KB；
/// 24 条大致控制在 1–2 MB，换机器/浏览器 localStorage 都不至于爆掉。
const MAX_ENTRIES: usize = 24;

/// 单条上限：再大就不落盘了，只留在内存里，避免一条长帖把配额吃光。
const MAX_ENTRY_BYTES: usize = 512 * 1024;

/// 条目最长可读年龄：一天。超过就当没有 —— 论坛内容早变了，
/// 拿旧数据糊在屏幕上比转圈更糟。
const MAX_AGE_SECS: u64 = 24 * 3600;

thread_local! {
    /// 进程内一级缓存：免去反复 JSON 解析与 `Storage` 读。
    /// 只在这个线程用，因此用 `RefCell` 而不是锁。
    static MEMORY: RefCell<HashMap<String, (String, u64)>> = RefCell::new(HashMap::new());
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    saved_at: u64,
    value: T,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn storage_key(key: &str) -> String {
    format!("{PREFIX}{key}")
}

/// 首页缓存键。
pub fn index_key() -> String {
    "index".to_string()
}

/// 版块列表缓存键。排序方式不同内容不同，必须进键。
pub fn list_key(fid: i32, page: i32, by_dateline: bool) -> String {
    format!("list:{fid}:{page}:{by_dateline}")
}

/// 主题详情缓存键。
pub fn detail_key(tid: &str, page: i32) -> String {
    format!("detail:{tid}:{page}")
}

/// 读缓存。返回 `(值, 存了多久)`；年龄让界面能说明「这是多久前的数据」。
pub fn get<T: DeserializeOwned>(key: &str) -> Option<(T, u64)> {
    let now = now_secs();

    let raw = MEMORY.with(|memory| {
        memory
            .borrow()
            .get(key)
            .map(|(raw, saved_at)| (raw.clone(), *saved_at))
    });
    let raw = match raw {
        Some(found) => found,
        None => {
            // 内存没有就回落到存储；存里没有才算未命中。
            let raw = Storage::get_item(&storage_key(key))?;
            let envelope: Envelope<serde_json::Value> = serde_json::from_str(&raw).ok()?;
            let saved_at = envelope.saved_at;
            MEMORY.with(|memory| {
                memory
                    .borrow_mut()
                    .insert(key.to_string(), (raw.clone(), saved_at));
            });
            (raw, saved_at)
        }
    };

    if is_expired(raw.1, now) {
        remove(key);
        return None;
    }

    let envelope: Envelope<T> = serde_json::from_str(&raw.0).ok()?;
    let age = now.saturating_sub(raw.1);
    // 命中日志：排查「为什么这一屏还是慢」时，先看缓存有没有命中。
    eprintln!("[cache] hit {key} age={age}s");
    Some((envelope.value, age))
}

/// 只判断有没有可用缓存，不解析内容（预取前用来跳过已缓存的页）。
pub fn contains(key: &str) -> bool {
    if MEMORY.with(|memory| memory.borrow().contains_key(key)) {
        return true;
    }
    Storage::get_item(&storage_key(key)).is_some()
}

/// 写缓存：先落内存，再尽力落存储。**任何失败都只是少一次加速**，
/// 不允许把异常抛给调用方影响浏览。
pub fn put<T: Serialize>(key: &str, value: &T) {
    let envelope = Envelope {
        saved_at: now_secs(),
        value,
    };
    let Ok(raw) = serde_json::to_string(&envelope) else {
        return;
    };
    MEMORY.with(|memory| {
        memory
            .borrow_mut()
            .insert(key.to_string(), (raw.clone(), envelope.saved_at));
    });
    if raw.len() > MAX_ENTRY_BYTES {
        // 超长详情只留内存，别把配额占满。
        return;
    }
    let path = storage_key(key);
    if Storage::set_item(&path, raw.clone()).is_err() {
        // 配额满了：先淘汰一半再试一次，仍失败就放弃这次缓存。
        prune(MAX_ENTRIES / 2);
        let _ = Storage::set_item(&path, raw);
    }
    prune(MAX_ENTRIES);
}

pub fn remove(key: &str) {
    MEMORY.with(|memory| {
        memory.borrow_mut().remove(key);
    });
    Storage::remove_item(&storage_key(key));
}

/// 清空本模块写的所有缓存（设置里的「清除缓存」入口用）。
pub fn clear() {
    MEMORY.with(|memory| memory.borrow_mut().clear());
    for key in Storage::get_all_keys() {
        if key.starts_with(PREFIX) {
            Storage::remove_item(&key);
        }
    }
}

/// 缓存条数（调试 / 设置页展示用）。
pub fn len() -> usize {
    Storage::get_all_keys()
        .iter()
        .filter(|key| key.starts_with(PREFIX))
        .count()
}

fn is_expired(saved_at: u64, now: u64) -> bool {
    now.saturating_sub(saved_at) > MAX_AGE_SECS
}

/// 超出上限时按写入时间淘汰最旧的（`Storage` 不保证枚举顺序）。
fn prune(limit: usize) {
    let mut entries: Vec<(String, u64)> = Storage::get_all_keys()
        .into_iter()
        .filter(|key| key.starts_with(PREFIX))
        .filter_map(|key| {
            let raw = Storage::get_item(&key)?;
            let envelope: Envelope<serde_json::Value> = serde_json::from_str(&raw).ok()?;
            Some((key, envelope.saved_at))
        })
        .collect();
    if entries.len() <= limit {
        return;
    }
    entries.sort_by_key(|(_, saved_at)| *saved_at);
    let drop_count = entries.len() - limit;
    for (key, _) in entries.into_iter().take(drop_count) {
        Storage::remove_item(&key);
        if let Some(name) = key.strip_prefix(PREFIX) {
            MEMORY.with(|memory| {
                memory.borrow_mut().remove(name);
            });
        }
    }
}

/// 「3 分钟前」这种人话，用来告诉用户屏幕上是旧数据。
pub fn humanize_age(age_secs: u64) -> String {
    match age_secs {
        0..=5 => "刚刚".to_string(),
        6..=59 => format!("{age_secs} 秒前"),
        60..=3599 => format!("{} 分钟前", age_secs / 60),
        3600..=86_399 => format!("{} 小时前", age_secs / 3600),
        _ => format!("{} 天前", age_secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_separate_sort_order_and_page() {
        // 排序方式不同，列表内容不同 —— 键里必须带，否则会拿到另一套顺序的缓存。
        assert_ne!(
            list_key(7, 1, false),
            list_key(7, 1, true),
            "排序方式必须included在键里"
        );
        assert_ne!(list_key(7, 1, false), list_key(7, 2, false));
        assert_eq!(detail_key("42", 1), "detail:42:1");
        assert_eq!(index_key(), "index");
    }

    #[test]
    fn age_humanizes_into_readable_windows() {
        assert_eq!(humanize_age(0), "刚刚");
        assert_eq!(humanize_age(30), "30 秒前");
        assert_eq!(humanize_age(120), "2 分钟前");
        assert_eq!(humanize_age(7200), "2 小时前");
        assert_eq!(humanize_age(86_400 * 2), "2 天前");
    }

    #[test]
    fn entries_expire_after_a_day() {
        let now = 1_000_000;
        assert!(!is_expired(now - 60, now), "一分钟前的还能用");
        assert!(!is_expired(now - MAX_AGE_SECS, now), "刚好一天仍可用");
        assert!(is_expired(now - MAX_AGE_SECS - 1, now), "超过一天就作废");
    }

    #[test]
    fn envelope_roundtrips_through_json() {
        let envelope = Envelope {
            saved_at: 42,
            value: vec!["a".to_string(), "b".to_string()],
        };
        let raw = serde_json::to_string(&envelope).unwrap();
        let back: Envelope<Vec<String>> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.saved_at, 42);
        assert_eq!(back.value, vec!["a".to_string(), "b".to_string()]);
    }
}
