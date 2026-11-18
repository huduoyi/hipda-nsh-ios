//! HTML 解析。选择器逐条对应 Java 的 `HiParserThreadList` / `HiParserThreadDetail`。
//!
//! **关键**：4D4Y 是 Discuz! 7.2 老模板，正文容器是
//! `div.defaultpost div.postmessage div.t_msgfontfix table td.t_msgfont`，
//! 不是 Discuz X 的 `td.t_f` / `div#postmessage_`（2026-11-13 实测后者 0 命中）。

use std::collections::BTreeSet;

use ego_tree::NodeRef;
use scraper::{ElementRef, Html, Node, Selector};

use crate::config::{query_param, Config, SMILES_PATTERN};
use crate::emoji::{replace_emoji, smiley_from_url};
use crate::model::{
    Blacklist, Block, Content, ContentKind, ForumEntry, ForumIndex, ForumSection, Notification, Poll,
    PollOption, Post, SimpleItem, SimpleList, Thread, ThreadDetail, ThreadList, UserInfo,
};
use crate::{Error, Result};

/// 解析后的文档。包装 `Html` 以便统一入口与后续扩展。
#[derive(Debug, Clone)]
pub struct Document {
    html: Html,
}

impl Document {
    pub fn parse(body: &str) -> Self {
        Self { html: Html::parse_document(body) }
    }

    pub fn root(&self) -> ElementRef<'_> {
        self.html.root_element()
    }

    /// 供测试与 UI 检查：文档标题。
    pub fn title(&self) -> String {
        self.text("title")
    }

    fn text(&self, selector: &str) -> String {
        first(self.root(), selector)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default()
    }
}

/// 把已解码为 UTF-8 的 HTML 解析成文档。
pub fn parse_document(body: &str) -> Document {
    Document::parse(body)
}

fn selector(s: &str) -> Option<Selector> {
    Selector::parse(s).ok()
}

/// 找第一个匹配元素。
fn first<'a>(root: ElementRef<'a>, s: &str) -> Option<ElementRef<'a>> {
    let sel = selector(s)?;
    root.select(&sel).next()
}

/// 找全部匹配元素。
fn all<'a>(root: ElementRef<'a>, s: &str) -> Vec<ElementRef<'a>> {
    match selector(s) {
        Some(sel) => root.select(&sel).collect(),
        None => Vec::new(),
    }
}

fn text_of(root: ElementRef<'_>, s: &str) -> String {
    first(root, s)
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default()
}

fn attr_of(root: ElementRef<'_>, s: &str, name: &str) -> String {
    first(root, s)
        .and_then(|e| e.attr(name))
        .unwrap_or("")
        .to_string()
}

/// 取 `input[name=formhash]`。实测登录页、版块页、详情页都有。
pub fn parse_form_hash(doc: &Document) -> String {
    attr_of(doc.root(), "input[name=formhash]", "value")
}

/// 用 `div#umenu` 是否含 `space.php` 判定页面登录态。
pub fn parse_login_state(doc: &Document) -> bool {
    first(doc.root(), "div#umenu")
        .map(|e| e.inner_html().contains("space.php"))
        .unwrap_or(false)
}

/// 从 `div#umenu cite a` 取用户名与 uid。
pub fn parse_current_user(doc: &Document) -> (String, String) {
    let Some(a) = first(doc.root(), "div#umenu cite a") else {
        return (String::new(), String::new());
    };
    let username = a.text().collect::<String>().trim().to_string();
    let uid = query_param(a.attr("href").unwrap_or(""), "uid");
    (username, uid)
}

/// 取 Discuz 提示页的错误/提示文本。
/// 未登录访问版块时实测返回「对不起，您还未登录，无权访问该版块。」
pub fn parse_error_message(doc: &Document) -> String {
    for s in ["div.alert_error", "div.alert_info"] {
        let t = text_of(doc.root(), s);
        if !t.is_empty() {
            return clean_error_text(&t);
        }
    }
    // `div.alert_win` 只是提示页的外壳，**成功页也套着它**（里面是
    // `div.alert_right`）。把它无条件当错误，会把「指定用户的消息已被屏蔽」
    // 这类成功提示判成失败 —— 2026-11-14 真实登录态复核时踩到。
    if first(doc.root(), "div.alert_right").is_none() {
        let t = text_of(doc.root(), "div.alert_win");
        if !t.is_empty() {
            return clean_error_text(&t);
        }
    }
    String::new()
}

