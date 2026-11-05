//! docx（本质是 zip）内部 `word/document.xml` 的**邮件合并域填充**与**多份合并**。
//!
//! 为什么不用字符串替换：模板里 `w:fldCharType="begin"/"separate"/"end"` 构成了域的
//! 结构边界，`«名称»` 那段「结果文本」被 Word 拆成了多个 run（4 个域各拆 3 段），
//! 直接替换 `«名称»` 会漏替换、留下半截残留。所以本模块按域边界整段替换：
//!
//! * 定位 `<w:fldChar w:fldCharType="begin"/>` 所在的 run → 该 run 的 `<w:rPr>` 作为取值 run 的格式；
//! * 一直吃到配套 `<w:fldCharType="end"/>` 所在 run 的 `</w:r>`；
//! * 整段换成 `<w:r>{原 rPr}<w:t>取值</w:t></w:r>`，域代码与结果文本一并消失，
//!   打印件不会被 Word 的「更新域」冲掉（FillStrategy::ReplaceFieldWithText）。
//!
//! 复核过的事实（2026-11-05）：begin/separate/end 各 11 次、无嵌套域、11 处域对应 9 个
//! 唯一域名；最终 `sectPr` 是 body 的最后一个 `<w:sectPr`。

use std::io::{Cursor, Read, Write};

pub const DOC_XML: &str = "word/document.xml";

const F_BEGIN: &str = r#"<w:fldChar w:fldCharType="begin"/>"#;
const F_SEP: &str = r#"<w:fldChar w:fldCharType="separate"/>"#;
const F_END: &str = r#"<w:fldChar w:fldCharType="end"/>"#;
/// 批量合并时两份之间的分页符：一份单据从新的一页开始。
const PAGE_BREAK: &str = r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FillError {
    #[error("模板里没有找到任何合并域，模板可能被改坏")]
    NoField,
    #[error("模板里的合并域没有对应取值：{0}")]
    UnknownField(String),
    #[error("生成结果仍残留未替换的合并域：{0}")]
    Residual(String),
    #[error("docx 结构异常：{0}")]
    Structure(String),
}

#[derive(Debug)]
struct FieldSpan {
    /// 起始 run 的 `<w:r` 位置
    start: usize,
    /// `<w:fldCharType="begin"/>` 的位置（取格式属性用）
    begin: usize,
    /// 结束 run 的 `</w:r>` 之后
    end: usize,
    /// 域名（已剥掉 `\# "0.00"` 之类的域开关）
    name: String,
}

/// 一个域的取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldValue {
    pub name: String,
    pub value: String,
}

impl FieldValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

fn marker_positions(xml: &str) -> Vec<(usize, u8)> {
    let mut out = Vec::new();
    for (i, _) in xml.match_indices("<w:fldChar") {
        let kind = if xml[i..].starts_with(F_BEGIN) {
            0
        } else if xml[i..].starts_with(F_SEP) {
            1
        } else if xml[i..].starts_with(F_END) {
            2
        } else {
            continue;
        };
        out.push((i, kind));
    }
    out.sort_by_key(|(p, _)| *p);
    out
}

/// 向前找包含该位置的 run 起点（`<w:r` 后必须是 `>` 或空白，避开 `<w:rPr`）。
fn run_start_before(xml: &str, pos: usize) -> usize {
    let mut idx = pos;
    while let Some(p) = xml[..idx].rfind("<w:r") {
        match xml.as_bytes().get(p + 4).copied() {
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') => return p,
            _ => idx = p,
        }
    }
    0
}

/// 从该位置起第一个 run 的结束边界（`</w:r>` 之后）。
fn run_end_after(xml: &str, pos: usize) -> usize {
    const CLOSE: &str = "</w:r>";
    match xml[pos..].find(CLOSE) {
        Some(i) => pos + i + CLOSE.len(),
        None => xml.len(),
    }
}

/// 取 run 起点到域开始之间的 `<w:rPr>…</w:rPr>`，让取值沿用模板原有字体字号。
fn run_props(xml: &str, run_start: usize, field_begin: usize) -> &str {
    let seg = &xml[run_start..field_begin];
    let open = match seg.find("<w:rPr>") {
        Some(i) => i,
        None => return "",
    };
    match seg[open..].find("</w:rPr>") {
        Some(i) => &seg[open..open + i + "</w:rPr>".len()],
        None => "",
    }
}

