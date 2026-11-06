//! Windows 系统打印后端。
//!
//! 只用系统自带的 GDI + 通用打印对话框（comdlg32），**不依赖 Word / Edge / 任何
//! 第三方软件**：程序自己弹打印对话框、自己把版面画到打印机 DC 上。
//!
//! 两条通道共用同一份 `paper` 版面：
//! * [`print_vouchers`] —— 弹「打印」对话框 → 取打印机 DC → 逐页输出，直接出纸；
//! * [`render_page`] —— 画到内存 32 位 DIB，转成 RGBA 给界面做预览，
//!   因此预览与出纸是同一套坐标、同一套字体，所见即所印。
//!
//! 已确认的异常口径：**一律报错中止**，不降级、不输出半成品（含纸型不是横向时的拦截）。

use crate::paper::{self, Align, Canvas, Face, TextStyle, Voucher, PAGE_H_MM, PAGE_W_MM};
use crate::print::{Preview, PrintError};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi as gdi;
use windows_sys::Win32::Storage::Xps as prn;
use windows_sys::Win32::UI::Controls::Dialogs as dlg;

/// 细线宽度（pt），与模板 0.5pt 边框一致。
const LINE_PT: f32 = 0.5;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------- 画布

/// 把 `paper` 的毫米坐标画到任意 HDC 上。
struct GdiCanvas {
    hdc: gdi::HDC,
    sx: f32,
    sy: f32,
    /// 1pt 对应多少像素（= dpi / 72）
    pt_px: f32,
    ox: f32,
    oy: f32,
    /// 字体对象缓存，键为（像素高、是否加粗、字体族）——两页字体族不同，必须分开缓存。
    fonts: Vec<((i32, bool, Face), gdi::HFONT)>,
}

impl GdiCanvas {
    fn new(hdc: gdi::HDC, sx: f32, sy: f32, ox: f32, oy: f32) -> Self {
        Self {
            hdc,
            sx,
            sy,
            pt_px: sy * 25.4 / 72.0,
            ox,
            oy,
            fonts: Vec::new(),
        }
    }

    fn x(&self, mm: f32) -> i32 {
        (mm * self.sx - self.ox).round() as i32
    }

    fn y(&self, mm: f32) -> i32 {
        (mm * self.sy - self.oy).round() as i32
    }

    /// 取（并按需创建）一个字体对象。
    ///
    /// 「宋体」「新宋体」都是 Windows 自带字体，无需随程序分发；字体族由版面
    /// 层按页指定（R-29 按模板实测：第 1 页新宋体、第 2 页宋体）。
    fn font(&mut self, size_pt: f32, bold: bool, face: Face) -> gdi::HFONT {
        // 负高度 = 字符高度（em box 高度），pt → px 换算就是 字号 × dpi / 72
        let height = -((size_pt * self.pt_px).round() as i32).max(8);
        let key = (height, bold, face);
        if let Some((_, f)) = self.fonts.iter().find(|(k, _)| *k == key) {
            return *f;
        }
        let name = wide(face.name());
        let weight = if bold { 700 } else { 400 };
        let font = unsafe {
            gdi::CreateFontW(
                height,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                1, // DEFAULT_CHARSET
                0,
                0,
                0,
                0,
                name.as_ptr(),
            )
        };
        self.fonts.push((key, font));
        font
    }

    fn align_flag(a: Align) -> u32 {
        match a {
            Align::Left => gdi::DT_LEFT,
            Align::Center => gdi::DT_CENTER,
            Align::Right => gdi::DT_RIGHT,
        }
    }
}

impl Drop for GdiCanvas {
    fn drop(&mut self) {
        for (_, f) in self.fonts.drain(..) {
            unsafe {
                gdi::DeleteObject(f as gdi::HGDIOBJ);
            }
        }
    }
}

impl Canvas for GdiCanvas {
    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let w = ((LINE_PT * self.pt_px).round() as i32).max(1);
        unsafe {
            let pen = gdi::CreatePen(gdi::PS_SOLID, w, 0x0000_0000);
            if pen.is_null() {
                return;
            }
            let old = gdi::SelectObject(self.hdc, pen as gdi::HGDIOBJ);
            gdi::MoveToEx(self.hdc, self.x(x1), self.y(y1), std::ptr::null_mut());
            gdi::LineTo(self.hdc, self.x(x2), self.y(y2));
            gdi::SelectObject(self.hdc, old);
            gdi::DeleteObject(pen as gdi::HGDIOBJ);
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.line(x, y, x + w, y);
        self.line(x, y + h, x + w, y + h);
        self.line(x, y, x, y + h);
        self.line(x + w, y, x + w, y + h);
    }

