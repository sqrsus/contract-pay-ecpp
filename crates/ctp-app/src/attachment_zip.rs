//! 合同附件打包：把已取出的附件字节流整理成标准 ZIP。
//!
//! 这里只做纯内存的数据变换——不查库、不写文件、不弹对话框，方便单测锁定
//! 「空列表不打包」「同名附件不互相覆盖」「文件名不带路径分隔符」三条不变量。
//! 取字节、弹「另存为」、写盘都由界面层负责。

use std::collections::HashSet;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// 一个待打包的附件。`category` 只在文件名撞车时用作前缀（附件类别名）。
pub struct ZipEntry {
    pub category: &'static str,
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// ZIP 内条目名净化：只取 basename，去掉路径分隔符与控制/保留字符。
///
/// 库里存的已经是 `service::sanitize_file_name` 的结果，这里再挡一次，
/// 防止旧库或手工写入的数据带上 `..\` 之类的路径段。
fn safe_entry_name(raw: &str) -> String {
    let base = raw.trim().rsplit(['/', '\\']).next().unwrap_or("").trim();
    let mut out = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        out.push(ch);
    }
    let out = out.trim().trim_start_matches('.');
    if out.is_empty() {
        return "未命名附件".to_string();
    }
    out.chars().take(120).collect()
}

/// 为同名附件生成唯一条目名：第一次用原名，重名时加类别前缀，
/// 仍重名则追加递增序号。分类名与中文都保留，ZIP 会自动置 UTF-8 标志。
fn unique_entry_name(base: &str, category: &str, used: &mut HashSet<String>) -> String {
    if !used.contains(base) {
        used.insert(base.to_string());
        return base.to_string();
    }

    let prefixed = format!("{}_{}", category, base);
    if !used.contains(&prefixed) {
        used.insert(prefixed.clone());
        return prefixed;
    }

    let mut n = 2;
    loop {
        let candidate = format!("{}_{}_{}", category, base, n);
        if !used.contains(&candidate) {
            used.insert(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

/// 由合同编号生成默认 ZIP 文件名，例如 `HT-2026-001_附件.zip`。
/// 编号为空或全是非法字符时退回 `合同附件.zip`。
pub fn zip_file_name(contract_no: &str) -> String {
    let name = safe_entry_name(contract_no);
    if name == "未命名附件" {
        return "合同附件.zip".to_string();
    }
    format!("{name}_附件.zip")
}

/// 把附件列表打包为 ZIP 字节。
///
/// 空列表直接报错——用户确认过不生成空包。整体在内存里完成：
/// 只有 `Ok` 返回后界面才弹「另存为」并落盘，失败时不留半成品文件。
pub fn pack_entries(entries: &[ZipEntry]) -> Result<Vec<u8>, String> {
    if entries.is_empty() {
        return Err("当前合同没有可打包的附件".to_string());
    }

    let mut used = HashSet::with_capacity(entries.len());
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for entry in entries {
            let base = safe_entry_name(&entry.file_name);
            let name = unique_entry_name(&base, entry.category, &mut used);
            zip.start_file(name.clone(), options)
                .map_err(|e| format!("创建 ZIP 条目「{name}」失败：{e}"))?;
            zip.write_all(&entry.bytes)
                .map_err(|e| format!("写入 ZIP 条目「{name}」失败：{e}"))?;
        }
        zip.finish()
            .map_err(|e| format!("完成 ZIP 打包失败：{e}"))?;
    }
    Ok(cursor.into_inner())
}

/// 测试辅助：读出 ZIP 内的 (条目名, 字节)，按写入顺序排列。
#[cfg(test)]
fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    use zip::ZipArchive;

    let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("ZIP 应可解析");
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).expect("条目应可读");
        let name = file.name().expect("条目名应为有效 UTF-8").to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).expect("条目内容应可读");
        out.push((name, data));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(category: &'static str, name: &str, bytes: &[u8]) -> ZipEntry {
        ZipEntry {
            category,
            file_name: name.to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn r83_空附件列表不生成空包() {
        let err = pack_entries(&[]).unwrap_err();
        assert!(err.contains("没有可打包的附件"), "错误应对用户可读：{err}");
    }

    #[test]
    fn r83_打包后条目名与内容保真() {
        let entries = vec![
            entry("中标通知书", "bid.pdf", b"bid-bytes"),
            entry("合同", "contract.pdf", b"contract-bytes"),
        ];
        let bytes = pack_entries(&entries).expect("应打包成功");
        let got = read_zip(&bytes);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("bid.pdf".to_string(), b"bid-bytes".to_vec()));
        assert_eq!(
            got[1],
            ("contract.pdf".to_string(), b"contract-bytes".to_vec())
        );
    }

    #[test]
    fn r83_同名附件加类别前缀且不覆盖() {
        let entries = vec![
            entry("中标通知书", "scan.pdf", b"first"),
            entry("合同", "scan.pdf", b"second"),
            entry("审计报告", "scan.pdf", b"third"),
        ];
        let got = read_zip(&pack_entries(&entries).expect("应打包成功"));
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["scan.pdf", "合同_scan.pdf", "审计报告_scan.pdf"],
            "重名条目必须各自唯一且保留类别信息"
        );
        let bodies: Vec<&[u8]> = got.iter().map(|(_, b)| b.as_slice()).collect();
        assert_eq!(bodies, vec![b"first".as_slice(), b"second", b"third"]);
    }

    #[test]
    fn r83_路径穿越与空名被净化() {
        let entries = vec![
            entry("合同", r"..\..\secret.pdf", b"x"),
            entry("验收材料", "   ", b"y"),
        ];
        let got = read_zip(&pack_entries(&entries).expect("应打包成功"));
        assert_eq!(got[0].0, "secret.pdf", "ZIP 内不得保留目录段");
        assert_eq!(got[1].0, "未命名附件");
    }

    #[test]
    fn r83_默认文件名去掉路径字符() {
        assert_eq!(zip_file_name("HT-2026-001"), "HT-2026-001_附件.zip");
        assert_eq!(zip_file_name(r"..\..\x/y"), "y_附件.zip");
        assert_eq!(zip_file_name("  "), "合同附件.zip");
    }
}
