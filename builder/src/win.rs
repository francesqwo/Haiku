//! The few Win32 calls Haiku Setup's window needs, declared by hand (no
//! crate): a window, GDI drawing into a bitmap, the file and folder
//! pickers, dropped files.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms, dead_code)]

use std::ffi::c_void;

pub type HWND = isize;
pub type HDC = isize;
pub type HGDIOBJ = isize;
pub type HINSTANCE = isize;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type BOOL = i32;
pub type WNDPROC = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: isize,
    pub hCursor: isize,
    pub hbrBackground: isize,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: isize,
}

#[repr(C)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: BOOL,
    pub rcPaint: RECT,
    pub fRestore: BOOL,
    pub fIncUpdate: BOOL,
    pub rgbReserved: [u8; 32],
}

#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [u32; 1],
}

#[repr(C)]
pub struct OPENFILENAMEW {
    pub lStructSize: u32,
    pub hwndOwner: HWND,
    pub hInstance: HINSTANCE,
    pub lpstrFilter: *const u16,
    pub lpstrCustomFilter: *mut u16,
    pub nMaxCustFilter: u32,
    pub nFilterIndex: u32,
    pub lpstrFile: *mut u16,
    pub nMaxFile: u32,
    pub lpstrFileTitle: *mut u16,
    pub nMaxFileTitle: u32,
    pub lpstrInitialDir: *const u16,
    pub lpstrTitle: *const u16,
    pub Flags: u32,
    pub nFileOffset: u16,
    pub nFileExtension: u16,
    pub lpstrDefExt: *const u16,
    pub lCustData: LPARAM,
    pub lpfnHook: *const c_void,
    pub lpTemplateName: *const u16,
    pub pvReserved: *const c_void,
    pub dwReserved: u32,
    pub FlagsEx: u32,
}

#[repr(C)]
pub struct BROWSEINFOW {
    pub hwndOwner: HWND,
    pub pidlRoot: *const c_void,
    pub pszDisplayName: *mut u16,
    pub lpszTitle: *const u16,
    pub ulFlags: u32,
    pub lpfn: *const c_void,
    pub lParam: LPARAM,
    pub iImage: i32,
}

#[repr(C)]
pub struct TRACKMOUSEEVENT {
    pub cbSize: u32,
    pub dwFlags: u32,
    pub hwndTrack: HWND,
    pub dwHoverTime: u32,
}

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
pub const WM_SETICON: u32 = 0x0080;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_DROPFILES: u32 = 0x0233;
pub const WM_DPICHANGED: u32 = 0x02E0;

pub const WS_OVERLAPPED: u32 = 0x0000_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_EX_ACCEPTFILES: u32 = 0x0000_0010;
pub const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
pub const SW_SHOW: i32 = 5;
pub const SW_HIDE: i32 = 0;
pub const IDC_ARROW: usize = 32512;
pub const IDC_HAND: usize = 32649;
pub const IDC_WAIT: usize = 32514;
pub const IMAGE_ICON: u32 = 1;
pub const LR_DEFAULTCOLOR: u32 = 0;
pub const ICON_SMALL: usize = 0;
pub const ICON_BIG: usize = 1;
pub const TRANSPARENT: i32 = 1;
pub const DT_LEFT: u32 = 0x0;
pub const DT_CENTER: u32 = 0x1;
pub const DT_RIGHT: u32 = 0x2;
pub const DT_VCENTER: u32 = 0x4;
pub const DT_WORDBREAK: u32 = 0x10;
pub const DT_SINGLELINE: u32 = 0x20;
pub const DT_NOPREFIX: u32 = 0x800;
pub const DT_CALCRECT: u32 = 0x400;
pub const DT_END_ELLIPSIS: u32 = 0x8000;
pub const DT_PATH_ELLIPSIS: u32 = 0x4000;
pub const FW_NORMAL: i32 = 400;
pub const FW_SEMIBOLD: i32 = 600;
pub const FW_BOLD: i32 = 700;
pub const DIB_RGB_COLORS: u32 = 0;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const NULL_BRUSH: i32 = 5;
pub const NULL_PEN: i32 = 8;
pub const PS_SOLID: i32 = 0;
pub const OFN_FILEMUSTEXIST: u32 = 0x1000;
pub const OFN_PATHMUSTEXIST: u32 = 0x800;
pub const OFN_EXPLORER: u32 = 0x80000;
pub const OFN_NOCHANGEDIR: u32 = 0x8;
pub const OFN_HIDEREADONLY: u32 = 0x4;
pub const BIF_RETURNONLYFSDIRS: u32 = 0x1;
pub const BIF_NEWDIALOGSTYLE: u32 = 0x40;
pub const TME_LEAVE: u32 = 0x2;
pub const VK_RETURN: usize = 0x0D;
pub const VK_ESCAPE: usize = 0x1B;
pub const MB_OK: u32 = 0x0;
pub const MB_ICONERROR: u32 = 0x10;
pub const MB_YESNO: u32 = 0x4;
pub const MB_ICONQUESTION: u32 = 0x20;
pub const IDYES: i32 = 6;
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
pub const CLEARTYPE_QUALITY: u32 = 5;
pub const COINIT_APARTMENTTHREADED: u32 = 0x2;

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: isize, inst: HINSTANCE, param: *mut c_void) -> HWND;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> BOOL;
    pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(code: i32);
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL;
    pub fn UpdateWindow(hwnd: HWND) -> BOOL;
    pub fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: BOOL) -> BOOL;
    pub fn SetTimer(hwnd: HWND, id: usize, ms: u32, f: *const c_void) -> usize;
    pub fn BeginPaint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(hwnd: HWND, ps: *const PAINTSTRUCT) -> BOOL;
    pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn LoadCursorW(inst: HINSTANCE, name: usize) -> isize;
    pub fn SetCursor(cursor: isize) -> isize;
    pub fn LoadImageW(inst: HINSTANCE, name: usize, kind: u32, cx: i32, cy: i32, flags: u32) -> isize;
    pub fn SendMessageW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetDpiForWindow(hwnd: HWND) -> u32;
    pub fn SetProcessDpiAwarenessContext(ctx: isize) -> BOOL;
    pub fn AdjustWindowRectExForDpi(rect: *mut RECT, style: u32, menu: BOOL, ex: u32, dpi: u32) -> BOOL;
    pub fn SetWindowPos(hwnd: HWND, after: HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> BOOL;
    pub fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn TrackMouseEvent(tme: *mut TRACKMOUSEEVENT) -> BOOL;
    pub fn FillRect(hdc: HDC, rect: *const RECT, brush: isize) -> i32;
    pub fn DrawTextW(hdc: HDC, text: *const u16, len: i32, rect: *mut RECT, format: u32) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateCompatibleDC(hdc: HDC) -> HDC;
    pub fn CreateDIBSection(hdc: HDC, info: *const BITMAPINFO, usage: u32, bits: *mut *mut c_void, section: isize, offset: u32) -> HGDIOBJ;
    pub fn SelectObject(hdc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
    pub fn DeleteObject(obj: HGDIOBJ) -> BOOL;
    pub fn DeleteDC(hdc: HDC) -> BOOL;
    pub fn BitBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, sx: i32, sy: i32, rop: u32) -> BOOL;
    pub fn CreateFontW(h: i32, w: i32, esc: i32, orient: i32, weight: i32, italic: u32, underline: u32, strike: u32, charset: u32, out: u32, clip: u32, quality: u32, pitch: u32, face: *const u16) -> HGDIOBJ;
    pub fn SetBkMode(hdc: HDC, mode: i32) -> i32;
    pub fn SetTextColor(hdc: HDC, colour: u32) -> u32;
    pub fn CreateSolidBrush(colour: u32) -> HGDIOBJ;
    pub fn CreatePen(style: i32, width: i32, colour: u32) -> HGDIOBJ;
    pub fn RoundRect(hdc: HDC, l: i32, t: i32, r: i32, b: i32, w: i32, h: i32) -> BOOL;
    pub fn GetStockObject(index: i32) -> HGDIOBJ;
    pub fn GdiFlush() -> BOOL;
    pub fn Ellipse(hdc: HDC, l: i32, t: i32, r: i32, b: i32) -> BOOL;
    pub fn MoveToEx(hdc: HDC, x: i32, y: i32, old: *mut POINT) -> BOOL;
    pub fn LineTo(hdc: HDC, x: i32, y: i32) -> BOOL;
}

