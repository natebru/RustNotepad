use rustnotepad::{Result, wide};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    UI::{
        Accessibility::*,
        Controls::*,
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    r | g << 8 | b << 16
}
pub const BACKGROUND: COLORREF = rgb(48, 56, 65);
pub const FOREGROUND: COLORREF = rgb(232, 237, 242);
const CHROME: COLORREF = rgb(62, 70, 80);
const INACTIVE: COLORREF = rgb(79, 87, 97);
const MUTED: COLORREF = rgb(188, 199, 211);
const HIGHLIGHT: COLORREF = rgb(79, 102, 126);
const ACCENT: COLORREF = rgb(139, 174, 132);
const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
const EM_SETCHARFORMAT: u32 = WM_USER + 68;
const EM_GETCHARFORMAT: u32 = WM_USER + 58;
const EM_SETEVENTMASK: u32 = WM_USER + 69;
const CFM_COLOR: u32 = 0x40000000;
const SCF_ALL: usize = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct CharFormat {
    size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    color: COLORREF,
    charset: u8,
    pitch: u8,
    face: [u16; 32],
}

struct Brush(HBRUSH);
impl Drop for Brush {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

thread_local! {
    static DARK: Cell<bool> = const { Cell::new(false) };
    static MENU_BRUSH: RefCell<Option<Brush>> = const { RefCell::new(None) };
    static LABELS: RefCell<HashMap<usize, (String, bool)>> = RefCell::new(HashMap::new());
}

pub fn is_dark() -> bool {
    DARK.get()
}

pub fn effective_dark(requested: bool, high_contrast: bool) -> bool {
    requested && !high_contrast
}

pub fn configure(requested: bool) -> Result<()> {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    if unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            &mut hc as *mut _ as *mut c_void,
            0,
        )
    } == 0
    {
        return Err(format!(
            "Read Windows high-contrast setting: {}",
            std::io::Error::last_os_error()
        ));
    }
    let dark = effective_dark(requested, hc.dwFlags & HCF_HIGHCONTRASTON != 0);
    if dark {
        MENU_BRUSH.with(|slot| {
            if slot.borrow().is_none() {
                let brush = unsafe { CreateSolidBrush(CHROME) };
                if brush.is_null() {
                    return Err("Cannot create dark menu background.".to_string());
                }
                *slot.borrow_mut() = Some(Brush(brush));
            }
            Ok(())
        })?;
    }
    DARK.set(dark);
    Ok(())
}

pub fn title_bar(hwnd: HWND) -> Result<()> {
    let dark = is_dark();
    for (attribute, value) in [
        (DWMWA_USE_IMMERSIVE_DARK_MODE, dark as u32),
        (
            DWMWA_CAPTION_COLOR,
            if dark { CHROME } else { DWMWA_COLOR_DEFAULT },
        ),
        (
            DWMWA_TEXT_COLOR,
            if dark {
                FOREGROUND
            } else {
                DWMWA_COLOR_DEFAULT
            },
        ),
    ] {
        let hr = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                &value as *const _ as *const c_void,
                size_of::<u32>() as u32,
            )
        };
        // Older Windows builds do not support the optional caption color attributes.
        if hr < 0 && hr != E_INVALIDARG && hr != 0x80263001u32 as i32 {
            return Err(format!(
                "Set window title appearance: HRESULT 0x{:08x}",
                hr as u32
            ));
        }
    }
    Ok(())
}

