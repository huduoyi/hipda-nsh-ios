//! GBK 编解码与表单编码。
//!
//! 4D4Y 实测响应头为 `text/html; charset=GBK`，POST 表单也必须按 GBK
//! 百分号编码。直接用 `serde_urlencoded` 会把中文按 UTF-8 编码，Discuz 解成乱码。

use std::collections::BTreeMap;

use crate::{Error, Result, ENCODE_GBK};

/// 从 Content-Type 里取 charset。
pub fn charset_of(content_type: &str) -> String {
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("charset=") {
            return v.trim_matches(|c| c == '"' || c == '\'').to_string();
        }
        if let Some(v) = part.to_ascii_lowercase().strip_prefix("charset=") {
            return v.trim_matches(|c| c == '"' || c == '\'').to_string();
        }
    }
    String::new()
}

/// 按 Content-Type 的 charset 把站点字节流转成 UTF-8。
/// GBK 解不了的字节回退 GB18030，避免生僻字导致整页解析失败。
pub fn decode_body(raw: &[u8], content_type: &str, fallback: &str) -> Result<String> {
    let mut enc = charset_of(content_type);
    if enc.is_empty() {
        enc = fallback.to_string();
    }
    match enc.to_ascii_uppercase().as_str() {
        "" | "UTF-8" | "UTF8" | "US-ASCII" => Ok(String::from_utf8_lossy(raw).into_owned()),
        "GBK" | "GB2312" | "GB18030" => {
            let (text, _, had_errors) = encoding_rs::GBK.decode(raw);
            if !had_errors {
                return Ok(text.into_owned());
            }
            let (text2, _, still_bad) = encoding_rs::GB18030.decode(raw);
            if still_bad {
                return Err(Error::Parse("GBK/GB18030 解码失败".into()));
            }
            Ok(text2.into_owned())
        }
        // BIG5 等编码尚未支持，原样返回；不假装解码成功。
        _ => Ok(String::from_utf8_lossy(raw).into_owned()),
    }
}

/// 把字符串按站点编码转成字节。不可映射字符退回 UTF-8 字节，
/// 与 Java `URLEncoder.encode(value, encode)` 的容错行为一致。
pub fn encode_value(s: &str, enc: &str) -> Vec<u8> {
    let upper = enc.to_ascii_uppercase();
    if upper == ENCODE_GBK || upper == "GB2312" || upper == "GB18030" {
        let (bytes, _, unmappable) = encoding_rs::GBK.encode(s);
        if !unmappable {
            return bytes.into_owned();
        }
    }
    s.as_bytes().to_vec()
}

/// 逐字节百分号编码，空格转 `+`（Java URLEncoder 语义）。
pub fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len());
    for &c in bytes {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(c as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(c >> 4) as usize] as char);
                out.push(HEX[(c & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// 生成 `application/x-www-form-urlencoded` 请求体。
/// key 稳定排序让请求体可复现，便于调试与录像回放。
pub fn encode_form(pairs: &[(String, String)], enc: &str) -> String {
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (k, v) in pairs {
        grouped.entry(k.as_str()).or_default().push(v.as_str());
    }
    let mut out = String::new();
    for (k, values) in grouped {
        for v in values {
            if !out.is_empty() {
                out.push('&');
            }
            out.push_str(&percent_encode(&encode_value(k, enc)));
            out.push('=');
            out.push_str(&percent_encode(&encode_value(v, enc)));
        }
    }
    out
}
