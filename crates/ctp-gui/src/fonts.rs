//! 中文界面字体：从系统字体目录挑一款 CJK 字体注入 egui。
//!
//! egui 自带字体不含中文，必须显式加载，否则全部显示为方块。
//! 优先单选 TTF（解析最稳），其次 TTC 集合字体。

use eframe::egui;

/// 候选字体路径（按优先级）。
const CANDIDATES: &[&str] = &[
    "C:/Windows/Fonts/msyh.ttc",   // 微软雅黑（界面观感最接近系统应用）
    "C:/Windows/Fonts/simhei.ttf", // 黑体
    "C:/Windows/Fonts/Deng.ttf",   // 等线（字重偏细，作为后备）
    "C:/Windows/Fonts/simsun.ttc", // 宋体
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/arphic/uming.ttc",
    "/System/Library/Fonts/PingFang.ttc",
];

/// 安装字体，返回实际使用的字体路径（找不到则返回 None，界面会显示方块）。
pub fn install(ctx: &egui::Context) -> Option<String> {
    let mut fonts = egui::FontDefinitions::default();
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(bytes));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(fam)
                    .or_default()
                    .insert(0, "cjk".to_owned());
            }
            ctx.set_fonts(fonts);
            return Some((*path).to_string());
        }
    }
    None
}