/// 从域代码区间（begin..separate）里拼出域名并剥掉域开关。
fn field_name(instr_region: &str) -> Option<String> {
    const TAG_OPEN: &str = "<w:instrText";
    let mut code = String::new();
    let mut rest = instr_region;
    while let Some(i) = rest.find(TAG_OPEN) {
        let after = &rest[i..];
        let gt = after.find('>')?;
        let body = &after[gt + 1..];
        let end = body.find("</w:instrText>")?;
        code.push_str(&body[..end]);
        rest = &body[end + "</w:instrText>".len()..];
    }
    let code = code.trim();
    let i = code.find("MERGEFIELD")?;
    let tail = code[i + "MERGEFIELD".len()..].trim_start();
    // 域名到第一个空白或 `\`（域开关）为止；模板里的域名本身不含空格
    let name: String = tail
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '\\')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn field_spans(xml: &str) -> Vec<FieldSpan> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut begin_pos = 0usize;
    let mut sep_pos: Option<usize> = None;
    for (pos, kind) in marker_positions(xml) {
        match kind {
            0 => {
                depth += 1;
                if depth == 1 {
                    begin_pos = pos;
                    sep_pos = None;
                }
            }
            1 => {
                if depth == 1 {
                    sep_pos = Some(pos);
                }
            }
            _ => {
                if depth == 1 {
                    let instr = &xml[begin_pos..sep_pos.unwrap_or(pos)];
                    if let Some(name) = field_name(instr) {
                        out.push(FieldSpan {
                            start: run_start_before(xml, begin_pos),
                            begin: begin_pos,
                            end: run_end_after(xml, pos),
                            name,
                        });
                    }
                }
                depth = depth.saturating_sub(1);
            }
        }
    }
    out
}

/// 模板里出现的域名（按出现顺序，含重复），供自检与界面提示。
pub fn field_names(xml: &str) -> Vec<String> {
    field_spans(xml).into_iter().map(|s| s.name).collect()
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 取值 run：换行转 `<w:br/>`，其余做 XML 转义。
fn value_run(rpr: &str, value: &str) -> String {
    let mut out = String::with_capacity(value.len() + rpr.len() + 64);
    out.push_str("<w:r>");
    out.push_str(rpr);
    for (i, line) in value.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<w:br/>");
        }
        out.push_str(r##"<w:t xml:space="preserve">"##);
        out.push_str(&escape_text(line));
        out.push_str("</w:t>");
    }
    out.push_str("</w:r>");
    out
}

/// 把一份 `document.xml` 片段里的所有合并域替换成文本。
pub fn fill(xml: &str, values: &[FieldValue]) -> Result<String, FillError> {
    let spans = field_spans(xml);
    if spans.is_empty() {
        return Err(FillError::NoField);
    }
    let mut out = xml.to_string();
    // 从后往前替换，前面段的字节位置才不受影响
    for span in spans.iter().rev() {
        let v = values
            .iter()
            .find(|v| v.name == span.name)
            .ok_or_else(|| FillError::UnknownField(span.name.clone()))?;
        let rpr = run_props(&out, span.start, span.begin);
        out.replace_range(span.start..span.end, &value_run(rpr, &v.value));
    }
    Ok(out)
}