pub fn editor(hwnd: HWND) -> Result<()> {
    editor_colors(
        hwnd,
        if is_dark() {
            FOREGROUND
        } else {
            unsafe { GetSysColor(COLOR_WINDOWTEXT) }
        },
    )?;
    unsafe {
        SendMessageW(
            hwnd,
            EM_SETBKGNDCOLOR,
            (!is_dark()) as usize,
            BACKGROUND as isize,
        );
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let updated = if is_dark() {
            style & !(WS_EX_CLIENTEDGE as i32)
        } else {
            style | WS_EX_CLIENTEDGE as i32
        };
        if updated != style {
            SetWindowLongW(hwnd, GWL_EXSTYLE, updated);
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        InvalidateRect(hwnd, null(), 1);
    }
    Ok(())
}

fn editor_colors(hwnd: HWND, color: COLORREF) -> Result<()> {
    unsafe {
        let format = CharFormat {
            size: size_of::<CharFormat>() as u32,
            mask: CFM_COLOR,
            color,
            ..zeroed()
        };
        let modified = SendMessageW(hwnd, EM_GETMODIFY, 0, 0);
        let events = SendMessageW(hwnd, EM_SETEVENTMASK, 0, 0);
        let all = SendMessageW(
            hwnd,
            EM_SETCHARFORMAT,
            SCF_ALL,
            &format as *const _ as isize,
        );
        let default = SendMessageW(hwnd, EM_SETCHARFORMAT, 0, &format as *const _ as isize);
        SendMessageW(hwnd, EM_SETMODIFY, modified as usize, 0);
        SendMessageW(hwnd, EM_SETEVENTMASK, 0, events);
        if all == 0 || default == 0 {
            return Err("The native editor could not apply the text color.".into());
        }
    }
    Ok(())
}

pub fn editor_foreground(hwnd: HWND) -> COLORREF {
    unsafe {
        let mut format: CharFormat = zeroed();
        format.size = size_of::<CharFormat>() as u32;
        SendMessageW(hwnd, EM_GETCHARFORMAT, 0, &mut format as *mut _ as isize);
        format.color
    }
}

pub fn with_print_colors<T>(hwnd: HWND, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let color = editor_foreground(hwnd);
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    if visible {
        unsafe {
            SendMessageW(hwnd, WM_SETREDRAW, 0, 0);
        }
    }
    let result = editor_colors(hwnd, 0).and_then(|()| operation());
    let restore = editor_colors(hwnd, color);
    if visible {
        unsafe {
            SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
            RedrawWindow(
                hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_FRAME | RDW_ERASE,
            );
        }
    }
    match (result, restore) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(e), Ok(())) | (Ok(_), Err(e)) => Err(e),
        (Err(a), Err(b)) => Err(format!("{a}\nRestoring editor appearance: {b}")),
    }
}

pub fn menu(menu: HMENU, top: bool) -> Result<()> {
    unsafe {
        let brush = if is_dark() {
            MENU_BRUSH.with(|b| b.borrow().as_ref().map_or(null_mut(), |b| b.0))
        } else {
            GetSysColorBrush(COLOR_MENU)
        };
        let info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_BACKGROUND,
            hbrBack: brush,
            ..zeroed()
        };
        if SetMenuInfo(menu, &info) == 0 {
            return Err("Cannot set menu background.".into());
        }
        for position in 0..GetMenuItemCount(menu) {
            let mut label = vec![0u16; 512];
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_FTYPE | MIIM_ID | MIIM_SUBMENU,
                dwTypeData: label.as_mut_ptr(),
                cch: label.len() as u32,
                ..zeroed()
            };
            if GetMenuItemInfoW(menu, position as u32, 1, &mut item) == 0 {
                return Err("Cannot read menu appearance.".into());
            }
            if !item.hSubMenu.is_null() {
                self::menu(item.hSubMenu, false)?;
            }
            if item.fType & MFT_SEPARATOR != 0 {
                continue;
            }
            let key = if top {
                0x10000 + position as usize
            } else {
                item.wID as usize
            };
            LABELS.with(|labels| {
                labels.borrow_mut().insert(
                    key,
                    (String::from_utf16_lossy(&label[..item.cch as usize]), top),
                );
            });
            let set = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA,
                fType: if is_dark() {
                    item.fType | MFT_OWNERDRAW
                } else {
                    item.fType & !MFT_OWNERDRAW
                },
                dwItemData: key,
                ..zeroed()
            };
            if SetMenuItemInfoW(menu, position as u32, 1, &set) == 0 {
                return Err("Cannot update menu appearance.".into());
            }
        }
    }
    Ok(())
}

