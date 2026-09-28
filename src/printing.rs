use crate::ui::{CharRange, selection};
use rustnotepad::{Result, wide};
use std::{
    cell::Cell,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    Storage::Xps::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::Dialogs::*, Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*},
};

const EM_FORMATRANGE: u32 = WM_USER + 57;

#[repr(C)]
struct FormatRange {
    hdc: HDC,
    target: HDC,
    area: RECT,
    page: RECT,
    chars: CharRange,
}

thread_local! { static CANCELED: Cell<bool> = const { Cell::new(false) }; }

#[derive(Default)]
pub struct Printer {
    devmode: HGLOBAL,
    devnames: HGLOBAL,
    margins: Option<RECT>,
}

impl Drop for Printer {
    fn drop(&mut self) {
        unsafe {
            if !self.devmode.is_null() {
                GlobalFree(self.devmode);
            }
            if !self.devnames.is_null() {
                GlobalFree(self.devnames);
            }
        }
    }
}

unsafe extern "system" fn progress_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if let Some(result) = crate::theme::paint_message(hwnd, msg, wp, lp) {
            return result;
        }
        if msg == WM_CLOSE || (msg == WM_COMMAND && wp & 0xffff == 1) {
            CANCELED.set(true);
            0
        } else {
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

unsafe extern "system" fn abort_proc(_: HDC, _: i32) -> i32 {
    unsafe {
        let mut msg: MSG = zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                CANCELED.set(true);
                PostQuitMessage(msg.wParam as i32);
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    (!CANCELED.get()) as i32
}

impl Printer {
    pub fn setup(&mut self, owner: HWND) -> Result<()> {
        unsafe {
            let mut page = PAGESETUPDLGW {
                lStructSize: size_of::<PAGESETUPDLGW>() as u32,
                hwndOwner: owner,
                hDevMode: self.devmode,
                hDevNames: self.devnames,
                Flags: PSD_INTHOUSANDTHSOFINCHES | PSD_MARGINS,
                rtMargin: self.margins.unwrap_or(RECT {
                    left: 1000,
                    top: 1000,
                    right: 1000,
                    bottom: 1000,
                }),
                ..zeroed()
            };
            let ok = PageSetupDlgW(&mut page);
            self.devmode = page.hDevMode;
            self.devnames = page.hDevNames;
            if ok != 0 {
                self.margins = Some(page.rtMargin);
            } else {
                let code = CommDlgExtendedError();
                if code != 0 {
                    return Err(format!("Page setup failed (0x{code:08x})."));
                }
            }
        }
        Ok(())
    }

    pub fn print(&mut self, owner: HWND, editor: HWND, title: &str) -> Result<()> {
        unsafe {
            let selected = selection(editor);
            let mut dialog = PRINTDLGW {
                lStructSize: size_of::<PRINTDLGW>() as u32,
                hwndOwner: owner,
                hDevMode: self.devmode,
                hDevNames: self.devnames,
                Flags: PD_RETURNDC
                    | PD_USEDEVMODECOPIESANDCOLLATE
                    | if selected.min == selected.max {
                        PD_NOSELECTION
                    } else {
                        0
                    },
                nFromPage: 1,
                nToPage: 1,
                nMinPage: 1,
                nMaxPage: u16::MAX,
                nCopies: 1,
                ..zeroed()
            };
            let ok = PrintDlgW(&mut dialog);
            self.devmode = dialog.hDevMode;
            self.devnames = dialog.hDevNames;
            if ok == 0 {
                let code = CommDlgExtendedError();
                if code != 0 {
                    return Err(format!("Print dialog failed (0x{code:08x})."));
                }
                return Ok(());
            }
            let dc = dialog.hDC;
            if dc.is_null() {
                return Err("Printer did not supply a device context.".into());
            }
            CANCELED.set(false);
            let progress = match create_progress(owner) {
                Ok(hwnd) => hwnd,
                Err(e) => {
                    DeleteDC(dc);
                    return Err(e);
                }
            };
            EnableWindow(owner, 0);
            let result = crate::theme::with_print_colors(editor, || {
                self.render(
                    editor,
                    dc,
                    title,
                    if dialog.Flags & PD_SELECTION != 0 {
                        selected
                    } else {
                        CharRange { min: 0, max: -1 }
                    },
                    if dialog.Flags & PD_PAGENUMS != 0 {
                        Some((dialog.nFromPage as usize, dialog.nToPage as usize))
                    } else {
                        None
                    },
                    None,
                )
            });
            SendMessageW(editor, EM_FORMATRANGE, 0, 0);
            DeleteDC(dc);
            EnableWindow(owner, 1);
            DestroyWindow(progress);
            SetForegroundWindow(owner);
            result
        }
    }

    unsafe fn render(
        &self,
        editor: HWND,
        dc: HDC,
        title: &str,
        chars: CharRange,
        range: Option<(usize, usize)>,
        output: Option<&str>,
    ) -> Result<()> {
        unsafe {
            let xdpi = GetDeviceCaps(dc, LOGPIXELSX as i32);
            let ydpi = GetDeviceCaps(dc, LOGPIXELSY as i32);
            if xdpi <= 0 || ydpi <= 0 {
                return Err("Printer reported invalid resolution.".into());
            }
            let twips = |value: i32, dpi: i32| (value as i64 * 1440 / dpi as i64) as i32;
            let page = RECT {
                left: 0,
                top: 0,
                right: twips(GetDeviceCaps(dc, PHYSICALWIDTH as i32), xdpi),
                bottom: twips(GetDeviceCaps(dc, PHYSICALHEIGHT as i32), ydpi),
            };
            let margins = self.margins.unwrap_or(RECT {
                left: 1000,
                top: 1000,
                right: 1000,
                bottom: 1000,
            });
            let offset_x = twips(GetDeviceCaps(dc, PHYSICALOFFSETX as i32), xdpi);
            let offset_y = twips(GetDeviceCaps(dc, PHYSICALOFFSETY as i32), ydpi);
            let area = RECT {
                left: (margins.left * 1440 / 1000 - offset_x).max(0),
                top: (margins.top * 1440 / 1000 - offset_y).max(0),
                right: (page.right - margins.right * 1440 / 1000 - offset_x)
                    .min(twips(GetDeviceCaps(dc, HORZRES as i32), xdpi)),
                bottom: (page.bottom - margins.bottom * 1440 / 1000 - offset_y)
                    .min(twips(GetDeviceCaps(dc, VERTRES as i32), ydpi)),
            };
            if area.right <= area.left || area.bottom <= area.top {
                return Err("Margins leave no printable area.".into());
            }
            let end = if chars.max < 0 {
                crate::ui::text(editor)?.encode_utf16().count() as i32
            } else {
                chars.max
            };
            let mut start = chars.min;
            let mut pages = Vec::new();
            loop {
                if abort_proc(dc, 0) == 0 {
                    return Ok(());
                }
                let format = FormatRange {
                    hdc: dc,
                    target: dc,
                    area,
                    page,
                    chars: CharRange {
                        min: start,
                        max: end,
                    },
                };
                let next =
                    SendMessageW(editor, EM_FORMATRANGE, 0, &format as *const _ as isize) as i32;
                if end > start && next <= start {
                    return Err("Printer could not paginate this document.".into());
                }
                pages.push(CharRange {
                    min: start,
                    max: next.min(end),
                });
                if next >= end {
                    break;
                }
                start = next;
                if pages.len() >= u16::MAX as usize {
                    return Err("Document exceeds 65,535 printable pages.".into());
                }
            }
            let (from, to) = range.unwrap_or((1, pages.len()));
            if from == 0 || from > to || from > pages.len() {
                return Err(format!(
                    "Page range is outside the document ({} pages).",
                    pages.len()
                ));
            }
            let name = wide(title);
            let output = output.map(wide);
            let info = DOCINFOW {
                cbSize: size_of::<DOCINFOW>() as i32,
                lpszDocName: name.as_ptr(),
                lpszOutput: output.as_ref().map_or(null(), |s| s.as_ptr()),
                ..zeroed()
            };
            if SetAbortProc(dc, Some(abort_proc)) <= 0 {
                return Err("Printer did not accept cancellation callback.".into());
            }
            if StartDocW(dc, &info) <= 0 {
                return Err(format!(
                    "Start printing: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let result = (|| {
                for chars in pages.iter().take(to).skip(from - 1) {
                    if abort_proc(dc, 0) == 0 {
                        return Err("Printing canceled.".into());
                    }
                    if StartPage(dc) <= 0 {
                        return Err("Printer could not start a page.".into());
                    }
                    let format = FormatRange {
                        hdc: dc,
                        target: dc,
                        area,
                        page,
                        chars: *chars,
                    };
                    let next =
                        SendMessageW(editor, EM_FORMATRANGE, 1, &format as *const _ as isize);
                    if next < chars.max as isize {
                        return Err("Printer rendered an incomplete page.".into());
                    }
                    if EndPage(dc) <= 0 {
                        return Err("Printer could not finish a page.".into());
                    }
                }
                if EndDoc(dc) <= 0 {
                    return Err("Printer could not finish the job.".into());
                }
                Ok(())
            })();
            if result.is_err() {
                AbortDoc(dc);
            }
            result
        }
    }
}

impl Printer {
    pub fn test_pdf(
        &self,
        editor: HWND,
        output: &std::path::Path,
        chars: CharRange,
        pages: Option<(usize, usize)>,
    ) -> Result<()> {
        unsafe {
            let dc = CreateDCW(
                wide("WINSPOOL").as_ptr(),
                wide("Microsoft Print to PDF").as_ptr(),
                null(),
                null(),
            );
            if dc.is_null() {
                return Err("Microsoft Print to PDF is unavailable.".into());
            }
            CANCELED.set(false);
            let result = crate::theme::with_print_colors(editor, || {
                self.render(
                    editor,
                    dc,
                    "Rust Notepad print test",
                    chars,
                    pages,
                    Some(&output.to_string_lossy()),
                )
            });
            SendMessageW(editor, EM_FORMATRANGE, 0, 0);
            DeleteDC(dc);
            result
        }
    }
}

unsafe fn create_progress(owner: HWND) -> Result<HWND> {
    unsafe {
        let class = wide("RustNotepad.PrintProgress");
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            hInstance: instance,
            lpfnWndProc: Some(progress_proc),
            lpszClassName: class.as_ptr(),
            hbrBackground: (COLOR_BTNFACE + 1) as HBRUSH,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class.as_ptr(),
            wide("Printing...").as_ptr(),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            280,
            120,
            owner,
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err("Cannot create print cancellation window.".into());
        }
        if let Err(error) = crate::theme::title_bar(hwnd) {
            DestroyWindow(hwnd);
            return Err(error);
        }
        CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Cancel printing").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            40,
            20,
            180,
            30,
            hwnd,
            1usize as HMENU,
            instance,
            null(),
        );
        ShowWindow(hwnd, SW_SHOW);
        Ok(hwnd)
    }
}
