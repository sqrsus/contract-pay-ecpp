//! docx → PDF：**交给本机 Microsoft Word 转换**。
//!
//! 环境已确认装有 Word（用户 2026-11-05 回答），因此不引入 PDF 渲染库：Word 的
//! `Document.ExportAsFixedFormat(dst, 17)` 是固定版式导出，字形、分页、表格宽度与
//! 打印件完全一致，避免自研渲染与打印结果不一致。
//!
//! 实现方式：把一段**纯 ASCII** 的 .ps1 落到临时目录，用参数把源/目标路径传进去，
//! 再以 `powershell -File` 调用。脚本不含中文，规避 PowerShell 5.1 的编码坑；路径
//! 走命令行参数按 UTF-16 传递，中文路径不受影响。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 导出脚本：`17` 是 Word 的 `wdExportFormatPDF`，固定版式。
const EXPORT_SCRIPT: &str = r#"param([string]$src, [string]$dst)
$ErrorActionPreference = 'Stop'
$word = $null
try {
    $word = New-Object -ComObject Word.Application
    $word.Visible = $false
    $word.DisplayAlerts = 0
    $doc = $word.Documents.Open($src, $false, $true)
    $doc.ExportAsFixedFormat($dst, 17)
    $doc.Close(0)
} catch {
    Write-Output ("PDF_EXPORT_ERROR: " + $_.Exception.Message)
    exit 1
} finally {
    if ($word -ne $null) { $word.Quit() }
}
exit 0
"#;

fn script_path() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("ctp-print");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    let path = dir.join("export_pdf.ps1");
    // 每次都重写：脚本随程序内置，不依赖用户环境
    std::fs::write(&path, EXPORT_SCRIPT).map_err(|e| format!("写入导出脚本失败：{e}"))?;
    Ok(path)
}

/// 用 Word 把 docx 导出成 PDF。失败一律返回错误（已确认异常口径为报错中止）。
pub fn docx_to_pdf(docx: &Path, pdf: &Path) -> Result<(), String> {
    if !docx.is_file() {
        return Err(format!("源文件不存在：{}", docx.display()));
    }
    if let Some(dir) = pdf.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
        }
    }
    // 目标已存在时 Word 会沿用旧文件，先删掉确保拿到的是这一次的产物
    if pdf.exists() {
        std::fs::remove_file(pdf).map_err(|e| format!("覆盖已有文件失败：{e}"))?;
    }

    let script = script_path()?;
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-src")
        .arg(docx)
        .arg("-dst")
        .arg(pdf)
        .output()
        .map_err(|e| format!("无法启动 Word 导出进程：{e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || stdout.contains("PDF_EXPORT_ERROR") {
        let detail = stdout
            .lines()
            .find(|l| l.contains("PDF_EXPORT_ERROR"))
            .or_else(|| stderr.lines().next())
            .unwrap_or("未知错误");
        return Err(format!(
            "Word 导出 PDF 失败（需本机安装 Microsoft Word）：{detail}"
        ));
    }
    if !pdf.is_file() {
        return Err("Word 报告导出成功，但没有生成 PDF 文件".to_string());
    }
    Ok(())
}

/// 供界面提示用的口径说明。
pub fn engine_note() -> &'static str {
    "PDF 由本机 Microsoft Word 导出（固定版式，与打印件同版）"
}