unsafe fn fill(dc: HDC, rect: &RECT, color: COLORREF) {
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, rect, GetStockObject(DC_BRUSH) as HBRUSH);
    }
}

unsafe fn label(dc: HDC, text: &str, rect: &mut RECT, color: COLORREF, flags: u32) {
    unsafe {
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        DrawTextW(
            dc,
            wide(text).as_ptr(),
            -1,
            rect,
            flags | DT_SINGLELINE | DT_VCENTER,
        );
    }
}

pub fn paint_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !is_dark() {
        return None;
    }
    unsafe {
        match msg {
            WM_MENUCHAR => {
                let menu = lp as HMENU;
                let key = char::from_u32(wp as u16 as u32)?.to_lowercase().next()?;
                for position in 0..GetMenuItemCount(menu) {
                    let mut item = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_DATA | MIIM_STATE,
                        ..zeroed()
                    };
                    if GetMenuItemInfoW(menu, position as u32, 1, &mut item) == 0
                        || item.fState & MFS_DISABLED != 0
                    {
                        continue;
                    }
                    let text = LABELS.with(|labels| labels.borrow().get(&item.dwItemData).cloned());
                    if let Some((text, _)) = text {
                        let mut chars = text.chars();
                        while let Some(c) = chars.next() {
                            if c == '&'
                                && let Some(c) = chars.next()
                                && c != '&'
                                && c.to_lowercase().next() == Some(key)
                            {
                                return Some(position as isize | (MNC_EXECUTE as isize) << 16);
                            }
                        }
                    }
                }
                Some((MNC_IGNORE as isize) << 16)
            }
            WM_NOTIFY => {
                if (*(lp as *const NMHDR)).code != NM_CUSTOMDRAW {
                    return None;
                }
                let draw = &*(lp as *const NMCUSTOMDRAW);
                let mut class = [0u16; 32];
                let count =
                    GetClassNameW(draw.hdr.hwndFrom, class.as_mut_ptr(), class.len() as i32);
                if String::from_utf16_lossy(&class[..count.max(0) as usize]) != "Button" {
                    return None;
                }
                if draw.dwDrawStage != CDDS_PREPAINT {
                    return Some(CDRF_DODEFAULT as isize);
                }
                let button = draw.hdr.hwndFrom;
                let saved = SaveDC(draw.hdc);
                let style = GetWindowLongW(button, GWL_STYLE) as u32 & BS_TYPEMASK as u32;
                let checkbox = style == BS_AUTOCHECKBOX as u32;
                let mut rect = draw.rc;
                fill(
                    draw.hdc,
                    &rect,
                    if checkbox {
                        BACKGROUND
                    } else if draw.uItemState & (CDIS_SELECTED | CDIS_HOT) != 0 {
                        HIGHLIGHT
                    } else {
                        INACTIVE
                    },
                );
                let mut text = vec![0u16; GetWindowTextLengthW(button).max(0) as usize + 1];
                GetWindowTextW(button, text.as_mut_ptr(), text.len() as i32);
                let font = SendMessageW(button, WM_GETFONT, 0, 0) as HFONT;
                SelectObject(draw.hdc, font);
                if checkbox {
                    let side = 14 * GetDpiForWindow(button).max(96) as i32 / 96;
                    let mut box_rect = RECT {
                        left: rect.left + 2,
                        top: (rect.top + rect.bottom - side) / 2,
                        right: rect.left + 2 + side,
                        bottom: (rect.top + rect.bottom + side) / 2,
                    };
                    fill(draw.hdc, &box_rect, INACTIVE);
                    if SendMessageW(button, BM_GETCHECK, 0, 0) != 0 {
                        label(
                            draw.hdc,
                            "\u{2713}",
                            &mut box_rect,
                            FOREGROUND,
                            DT_CENTER | DT_NOPREFIX,
                        );
                    }
                    rect.left += side + 8;
                }
                label(
                    draw.hdc,
                    &String::from_utf16_lossy(&text[..text.len() - 1]),
                    &mut rect,
                    FOREGROUND,
                    if checkbox { DT_LEFT } else { DT_CENTER },
                );
                if draw.uItemState & CDIS_FOCUS != 0 {
                    InflateRect(&mut rect, -3, -3);
                    DrawFocusRect(draw.hdc, &rect);
                }
                RestoreDC(draw.hdc, saved);
                Some(CDRF_SKIPDEFAULT as isize)
            }
            WM_ERASEBKGND => {
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect);
                fill(wp as HDC, &rect, BACKGROUND);
                Some(1)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN => {
                let dc = wp as HDC;
                SetTextColor(dc, FOREGROUND);
                SetBkColor(dc, BACKGROUND);
                SetDCBrushColor(dc, BACKGROUND);
                Some(GetStockObject(DC_BRUSH) as isize)
            }
            WM_MEASUREITEM => {
                let item = &mut *(lp as *mut MEASUREITEMSTRUCT);
                if item.CtlType != ODT_MENU {
                    return None;
                }
                let entry = LABELS.with(|labels| labels.borrow().get(&item.itemData).cloned())?;
                let dc = GetDC(hwnd);
                let old = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
                let text = wide(&entry.0.replace('&', "").replace('\t', "    "));
                let mut size = SIZE::default();
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                item.itemWidth = (size.cx + (if entry.1 { 16 } else { 64 }) * dpi / 96) as u32;
                item.itemHeight = (size.cy + 10 * dpi / 96) as u32;
                SelectObject(dc, old);
                ReleaseDC(hwnd, dc);
                Some(1)
            }
            WM_DRAWITEM => {
                let item = &*(lp as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_MENU {
                    return None;
                }
                let (text, top) =
                    LABELS.with(|labels| labels.borrow().get(&item.itemData).cloned())?;
                let dc = item.hDC;
                let saved = SaveDC(dc);
                SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
                fill(
                    dc,
                    &item.rcItem,
                    if item.itemState & (ODS_SELECTED | ODS_HOTLIGHT) != 0 {
                        HIGHLIGHT
                    } else {
                        CHROME
                    },
                );
                let color = if item.itemState & (ODS_GRAYED | ODS_DISABLED) != 0 {
                    MUTED
                } else {
                    FOREGROUND
                };
                let mut rect = item.rcItem;
                let padding =
                    (if top { 8 } else { 28 }) * GetDpiForWindow(hwnd).max(96) as i32 / 96;
                rect.left += padding;
                rect.right -= 12;
                let (name, shortcut) = text.split_once('\t').unwrap_or((&text, ""));
                label(
                    dc,
                    name,
                    &mut rect,
                    color,
                    DT_LEFT
                        | if item.itemState & ODS_NOACCEL != 0 {
                            DT_HIDEPREFIX
                        } else {
                            0
                        },
                );
                if !shortcut.is_empty() {
                    label(dc, shortcut, &mut rect, MUTED, DT_RIGHT | DT_NOPREFIX);
                }
                if item.itemState & ODS_CHECKED != 0 {
                    let mut check = item.rcItem;
                    check.right = check.left + padding;
                    label(
                        dc,
                        "\u{2713}",
                        &mut check,
                        FOREGROUND,
                        DT_CENTER | DT_NOPREFIX,
                    );
                }
                RestoreDC(dc, saved);
                Some(1)
            }
            _ => None,
        }
    }
}