#[link(name = "comdlg32")]
extern "system" {
    pub fn GetOpenFileNameW(ofn: *mut OPENFILENAMEW) -> BOOL;
}

#[link(name = "shell32")]
extern "system" {
    pub fn SHBrowseForFolderW(bi: *const BROWSEINFOW) -> *mut c_void;
    pub fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> BOOL;
    pub fn DragAcceptFiles(hwnd: HWND, accept: BOOL);
    pub fn DragQueryFileW(drop: WPARAM, index: u32, file: *mut u16, len: u32) -> u32;
    pub fn DragFinish(drop: WPARAM);
    pub fn ShellExecuteW(hwnd: HWND, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
}

#[link(name = "ole32")]
extern "system" {
    pub fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
    pub fn CoTaskMemFree(p: *const c_void);
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

pub fn wide_os(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain([0]).collect()
}

pub fn from_wide(buf: &[u16]) -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    std::path::PathBuf::from(std::ffi::OsString::from_wide(&buf[..end]))
}

/// a COLORREF from r, g, b.
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// the file picker: a file path, or none when cancelled.
pub fn pick_file(owner: HWND, title: &str, filter: &[(&str, &str)]) -> Option<std::path::PathBuf> {
    let mut filter_w: Vec<u16> = Vec::new();
    for (name, pattern) in filter {
        filter_w.extend(name.encode_utf16());
        filter_w.push(0);
        filter_w.extend(pattern.encode_utf16());
        filter_w.push(0);
    }
    filter_w.push(0);
    let title_w = wide(title);
    let mut buf = vec![0u16; 4096];
    // SAFETY: a zeroed OPENFILENAMEW is valid; every pointer outlives the call
    unsafe {
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter_w.as_ptr();
        ofn.nFilterIndex = 1;
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title_w.as_ptr();
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_HIDEREADONLY;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    Some(from_wide(&buf))
}

/// the folder picker.
pub fn pick_folder(owner: HWND, title: &str) -> Option<std::path::PathBuf> {
    let title_w = wide(title);
    let mut name = vec![0u16; 1024];
    // SAFETY: a zeroed BROWSEINFOW is valid; the PIDL is freed after use
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED);
        let mut bi: BROWSEINFOW = std::mem::zeroed();
        bi.hwndOwner = owner;
        bi.pszDisplayName = name.as_mut_ptr();
        bi.lpszTitle = title_w.as_ptr();
        bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut path = vec![0u16; 4096];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        CoTaskMemFree(pidl);
        (ok != 0).then(|| from_wide(&path))
    }
}
