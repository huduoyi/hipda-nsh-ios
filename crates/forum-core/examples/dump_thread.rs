//! 调试用：把一份保存下来的主题页 HTML 按内核解析结果打出来。
//!
//! 排查「图片/正文没显示」时，先用它确认问题在解析还是在渲染：
//!
//! ```text
//! curl -o /tmp/t.html 'https://www.4d4y.com/forum/viewthread.php?tid=448060'
//! cargo run -p forum-core --example dump_thread -- /tmp/t.html
//! ```

use std::fs;

use forum_core::{parse_document, parse_thread_detail, Config, ContentKind};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("用法：dump_thread <保存的 html 路径>");
        std::process::exit(2);
    };
    let raw = fs::read(&path).unwrap_or_else(|e| {
        eprintln!("读取 {path} 失败：{e}");
        std::process::exit(2);
    });
    // 站点是 GBK；本地样本也可能是 UTF-8，按解码结果里是否出现替换字符判断。
    let body = match String::from_utf8(raw.clone()) {
        Ok(text) if text.contains("Discuz") => text,
        _ => String::from_utf8_lossy(
            &forum_core::encoding::decode_body(&raw, "text/html; charset=GBK", forum_core::ENCODE_GBK)
                .unwrap_or_default()
                .into_bytes(),
        )
        .to_string(),
    };

    let detail = parse_thread_detail(&parse_document(&body), &Config::default(), "", 1);
    println!("tid={} fid={} 标题={}", detail.tid, detail.fid, detail.title);
    println!("楼层={} 总页数={} 投票={}", detail.posts.len(), detail.max_page, detail.poll.is_some());
    if let Some(poll) = &detail.poll {
        println!(
            "      投票: 「{}」选项 {} 个，上限 {}，footer=「{}」，formhash={}",
            poll.title, poll.options.len(), poll.max_answer, poll.footer,
            if poll.form_hash.is_empty() { "缺" } else { "有" }
        );
        for o in poll.options.iter().take(5) {
            println!("        - {} {}", o.id, o.text);
        }
    }
    for post in &detail.posts {
        let images: Vec<&str> = post
            .contents
            .iter()
            .filter(|c| c.kind == ContentKind::Image)
            .map(|c| c.url.as_str())
            .collect();
        println!(
            "--- {} 楼 {} (uid {}) · 内容块 {} · 图片 {}",
            post.floor,
            post.author,
            post.author_id,
            post.contents.len(),
            images.len()
        );
        if !post.avatar_url.is_empty() {
            println!("      头像: {}", post.avatar_url);
        }
        for url in images.iter().take(4) {
            println!("      img: {url}");
        }
        let text = post.copy_text();
        println!("      正文: {}", text.chars().take(60).collect::<String>());
        // 块结构：判断「引用/列表/代码有没有被拍平」只看这一行。
        let kinds: Vec<String> = post
            .blocks
            .iter()
            .map(|b| match b {
                forum_core::model::Block::Paragraph(t) => {
                    format!("段落({}字)", t.chars().count())
                }
                forum_core::model::Block::Quote { author, blocks, .. } => {
                    format!("引用[{}]({}块)", author, blocks.len())
                }
                forum_core::model::Block::List { ordered, items } => format!(
                    "{}列表({}项)",
                    if *ordered { "有序" } else { "无序" },
                    items.len()
                ),
                forum_core::model::Block::Code(c) => {
                    format!("代码({}行)", c.lines().count())
                }
                forum_core::model::Block::Image { .. } => "图片".to_string(),
                forum_core::model::Block::Attach { name, .. } => format!("附件({name})"),
                forum_core::model::Block::Link { url, .. } => format!("链接({url})"),
                forum_core::model::Block::Heading(t) => format!("标题({t})"),
                forum_core::model::Block::Align { center, .. } => {
                    if *center { "居中".to_string() } else { "靠右".to_string() }
                }
                forum_core::model::Block::Rule => "分隔线".to_string(),
            })
            .collect();
        println!("      块: {}", kinds.join(" / "));
    }
}