pub fn install_chrome(tabs: HWND, status: HWND) -> Result<()> {
    unsafe {
        if SetWindowSubclass(tabs, Some(chrome_proc), 2, 1) == 0
            || SetWindowSubclass(status, Some(chrome_proc), 2, 0) == 0
        {
            return Err("Cannot initialize themed tab/status controls.".into());
        }
    }
    Ok(())
}

unsafe extern "system" fn chrome_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    tabs: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(chrome_proc), 2);
        }
        if is_dark() {
            if msg == WM_ERASEBKGND {
                return 1;
            }
            if msg == WM_PAINT {
                let mut ps: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                paint_chrome(hwnd, dc, tabs != 0);
                EndPaint(hwnd, &ps);
                return 0;
            }
            if msg == WM_PRINTCLIENT {
                paint_chrome(hwnd, wp as HDC, tabs != 0);
                return 0;
            }
        }
        let result = DefSubclassProc(hwnd, msg, wp, lp);
        if is_dark()
            && [
                TCM_SETCURSEL,
                TCM_SETITEMW,
                TCM_INSERTITEMW,
                TCM_DELETEITEM,
                SB_SETTEXTW,
                WM_SETFOCUS,
                WM_KILLFOCUS,
            ]
            .contains(&msg)
        {
            InvalidateRect(hwnd, null(), 0);
        }
        result
    }
}