/// Discuz 的「欢迎您回来 / 无权访问」提示页把 `<script>` 里的
/// `setTimeout("window.location.href=...")` 也当文本，直接展示会把整页脚本
/// 塞进界面状态栏。去掉脚本文字、压平空白并限长。
fn clean_error_text(raw: &str) -> String {
    let mut text = raw.to_string();
    for marker in ["setTimeout(", "window.location"] {
        if let Some(i) = text.find(marker) {
            text.truncate(i);
        }
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    trunc(&flat, 120)
}

/// 把元素里的文本节点压成单行，去掉模板里的换行与缩进。
fn flat_text(e: ElementRef<'_>) -> String {
    e.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 解析论坛首页 `index.php` 的分区 / 版块索引。
///
/// 站点是 Discuz! 7.2 老模板：每个分区是 `div.mainbox.list`，标题在
/// `h3 a[href^='index.php?gid=']`，版块行是 `table[id^=category_]`
/// 里的 `tbody[id^=forum]`。**游客与登录态看到的分区不同**（2026-11-13
/// 实测：游客只有「技术版区」8 个版块；登录态还有「4D4Y」「生活版区」
/// 共 3 个分区 14 个版块），所以首页必须动态解析，不能用本地 `FORUMS`
/// 硬编码表代替。
pub fn parse_forum_index(doc: &Document) -> ForumIndex {
    let mut index = ForumIndex {
        logged_in: parse_login_state(doc),
        ..Default::default()
    };
    for block in all(doc.root(), "div.mainbox.list") {
        let Some(heading) = first(block, "h3 a") else { continue };
        let section_name = flat_text(heading);
        let gid = query_param(heading.attr("href").unwrap_or(""), "gid")
            .parse::<i32>()
            .unwrap_or(0);
        let mut section = ForumSection { gid, name: section_name, forums: Vec::new() };
        for row in all(block, "tbody[id^=forum]") {
            let Some(link) = first(row, "h2 a") else { continue };
            let href = link.attr("href").unwrap_or("");
            let fid = query_param(href, "fid").parse::<i32>().unwrap_or(0);
            if fid == 0 {
                continue;
            }
            let numbers = first(row, "td.forumnums")
                .map(flat_text)
                .unwrap_or_default();
            let mut parts = numbers.split('/');
            let last_link = first(row, "td.forumlast p a");
            let cite = first(row, "td.forumlast cite")
                .map(flat_text)
                .unwrap_or_default();
            let (last_author, last_time) = match cite.split_once(" - ") {
                Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
                None => (cite.clone(), String::new()),
            };
            section.forums.push(ForumEntry {
                fid,
                name: flat_text(link),
                description: first(row, "th p").map(flat_text).unwrap_or_default(),
                threads: parts.next().unwrap_or("").trim().to_string(),
                posts: parts.next().unwrap_or("").trim().to_string(),
                today: first(row, "h2 em")
                    .map(flat_text)
                    .unwrap_or_default()
                    .chars()
                    .filter(char::is_ascii_digit)
                    .collect(),
                last_title: last_link.map(flat_text).unwrap_or_default(),
                last_tid: last_link
                    .and_then(|a| a.attr("href"))
                    .map(|u| query_param(u, "tid"))
                    .unwrap_or_default(),
                last_author,
                last_time,
            });
        }
        if !section.forums.is_empty() {
            index.sections.push(section);
        }
    }
    index
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityOption {
    pub value: String,
    pub text: String,
}

/// 解析安全提问下拉框，实测 8 项（0..7）。
pub fn parse_security_questions(doc: &Document) -> Vec<SecurityOption> {
    all(doc.root(), "select#questionid option")
        .iter()
        .enumerate()
        .map(|(i, e)| SecurityOption {
            value: e.attr("value").map(str::to_string).unwrap_or_else(|| i.to_string()),
            text: e.text().collect::<String>().trim().to_string(),
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadType {
    pub name: String,
    pub type_id: String,
}

/// 动态解析版块主题分类。2026-11-13 实测 Discovery 版有 15 个分类，
/// 与源码硬编码的 `BS_TYPES`（9 项）已不一致。
pub fn parse_thread_types(doc: &Document) -> Vec<ThreadType> {
    let mut out: Vec<ThreadType> = Vec::new();
    for a in all(doc.root(), "a[href*='typeid=']") {
        let href = a.attr("href").unwrap_or("");
        let id = query_param(href, "typeid");
        let name = a.text().collect::<String>().trim().trim_matches(['[', ']']).to_string();
        if id.is_empty() || name.is_empty() || out.iter().any(|t| t.type_id == id) {
            continue;
        }
        out.push(ThreadType { name, type_id: id });
    }
    out
}

/// 从分页区取最大页码。
pub fn parse_max_page(doc: &Document) -> i32 {
    let mut max = 1;
    for a in all(doc.root(), "div.pages a, span.threadpages a, a[href*='page=']") {
        let href = a.attr("href").unwrap_or("");
        let n1 = atoi(&query_param(href, "page"));
        let n2 = atoi(&a.text().collect::<String>());
        max = max.max(n1).max(n2);
    }
    max
}

/// 解析主题列表。选择器逐条对应 `HiParserThreadList.java:55-199`。
pub fn parse_thread_list(doc: &Document, cfg: &Config, page: i32) -> ThreadList {
    let mut result = ThreadList {
        page,
        max_page: parse_max_page(doc),
        ..Default::default()
    };
    result.uid = parse_current_user(doc).1;

    for row in all(doc.root(), "tbody[id^='normalthread_'], tbody[id^='stickthread_']") {
        let folder_src = attr_of(row, "td.folder img", "src");
        let icon_src = attr_of(row, "td.icon img", "src");

        let Some(title_link) = first(row, "th.subject span a") else { continue };
        let tid = query_param(title_link.attr("href").unwrap_or(""), "tid");
        if tid.is_empty() {
            continue;
        }

        let style = title_link.attr("style").unwrap_or("");
        let title_color = style
            .find("color:")
            .map(|i| style[i + "color:".len()..].trim().trim_end_matches(';').trim().to_string())
            .unwrap_or_default();

        let author_cell = match first(row, "td.author") {
            Some(v) => v,
            None => continue,
        };
        let Some(author_link) = first(author_cell, "cite a") else { continue };
        let author_id = query_param(author_link.attr("href").unwrap_or(""), "uid");

        let nums = first(row, "td.nums").unwrap_or(row);
        let with_picture = all(row, "img.attach")
            .iter()
            .any(|img| img.attr("src").unwrap_or("").ends_with("image_s.gif"));

        let mut max_page = 1;
        let pages = all(row, "span.threadpages a");
        if let Some(last) = pages.last() {
            let n = atoi(&query_param(last.attr("href").unwrap_or(""), "page"));
            if n > 1 {
                max_page = n;
            }
        }

        result.threads.push(Thread {
            tid,
            title: replace_emoji(title_link.text().collect::<String>().trim()),
            title_color,
            sticky: folder_src.contains("/pin_"),
            poll: icon_src.contains("/poll"),
            thread_type: text_of(row, "th.subject em a"),
            author: author_link.text().collect::<String>().trim().to_string(),
            avatar_url: cfg.avatar_url(&author_id),
            author_id,
            time_create: text_of(author_cell, "em"),
            time_update: text_of(row, "td.lastpost em a"),
            last_post_by: text_of(row, "td.lastpost cite"),
            reply_count: atoi(&text_of(nums, "strong")) as i64,
            view_count: atoi(&text_of(nums, "em")) as i64,
            with_picture,
            is_new: folder_src.contains("new"),
            max_page,
        });
    }
    result
}

/// 解析帖子详情。选择器对应 `HiParserThreadDetail.java:100-374`。
pub fn parse_thread_detail(
    doc: &Document,
    cfg: &Config,
    tid: &str,
    page: i32,
) -> ThreadDetail {
    let mut d = ThreadDetail {
        tid: tid.to_string(),
        page,
        max_page: parse_max_page(doc),
        title: text_of(doc.root(), "div#threadtitle h1"),
        // 投票表单在主题级（`form#poll`），只在第 1 页出现。
        poll: if page <= 1 { parse_poll(doc) } else { None },
        ..Default::default()
    };
    let Some(postlist) = first(doc.root(), "div#wrap div#postlist") else {
        return d;
    };

    for post_el in postlist.child_elements() {
        let id = post_el.attr("id").unwrap_or("");
        let Some(post_id) = id.strip_prefix("post_") else { continue };
        let time_text = text_of(
            post_el,
            "table tbody tr td.postcontent div.postinfo div.posterinfo div.authorinfo em",
        );
        if time_text.is_empty() {
            continue;
        }
        let floor_text = text_of(
            post_el,
            "table tbody tr td.postcontent div.postinfo strong a em",
        );
        if floor_text.is_empty() {
            continue;
        }
        let Some(author_link) = first(
            post_el,
            "table tbody tr td.postauthor div.postinfo a",
        ) else { continue };
        let author_id = query_param(author_link.attr("href").unwrap_or(""), "uid");
        if author_id.is_empty() {
            continue;
        }
        let author = author_link.text().collect::<String>().trim().to_string();

        let mut post = Post {
            post_id: post_id.to_string(),
            floor: atoi(&floor_text),
            author: author.clone(),
            avatar_url: cfg.avatar_url(&author_id),
            author_id,
            time_post: time_text.trim_start_matches("发表于").trim().to_string(),
            warned: attr_of(
                post_el,
                "table tbody tr td.postcontent span.postratings a",
                "href",
            ).contains("viewwarning"),
            signature: text_of(post_el, "td.postbottom > div.signatures"),
            ..Default::default()
        };

        let body_sel = "table tbody tr td.postcontent div.defaultpost div.postmessage div.t_msgfontfix table tbody tr td.t_msgfont";
        if let Some(body) = first(post_el, body_sel) {
            walk(*body, &mut post.contents);
            // 块级正文与扁平 contents 并行产出：界面按块排版，
            // 复制/搜索仍走扁平文本，互不影响。
            post.blocks = parse_blocks(*body);
        } else if let Some(locked) = first(
            post_el,
            "table tbody tr td.postcontent div.defaultpost div.postmessage div.locked",
        ) {
            let text = locked.text().collect::<String>().trim().to_string();
            post.contents.push(Content {
                kind: ContentKind::Text,
                text: text.clone(),
                ..Default::default()
            });
            post.blocks = vec![Block::Paragraph(text)];
        } else if let Some(special) = first(
            post_el,
            "table tbody tr td.postcontent div.defaultpost div.postmessage div.specialmsg table tbody tr td.t_msgfont",
        ) {
            walk(*special, &mut post.contents);
            post.blocks = parse_blocks(*special);
        } else {
            let text = "找不到帖子内容，可能被屏蔽".to_string();
            post.contents.push(Content {
                kind: ContentKind::Text,
                text: text.clone(),
                ..Default::default()
            });
            post.blocks = vec![Block::Paragraph(text)];
        }

        // 图片附件：div.postattachlist dl.attachimg
        for dl in all(
            post_el,
            "table tbody tr td.postcontent div.defaultpost div.postmessage div.t_msgfontfix div.postattachlist dl.attachimg",
        ) {
            let Some(img) = first(dl, "img") else { continue };
            let src = img.attr("src").unwrap_or("");
            let Some(url) = content_image(img.attr("file").unwrap_or(""), src) else {
                continue;
            };
            let size_text = text_of(dl, "em");
            let thumb = if is_site_chrome(src) { String::new() } else { src.to_string() };
            post.contents.push(Content {
                kind: ContentKind::Image,
                url: url.clone(),
                thumb_url: thumb.clone(),
                author: author.clone(),
                text: size_text.clone(),
                file_size: parse_size(&middle_string(&size_text, "(", ")")),
                ..Default::default()
            });
            post.blocks.push(Block::Image {
                url,
                thumb_url: thumb,
            });
        }

        // 其他附件
        for at in all(post_el, "dl.t_attachlist p.attachname") {
            let Some(link) = first(at, "a[href]") else { continue };
            let href = link.attr("href").unwrap_or("");
            if !href.starts_with("attachment.php?") {
                continue;
            }
            let name = link.text().collect::<String>().trim().to_string();
            post.contents.push(Content {
                kind: ContentKind::Attach,
                url: href.to_string(),
                text: name.clone(),
                ..Default::default()
            });
            post.blocks.push(Block::Attach {
                name,
                url: href.to_string(),
                size: 0,
            });
        }

        d.posts.push(post);
    }
    d
}

/// 把正文 DOM 拍平成 `Content` 序列。
/// 对应 Java 版 `parseNode()` 的手写 DFS，用 scraper 的树遍历替代。
fn walk(node: NodeRef<'_, Node>, out: &mut Vec<Content>) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => {
                let txt = t.text.trim_end_matches(['\r', '\n', '\t']);
                if !txt.is_empty() {
                    out.push(Content {
                        kind: ContentKind::Text,
                        text: replace_emoji(txt),
                        ..Default::default()
                    });
                }
            }
            Node::Element(_) => {
                let Some(el) = ElementRef::wrap(child) else { continue };
                let name: &str = el.value().name();
                let class = el.attr("class").unwrap_or("");
                match name {
                    "br" => out.push(Content {
                        kind: ContentKind::Text,
                        text: "\n".into(),
                        ..Default::default()
                    }),
                    "script" | "style" => {}
                    // Discuz 用 `<i>` 画小图标，但也用来写斜体正文：有文字就收下，
                    // 空标签（图标）才跳过。扁平内容流也保留斜体文字。
                    "i" => {
                        let text = el.text().collect::<String>();
                        if !text.trim().is_empty() {
                            out.push(Content {
                                kind: ContentKind::Text,
                                text: clean_bbcode(&text),
                                ..Default::default()
                            });
                        }
                    }
                    "img" => {
                        let src = el.attr("src").unwrap_or("");
                        if src.contains(SMILES_PATTERN) || src.contains("/images/smilies/") {
                            // 按 Java 的完整相对路径还原表情；未知资源才回退成
                            // 短码，避免 coolmonkey/01 与 grapeman/01 都变 `:01:`。
                            let text = smiley_from_url(src)
                                .unwrap_or_else(|| format!(":{}:", base_name_no_ext(src)));
                            out.push(Content {
                                kind: ContentKind::Text,
                                text,
                                ..Default::default()
                            });
                            continue;
                        }
                        let file = el.attr("file").unwrap_or("");
                        // 附件图的真实地址可能在 `file`，也可能只写在
                        // `onclick="zoom(this, '…')"` 里（新上传的图都这样）。
                        if file.trim().is_empty() {
                            if let Some(zoom) = zoom_url(&el) {
                                out.push(Content {
                                    kind: ContentKind::Image,
                                    url: zoom,
                                    thumb_url: src.to_string(),
                                    ..Default::default()
                                });
                                continue;
                            }
                        }
                        // 模板装饰图不是正文内容。
                        let Some(url) = content_image(file, src) else {
                            continue;
                        };
                        out.push(Content {
                            kind: ContentKind::Image,
                            url,
                            thumb_url: if is_site_chrome(src) { String::new() } else { src.to_string() },
                            ..Default::default()
                        });
                    }
                    "a" => {
                        let href = el.attr("href").unwrap_or("");
                        let text = el.text().collect::<String>().trim().to_string();
                        if href.contains("attachment.php?") {
                            // 链接里包着缩略图时按图片算，与块级解析保持一致。
                            let inner_img = first(el, "img").and_then(|img| {
                                content_image(
                                    img.attr("file").unwrap_or(""),
                                    img.attr("src").unwrap_or(""),
                                )
                            });
                            if let Some(url) = inner_img {
                                out.push(Content {
                                    kind: ContentKind::Image,
                                    url,
                                    ..Default::default()
                                });
                            } else {
                                out.push(Content {
                                    kind: ContentKind::Attach,
                                    url: href.to_string(),
                                    text,
                                    ..Default::default()
                                });
                            }
                        } else if href.contains("redirect.php") || href.contains("gotopost.php") {
                            out.push(Content {
                                kind: ContentKind::GotoFloor,
                                text,
                                post_id: query_param(href, "pid"),
                                tid: query_param(href, "ptid"),
                                url: href.to_string(),
                                ..Default::default()
                            });
                        } else if href.starts_with("javascript:") || href == "#" {
                            // 「点击放大」的占位链接：文字没意义，但要继续往下走，
                            // 否则里面包着的图片就丢了。
                            walk(child, out);
                        } else {
                            out.push(Content {
                                kind: ContentKind::Link,
                                url: href.to_string(),
                                text,
                                ..Default::default()
                            });
                        }
                    }
                    "blockquote" | "quote" | "fieldset" => out.push(Content {
                        kind: ContentKind::Quote,
                        text: el.text().collect::<String>().trim().to_string(),
                        ..Default::default()
                    }),
                    _ if class.contains("quote") => out.push(Content {
                        kind: ContentKind::Quote,
                        text: el.text().collect::<String>().trim().to_string(),
                        ..Default::default()
                    }),
                    _ if class.contains("pstatus") => {}
                    _ => walk(child, out),
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------ 块级正文
//
// `walk` 产出的是扁平 `contents`（复制、搜索用）；这里的 `blocks` 保留结构，
// 让界面能画出引用竖条、列表符号与代码块底色。Discuz 7.2 的正文结构固定，
// 通用 HTML→Markdown 库反而会丢掉「引用自谁、哪一楼」这类站点特有信息，
// 所以直接按站点结构转块，不引入额外依赖。

struct BlockBuilder {
    blocks: Vec<Block>,
    buf: String,
}

impl BlockBuilder {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            buf: String::new(),
        }
    }

    fn push_text(&mut self, text: &str) {
        if !text.trim().is_empty() {
            self.buf.push_str(text);
        }
    }

    fn line_break(&mut self) {
        // 每个 `<br>` 都算一个换行：Discuz 用 `<br><br>` 分段，
        // 若在这里去重，段落之间就只剩单换行，整层楼会糊成一段。
        // 连续多个 br 造成的多余空行由 `normalize_block_text` 收敛成一个。
        if !self.buf.is_empty() {
            self.buf.push('\n');
        }
    }

    fn flush(&mut self) {
        let text = normalize_block_text(&self.buf);
        self.buf.clear();
        if text.is_empty() {
            return;
        }
        self.blocks.push(Block::Paragraph(text));
    }

    fn push_block(&mut self, block: Block) {
        self.flush();
        self.blocks.push(block);
    }
}

/// 去掉站点没解析掉的 BBCode 标记，保留文字。
///
/// 4D4Y 是 Discuz 7.2：多数标签会渲染成 HTML，但 `[backcolor]`、`[align]`
/// 这类老标签会被原样输出。直接把 `[backcolor=Yellow]背景色[/backcolor]`
/// 显示给用户，等于让人看源码。
fn clean_bbcode(raw: &str) -> String {
    const TAGS: &[&str] = &[
        "b", "i", "u", "strike", "color", "size", "font", "backcolor", "align", "url", "quote",
        "code", "email", "hide", "free", "flash", "media", "table", "tr", "td", "list", "indent",
    ];
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find(']').map(|offset| start + offset) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &rest[start + 1..end];
        let tag = inner.trim_start_matches('/');
        let name = tag.split(['=', ' ', ']']).next().unwrap_or("").to_ascii_lowercase();
        // 只清已知 BBCode，`[图片]`、`[1]` 这类正文里的方括号原样保留。
        let is_tag = TAGS.contains(&name.as_str())
            && (inner.starts_with('/') || inner.contains('=') || inner == name);
        if is_tag && (inner.starts_with('/') || inner.len() == name.len() || inner.contains('=')) {
            rest = &rest[end + 1..];
        } else {
            out.push_str(&rest[start..=end]);
            rest = &rest[end + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// 段落整理：行尾空白去掉、连续空行压成一个、首尾空行去掉。
fn normalize_block_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank = 0;
    for line in raw.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// 把一层楼的内容转成块序列。
pub fn parse_blocks(node: NodeRef<'_, Node>) -> Vec<Block> {
    let mut builder = BlockBuilder::new();
    for child in node.children() {
        walk_block(child, &mut builder);
    }
    builder.flush();
    builder.blocks
}

fn walk_block(node: NodeRef<'_, Node>, out: &mut BlockBuilder) {
    match node.value() {
        Node::Text(text) => {
            let raw = text.text.trim_end_matches(['\r', '\t']);
            // 站点会把没解析的 BBCode 原样输出（如 [backcolor=Yellow]…），
            // 直接显示就是给用户看源码，这里只去掉标记、保留文字。
            out.push_text(&replace_emoji(&clean_bbcode(raw)));
        }
        Node::Element(_) => {
            let Some(el) = ElementRef::wrap(node) else {
                return;
            };
            let name: &str = el.value().name();
            let class = el.attr("class").unwrap_or("");
            match name {
                "br" => out.line_break(),
                "script" | "style" => {}
                // 同一条 `<i>` 既当图标又当斜体：有文字就收，空标签才跳过。
                "i" => {
                    let text = el.text().collect::<String>();
                    if !text.trim().is_empty() {
                        out.push_text(&clean_bbcode(&text));
                    }
                }
                "hr" => out.push_block(Block::Rule),
                "img" => match image_block(&el) {
                    Some(ImageBlock::Emoji(code)) => out.push_text(&code),
                    Some(ImageBlock::Image(url, thumb)) => {
                        out.push_block(Block::Image {
                            url,
                            thumb_url: thumb,
                        });
                    }
                    None => {}
                },
                "a" => walk_anchor(&el, out),
                "blockquote" | "fieldset" => out.push_block(quote_block(&el)),
                // 代码块：`div.blockcode > div > ol > li`，每行一个 li。
                // 不特殊处理就会被当成「有序列表」，代码成了 1./2./3.。
                _ if class.contains("blockcode") => {
                    let lines: Vec<String> = all(el, "li")
                        .iter()
                        .map(|li| {
                            li.text()
                                .collect::<String>()
                                .replace('\u{a0}', " ")
                                .trim_end()
                                .to_string()
                        })
                        .collect();
                    let code = lines.join("\n");
                    if !code.trim().is_empty() {
                        out.push_block(Block::Code(code));
                    }
                }
                "ul" | "ol" => {
                    // Discuz 的「有序列表」不是 <ol>，而是 `<ul type="1">`
                    // （class=litype_1）。只认 <ol> 会把编号列表渲染成圆点。
                    let ordered = name == "ol"
                        || el.attr("type").is_some_and(|t| t.trim() == "1")
                        || class.contains("litype_1");
                    out.push_block(list_block(&el, ordered));
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let text = collapse_ws(&el.text().collect::<String>());
                    if !text.is_empty() {
                        out.push_block(Block::Heading(text));
                    }
                }
                // 站点没有 h 标签时用 `<font size="5"><strong>标题</strong></font>`。
                //
                // 但 `<font size="5">` 也常被拿来包整段正文（有人喜欢大字号发帖）。
                // 早先只要 size≥4 就当标题并 `el.text()`，结果把里面的 `<img>`
                // 一起吞掉 —— 表现为「附件图片不显示」。所以只有「短且不含
                // 媒体/换行」才算标题，其余照常逐节点递归。
                "font" => {
                    let size = el
                        .attr("size")
                        .and_then(|s| s.trim().parse::<i32>().ok())
                        .unwrap_or(3);
                    let text = collapse_ws(&el.text().collect::<String>());
                    let has_media = first(el, "img, a, br, table, div, blockquote").is_some();
                    let short = text.chars().count() <= 40;
                    if size >= 4 && !text.is_empty() && short && !has_media {
                        out.push_block(Block::Heading(text));
                    } else {
                        for child in el.children() {
                            walk_block(child, out);
                        }
                    }
                }
                // 居中 / 靠右：`<p align="center">`。其它 <p> 当普通段落继续走。
                "p" => {
                    let align = el.attr("align").unwrap_or("").trim().to_ascii_lowercase();
                    let text = collapse_ws(&el.text().collect::<String>());
                    if !text.is_empty() && (align == "center" || align == "right") {
                        out.push_block(Block::Align {
                            center: align == "center",
                            text,
                        });
                    } else {
                        for child in el.children() {
                            walk_block(child, out);
                        }
                    }
                }
                "pre" => {
                    let code = el.text().collect::<String>();
                    let code = code.trim_matches('\n').to_string();
                    if !code.trim().is_empty() {
                        out.push_block(Block::Code(code));
                    }
                }
                "code" => {
                    // 行内 `code`（Discuz 少见）按普通文字处理，整块的才当代码。
                    let text = el.text().collect::<String>();
                    if text.contains('\n') {
                        out.push_block(Block::Code(text.trim().to_string()));
                    } else {
                        out.push_text(&text);
                    }
                }
                "table" => {
                    // 正文表格不常见，按行转成「单元格 · 单元格」的段落，
                    // 比强行排版成表格更稳（列宽在窄窗口里必然错位）。
                    let mut rows: Vec<String> = Vec::new();
                    for tr in all(el, "tr") {
                        let cells: Vec<String> = all(tr, "td, th")
                            .iter()
                            .map(|cell| collapse_ws(&cell.text().collect::<String>()))
                            .filter(|cell| !cell.is_empty())
                            .collect();
                        if !cells.is_empty() {
                            rows.push(cells.join(" · "));
                        }
                    }
                    if !rows.is_empty() {
                        out.push_block(Block::Paragraph(rows.join("\n")));
                    }
                }
                _ if class.contains("signatures") => {}
                // 附件悬停菜单（`div.t_attach`，`display: none`）：里面只有
                // 「下载」链接和体积说明，属于交互浮层而不是正文内容。
                "div" if class.contains("t_attach") => {}
                _ if class.contains("quote") => out.push_block(quote_block(&el)),
                _ => {
                    for child in el.children() {
                        walk_block(child, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// 从 `onclick="zoom(this, 'https://…/a.jpeg')"` 里取原图地址。
///
/// 4D4Y 的**新上传图片**就是这种写法：`<img src="….thumb.jpg"
/// onclick="zoom(this, '….jpeg')">` —— 没有 `file` 属性，原图地址只在 JS 里。
/// 只认 `src` 的话拿到的是缩略图，只认 `file` 就一张都取不到。
fn zoom_url(el: &ElementRef) -> Option<String> {
    let onclick = el.attr("onclick")?;
    let start = onclick.find("zoom(")?;
    let rest = &onclick[start..];
    let quote_pos = rest.find(['\'', '"'])?;
    let quote = rest[quote_pos..].chars().next()?;
    let tail = &rest[quote_pos + quote.len_utf8()..];
    let end = tail.find(quote)?;
    let url = tail[..end].trim();
    // `zoom(this, url)` 的第二个参数才是图片；URL 必须像地址才收。
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("attachments/") {
        Some(url.to_string())
    } else {
        None
    }
}

enum ImageBlock {
    Emoji(String),
    Image(String, String),
}

fn image_block(el: &ElementRef) -> Option<ImageBlock> {
    let src = el.attr("src").unwrap_or("");
    if src.contains(SMILES_PATTERN) || src.contains("/images/smilies/") {
        let code = smiley_from_url(src).unwrap_or_else(|| format!(":{}:", base_name_no_ext(src)));
        return Some(ImageBlock::Emoji(code));
    }
    // 原图地址可能在 `file`（附件）、`zoomfile`（点开大图）或懒加载属性里，
    // 只认 `src` 会把「占位小图 + 真地址在别的属性」的帖子判成没有图。
    let file = ["file", "zoomfile", "data-src", "data-original"]
        .iter()
        .map(|attr| el.attr(attr).unwrap_or(""))
        .find(|value| !value.trim().is_empty())
        .unwrap_or("");
    // 没有属性时再看 `onclick="zoom(...)"`：新上传的附件图只把原图放这里。
    if file.trim().is_empty() {
        if let Some(zoom) = zoom_url(el) {
            return Some(ImageBlock::Image(zoom.clone(), src.to_string()));
        }
    }
    let url = content_image(file, src)?;
    Some(ImageBlock::Image(
        url,
        if is_site_chrome(src) {
            String::new()
        } else {
            src.to_string()
        },
    ))
}

fn walk_anchor(el: &ElementRef, out: &mut BlockBuilder) {
    let href = el.attr("href").unwrap_or("");
    let text = el.text().collect::<String>().trim().to_string();

    // 图片常被 `<a href="javascript:;">` 包起来（点击放大）。先看里面有没有图，
    // 有就按图片处理 —— 早先这里直接走「普通链接」分支，图片被整个跳过，
    // 界面上只剩一行 "javascript:;"。
    if let Some(img) = first(*el, "img") {
        if let Some(ImageBlock::Image(url, thumb)) = image_block(&img) {
            out.push_block(Block::Image {
                url,
                thumb_url: thumb,
            });
            return;
        }
    }
    if href.starts_with("javascript:") || href == "#" {
        // 就是「点开大图」的占位链接，没有可读文字，直接跳过。
        if !text.is_empty() {
            out.push_text(&text);
        }
        return;
    }
    if href.contains("attachment.php?") {
        // 附件链接可能**包着缩略图**：`<a href="attachment.php?..."><img …></a>`。
        // 早先只记一条「附件」文字就 return，里面的图片被整个跳过 ——
        // 网页端看得到图、客户端只有一行文件名，正是「附件图片不显示」。
        if let Some(img) = first(*el, "img") {
            if let Some(ImageBlock::Image(url, thumb)) = image_block(&img) {
                out.push_block(Block::Image {
                    url,
                    thumb_url: thumb,
                });
                return;
            }
        }
        out.push_block(Block::Attach {
            name: text,
            url: href.to_string(),
            size: 0,
        });
        return;
    }
    if href.contains("redirect.php") || href.contains("gotopost.php") {
        // 楼层跳转：保留「第 N 楼」这种文字，链接本身不展示。
        out.push_text(&text);
        return;
    }
    // 裸 URL（文字本身就是地址）单独成块，界面才能做成可点击的一行；
    // 句子里的锚点仍只取文字，避免在正文中间插一块可点区域打断阅读。
    let is_bare_url = text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("www.");
    if is_bare_url || text.is_empty() {
        let url = if text.is_empty() { href.to_string() } else { text.clone() };
        out.push_block(Block::Link { text, url });
        return;
    }
    out.push_text(&text);
}

fn quote_block(el: &ElementRef) -> Block {
    let inner = first(*el, "blockquote").unwrap_or(*el);
    // 4D4Y 是英文模板：`Originally posted by <i>skyraider</i> at 2004-7-26 11:34 AM:`；
    // 中文模板则是 `某某 发表于 时间`。作者优先取 `<i>`（模板把名字包在里面），
    // 退化到第一个链接的文字。
    let author = first(inner, "i")
        .map(|i| i.text().collect::<String>().trim().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            first(inner, "a").map(|a| a.text().collect::<String>().trim().to_string())
        })
        .unwrap_or_default();
    // 引用头一直延续到第一个 <br>：模板把「谁、什么时候」写成一整句，
    // 只取第一个元素会漏掉前面的 "Originally posted by" 而取不到时间。
    // 中文模板把头部写在 `<p><font>…发表于…</font></p>` 里，英文模板直接
    // 铺一段文本到第一个 `<br>`。两条路都试，取到哪个用哪个。
    let header = match first(inner, "p") {
        Some(p) => collapse_ws(&p.text().collect::<String>()),
        None => collapse_ws(
            &inner
                .children()
                .take_while(|child| {
                    !matches!(child.value(), Node::Element(el) if el.name() == "br")
                })
                .map(|child| match child.value() {
                    Node::Text(text_node) => text_node.text.to_string(),
                    _ => ElementRef::wrap(child)
                        .map(|el| el.text().collect::<String>())
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
    };
    let time = header
        .split_once(" at ")
        .map(|(_, rest)| rest.trim().trim_end_matches(':').trim().to_string())
        .or_else(|| {
            header
                .split_once("发表于")
                .map(|(_, rest)| rest.trim().to_string())
        })
        .unwrap_or_default();

    let mut builder = BlockBuilder::new();
    for child in inner.children() {
        // 头部那句「谁 于何时」已经被抽成引用标题，不再重复进正文。
        if let Some(child_el) = ElementRef::wrap(child) {
            let text = child_el.text().collect::<String>();
            let is_header = (text.contains("Originally posted by")
                || text.contains("发表于"))
                && text.chars().count() < 120;
            if is_header {
                continue;
            }
        }
        walk_block(child, &mut builder);
    }
    builder.flush();
    Block::Quote {
        author,
        time,
        blocks: builder.blocks,
    }
}

fn list_block(el: &ElementRef, ordered: bool) -> Block {
    let items: Vec<String> = all(*el, "li")
        .iter()
        .map(|li| collapse_ws(&li.text().collect::<String>()))
        .filter(|item| !item.is_empty())
        .collect();
    Block::List { ordered, items }
}

fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 站点模板自带的装饰图：1×1 占位符（`images/common/none.gif`）、
/// 附件图标（`images/attachicons/*.gif`）、等级星、按钮、模板 logo。
///
/// 它们出现在正文 DOM 里但**不是内容**。2026-11-13 实机排查：详情每层只取
/// 前 3 张图，这些占位符排在真实附件前面，把真图挤出显示额度，而 1×1 的
/// GIF 拉伸后就是一块空白 —— 表现为「帖子里的图片不显示」。
fn is_site_chrome(src: &str) -> bool {
    const CHROME: &[&str] = &[
        "/images/default/",
        "/images/common/",
        "/images/attachicons/",
        "/images/smilies/",
        "/templates/",
        "images/default/",
        "images/common/",
        "images/attachicons/",
        "templates/",
    ];
    let lower = src.trim().to_ascii_lowercase();
    CHROME.iter().any(|p| lower.contains(p))
}

/// 从 `<img src file>` 里取正文图的真实地址：`file` 优先（Discuz 附件的
/// 真实 URL），否则用 `src`，但模板装饰图一律丢弃。
fn content_image(file: &str, src: &str) -> Option<String> {
    let file = file.trim();
    if !file.is_empty() {
        return Some(file.to_string());
    }
    let src = src.trim();
    if src.is_empty() || is_site_chrome(src) {
        return None;
    }
    Some(src.to_string())
}

/// 从 `#myprompt_menu` 里的 `prompt_*` 文本取「(N)」。
pub fn parse_notification(doc: &Document) -> Notification {
    let count = |selector: &str| {
        let txt = text_of(doc.root(), selector);
        atoi(&middle_string(&txt, "(", ")"))
    };
    let (username, uid) = parse_current_user(doc);
    Notification {
        sms_count: count("li#prompt_pm"),
        thread_count: count("li#prompt_threads"),
        username,
        uid,
        ..Default::default()
    }
}

/// 解析上传响应 `DISCUZUPLOAD|0|<attachID>|<status>`。
pub fn parse_upload_response(body: &str) -> Result<String> {
    let body = body.trim();
    let parts: Vec<&str> = body.split('|').collect();
    if parts.len() < 3
        || parts[0] != "DISCUZUPLOAD"
        || parts[1] != "0"
        || parts[2] == "0"
        || parts[2].is_empty()
    {
        return Err(Error::Protocol(format!("上传失败或图片 ID 无效：{}", trunc(body, 200))));
    }
    Ok(parts[2].to_string())
}

fn middle_string(s: &str, from: &str, to: &str) -> String {
    let Some(i) = s.find(from) else { return String::new() };
    let rest = &s[i + from.len()..];
    if to.is_empty() {
        return rest.to_string();
    }
    rest.find(to).map(|j| rest[..j].to_string()).unwrap_or_else(|| rest.to_string())
}

fn atoi(s: &str) -> i32 {
    s.trim().replace(',', "").parse::<i32>().unwrap_or(0)
}

fn base_name_no_ext(p: &str) -> String {
    let name = p.rsplit('/').next().unwrap_or(p);
    name.rsplit_once('.').map(|(n, _)| n).unwrap_or(name).to_string()
}

fn parse_size(s: &str) -> i64 {
    let s = s.trim().to_ascii_uppercase();
    let (mult, num) = if s.ends_with("MB") {
        (1i64 << 20, &s[..s.len() - 2])
    } else if s.ends_with('M') {
        (1i64 << 20, &s[..s.len() - 1])
    } else if s.ends_with("KB") {
        (1i64 << 10, &s[..s.len() - 2])
    } else if s.ends_with('K') {
        (1i64 << 10, &s[..s.len() - 1])
    } else {
        (1, s.as_str())
    };
    num.trim().parse::<f64>().map(|v| (v * mult as f64) as i64).unwrap_or(0)
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    format!("{}...", s.chars().take(n).collect::<String>())
}

/// 通用列表页的 DOM 结构。
///
/// 原客户端 `SimpleListJob` 定义了 11 种 type（我的回复 / 我的帖子 / 搜索 /
/// 私信 / 通知 / 收藏 / 关注 / 最新帖子 …），但 `HiParser` 里实际只有三套
/// 选择器，按结构归并比按 type 分支更抗站点改版。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SimpleListKind {
    /// 搜索、我的帖子、收藏、关注、最新帖子：主题表格 `tbody tr th.subject a`。
    /// 我的回复用 `redirect.php?goto=` 链接，同一个函数兼容。
    #[default]
    Threads,
    /// 通知：`ul.feed li`（`HiParser.parseNotify`）。
    Notify,
    /// 私信：`li.s_clear`（`HiParser.parseSMS` / `parseSmsDetail`）。
    Sms,
}

/// 解析通用列表页，对应 `HiParser.parseSearch` / `parseFavorites` /
/// `parseNotify` / `parseSMS`。
///
/// 与 `parse_thread_list` 的区别：这些页面的 `tbody` **没有**
/// `normalthread_` id，所以不能用主题列表的选择器；且搜索翻页依赖
/// `searchid`，必须一并带回。
pub fn parse_simple_list(doc: &Document, cfg: &Config, kind: SimpleListKind) -> SimpleList {
    let mut list = SimpleList {
        max_page: parse_max_page(doc),
        form_hash: parse_form_hash(doc),
        ..Default::default()
    };
    for a in all(doc.root(), "div.pages a[href*='searchid=']") {
        let id = query_param(a.attr("href").unwrap_or(""), "searchid");
        if !id.is_empty() {
            list.search_id = id;
            break;
        }
    }

    let mut items = match kind {
        SimpleListKind::Threads => parse_thread_like_rows(doc, cfg),
        SimpleListKind::Notify => parse_feed_rows(doc, cfg),
        SimpleListKind::Sms => parse_sms_rows(doc, cfg),
    };
    // 表头 / 空行不该变成界面上的空白卡片。
    items.retain(|item| !(item.title.is_empty() && item.info.is_empty()));
    list.items = items;
    list
}

/// 从 `tid=` 或 `ptid=` 里取主题号（回复列表走 `redirect.php?ptid=`）。
fn tid_of(href: &str) -> String {
    let tid = query_param(href, "tid");
    if tid.is_empty() {
        query_param(href, "ptid")
    } else {
        tid
    }
}

fn parse_thread_like_rows(doc: &Document, cfg: &Config) -> Vec<SimpleItem> {
    let mut out = Vec::new();
    for row in all(doc.root(), "tbody") {
        let Some(link) = first(row, "tr th.subject a")
            .or_else(|| first(row, "th a[href*='redirect.php']"))
        else {
            continue;
        };
        let href = link.attr("href").unwrap_or("");
        let tid = tid_of(href);
        if tid.is_empty() {
            continue;
        }
        let (author, author_id) = first(row, "td.author cite a")
            .map(|a| {
                (
                    replace_emoji(a.text().collect::<String>().trim()),
                    query_param(a.attr("href").unwrap_or(""), "uid"),
                )
            })
            .unwrap_or_default();
        out.push(SimpleItem {
            tid,
            pid: query_param(href, "pid"),
            title: replace_emoji(link.text().collect::<String>().trim()),
            info: text_of(row, "td.lastpost"),
            forum: text_of(row, "td.forum"),
            time: text_of(row, "td.author em"),
            uid: author_id.clone(),
            author,
            avatar_url: cfg.avatar_url(&author_id),
            is_new: attr_of(row, "td.folder img", "src").contains("new"),
            has_form_hash: false,
        });
    }
    out
}

fn parse_feed_rows(doc: &Document, cfg: &Config) -> Vec<SimpleItem> {
    let mut out = Vec::new();
    for li in all(doc.root(), "ul.feed li") {
        let info = li.text().collect::<Vec<_>>().join(" ").trim().to_string();
        if info.is_empty() {
            continue;
        }
        let (tid, title) = first(li, "a[href*='tid='], a[href*='ptid='], a[href*='redirect.php']")
            .map(|a| {
                (
                    tid_of(a.attr("href").unwrap_or("")),
                    replace_emoji(a.text().collect::<String>().trim()),
                )
            })
            .unwrap_or_default();
        let (author, uid) = first(li, "a[href*='space.php']")
            .map(|a| {
                (
                    a.text().collect::<String>().trim().to_string(),
                    query_param(a.attr("href").unwrap_or(""), "uid"),
                )
            })
            .unwrap_or_default();
        out.push(SimpleItem {
            tid,
            title: if title.is_empty() { trunc(&info, 40) } else { title },
            info,
            uid: uid.clone(),
            author,
            avatar_url: cfg.avatar_url(&uid),
            is_new: attr_of(li, "img", "src").contains("newpm"),
            ..Default::default()
        });
    }
    out
}

fn parse_sms_rows(doc: &Document, cfg: &Config) -> Vec<SimpleItem> {
    let mut out = Vec::new();
    for li in all(doc.root(), "li.s_clear, ul.pm_list li") {
        let author = text_of(li, "p.cite cite");
        if author.is_empty() {
            continue;
        }
        let uid = query_param(&attr_of(li, "a[href*='uid=']", "href"), "uid");
        let summary = text_of(li, "div.summary");
        // `p.cite` 的 ownText 是时间，减去 cite（作者名）即可得到。
        let cite_text = text_of(li, "p.cite");
        let time = cite_text.strip_prefix(&author).unwrap_or(&cite_text).trim().to_string();
        out.push(SimpleItem {
            title: trunc(&summary, 60),
            info: summary,
            author,
            uid: uid.clone(),
            avatar_url: cfg.avatar_url(&uid),
            time,
            is_new: attr_of(li, "p.cite img", "src").contains("newpm"),
            ..Default::default()
        });
    }
    out
}

// ------------------------------------------------- 投票 / 资料 / 黑名单 / 外链图片

/// 解析主题首楼的投票表单 `form#poll`（`HiParserThreadDetail:204-252`）。
/// Discuz 把「选项行」与「票数行」放在同一张表的相邻 `tr`：带
/// `td.polloption label` 的是选项，带 `td em` 的是上一选项的得票。
pub fn parse_poll(doc: &Document) -> Option<Poll> {
    let form = first(doc.root(), "form#poll")?;
    let info = first(form, "div.pollinfo")?;
    let mut title = flat_text(info);
    if let Some(timer) = first(form, "p.polltimer") {
        let t = flat_text(timer);
        if !t.is_empty() {
            title.push(' ');
            title.push_str(&t);
        }
    }
    let info_text = flat_text(info);
    let max_answer = if info_text.trim().starts_with("多选投票") {
        atoi(&middle_string(&info_text, "最多可选", "项")).max(1)
    } else {
        1
    };
    let mut options: Vec<PollOption> = Vec::new();
    let mut footer = String::new();
    for tr in all(form, "div.pollchart > table > tbody > tr") {
        if let Some(label) = first(tr, "td.polloption label") {
            options.push(PollOption {
                id: attr_of(tr, "td.selector input", "value"),
                text: flat_text(label),
                rates: String::new(),
            });
        } else if let Some(last) = options.last_mut() {
            if let Some(td) = all(tr, "td").last() {
                if first(*td, "em").is_some() {
                    last.rates = flat_text(*td);
                }
            }
        }
        // 最后一行的最后一个 td 是投票说明 / 截止信息（去掉按钮文字）。
        if let Some(td) = all(tr, "td").last() {
            let mut t = flat_text(*td);
            if let Some(btn) = first(*td, "button, input[type=submit]") {
                t = t.replace(&flat_text(btn), "");
            }
            footer = t.trim().to_string();
        }
    }
    Some(Poll {
        title,
        footer,
        max_answer,
        form_hash: attr_of(form, "input[name=formhash]", "value"),
        options,
    })
}

/// 用户资料页 `space.php?uid=`（`HiParser.parseUserInfo`）。
pub fn parse_user_info(doc: &Document, cfg: &Config) -> UserInfo {
    let username = text_of(doc.root(), "div#profilecontent div.itemtitle h1");
    let online = attr_of(doc.root(), "div#profilecontent div.itemtitle img", "src")
        .contains("online");
    let uid_text = text_of(doc.root(), "div#profilecontent div.itemtitle ul li");
    let uid = middle_string(&uid_text, "(UID:", ")").trim().to_string();
    let mut avatar_url = attr_of(doc.root(), "div.side div.profile_side div.avatar img", "src");
    if !avatar_url.is_empty() && !avatar_url.starts_with("http") {
        avatar_url = format!("{}{}", cfg.base_url(), avatar_url.trim_start_matches('/'));
    }
    // formhash 可能藏在 `#umenu` 的退出登录链接里，而不在 input 中。
    let mut form_hash = parse_form_hash(doc);
    if form_hash.is_empty() {
        for a in all(doc.root(), "div#umenu a[href*='formhash=']") {
            let h = query_param(a.attr("href").unwrap_or(""), "formhash");
            if !h.is_empty() {
                form_hash = h;
                break;
            }
        }
    }
    let mut detail = String::new();
    for (i, title) in all(doc.root(), "h3.blocktitle").iter().take(2).enumerate() {
        detail.push_str(&flat_text(*title));
        detail.push('\n');
        if i == 0 {
            for li in all(doc.root(), "div.main div.s_clear ul.commonlist li") {
                detail.push_str(&flat_text(li));
                detail.push('\n');
            }
        }
        detail.push('\n');
    }
    UserInfo {
        username,
        uid,
        avatar_url,
        online,
        detail: detail.trim().to_string(),
        form_hash,
    }
}

/// 黑名单页 `pm.php?action=viewblack`（`HiParser.parseBlacklist`）。
/// 用户名在 `space.php?username=` 的查询参数里，少数模板只有链接文本。
pub fn parse_blacklist(doc: &Document) -> Blacklist {
    let mut users: Vec<String> = Vec::new();
    for a in all(doc.root(), "ul.commonlist a[href*='space.php']") {
        let href = a.attr("href").unwrap_or("");
        let name = query_param(href, "username");
        let name = if name.is_empty() { flat_text(a) } else { name };
        if !name.is_empty() && !users.contains(&name) {
            users.push(name);
        }
    }
    let body = text_of(doc.root(), "div.blacklist");
    Blacklist {
        users,
        form_hash: parse_form_hash(doc),
        empty_hint: body.contains("暂无数据"),
    }
}

/// 私信 / 投票这类 AJAX 响应：成功页含 `class="summary"`，失败是
/// `<root>错误文本</root>` 的 XML 或带 `div.alert_error` 的提示页。
/// discuz 的 fastpost 响应把提示和一段 JS 写在同一个文本节点里：
/// `…请返回修改。 if(typeof messagehandle_fastpost =='function') {…}`
/// 直接把整段当文案显示，用户会看到代码，所以只留提示本身。
fn strip_inline_script(text: &str) -> String {
    let cut = text
        .find("if(typeof ")
        .or_else(|| text.find("<script"))
        .unwrap_or(text.len());
    text[..cut].trim().to_string()
}

pub fn parse_sms_result(body: &str) -> (bool, String) {
    if body.contains("class=\"summary\"") {
        return (true, "操作成功".to_string());
    }
    let doc = parse_document(body);
    // Discuz! 7.2 的提示页约定：`div.alert_right` 是成功，`div.alert_error`
    // 是失败。黑名单的「指定用户的消息已被屏蔽 / 屏蔽设置已移除」都走前者，
    // 旧实现只认「成功 / 感谢」两个词，把真实成功判成了失败。
    if body.contains("alert_right") {
        let text = first(doc.root(), "div.alert_right p")
            .map(flat_text)
            .unwrap_or_default();
        let text = clean_error_text(&text);
        return (
            true,
            if text.is_empty() { "操作成功".to_string() } else { text },
        );
    }
    let error = parse_error_message(&doc);
    if !error.is_empty() {
        return (false, error);
    }
    // ajax 响应里的 `<![CDATA[…]]>` 会被 HTML 解析器当成注释吞掉，DOM 里
    // 读不到文字，只能回原文取。
    // 两种响应形态都要照顾：
    // - fastpost 的拒绝：提示文字在前，后面拼一段 `if(typeof …)`，取前段；
    // - ajax 的 CDATA：DOM 读不到文本，只能回原文用引号里的那句子。
    let raw = flat_text(doc.root());
    let mut text = strip_inline_script(raw.split('<').next().unwrap_or("").trim());
    if text.is_empty() {
        text = cdata_message(body).unwrap_or_default();
    }
    let text = strip_inline_script(text.trim());

    // 失败词优先：「操作失败，请稍后重试」里也有「成功」两个字的邻居。
    const NEGATIVE: [&str; 8] = ["不能", "抱歉", "无权", "失败", "错误", "非法", "禁止", "不存在"];
    if NEGATIVE.iter().any(|w| text.contains(w)) {
        return (false, trunc(&text, 120));
    }
    const POSITIVE: [&str; 6] = ["成功", "感谢", "已被", "已移除", "已提交", "已发送"];
    let ok = POSITIVE.iter().any(|w| text.contains(w));
    let message = if ok {
        text.to_string()
    } else if text.is_empty() {
        "操作失败，站点未返回说明".to_string()
    } else {
        trunc(&text, 120)
    };
    (ok, message)
}

/// 从 `messagehandle_xxx('0', '消息');` 里取出消息正文。
fn cdata_message(text: &str) -> Option<String> {
    let start = text.find("', '")? + 4;
    let rest = &text[start..];
    let end = rest.rfind("');").or_else(|| rest.rfind(')'))?;
    let message = rest[..end].trim().trim_end_matches('\'').trim();
    if message.is_empty() {
        None
    } else {
        Some(message.to_string())
    }
}

/// 从页面所有 `<img src>` 里提取**绝对外链**主机名，排除已在白名单中的域名。
/// 相对路径由 `ForumClient::forum_url()` 拼到主站，不需要额外授权。
/// 用于维护 `nsh.json` 的 `capabilities.network.hosts`。
pub fn external_image_hosts(doc: &Document, allowed: &[&str]) -> Vec<String> {
    let urls: Vec<&str> = all(doc.root(), "img[src]")
        .iter()
        .map(|img| img.attr("src").unwrap_or(""))
        .collect();
    unlisted_image_hosts(urls, allowed)
}

/// 从一批图片 URL 里找出**未授权**域名，去重并按字典序返回。
/// 相对地址（`attachments/a.jpg`）与论坛表情图没有主机名，由 `forum_url`
/// 拼到主站，视为已授权；`*.4d4y.com` 形式的规则匹配任意子域。
pub fn unlisted_image_hosts<'a>(
    urls: impl IntoIterator<Item = &'a str>,
    allowed: &[&str],
) -> Vec<String> {
    let normalized = normalized_hosts(allowed);
    let mut hosts = BTreeSet::new();
    for url in urls {
        if let Some(host) = image_host(url) {
            if !host_matches(&host, &normalized) {
                hosts.insert(host);
            }
        }
    }
    hosts.into_iter().collect()
}

/// 单个图片 URL 是否在白名单内。UI 用它决定是否发起图片加载。
pub fn image_host_allowed(url: &str, allowed: &[&str]) -> bool {
    match image_host(url) {
        None => true,
        Some(host) => host_matches(&host, &normalized_hosts(allowed)),
    }
}

/// 取绝对 URL 的主机名（小写、去端口）；相对地址返回 `None`。
pub fn image_host(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("//"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

fn normalized_hosts(allowed: &[&str]) -> Vec<String> {
    allowed
        .iter()
        .map(|a| a.trim().trim_start_matches("*.").to_ascii_lowercase())
        .filter(|a| !a.is_empty())
        .collect()
}

fn host_matches(host: &str, normalized: &[String]) -> bool {
    normalized
        .iter()
        .any(|a| host == a || host.ends_with(&format!(".{a}")))
}

#[cfg(test)]
mod chrome_tests {
    use super::*;

    /// 站点装饰图（1×1 占位符、附件图标、等级星、模板 logo）不是正文图片。
    #[test]
    fn site_chrome_is_not_content_image() {
        for chrome in [
            "https://img02.4d4y.com/forum/images/default/attachimg.gif",
            "https://img02.4d4y.com/forum/images/common/none.gif",
            "https://img02.4d4y.com/forum/images/attachicons/binary.gif",
            "templates/default/images/logo.gif",
            "images/default/star_level3.gif",
        ] {
            assert!(is_site_chrome(chrome), "{chrome} 应判为装饰图");
            assert_eq!(content_image("", chrome), None, "{chrome} 不应成为正文图");
        }
        let real = "https://img02.4d4y.com/forum/attachments/day_081128/a.jpg";
        assert!(!is_site_chrome(real));
        assert_eq!(content_image("", real).as_deref(), Some(real));
    }

    /// 附件的 `src` 是 none.gif 占位符，真实地址在 `file`，必须优先用 `file`。
    #[test]
    fn attachment_uses_file_attribute_over_placeholder() {
        let file = "https://img02.4d4y.com/forum/attachments/day_081129/07.jpg";
        let src = "https://img02.4d4y.com/forum/images/common/none.gif";
        assert_eq!(content_image(file, src).as_deref(), Some(file));
        // 没有 file 属性时，占位符不能被当成图片。
        assert_eq!(content_image("", src), None);
    }

    /// 真实登录态（2026-11-14）抓到的黑名单响应：成功走 `alert_right`，
    /// 外壳统一是 `alert_win`。旧实现把外壳当错误，成功被判成失败。
    #[test]
    fn sms_result_reads_alert_right_as_success() {
        let ok_body = r#"<div class="fcontent alert_win"><h3><em>4D4Y 提示信息</em></h3>
            <div class="postbox"><div class="alert_right"><p>指定用户的消息已被屏蔽<script>setTimeout("window.location.href ='pm.php?action=viewblack';", 1000);</script></p>
            <p class="alert_btnleft"><a href="pm.php?action=viewblack">如果您的浏览器没有自动跳转，请点击此链接</a></p></div></div></div>"#;
        let (ok, message) = parse_sms_result(ok_body);
        assert!(ok, "alert_right 应判成功：{message}");
        assert_eq!(message, "指定用户的消息已被屏蔽");

        let err_body = r#"<div class="fcontent alert_win"><div class="alert_error"><p>抱歉，您没有权限进行此操作</p></div></div>"#;
        let (ok, message) = parse_sms_result(err_body);
        assert!(!ok, "alert_error 应判失败");
        assert!(message.contains("没有权限"), "{message}");
    }

    /// 真实抓到的私信失败响应：Discuz 用 JS 回调包装消息，不能原样展示。
    #[test]
    fn sms_result_extracts_message_from_ajax_js_wrapper() {
        let body = r#"<root><![CDATA[if(typeof messagehandle_pmreply =='function') {messagehandle_pmreply('0', '您不能给自己发消息，请修改。');} ]]></root>"#;
        let (ok, message) = parse_sms_result(body);
        assert!(!ok, "站点拒绝时应判失败");
        assert_eq!(message, "您不能给自己发消息，请修改。");
    }

    /// 引用块：原客户端会把「引用自谁、什么时候」单独显示一行，
    /// 扁平文本做不到这一点 —— 这里锁住作者、时间与嵌套正文。
    #[test]
    fn blocks_keep_quote_author_and_time() {
        let html = r#"<div class="postmessage">
            <div class="quote"><blockquote>
                <p><font size="2"><a href="space.php?uid=9">老张</a> 发表于 2026-11-1 10:20</font></p>
                被引用的内容<br>第二行
            </blockquote></div>
            我的回复
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);
        let quote = blocks
            .iter()
            .find_map(|b| match b {
                Block::Quote { author, time, blocks } => Some((author, time, blocks)),
                _ => None,
            })
            .expect("应有引用块");
        assert_eq!(quote.0, "老张");
        assert_eq!(quote.1, "2026-11-1 10:20");
        assert_eq!(*quote.2, vec![Block::Paragraph("被引用的内容\n第二行".into())]);
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(t) if t.contains("我的回复"))),
            "引用之外的正文要保留"
        );
    }

    /// 有序 / 无序列表要保留「这是一份列表」，不能糊成一行。
    #[test]
    fn blocks_keep_list_items_in_order() {
        let html = r#"<div class="postmessage">
            <ol><li>第一项</li><li>第二项</li></ol>
            <ul><li>苹果</li><li>香蕉</li></ul>
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);
        assert_eq!(
            blocks,
            vec![
                Block::List {
                    ordered: true,
                    items: vec!["第一项".into(), "第二项".into()]
                },
                Block::List {
                    ordered: false,
                    items: vec!["苹果".into(), "香蕉".into()]
                },
            ]
        );
    }

    /// 代码块保留换行；`<br>` 分段；站点装饰图不进正文。
    #[test]
    fn blocks_keep_code_and_drop_site_chrome() {
        let html = r#"<div class="postmessage">
            <pre>fn main() {
    println!("hi");
}</pre>
            第一段<br><br><br>第二段
            <img src="images/common/none.gif">
            <img src="attachments/photo.jpg" file="https://img02.4d4y.com/attachments/photo.jpg">
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Code(c) if c.contains("println!"))),
            "代码块丢了：{blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(t) if t == "第一段\n\n第二段")),
            "连续 <br> 应折成一个空行：{blocks:?}"
        );
        assert_eq!(
            blocks
                .iter()
                .filter(|b| matches!(b, Block::Image { .. }))
                .count(),
            1,
            "只保留真实附件图"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Image { url, .. } if url.ends_with("photo.jpg"))),
            "附件图应从 file 属性取地址"
        );
    }

    /// 4D4Y 用的是英文模板：`Originally posted by <i>谁</i> at 时间:`。
    /// 作者被包在 `<i>` 里，早先只找 `<a>` 会得到空的引用标题。
    #[test]
    fn blocks_read_english_quote_header() {
        let html = r#"<div class="postmessage">
            <div class="quote"><blockquote>Originally posted by <i>skyraider</i> at 2004-7-26 11:34 AM:<br>
            不敢相信，这个是九十年代初的PDA吗？ </blockquote></div>
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);
        match &blocks[0] {
            Block::Quote { author, time, blocks } => {
                assert_eq!(author, "skyraider");
                assert_eq!(time, "2004-7-26 11:34 AM");
                assert!(
                    matches!(&blocks[0], Block::Paragraph(t) if t.contains("九十年代初")),
                    "{blocks:?}"
                );
            }
            other => panic!("应是引用块：{other:?}"),
        }
    }

    /// 真实测试帖（tid 3476475「阅读器格式样式样本」）暴露的四个坑：
    /// 代码块是 `div.blockcode > ol > li`、有序列表是 `<ul type="1">`、
    /// 斜体正文写在 `<i>` 里、以及站点没解析的 `[backcolor]` BBCode。
    #[test]
    fn blocks_handle_discuz_editor_markup() {
        let html = r#"<div class="postmessage">
            <font size="5"><strong>一级标题</strong></font><br>
            正常<strong>粗体</strong>、<i>斜体</i>、[backcolor=Yellow]背景色[/backcolor]。<br>
            <p align="center">居中的一行</p>
            <ul type="1" class="litype_1"><li>第一项<br><li>第二项<br></ul>
            <div class="blockcode"><div id="code0"><ol><li>fn main() {<br><li>&nbsp; &nbsp; println!();<br><li>}</ol></div><em>复制代码</em></div>
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);

        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Heading(t) if t == "一级标题")),
            "font size=5 应识别为标题：{blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(t) if t.contains("斜体"))),
            "斜体文字不能丢：{blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(t) if t.contains("背景色") && !t.contains("backcolor"))),
            "BBCode 标记要清掉、文字要留：{blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Align { center: true, text } if text == "居中的一行")),
            "p[align=center] 应保留对齐：{blocks:?}"
        );
        assert!(
            blocks.iter().any(|b| matches!(
                b,
                Block::List { ordered: true, items } if items.len() == 2 && items[0] == "第一项"
            )),
            "ul[type=1] 是有序列表：{blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Code(c) if c.starts_with("fn main() {") && c.contains("println!();"))),
            "blockcode 要当代码块，不能变成编号列表：{blocks:?}"
        );
    }

    /// 真实帖子里有人用 `<font size="5">` 包住整段正文，甚至包住图片。
    /// 早先只要字号大就当标题、并 `el.text()` 取文字，图片会被整段吞掉。
    #[test]
    fn blocks_keep_images_inside_large_font() {
        let html = r#"<div class="postmessage">
            <font size="5">一段用大字号发的正文<br>
            <img src="attachments/pic.jpg" file="https://img02.4d4y.com/attachments/pic.jpg">
            </font>
            <font size="5"><strong>短标题</strong></font>
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);

        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Image { url, .. } if url.ends_with("pic.jpg"))),
            "大字号正文里的图片不能丢：{blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Heading(t) if t == "短标题")),
            "短的大字号文字仍应识别为标题：{blocks:?}"
        );
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, Block::Heading(t) if t.contains("一段用大字号发的正文"))),
            "长正文不该被当成标题：{blocks:?}"
        );
    }

    /// 附件常写成 `<a href="attachment.php?…"><img …></a>`：网页端显示缩略图，
    /// 客户端早先只留一行「附件」文字，图片被 return 掉。
    #[test]
    fn blocks_keep_image_inside_attachment_link() {
        let html = r#"<div class="postmessage">
            看这张图：
            <a href="attachment.php?aid=xyz&amp;nothumb=yes" target="_blank">
                <img src="attachments/day_260601/thumb.jpg" file="https://img02.4d4y.com/attachments/day_260601/pic.jpg">
            </a>
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Image { url, .. } if url.ends_with("pic.jpg"))),
            "附件链接里的图片要保留：{blocks:?}"
        );
    }

    /// 真实帖子（tid 3476991）：新上传的附件图没有 `file` 属性，
    /// 原图地址只写在 `onclick="zoom(this, '…')"` 里，外面还套着
    /// `<a href="javascript:;">`。早先这条路径只输出一行 "javascript:;"。
    #[test]
    fn blocks_read_zoom_images_behind_javascript_anchor() {
        let html = r#"<div class="postmessage">
            一张比扑克牌还小的纸 25 块<br>
            <a href="javascript:;"><img onclick="zoom(this, 'https://img02.4d4y.com/forum/attachments/day_261117/a.jpeg')"
                src="https://img02.4d4y.com/forum/attachments/day_261117/a.jpeg.thumb.jpg"></a>
            <div class="t_attach" id="aimg_1_menu" style="position: absolute; display: none">
                <a href="attachment.php?aid=x" target="_blank"><strong>下载</strong></a> (711.82 KB)
            </div>
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);

        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Image { url, .. } if url.ends_with("a.jpeg"))),
            "zoom 里的原图要取到：{blocks:?}"
        );
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, Block::Attach { name, .. } if name == "下载")),
            "隐藏的附件菜单不该进正文：{blocks:?}"
        );
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(t) if t.contains("javascript:"))),
            "javascript: 占位链接不该显示成文字：{blocks:?}"
        );
    }

    /// fastpost 的拒绝响应带一段内联 JS，不能原样展示给用户。
    #[test]
    fn sms_result_strips_inline_script_from_fastpost() {
        let body = "对不起，您的帖子小于 5 个字符的限制，请返回修改。 if(typeof messagehandle_fastpost =='function') {messagehandle_fastpost('0', '对不起…');}";
        let (ok, message) = parse_sms_result(body);
        assert!(!ok);
        assert_eq!(message, "对不起，您的帖子小于 5 个字符的限制，请返回修改。");
    }

    /// 正文里的裸 URL 要单独成块，界面才能点击打开。
    #[test]
    fn blocks_keep_bare_urls_as_links() {
        let html = r#"<div class="postmessage">
            看这个视频<br>
            <a href="https://v.douyin.com/abc/" target="_blank">https://v.douyin.com/abc/</a><br>
            句子里提到的 <a href="https://www.apple.com/">苹果官网</a> 保持普通文字
        </div>"#;
        let doc = parse_document(html);
        let body = first(doc.root(), "div.postmessage").expect("body");
        let blocks = parse_blocks(*body);
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Link { url, .. } if url.contains("douyin.com"))),
            "裸 URL 应成为可点击链接块：{blocks:?}"
        );
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Paragraph(t) if t.contains("苹果官网"))),
            "句子内链接仍按文字渲染：{blocks:?}"
        );
    }
}
