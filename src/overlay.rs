//! Transparent, always-on-top, click-through overlay window painted from a CPU buffer.
//!
//! The spike question is whether per-pixel alpha plus input passthrough actually work on
//! Windows, so this is raw `WS_EX_LAYERED` + `UpdateLayeredWindow` glue with no GUI framework
//! in the way. [`Frame`] is the pure part (pixel math, unit-tested); [`Overlay`] is Win32.

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP,
    HDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
    GetSystemMetrics, GetWindowLongPtrW, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN,
    SM_CYSCREEN, SetWindowLongPtrW, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_QUIT,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    WS_VISIBLE,
};
use windows::core::PCWSTR;

/// A rectangle in frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Colour as the caller thinks about it: straight (not premultiplied) RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Straight RGBA to the premultiplied BGRA32 layout `UpdateLayeredWindow` requires.
///
/// Rounding is up by `+ 127` so 50% alpha over black keeps its last unit; truncating here is
/// how translucent keycaps end up with visible dark edges.
fn premultiply(c: Rgba) -> [u8; 4] {
    let a = c.a as u32;
    let mul = |v: u8| (((v as u32) * a + 127) / 255) as u8;
    [mul(c.b), mul(c.g), mul(c.r), c.a]
}

/// One frame of premultiplied BGRA pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Frame {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            px: vec![0; (w as usize) * (h as usize) * 4],
        }
    }

    /// Fully transparent — the part of the screen the user sees through.
    pub fn clear(&mut self) {
        self.px.iter_mut().for_each(|p| *p = 0);
    }

    /// Fill `rect`, clipped to the frame. A fully outside rectangle paints nothing.
    pub fn fill_rect(&mut self, rect: Rect, colour: Rgba) {
        let pixel = premultiply(colour);
        let x1 = rect.x.min(self.w);
        let y1 = rect.y.min(self.h);
        let x2 = (rect.x + rect.w).min(self.w);
        let y2 = (rect.y + rect.h).min(self.h);
        for y in y1..y2 {
            let row = (y as usize) * (self.w as usize);
            for x in (x1..x2).rev() {
                let i = (row + x as usize) * 4;
                self.px[i..i + 4].copy_from_slice(&pixel);
            }
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Switch to per-monitor-aware-v2. Must be called before the first window exists, and it
/// returns false if something else already set a context (then we keep what was there).
pub fn make_dpi_aware() -> bool {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.is_ok()
}

/// System DPI / 96. Used to size the window before it exists.
pub fn system_scale() -> f32 {
    (unsafe { GetDpiForSystem() }) as f32 / 96.0
}

unsafe extern "system" fn wndproc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// A live overlay window plus the DIB it is painted from.
pub struct Overlay {
    window: HWND,
    screen_dc: HDC,
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_object: HGDIOBJ,
    bits: *mut u8,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    click_through: bool,
}

impl Overlay {
    /// Create the window at `x`/`y` in physical pixels, sized `w`x`h`.
    ///
    /// Styles: `WS_EX_LAYERED` (per-pixel alpha), `WS_EX_TRANSPARENT` (clicks fall through),
    /// `WS_EX_NOACTIVATE` (never takes focus), `WS_EX_TOOLWINDOW` (absent from taskbar and
    /// Alt-Tab), `WS_EX_TOPMOST`.
    pub fn create(x: i32, y: i32, w: i32, h: i32) -> Result<Self, String> {
        let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|err| format!("GetModuleHandleW: {err}"))?;
        let class_name = wide("kmoverlay-overlay");

        let class = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: windows::Win32::Foundation::HINSTANCE(instance.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(format!(
                "RegisterClassW: {}",
                std::io::Error::last_os_error()
            ));
        }

        let ex_style =
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        let title = wide("kmoverlay");
        let window = unsafe {
            CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                w,
                h,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|err| format!("CreateWindowExW: {err}"))?;

        let screen_dc = unsafe { GetDC(None) };
        let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // negative height = top-down rows, so nothing has to be flipped later
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap =
            unsafe { CreateDIBSection(Some(mem_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|err| format!("CreateDIBSection: {err}"))?;
        if bits.is_null() {
            return Err("CreateDIBSection returned no pixel memory".into());
        }
        let old_object = unsafe { SelectObject(mem_dc, HGDIOBJ(bitmap.0)) };

        Ok(Self {
            window,
            screen_dc,
            mem_dc,
            bitmap,
            old_object,
            bits: bits as *mut u8,
            x,
            y,
            w,
            h,
            click_through: true,
        })
    }

    /// Primary-screen extent in physical pixels, for callers that anchor to it.
    pub fn primary_width() -> i32 {
        unsafe { GetSystemMetrics(SM_CXSCREEN) }
    }

    pub fn primary_height() -> i32 {
        unsafe { GetSystemMetrics(SM_CYSCREEN) }
    }

    /// Push `frame` to the screen. It must match the window size.
    pub fn present(&self, frame: &Frame) -> Result<(), String> {
        if frame.w as i32 != self.w || frame.h as i32 != self.h {
            return Err(format!(
                "frame {}x{} does not match window {}x{}",
                frame.w, frame.h, self.w, self.h
            ));
        }
        let source = POINT { x: 0, y: 0 };
        let destination = POINT {
            x: self.x,
            y: self.y,
        };
        let size = SIZE {
            cx: self.w,
            cy: self.h,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            std::ptr::copy_nonoverlapping(frame.px.as_ptr(), self.bits, frame.px.len());
            UpdateLayeredWindow(
                self.window,
                Some(self.screen_dc),
                Some(&destination),
                Some(&size),
                Some(self.mem_dc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .map_err(|err| format!("UpdateLayeredWindow: {err}"))
        }
    }

    /// DPI of the monitor actually hosting this window. Differs from [`system_scale`] on
    /// mixed-DPI multi-monitor setups, which is the scaling caveat this spike has to measure.
    pub fn scale(&self) -> f32 {
        (unsafe { GetDpiForWindow(self.window) }) as f32 / 96.0
    }

    /// Drain pending window messages. Returns false once a quit message arrives, so the
    /// caller can stop instead of spinning on a dead window.
    pub fn pump(&mut self) -> bool {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
            if message.message == WM_QUIT {
                return false;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        true
    }

    /// Toggle input passthrough. Off is what a future drag mode needs; the overlay stays
    /// non-activating either way.
    ///
    /// `shortcut:` sets the ex-style bits directly, the same route winit users take
    /// (winit#2059). Revisit if a window framework ever owns this window.
    pub fn set_click_through(&mut self, on: bool) {
        self.click_through = on;
        unsafe {
            let style = GetWindowLongPtrW(self.window, GWL_EXSTYLE);
            let style = if on {
                style | WS_EX_TRANSPARENT.0 as isize
            } else {
                style & !(WS_EX_TRANSPARENT.0 as isize)
            };
            let _ = SetWindowLongPtrW(self.window, GWL_EXSTYLE, style);
        }
    }

    pub fn click_through(&self) -> bool {
        self.click_through
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.mem_dc, self.old_object);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.mem_dc);
            let _ = ReleaseDC(None, self.screen_dc);
            let _ = DestroyWindow(self.window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = ((y as usize) * (frame.w as usize) + x as usize) * 4;
        [
            frame.px[i],
            frame.px[i + 1],
            frame.px[i + 2],
            frame.px[i + 3],
        ]
    }

    #[test]
    fn premultiply_is_bgra_and_rounds_up() {
        assert_eq!(
            premultiply(Rgba {
                r: 255,
                g: 128,
                b: 0,
                a: 255
            }),
            [0, 128, 255, 255]
        );
        assert_eq!(
            premultiply(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 128
            }),
            [128, 128, 128, 128]
        );
        assert_eq!(
            premultiply(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 0
            }),
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn fill_rect_paints_and_clears_to_transparent() {
        let mut frame = Frame::new(4, 2);
        frame.fill_rect(
            Rect {
                x: 1,
                y: 0,
                w: 2,
                h: 2,
            },
            Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 255,
            },
        );
        assert_eq!(pixel(&frame, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&frame, 1, 1), [255, 0, 0, 255]);
        assert_eq!(pixel(&frame, 3, 1), [0, 0, 0, 0]);
        frame.clear();
        assert_eq!(pixel(&frame, 1, 1), [0, 0, 0, 0]);
    }

    #[test]
    fn out_of_bounds_rects_are_clipped_not_wrapping() {
        let mut frame = Frame::new(4, 4);
        frame.fill_rect(
            Rect {
                x: 2,
                y: 2,
                w: 100,
                h: 100,
            },
            Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            },
        );
        assert_eq!(pixel(&frame, 3, 3), [30, 20, 10, 255]);
        assert_eq!(pixel(&frame, 0, 3), [0, 0, 0, 0]);
        frame.fill_rect(
            Rect {
                x: 9,
                y: 9,
                w: 2,
                h: 2,
            },
            Rgba {
                r: 1,
                g: 1,
                b: 1,
                a: 255,
            },
        );
        assert_eq!(
            pixel(&frame, 3, 3),
            [30, 20, 10, 255],
            "fully outside must not paint"
        );
    }
}