    fn draw_text(&mut self, x: f32, y: f32, w: f32, h: f32, text: &str, style: TextStyle) {
        if text.is_empty() {
            return;
        }
        let font = self.font(style.size_pt, style.bold, style.face);
        let mut unit: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: self.x(x),
            top: self.y(y),
            right: self.x(x + w),
            bottom: self.y(y + h),
        };
        unsafe {
            let old = gdi::SelectObject(self.hdc, font as gdi::HGDIOBJ);
            gdi::SetBkMode(self.hdc, gdi::TRANSPARENT as i32);
            gdi::SetTextColor(self.hdc, 0x0000_0000);
            gdi::DrawTextW(
                self.hdc,
                unit.as_mut_ptr(),
                unit.len() as i32,
                &mut rc,
                gdi::DT_WORDBREAK | gdi::DT_NOPREFIX | Self::align_flag(style.align),
            );
            gdi::SelectObject(self.hdc, old);
        }
    }

    fn measure_text(&mut self, text: &str, style: TextStyle, w: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let font = self.font(style.size_pt, style.bold, style.face);
        let mut unit: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: (w * self.sx).round() as i32,
            bottom: 0,
        };
        unsafe {
            let old = gdi::SelectObject(self.hdc, font as gdi::HGDIOBJ);
            gdi::DrawTextW(
                self.hdc,
                unit.as_mut_ptr(),
                unit.len() as i32,
                &mut rc,
                gdi::DT_CALCRECT
                    | gdi::DT_WORDBREAK
                    | gdi::DT_NOPREFIX
                    | Self::align_flag(style.align),
            );
            gdi::SelectObject(self.hdc, old);
        }
        (rc.bottom - rc.top) as f32 / self.sy
    }
}

// ---------------------------------------------------------------- 打印

/// 弹系统打印对话框，取到打印机 DC。
///
/// `owner` 可为空（对话框不挂主窗）；返回 `Ok(None)` 表示用户取消。
fn ask_printer(owner: *mut c_void) -> Result<Option<gdi::HDC>, PrintError> {
    let mut pd: dlg::PRINTDLGW = unsafe { std::mem::zeroed() };
    pd.lStructSize = std::mem::size_of::<dlg::PRINTDLGW>() as u32;
    pd.hwndOwner = owner;
    pd.Flags = dlg::PD_RETURNDC | dlg::PD_USEDEVMODECOPIESANDCOLLATE;
    pd.nMinPage = 1;
    pd.nMaxPage = 1;
    pd.nCopies = 1;

    let ok = unsafe { dlg::PrintDlgW(&mut pd) };
    if ok == 0 {
        // 不确定是取消还是失败：hDC 为空即视为取消
        return Ok(None);
    }
    if pd.hDC.is_null() {
        return Err(PrintError::Print("打印对话框没有返回打印机句柄".into()));
    }
    Ok(Some(pd.hDC))
}

/// 打印机可打印区的物理尺寸（mm）与 DPI/偏移。
struct Metrics {
    sx: f32,
    sy: f32,
    ox: f32,
    oy: f32,
    w_mm: f32,
    h_mm: f32,
}

fn metrics(hdc: gdi::HDC) -> Metrics {
    unsafe {
        let dpi_x = gdi::GetDeviceCaps(hdc, gdi::LOGPIXELSX as i32).max(1) as f32;
        let dpi_y = gdi::GetDeviceCaps(hdc, gdi::LOGPIXELSY as i32).max(1) as f32;
        let px_w = gdi::GetDeviceCaps(hdc, gdi::HORZRES as i32).max(1) as f32;
        let px_h = gdi::GetDeviceCaps(hdc, gdi::VERTRES as i32).max(1) as f32;
        Metrics {
            sx: dpi_x / 25.4,
            sy: dpi_y / 25.4,
            ox: gdi::GetDeviceCaps(hdc, gdi::PHYSICALOFFSETX as i32).max(0) as f32,
            oy: gdi::GetDeviceCaps(hdc, gdi::PHYSICALOFFSETY as i32).max(0) as f32,
            w_mm: px_w / dpi_x * 25.4,
            h_mm: px_h / dpi_y * 25.4,
        }
    }
}