unsafe fn paint_chrome(hwnd: HWND, dc: HDC, tabs: bool) {
    unsafe {
        let saved = SaveDC(dc);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        fill(dc, &client, CHROME);
        let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        SelectObject(
            dc,
            if font.is_null() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                font
            },
        );
        if tabs {
            let active = SendMessageW(hwnd, TCM_GETCURSEL, 0, 0);
            for i in 0..SendMessageW(hwnd, TCM_GETITEMCOUNT, 0, 0) {
                let mut rect = RECT::default();
                if SendMessageW(
                    hwnd,
                    TCM_GETITEMRECT,
                    i as usize,
                    &mut rect as *mut _ as isize,
                ) == 0
                {
                    continue;
                }
                if rect.right < 0 || rect.left > client.right {
                    continue;
                }
                let mut buffer = vec![0u16; 512];
                let mut item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: buffer.as_mut_ptr(),
                    cchTextMax: buffer.len() as i32,
                    ..zeroed()
                };
                SendMessageW(hwnd, TCM_GETITEMW, i as usize, &mut item as *mut _ as isize);
                let n = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                let title = String::from_utf16_lossy(&buffer[..n]);
                rect.right -= 1;
                rect.bottom = client.bottom;
                fill(dc, &rect, if i == active { BACKGROUND } else { INACTIVE });
                let mut text = rect;
                text.left += 10;
                text.right -= 10;
                if title.starts_with('*') {
                    text.right -= 12;
                    let dot = RECT {
                        left: rect.right - 12,
                        top: (rect.top + rect.bottom) / 2 - 2,
                        right: rect.right - 7,
                        bottom: (rect.top + rect.bottom) / 2 + 3,
                    };
                    fill(dc, &dot, ACCENT);
                }
                label(
                    dc,
                    title.strip_prefix('*').unwrap_or(&title),
                    &mut text,
                    if i == active { FOREGROUND } else { MUTED },
                    DT_LEFT | DT_END_ELLIPSIS,
                );
                if i == active
                    && windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd
                {
                    DrawFocusRect(dc, &rect);
                }
            }
        } else {
            let length = SendMessageW(hwnd, SB_GETTEXTLENGTHW, 0, 0) as usize & 0xffff;
            let mut text = vec![0u16; length + 1];
            SendMessageW(hwnd, SB_GETTEXTW, 0, text.as_mut_ptr() as isize);
            client.left += 6;
            label(
                dc,
                &String::from_utf16_lossy(&text[..length]),
                &mut client,
                MUTED,
                DT_LEFT | DT_NOPREFIX,
            );
        }
        RestoreDC(dc, saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn high_contrast_overrides_only_the_effective_theme() {
        assert!(effective_dark(true, false));
        assert!(!effective_dark(true, true));
        assert!(!effective_dark(false, false));
        assert!(!effective_dark(false, true));
        assert_eq!(size_of::<CharFormat>(), 92);
    }
}