/// 生成结果里残留的合并域名（正常应为空）。生成后自检用。
pub fn residual_fields(xml: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in field_names(xml) {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if xml.contains('«') || xml.contains('»') {
        out.push("«…» 结果文本".to_string());
    }
    out
}

/// 正文三段切分：`<w:body>` 之后 / 最后一个 `<w:sectPr` 之前 / 其余（节属性 + 收尾）。
fn body_bounds(xml: &str) -> Result<(usize, usize), FillError> {
    let open = xml
        .find("<w:body>")
        .map(|i| i + "<w:body>".len())
        .ok_or_else(|| FillError::Structure("缺少 <w:body>".to_string()))?;
    let sect = xml
        .rfind("<w:sectPr")
        .ok_or_else(|| FillError::Structure("缺少 <w:sectPr>".to_string()))?;
    if sect < open {
        return Err(FillError::Structure("<w:sectPr> 位于 <w:body> 之前".to_string()));
    }
    Ok((open, sect))
}

/// 依次填充每一份，并按「一份一页」拼成同一个 document.xml。
///
/// `copies` 为空时按单份处理；单份与批量走同一条路径，避免两条实现分叉。
pub fn render_document(template: &str, copies: &[Vec<FieldValue>]) -> Result<String, FillError> {
    if copies.is_empty() {
        return Err(FillError::Structure("没有待输出的单据".to_string()));
    }
    let (open, sect) = body_bounds(template)?;
    let inner = &template[open..sect];
    let mut out = String::with_capacity(template.len() * copies.len().max(1));
    out.push_str(&template[..open]);
    for (i, values) in copies.iter().enumerate() {
        if i > 0 {
            out.push_str(PAGE_BREAK);
        }
        out.push_str(&fill(inner, values)?);
    }
    out.push_str(&template[sect..]);
    if let Some(first) = residual_fields(&out).first() {
        return Err(FillError::Residual(first.clone()));
    }
    Ok(out)
}

// ---------------------------------------------------------------- zip 读写

fn zip_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 读 docx 里的一个文本条目。
pub fn read_entry(zip_bytes: &[u8], name: &str) -> Result<String, String> {
    let mut ar = zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(zip_err)?;
    let mut f = ar.by_name(name).map_err(zip_err)?;
    let mut s = String::new();
    f.read_to_string(&mut s).map_err(zip_err)?;
    Ok(s)
}

/// 用新内容替换 docx 里的一个条目，其余条目原样保留（顺序不变）。
pub fn rewrite_entry(zip_bytes: &[u8], name: &str, content: &str) -> Result<Vec<u8>, String> {
    let mut ar = zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(zip_err)?;
    let mut out: Vec<u8> = Vec::with_capacity(zip_bytes.len() + content.len());
    {
        let mut zw = zip::ZipWriter::new(Cursor::new(&mut out));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut found = false;
        for i in 0..ar.len() {
            let mut f = ar.by_index(i).map_err(zip_err)?;
            let entry_name = f.name().to_string();
            zw.start_file(&entry_name, opts).map_err(zip_err)?;
            if entry_name == name {
                found = true;
                zw.write_all(content.as_bytes()).map_err(zip_err)?;
            } else {
                std::io::copy(&mut f, &mut zw).map_err(zip_err)?;
            }
        }
        if !found {
            return Err(format!("docx 里没有 {name} 条目"));
        }
        zw.finish().map_err(zip_err)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与真实模板同构的最小片段：结果文本被拆成 3 个 run、域代码带 `\# "0.00"` 开关。
    fn sample() -> String {
        let mut s = String::from("<w:document><w:body><w:p>");
        s.push_str(
            r#"<w:r><w:rPr><w:sz w:val="29"/></w:rPr><w:fldChar w:fldCharType="begin"/></w:r>"#,
        );
        s.push_str(r#"<w:r><w:instrText> MERGEFIELD </w:instrText></w:r>"#);
        s.push_str(r#"<w:r><w:instrText>本次付款小写\# "0.00"</w:instrText></w:r>"#);
        s.push_str(r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#);
        s.push_str(r#"<w:r><w:t>«</w:t></w:r><w:r><w:t>本次付款小写</w:t></w:r>"#);
        s.push_str(r#"<w:r><w:t>»</w:t></w:r>"#);
        s.push_str(r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#);
        s.push_str("</w:p><w:sectPr><w:pgSz w:w=\"100\"/></w:sectPr></w:body></w:document>");
        s
    }

    #[test]
    fn 域名剥掉域开关且跨_run_拼接() {
        assert_eq!(field_names(&sample()), vec!["本次付款小写"]);
    }

    #[test]
    fn 整域替换为纯文本并沿用原有字体() {
        let out = fill(&sample(), &[FieldValue::new("本次付款小写", "1234.50")]).unwrap();
        assert!(!out.contains("MERGEFIELD"), "{out}");
        assert!(!out.contains('«'), "{out}");
        assert!(out.contains(r#"<w:t xml:space="preserve">1234.50</w:t>"#));
        assert!(out.contains(r#"<w:sz w:val="29"/>"#), "应保留模板字体字号");
        assert!(residual_fields(&out).is_empty());
    }

    #[test]
    fn 未映射的域报错而不是静默留空() {
        let e = fill(&sample(), &[FieldValue::new("别的域", "x")]).unwrap_err();
        assert_eq!(e, FillError::UnknownField("本次付款小写".into()));
    }

    #[test]
    fn 取值里的尖括号与换行被安全处理() {
        let out = fill(&sample(), &[FieldValue::new("本次付款小写", "a<b&c\n第二行")]).unwrap();
        assert!(out.contains("a&lt;b&amp;c"), "{out}");
        assert!(out.contains("<w:br/>"), "{out}");
        assert!(!out.contains("a<b&c"), "{out}");
    }

    #[test]
    fn 多份合并按一份一页拼接且只保留一个节属性() {
        let out = render_document(
            &sample(),
            &[
                vec![FieldValue::new("本次付款小写", "1.00")],
                vec![FieldValue::new("本次付款小写", "2.00")],
            ],
        )
        .unwrap();
        assert_eq!(out.matches(r#"w:fldCharType="begin""#).count(), 0);
        assert_eq!(out.matches("<w:br w:type=\"page\"/>").count(), 1);
        assert!(out.contains("1.00") && out.contains("2.00"));
        assert_eq!(out.matches("<w:sectPr").count(), 1, "节属性只应出现一次");
    }

    #[test]
    fn 空批次直接报错() {
        assert!(render_document(&sample(), &[]).is_err());
    }
}