/// 打印一批单据：一张单据两页，一次作业连续输出。
///
/// 返回实际输出页数。用户取消对话框时返回 `Ok(0)`。
pub fn print_vouchers(owner: *mut c_void, vs: &[Voucher], name: &str) -> Result<usize, PrintError> {
    if vs.is_empty() {
        return Err(PrintError::NothingSelected);
    }
    let Some(hdc) = ask_printer(owner)? else {
        return Ok(0);
    };

    let m = metrics(hdc);
    // A4 横向才装得下版面；纵向纸会被裁掉右半边，一律拦下让用户改纸型
    if m.w_mm <= m.h_mm {
        unsafe { gdi::DeleteDC(hdc) };
        return Err(PrintError::Print(format!(
            "当前打印机纸型是纵向（{:.0}×{:.0} mm），单据需要 A4 横向；\
             请在打印对话框的「首选项」里把纸张方向改为横向后重试",
            m.w_mm, m.h_mm
        )));
    }

    let doc = wide(name);
    let info = prn::DOCINFOW {
        cbSize: std::mem::size_of::<prn::DOCINFOW>() as i32,
        lpszDocName: doc.as_ptr(),
        lpszOutput: std::ptr::null(),
        lpszDatatype: std::ptr::null(),
        fwType: 0,
    };

    unsafe {
        if prn::StartDocW(hdc, &info) <= 0 {
            gdi::DeleteDC(hdc);
            return Err(PrintError::Print("无法开始打印作业（检查打印机是否就绪）".into()));
        }
        let mut pages = 0usize;
        for v in vs {
            for page in 0..paper::PAGES_PER_VOUCHER {
                if prn::StartPage(hdc) <= 0 {
                    prn::EndDoc(hdc);
                    gdi::DeleteDC(hdc);
                    return Err(PrintError::Print(format!("第 {} 页开始失败", pages + 1)));
                }
                {
                    let mut c = GdiCanvas::new(hdc, m.sx, m.sy, m.ox, m.oy);
                    paper::paint(&mut c, v, page);
                }
                if prn::EndPage(hdc) <= 0 {
                    prn::EndDoc(hdc);
                    gdi::DeleteDC(hdc);
                    return Err(PrintError::Print(format!("第 {} 页输出失败", pages + 1)));
                }
                pages += 1;
            }
        }
        prn::EndDoc(hdc);
        gdi::DeleteDC(hdc);
        Ok(pages)
    }
}

// ---------------------------------------------------------------- 预览

/// 把一页版面渲染成位图，供界面预览。
///
/// 与打印共用 `paper::paint`，只是把目标 DC 从打印机换成内存 DIB。
pub fn render_page(v: &Voucher, page: usize, dpi: f32) -> Result<Preview, PrintError> {
    let dpi = dpi.clamp(72.0, 300.0);
    let w = (PAGE_W_MM / 25.4 * dpi).round() as i32;
    let h = (PAGE_H_MM / 25.4 * dpi).round() as i32;
    let sx = dpi / 25.4;
    let sy = sx;

    unsafe {
        let mem = gdi::CreateCompatibleDC(std::ptr::null_mut());
        if mem.is_null() {
            return Err(PrintError::Print("创建预览画布失败".into()));
        }
        let mut bmi: gdi::BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<gdi::BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // 负高度 = 自顶向下，省一次翻转
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = gdi::BI_RGB;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = gdi::CreateDIBSection(
            mem,
            &bmi,
            gdi::DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            gdi::DeleteDC(mem);
            return Err(PrintError::Print("创建预览位图失败".into()));
        }
        let old = gdi::SelectObject(mem, dib as gdi::HGDIOBJ);

        // 白底：alpha 也置 255，GDI 只写 RGB，透明通道因此保持不透明
        let len = (w as usize) * (h as usize) * 4;
        std::slice::from_raw_parts_mut(bits as *mut u8, len).fill(0xFF);

        {
            let mut c = GdiCanvas::new(mem, sx, sy, 0.0, 0.0);
            paper::paint(&mut c, v, page);
        }

        // BGRA → RGBA（必须在删除 DIB 之前把像素拷出来）
        let rgba = {
            let src = std::slice::from_raw_parts(bits as *const u8, len);
            let mut out = Vec::with_capacity(len);
            for px in src.chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            out
        };

        gdi::SelectObject(mem, old);
        gdi::DeleteObject(dib as gdi::HGDIOBJ);
        gdi::DeleteDC(mem);

        Ok(Preview {
            width: w as u32,
            height: h as u32,
            rgba,
        })
    }
}
