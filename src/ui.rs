use rustnotepad::{
    Result, TEXT_LIMIT,
    document::{Document, Text},
    encoding::{self, Encoding},
    file_io, search,
    session::{self, Session},
    wide,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ffi::c_void,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::mpsc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Globalization::GetACP,
    Graphics::Gdi::*,
    System::{DataExchange::*, LibraryLoader::*, Memory::*, Threading::*},
    UI::{
        Controls::Dialogs::*, Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*,
        WindowsAndMessaging::*,
    },
};

pub const EM_EXGETSEL_: u32 = WM_USER + 52;
pub const EM_EXSETSEL_: u32 = WM_USER + 55;
const EM_EXLIMITTEXT_: u32 = WM_USER + 53;
const EM_GETTEXTEX_: u32 = WM_USER + 94;
const EM_GETTEXTLENGTHEX_: u32 = WM_USER + 95;
const EM_SETTEXTEX_: u32 = WM_USER + 97;
const EM_SETTEXTMODE_: u32 = WM_USER + 89;
const EM_SETUNDOLIMIT_: u32 = WM_USER + 82;
const EM_SETTARGETDEVICE_: u32 = WM_USER + 72;
const EM_SETZOOM_: u32 = WM_USER + 225;
const EM_GETSCROLLPOS_: u32 = WM_USER + 221;
const EM_SETSCROLLPOS_: u32 = WM_USER + 222;
const EM_SETEVENTMASK_: u32 = WM_USER + 69;
const EM_GETTEXTRANGE_: u32 = WM_USER + 75;
const EM_REDO_: u32 = WM_USER + 84;
const CF_UNICODETEXT_: u32 = 13;
const CLASS: &str = "RustNotepad.Main.v1";
const WINDOW_TITLE: &str = "Rust Notepad";
const IPC_TAG: usize = 0x52534e50;

const NEW: usize = 101;
const OPEN: usize = 102;
const SAVE: usize = 103;
const SAVE_AS: usize = 104;
const CLOSE: usize = 105;
const EXIT: usize = 106;
const OPEN_ANSI: usize = 107;
const PRINT: usize = 108;
const PAGE_SETUP: usize = 109;
const UNDO: usize = 201;
const REDO: usize = 202;
const CUT: usize = 203;
const COPY: usize = 204;
const PASTE: usize = 205;
const DELETE: usize = 206;
const SELECT_ALL: usize = 207;
const FIND: usize = 208;
const NEXT: usize = 209;
const PREVIOUS: usize = 210;
const REPLACE: usize = 211;
const GOTO: usize = 212;
const DATETIME: usize = 213;
const WRAP: usize = 301;
const FONT: usize = 302;
const ZOOM_IN: usize = 401;
const ZOOM_OUT: usize = 402;
const ZOOM_RESET: usize = 403;
const STATUS: usize = 404;
const RESTORE: usize = 405;
const CLEAR_RECOVERY: usize = 406;
const NEXT_TAB: usize = 407;
const PREVIOUS_TAB: usize = 408;
const RETRY_RECOVERY: usize = 409;
const DARK_MODE: usize = 410;
const ABOUT: usize = 501;
const ENCODING_BASE: usize = 600;
const EOL_BASE: usize = 700;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CharRange {
    pub min: i32,
    pub max: i32,
}
#[repr(C)]
struct GetText {
    cb: u32,
    flags: u32,
    codepage: u32,
    default_char: *const u8,
    used: *mut i32,
}
#[repr(C)]
struct GetLength {
    flags: u32,
    codepage: u32,
}
#[repr(C)]
struct SetText {
    flags: u32,
    codepage: u32,
}

#[repr(C)]
struct TextRange {
    range: CharRange,
    text: *mut u16,
}

enum Event {
    Command(usize),
    Resize,
    ThemeChanged,
    Changed(HWND, Option<(usize, usize, bool)>),
    Status,
    SelectTab,
    Exit,
    Tick,
    Drop(HDROP),
    OpenPaths(Vec<PathBuf>),
    Insert(String),
    Error(String),
}

thread_local! {
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static SUPPRESS: Cell<bool> = const { Cell::new(false) };
    static CHECKPOINT_CURRENT: Cell<bool> = const { Cell::new(true) };
    static EDIT_HINT: Cell<Option<(usize, usize, bool)>> = const { Cell::new(None) };
    static HIGH_SURROGATE: Cell<Option<(HWND, u16)>> = const { Cell::new(None) };
}

fn enqueue(event: Event) {
    EVENTS.with(|q| q.borrow_mut().push_back(event));
}

pub fn error(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            owner,
            wide(text).as_ptr(),
            wide(WINDOW_TITLE).as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn ask(owner: HWND, text: &str, flags: u32) -> i32 {
    unsafe {
        MessageBoxW(
            owner,
            wide(text).as_ptr(),
            wide(WINDOW_TITLE).as_ptr(),
            flags,
        )
    }
}

fn os_error(operation: &str) -> String {
    format!("{operation}: {}", std::io::Error::last_os_error())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Commands queue owned events; painting never accesses App. Modal calls cannot reenter App mutably.
    unsafe {
        if let Some(result) = crate::theme::paint_message(hwnd, msg, wp, lp) {
            return result;
        }
        match msg {
            WM_COMMAND => {
                if (wp >> 16) as u32 == EN_CHANGE {
                    if !SUPPRESS.get() {
                        CHECKPOINT_CURRENT.set(false);
                        enqueue(Event::Changed(lp as HWND, EDIT_HINT.get()));
                    }
                } else if lp == 0 {
                    enqueue(Event::Command(wp & 0xffff));
                }
                0
            }
            WM_NOTIFY => {
                let hdr = &*(lp as *const NMHDR);
                if hdr.code == TCN_SELCHANGE {
                    enqueue(Event::SelectTab);
                } else {
                    enqueue(Event::Status);
                }
                0
            }
            WM_SIZE => {
                enqueue(Event::Resize);
                0
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
                enqueue(Event::ThemeChanged);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DPICHANGED => {
                let rect = &*(lp as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                enqueue(Event::Resize);
                0
            }
            WM_CLOSE => {
                enqueue(Event::Exit);
                0
            }
            WM_QUERYENDSESSION => CHECKPOINT_CURRENT.get() as isize,
            WM_ENDSESSION => {
                if wp != 0 {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_TIMER => {
                enqueue(Event::Tick);
                0
            }
            WM_DROPFILES => {
                enqueue(Event::Drop(wp as HDROP));
                0
            }
            WM_COPYDATA => {
                let data = &*(lp as *const COPYDATASTRUCT);
                if data.dwData != IPC_TAG || data.cbData > 1024 * 1024 || data.lpData.is_null() {
                    return 0;
                }
                let bytes =
                    std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize);
                match serde_json::from_slice::<Vec<PathBuf>>(bytes) {
                    Ok(paths) if paths.len() <= 256 => {
                        enqueue(Event::OpenPaths(paths));
                        1
                    }
                    _ => 0,
                }
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

pub fn text(hwnd: HWND) -> Result<String> {
    unsafe {
        let length = GetLength {
            flags: 8 | 2,
            codepage: 1200,
        };
        let n = SendMessageW(hwnd, EM_GETTEXTLENGTHEX_, &length as *const _ as usize, 0);
        if n < 0 || n as usize > TEXT_LIMIT / 2 {
            return Err("Editor text exceeds supported capacity.".into());
        }
        let mut buf = vec![0u16; n as usize + 1];
        let get = GetText {
            cb: (buf.len() * 2) as u32,
            flags: 0,
            codepage: 1200,
            default_char: null(),
            used: null_mut(),
        };
        let copied = SendMessageW(
            hwnd,
            EM_GETTEXTEX_,
            &get as *const _ as usize,
            buf.as_mut_ptr() as isize,
        );
        if copied < 0 || copied as usize != n as usize {
            return Err("Could not read complete editor text.".into());
        }
        buf.truncate(copied as usize);
        let raw = String::from_utf16(&buf)
            .map_err(|_| "Editor returned an incomplete Unicode character.")?;
        Ok(if raw.contains('\r') {
            Text::parse(&raw).body
        } else {
            raw
        })
    }
}

fn changed_body(
    editor: HWND,
    document: &Document,
    hint: Option<(usize, usize, bool)>,
) -> Result<String> {
    let Some((old_start, _, _)) = hint else {
        return text(editor);
    };
    let now = selection(editor);
    let old_len = rustnotepad::utf16_len(&document.text.body);
    let length = GetLength {
        flags: 8 | 2,
        codepage: 1200,
    };
    let new_len =
        unsafe { SendMessageW(editor, EM_GETTEXTLENGTHEX_, &length as *const _ as usize, 0) };
    if new_len < 0 || new_len as usize > TEXT_LIMIT / 2 {
        return Err("Editor text exceeds supported capacity.".into());
    }
    let start = old_start.min(now.min.max(0) as usize);
    let new_end = now.max.max(0) as usize;
    let old_end = old_len as isize - new_len + new_end as isize;
    if old_end < start as isize
        || old_end as usize > old_len
        || new_end < start
        || new_end > new_len as usize
    {
        return text(editor);
    }
    let mut units = vec![0u16; new_end - start + 1];
    let range = TextRange {
        range: CharRange {
            min: start as i32,
            max: new_end as i32,
        },
        text: units.as_mut_ptr(),
    };
    let n = unsafe { SendMessageW(editor, EM_GETTEXTRANGE_, 0, &range as *const _ as isize) };
    if n < 0 || n as usize != new_end - start {
        return text(editor);
    }
    let replacement = String::from_utf16(&units[..n as usize])
        .map_err(|_| "Incomplete Unicode character from editor.")?;
    let mut body = document.text.body.clone();
    let begin = search::utf16_to_byte(&body, start);
    let end = search::utf16_to_byte(&body, old_end as usize);
    body.replace_range(begin..end, &Text::parse(&replacement).body);
    if rustnotepad::utf16_len(&body) != new_len as usize {
        return text(editor);
    }
    Ok(body)
}

pub fn selection(hwnd: HWND) -> CharRange {
    let mut range = CharRange::default();
    unsafe {
        SendMessageW(hwnd, EM_EXGETSEL_, 0, &mut range as *mut _ as isize);
    }
    range
}

fn select(hwnd: HWND, range: CharRange) {
    unsafe {
        SendMessageW(hwnd, EM_EXSETSEL_, 0, &range as *const _ as isize);
    }
}

fn set_text(hwnd: HWND, body: &str) -> Result<()> {
    let native = wide(&body.replace('\n', "\r"));
    SUPPRESS.set(true);
    unsafe {
        let set = SetText {
            flags: 0,
            codepage: 1200,
        };
        SendMessageW(
            hwnd,
            EM_SETTEXTEX_,
            &set as *const _ as usize,
            native.as_ptr() as isize,
        );
    }
    SUPPRESS.set(false);
    if text(hwnd)? != body {
        return Err(
            "Native editor could not represent this text exactly; no file was written.".into(),
        );
    }
    Ok(())
}

fn clipboard_text(hwnd: HWND) -> Result<String> {
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return Err(os_error("Open clipboard"));
        }
        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT_);
            if handle.is_null() {
                return Err("Clipboard does not contain Unicode text.".into());
            }
            let bytes = GlobalSize(handle);
            if bytes > TEXT_LIMIT + 2 {
                return Err("Clipboard exceeds the text limit.".into());
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                return Err(os_error("Read clipboard"));
            }
            let slice = std::slice::from_raw_parts(ptr, bytes / 2);
            let end = slice.iter().position(|&c| c == 0);
            let result = match end {
                Some(end) => String::from_utf16(&slice[..end])
                    .map_err(|_| "Clipboard contains invalid UTF-16.".into()),
                None => Err("Clipboard text is not terminated.".into()),
            };
            GlobalUnlock(handle);
            result
        })();
        CloseClipboard();
        result
    }
}

fn copy_text(hwnd: HWND, text: &str) -> Result<()> {
    unsafe {
        let units = wide(&text.replace('\n', "\r\n"));
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if memory.is_null() {
            return Err(os_error("Allocate clipboard text"));
        }
        let ptr = GlobalLock(memory) as *mut u16;
        if ptr.is_null() {
            GlobalFree(memory);
            return Err(os_error("Lock clipboard text"));
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
        GlobalUnlock(memory);
        if OpenClipboard(hwnd) == 0 {
            GlobalFree(memory);
            return Err(os_error("Open clipboard"));
        }
        let result = if EmptyClipboard() == 0 || SetClipboardData(CF_UNICODETEXT_, memory).is_null()
        {
            GlobalFree(memory);
            Err(os_error("Copy text to clipboard"))
        } else {
            Ok(())
        };
        CloseClipboard();
        result
    }
}

unsafe extern "system" fn editor_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        if let Some(result) = crate::theme::paint_message(hwnd, msg, wp, lp) {
            return result;
        }
        match msg {
            WM_CHAR if (0xd800..=0xdbff).contains(&wp) => {
                HIGH_SURROGATE.set(Some((hwnd, wp as u16)));
                0
            }
            WM_CHAR if (0xdc00..=0xdfff).contains(&wp) => {
                match HIGH_SURROGATE.take() {
                    Some((owner, high)) if owner == hwnd => {
                        match String::from_utf16(&[high, wp as u16]) {
                            Ok(text) => enqueue(Event::Insert(text)),
                            Err(_) => {
                                enqueue(Event::Error("Invalid Unicode input was rejected.".into()))
                            }
                        }
                    }
                    _ => enqueue(Event::Error(
                        "Incomplete Unicode input was rejected.".into(),
                    )),
                }
                0
            }
            WM_UNDO => {
                enqueue(Event::Command(UNDO));
                0
            }
            EM_REDO_ => {
                enqueue(Event::Command(REDO));
                0
            }
            WM_PASTE => {
                enqueue(Event::Command(PASTE));
                0
            }
            WM_CUT => {
                enqueue(Event::Command(CUT));
                0
            }
            WM_COPY => {
                enqueue(Event::Command(COPY));
                0
            }
            WM_CLEAR => {
                enqueue(Event::Command(DELETE));
                0
            }
            WM_CONTEXTMENU => {
                let menu = CreatePopupMenu();
                for (id, label) in [
                    (UNDO, "Undo"),
                    (REDO, "Redo"),
                    (CUT, "Cut"),
                    (COPY, "Copy"),
                    (PASTE, "Paste"),
                    (DELETE, "Delete"),
                    (SELECT_ALL, "Select All"),
                ] {
                    AppendMenuW(menu, MF_STRING, id, wide(label).as_ptr());
                }
                if let Err(error) = crate::theme::menu(menu, false) {
                    DestroyMenu(menu);
                    enqueue(Event::Error(error));
                    return 0;
                }
                let mut point = POINT {
                    x: (lp as u16) as i16 as i32,
                    y: ((lp >> 16) as u16) as i16 as i32,
                };
                if lp == -1 {
                    GetCaretPos(&mut point);
                    ClientToScreen(hwnd, &mut point);
                }
                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    0,
                    hwnd,
                    null(),
                );
                DestroyMenu(menu);
                if command != 0 {
                    enqueue(Event::Command(command as usize));
                }
                0
            }
            WM_KEYDOWN if GetKeyState(VK_CONTROL as i32) < 0 && wp == b'Z' as usize => {
                enqueue(Event::Command(UNDO));
                0
            }
            WM_KEYDOWN if GetKeyState(VK_CONTROL as i32) < 0 && wp == b'Y' as usize => {
                enqueue(Event::Command(REDO));
                0
            }
            WM_CHAR if wp == 26 || wp == 25 || wp == 22 => 0,
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(editor_proc), 1);
                DefSubclassProc(hwnd, msg, wp, lp)
            }
            _ => {
                let editing = msg == WM_CHAR || msg == WM_KEYDOWN || msg == 0x010f;
                if editing {
                    let before = selection(hwnd);
                    EDIT_HINT.set(Some((
                        before.min.max(0) as usize,
                        before.max.max(0) as usize,
                        wp == VK_BACK as usize,
                    )));
                }
                let result = DefSubclassProc(hwnd, msg, wp, lp);
                if editing {
                    EDIT_HINT.set(None);
                }
                result
            }
        }
    }
}

fn create_editor(parent: HWND, document: &Document) -> Result<HWND> {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            wide("RICHEDIT50W").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | ES_AUTOHSCROLL as u32
                | ES_NOHIDESEL as u32,
            0,
            0,
            100,
            100,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            return Err(os_error("Create native editor"));
        }
        if SendMessageW(hwnd, EM_SETTEXTMODE_, 1 | 8 | 32, 0) != 0 {
            DestroyWindow(hwnd);
            return Err("Could not enable plain-text mode.".into());
        }
        SendMessageW(hwnd, EM_SETUNDOLIMIT_, 0, 0);
        SendMessageW(hwnd, EM_EXLIMITTEXT_, 0, (TEXT_LIMIT / 2) as isize);
        if let Err(e) = set_text(hwnd, &document.text.body) {
            DestroyWindow(hwnd);
            return Err(e);
        }
        SendMessageW(hwnd, EM_SETEVENTMASK_, 0, 1 | 0x80000);
        if SetWindowSubclass(hwnd, Some(editor_proc), 1, 0) == 0 {
            DestroyWindow(hwnd);
            return Err(os_error("Subclass editor"));
        }
        Ok(hwnd)
    }
}

struct App {
    hwnd: HWND,
    tabs: HWND,
    status: HWND,
    editors: Vec<HWND>,
    session: Session,
    root: PathBuf,
    font: HFONT,
    find: String,
    replacement: String,
    match_case: bool,
    search_wrap: bool,
    pending: bool,
    checkpoint_error: bool,
    last_checkpoint: Instant,
    printer: crate::printing::Printer,
    revision: u64,
    worker: Option<(u64, mpsc::Receiver<Result<()>>)>,
}

impl App {
    fn apply_theme(&mut self) -> Result<()> {
        crate::theme::configure(self.session.settings.dark_mode)?;
        crate::theme::title_bar(self.hwnd)?;
        for &editor in &self.editors {
            crate::theme::editor(editor)?;
        }
        crate::theme::menu(unsafe { GetMenu(self.hwnd) }, true)?;
        self.layout();
        unsafe {
            DrawMenuBar(self.hwnd);
            RedrawWindow(
                self.hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
            );
        }
        Ok(())
    }

    fn active(&self) -> usize {
        self.session.active
    }
    fn editor(&self) -> HWND {
        self.editors[self.active()]
    }
    fn other_bytes(&self, index: usize) -> usize {
        self.session
            .documents
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, d)| d.text.bytes())
            .sum()
    }
    fn mark_pending(&mut self) {
        self.pending = true;
        self.revision = self.revision.wrapping_add(1);
        CHECKPOINT_CURRENT.set(false);
    }

    fn snapshot_view(&mut self) {
        for (i, &editor) in self.editors.iter().enumerate() {
            let s = selection(editor);
            let mut point = POINT::default();
            unsafe {
                SendMessageW(editor, EM_GETSCROLLPOS_, 0, &mut point as *mut _ as isize);
            }
            let doc = &mut self.session.documents[i];
            doc.selection = (s.min, s.max);
            doc.scroll = (point.x, point.y);
        }
    }

    fn checkpoint(&mut self) -> Result<()> {
        self.wait_checkpoint()?;
        self.snapshot_view();
        let mut snapshot = self.session.snapshot();
        if !snapshot.settings.restore {
            snapshot.documents.clear();
            snapshot.active = 0;
        }
        let root = self.root.clone();
        io_wait(self.hwnd, move || {
            session::checkpoint(&root, &snapshot)?;
            if !snapshot.settings.restore {
                session::clear_previous(&root)?;
            }
            Ok(())
        })?;
        self.pending = false;
        self.checkpoint_error = false;
        self.last_checkpoint = Instant::now();
        CHECKPOINT_CURRENT.set(true);
        Ok(())
    }

    fn wait_checkpoint(&mut self) -> Result<()> {
        if let Some((_, receiver)) = self.worker.take() {
            io_wait(self.hwnd, move || {
                receiver
                    .recv()
                    .map_err(|_| "Recovery worker disconnected.".to_string())?
            })?;
        }
        Ok(())
    }

    fn background_checkpoint(&mut self) -> Result<()> {
        if let Some((revision, receiver)) = self.worker.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    result?;
                    if self.revision == revision {
                        self.pending = false;
                        CHECKPOINT_CURRENT.set(true);
                    }
                    self.last_checkpoint = Instant::now();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.worker = Some((revision, receiver));
                    return Ok(());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("Recovery worker disconnected.".into());
                }
            }
        }
        if !self.pending
            || self.checkpoint_error
            || self.last_checkpoint.elapsed() < Duration::from_secs(2)
        {
            return Ok(());
        }
        self.snapshot_view();
        let mut snapshot = self.session.snapshot();
        if !snapshot.settings.restore {
            snapshot.documents.clear();
            snapshot.active = 0;
        }
        let root = self.root.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = session::checkpoint(&root, &snapshot).and_then(|()| {
                if !snapshot.settings.restore {
                    session::clear_previous(&root)
                } else {
                    Ok(())
                }
            });
            let _ = sender.send(result);
        });
        self.worker = Some((self.revision, receiver));
        Ok(())
    }

    fn refresh(&mut self) {
        unsafe {
            for (i, d) in self.session.documents.iter().enumerate() {
                let mut label = wide(&d.title().replace('&', "&&"));
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: label.as_mut_ptr(),
                    ..zeroed()
                };
                SendMessageW(self.tabs, TCM_SETITEMW, i, &item as *const _ as isize);
            }
            SetWindowTextW(
                self.hwnd,
                wide(&format!(
                    "{} - {}",
                    self.session.documents[self.active()].title(),
                    WINDOW_TITLE
                ))
                .as_ptr(),
            );
            let d = &self.session.documents[self.active()];
            let sel = selection(self.editor());
            let (line, column) = search::line_column(&d.text.body, sel.max.max(0) as usize);
            let status = format!(
                " Ln {line}, Col {column}    |    {}%    |    {}    |    {}{}",
                d.zoom,
                d.encoding,
                d.text.eol_label(),
                if self.checkpoint_error {
                    "    |    Recovery checkpoint FAILED"
                } else {
                    ""
                }
            );
            SendMessageW(self.status, SB_SETTEXTW, 0, wide(&status).as_ptr() as isize);
            let menu = GetMenu(self.hwnd);
            for (id, checked) in [
                (WRAP, self.session.settings.wrap),
                (STATUS, self.session.settings.status),
                (DARK_MODE, self.session.settings.dark_mode),
                (RESTORE, self.session.settings.restore),
            ] {
                CheckMenuItem(
                    menu,
                    id as u32,
                    MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED },
                );
            }
        }
    }

    fn layout(&mut self) {
        unsafe {
            let mut client = RECT::default();
            GetClientRect(self.hwnd, &mut client);
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            SendMessageW(
                self.tabs,
                TCM_SETMINTABWIDTH,
                0,
                if crate::theme::is_dark() {
                    (160 * dpi / 96) as isize
                } else {
                    -1
                },
            );
            let tab_height = (30 * dpi / 96) as i32;
            let status_height = if self.session.settings.status {
                (24 * dpi / 96) as i32
            } else {
                0
            };
            MoveWindow(self.tabs, 0, 0, client.right, tab_height, 1);
            ShowWindow(
                self.status,
                if self.session.settings.status {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
            MoveWindow(
                self.status,
                0,
                client.bottom - status_height,
                client.right,
                status_height,
                1,
            );
            for (i, &editor) in self.editors.iter().enumerate() {
                MoveWindow(
                    editor,
                    0,
                    tab_height,
                    client.right,
                    (client.bottom - tab_height - status_height).max(0),
                    1,
                );
                ShowWindow(editor, if i == self.active() { SW_SHOW } else { SW_HIDE });
            }
        }
    }

    fn apply_font(&mut self) -> Result<()> {
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            let mut lf: LOGFONTW = zeroed();
            lf.lfHeight = -(self.session.settings.font_points * dpi as i32 / 72);
            lf.lfCharSet = DEFAULT_CHARSET;
            for (a, b) in lf
                .lfFaceName
                .iter_mut()
                .zip(self.session.settings.face.encode_utf16())
            {
                *a = b;
            }
            let font = CreateFontIndirectW(&lf);
            if font.is_null() {
                return Err(os_error("Create font"));
            }
            for &editor in &self.editors {
                SendMessageW(editor, WM_SETFONT, font as usize, 1);
            }
            if !self.font.is_null() {
                DeleteObject(self.font);
            }
            self.font = font;
        }
        Ok(())
    }

    fn activate(&mut self, index: usize) {
        if index >= self.editors.len() {
            return;
        }
        self.session.active = index;
        unsafe {
            SendMessageW(self.tabs, TCM_SETCURSEL, index, 0);
            SetFocus(self.editor());
        }
        self.layout();
        self.refresh();
        self.mark_pending();
    }

    fn add_document(&mut self, document: Document) -> Result<()> {
        if self.editors.len() >= 256 {
            return Err("The limit is 256 open tabs.".into());
        }
        if self
            .session
            .documents
            .iter()
            .map(|d| d.text.bytes())
            .sum::<usize>()
            + document.text.bytes()
            > TEXT_LIMIT
        {
            return Err("Opening this file would exceed 100 MiB of decoded text.".into());
        }
        let editor = create_editor(self.hwnd, &document)?;
        if let Err(error) = crate::theme::editor(editor) {
            unsafe {
                DestroyWindow(editor);
            }
            return Err(error);
        }
        let i = self.editors.len();
        unsafe {
            SendMessageW(editor, WM_SETFONT, self.font as usize, 1);
            SendMessageW(
                editor,
                EM_SETTARGETDEVICE_,
                0,
                if self.session.settings.wrap { 0 } else { 1 },
            );
            SendMessageW(editor, EM_SETZOOM_, document.zoom as usize, 100);
            select(
                editor,
                CharRange {
                    min: document.selection.0,
                    max: document.selection.1,
                },
            );
            let point = POINT {
                x: document.scroll.0,
                y: document.scroll.1,
            };
            SendMessageW(editor, EM_SETSCROLLPOS_, 0, &point as *const _ as isize);
            let mut label = wide(&document.title());
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: label.as_mut_ptr(),
                ..zeroed()
            };
            if SendMessageW(self.tabs, TCM_INSERTITEMW, i, &item as *const _ as isize) == -1 {
                DestroyWindow(editor);
                return Err("Cannot create document tab.".into());
            }
        }
        self.editors.push(editor);
        self.session.documents.push(document);
        self.activate(i);
        Ok(())
    }

    fn open(&mut self, path: &Path, ansi: bool) -> Result<()> {
        let path =
            std::fs::canonicalize(path).map_err(|e| format!("Open {}: {e}", path.display()))?;
        if let Some(i) = self
            .session
            .documents
            .iter()
            .position(|d| d.path.as_ref() == Some(&path))
        {
            self.activate(i);
            if !ansi {
                return Ok(());
            }
            if self.session.documents[i].dirty && !self.confirm_close()? {
                return Ok(());
            }
            let worker_path = path.clone();
            let bytes = io_wait(self.hwnd, move || file_io::read(&worker_path))?;
            let (raw, enc) = encoding::decode(&bytes, Some(unsafe { GetACP() }))?;
            let mut d = Document::from_text(&raw, enc);
            d.path = Some(path);
            d.disk_hash = Some(file_io::hash(&bytes));
            if d.text.bytes() + self.other_bytes(i) > TEXT_LIMIT {
                return Err("Decoded text limit exceeded.".into());
            }
            set_text(self.editor(), &d.text.body)?;
            self.session.documents[i] = d;
            self.refresh();
            self.mark_pending();
            return Ok(());
        }
        let bytes = file_io::read(&path)?;
        let (raw, enc) = encoding::decode(
            &bytes,
            if ansi {
                Some(unsafe { GetACP() })
            } else {
                None
            },
        )?;
        let mut document = Document::from_text(&raw, enc);
        document.path = Some(path);
        document.disk_hash = Some(file_io::hash(&bytes));
        self.add_document(document)
    }

    fn changed(&mut self, editor: HWND, hint: Option<(usize, usize, bool)>) -> Result<()> {
        let Some(i) = self.editors.iter().position(|&e| e == editor) else {
            return Ok(());
        };
        let body = changed_body(editor, &self.session.documents[i], hint);
        let other = self.other_bytes(i);
        let result = body.and_then(|body| self.session.documents[i].update_hint(body, other, hint));
        match result {
            Ok(true) => {
                self.mark_pending();
                self.refresh();
            }
            Ok(false) => (),
            Err(error) => {
                set_text(editor, &self.session.documents[i].text.body)?;
                return Err(error);
            }
        }
        Ok(())
    }

    fn replace_selection(&mut self, replacement: &str) -> Result<()> {
        let editor = self.editor();
        let range = selection(editor);
        let i = self.active();
        let old = &self.session.documents[i].text.body;
        let start = search::utf16_to_byte(old, range.min.max(0) as usize);
        let end = search::utf16_to_byte(old, range.max.max(0) as usize);
        let replacement = Text::parse(replacement).body;
        let mut next = old.clone();
        next.replace_range(start..end, &replacement);
        self.apply_text(
            next,
            Some((range.min.max(0) as usize, range.max.max(0) as usize, false)),
        )?;
        let position = self.session.documents[i].text.body[..start + replacement.len()]
            .encode_utf16()
            .count() as i32;
        select(
            editor,
            CharRange {
                min: position,
                max: position,
            },
        );
        self.mark_pending();
        self.refresh();
        Ok(())
    }

    fn apply_text(&mut self, next: String, hint: Option<(usize, usize, bool)>) -> Result<()> {
        let i = self.active();
        encoding::validate_text(&next)?;
        if rustnotepad::utf16_len(&next) * 2 + self.other_bytes(i) > TEXT_LIMIT {
            return Err("The 100 MiB decoded-text limit would be exceeded.".into());
        }
        let selected = selection(self.editor());
        if let Err(error) = set_text(self.editor(), &next) {
            set_text(self.editor(), &self.session.documents[i].text.body)?;
            select(self.editor(), selected);
            return Err(error);
        }
        let other = self.other_bytes(i);
        self.session.documents[i].update_hint(next, other, hint)?;
        Ok(())
    }

    fn save(&mut self, save_as: bool) -> Result<bool> {
        let i = self.active();
        let current = self.session.documents[i].path.clone();
        let chosen = if save_as || current.is_none() {
            file_dialog(self.hwnd, true, current.as_deref())?
        } else {
            current.clone()
        };
        let Some(path) = chosen else {
            return Ok(false);
        };
        let path = if path.exists() {
            std::fs::canonicalize(&path).map_err(|e| e.to_string())?
        } else {
            path
        };
        if self
            .session
            .documents
            .iter()
            .enumerate()
            .any(|(n, d)| n != i && d.path.as_ref() == Some(&path))
        {
            return Err("This file is open in another tab. Save to a different path.".into());
        }
        let bytes = encoding::encode(
            &self.session.documents[i].text.raw(),
            self.session.documents[i].encoding,
        )?;
        let expected = if path.exists() {
            let disk = file_io::read(&path)?;
            let current_hash = file_io::hash(&disk);
            if current.as_ref() == Some(&path)
                && Some(current_hash) != self.session.documents[i].disk_hash
                && ask(
                    self.hwnd,
                    "This file changed on disk. Overwrite the disk version with this tab?\nChoose No to keep both and use Save As.",
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                ) != IDYES
            {
                return Ok(false);
            }
            Some(current_hash)
        } else {
            if current.as_ref() == Some(&path)
                && self.session.documents[i].disk_hash.is_some()
                && ask(
                    self.hwnd,
                    "The original file was removed. Recreate it?",
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                ) != IDYES
            {
                return Ok(false);
            }
            None
        };
        let worker_path = path.clone();
        let saved_hash = file_io::hash(&bytes);
        io_wait(self.hwnd, move || {
            file_io::save(&worker_path, &bytes, expected)
        })?;
        let d = &mut self.session.documents[i];
        d.path = Some(
            std::fs::canonicalize(&path)
                .map_err(|e| format!("File saved, but resolving its path failed: {e}"))?,
        );
        d.disk_hash = Some(saved_hash);
        d.mark_saved();
        self.mark_pending();
        self.refresh();
        Ok(true)
    }

    fn confirm_close(&mut self) -> Result<bool> {
        if !self.session.documents[self.active()].dirty {
            return Ok(true);
        }
        match ask(
            self.hwnd,
            &format!(
                "Save changes to {}?",
                self.session.documents[self.active()].title()
            ),
            MB_YESNOCANCEL | MB_ICONWARNING,
        ) {
            IDYES => self.save(false),
            IDNO => Ok(true),
            _ => Ok(false),
        }
    }

    fn close_tab(&mut self) -> Result<()> {
        if !self.confirm_close()? {
            return Ok(());
        }
        self.wait_checkpoint()?;
        self.snapshot_view();
        let i = self.active();
        let removed = self.session.documents.remove(i);
        let active = self.session.active;
        self.session.active = i.min(self.session.documents.len().saturating_sub(1));
        // Commit the discard before destroying the editor; a failed commit leaves the tab intact.
        let mut snapshot = self.session.snapshot();
        if !snapshot.settings.restore {
            snapshot.documents.clear();
            snapshot.active = 0;
        }
        if let Err(e) = session::checkpoint(&self.root, &snapshot) {
            self.session.documents.insert(i, removed);
            self.session.active = active;
            return Err(e);
        }
        unsafe {
            DestroyWindow(self.editors.remove(i));
            SendMessageW(self.tabs, TCM_DELETEITEM, i, 0);
        }
        if self.editors.is_empty() {
            self.add_document(Document::default())?;
        }
        self.activate(self.session.active);
        self.checkpoint()?;
        Ok(())
    }

    fn exit(&mut self) -> Result<()> {
        if !self.session.settings.restore {
            let old = self.active();
            for i in 0..self.editors.len() {
                self.activate(i);
                if !self.confirm_close()? {
                    return Ok(());
                }
            }
            self.activate(old);
        }
        self.checkpoint()?;
        unsafe {
            DestroyWindow(self.hwnd);
        }
        Ok(())
    }

    fn find_next(&mut self, backward: bool) -> Result<()> {
        if self.find.is_empty() {
            return self.find_dialog(false);
        }
        let editor = self.editor();
        let selection = selection(editor);
        let body = &self.session.documents[self.active()].text.body;
        let from = search::utf16_to_byte(
            body,
            if backward {
                selection.min
            } else {
                selection.max
            }
            .max(0) as usize,
        );
        match search::find(
            body,
            &self.find,
            from,
            self.match_case,
            backward,
            self.search_wrap,
        )? {
            Some(range) => {
                select(
                    editor,
                    CharRange {
                        min: body[..range.start].encode_utf16().count() as i32,
                        max: body[..range.end].encode_utf16().count() as i32,
                    },
                );
                unsafe {
                    SendMessageW(editor, EM_SCROLLCARET, 0, 0);
                }
                self.refresh();
            }
            None => {
                ask(self.hwnd, "Text not found.", MB_OK | MB_ICONINFORMATION);
            }
        }
        Ok(())
    }

    fn find_dialog(&mut self, replace: bool) -> Result<()> {
        let Some(values) = prompt(
            self.hwnd,
            if replace { "Replace" } else { "Find" },
            &[
                ("Find what:", self.find.as_str()),
                ("Replace with:", self.replacement.as_str()),
            ][..if replace { 2 } else { 1 }],
            true,
            self.match_case,
            self.search_wrap,
            replace,
        )?
        else {
            return Ok(());
        };
        self.find = values.text[0].clone();
        self.match_case = values.case;
        self.search_wrap = values.wrap;
        if self.find.is_empty() {
            return Err("Enter text to find.".into());
        }
        if !replace {
            return self.find_next(false);
        }
        self.replacement = values.text[1].clone();
        if values.all {
            let i = self.active();
            let body = &self.session.documents[i].text.body;
            let ranges = search::ranges(body, &self.find, self.match_case)?;
            let mut count = 0;
            let mut next = String::with_capacity(body.len());
            let mut end = 0;
            for range in ranges {
                count += 1;
                next.push_str(&body[end..range.start]);
                next.push_str(&self.replacement);
                end = range.end;
                if next.len() > TEXT_LIMIT * 2 {
                    return Err("Replacement exceeds the text limit.".into());
                }
            }
            if count == 0 {
                ask(self.hwnd, "Text not found.", MB_OK);
                return Ok(());
            }
            next.push_str(&body[end..]);
            self.apply_text(next, None)?;
            self.mark_pending();
            self.refresh();
            ask(
                self.hwnd,
                &format!("Replaced {count} occurrence(s)."),
                MB_OK,
            );
        } else {
            let s = selection(self.editor());
            let body = &self.session.documents[self.active()].text.body;
            let range = search::utf16_to_byte(body, s.min.max(0) as usize)
                ..search::utf16_to_byte(body, s.max.max(0) as usize);
            let found = search::matches(&body[range.clone()], &self.find, self.match_case)?;
            if found.len() == 1 && found[0] == (0..range.len()) {
                self.replace_selection(&self.replacement.clone())?;
            }
            self.find_next(false)?;
        }
        Ok(())
    }

    fn command(&mut self, command: usize) -> Result<()> {
        match command {
            NEW => self.add_document(Document::default())?,
            OPEN | OPEN_ANSI => {
                if let Some(path) = file_dialog(self.hwnd, false, None)? {
                    self.open(&path, command == OPEN_ANSI)?;
                }
            }
            SAVE | SAVE_AS => {
                self.save(command == SAVE_AS)?;
            }
            CLOSE => self.close_tab()?,
            EXIT => self.exit()?,
            UNDO | REDO => {
                let i = self.active();
                let other = self.other_bytes(i);
                self.session.documents[i].undo(command == REDO, other)?;
                let d = &self.session.documents[i];
                set_text(self.editor(), &d.text.body)?;
                select(
                    self.editor(),
                    CharRange {
                        min: d.selection.0,
                        max: d.selection.1,
                    },
                );
                self.mark_pending();
            }
            CUT | COPY => {
                let selection = selection(self.editor());
                let body = &self.session.documents[self.active()].text.body;
                let start = search::utf16_to_byte(body, selection.min.max(0) as usize);
                let end = search::utf16_to_byte(body, selection.max.max(0) as usize);
                if start == end {
                    return Ok(());
                }
                copy_text(self.hwnd, &body[start..end])?;
                if command == CUT {
                    self.replace_selection("")?;
                }
            }
            PASTE => {
                self.replace_selection(&clipboard_text(self.hwnd)?)?;
            }
            DELETE => self.replace_selection("")?,
            SELECT_ALL => select(self.editor(), CharRange { min: 0, max: -1 }),
            FIND | REPLACE => self.find_dialog(command == REPLACE)?,
            NEXT | PREVIOUS => self.find_next(command == PREVIOUS)?,
            GOTO => {
                if let Some(values) = prompt(
                    self.hwnd,
                    "Go to line",
                    &[("Line number:", "1")],
                    false,
                    false,
                    false,
                    false,
                )? {
                    let line = values.text[0]
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "Enter a positive line number.")?;
                    let body = &self.session.documents[self.active()].text.body;
                    let offset = search::line_offset(body, line)?;
                    let position = body[..offset].encode_utf16().count() as i32;
                    select(
                        self.editor(),
                        CharRange {
                            min: position,
                            max: position,
                        },
                    );
                    unsafe {
                        SendMessageW(self.editor(), EM_SCROLLCARET, 0, 0);
                    }
                }
            }
            DATETIME => {
                use windows_sys::Win32::Globalization::*;
                let mut time = [0u16; 128];
                let mut date = [0u16; 128];
                unsafe {
                    let tn = GetTimeFormatEx(
                        null(),
                        TIME_NOSECONDS,
                        null(),
                        null(),
                        time.as_mut_ptr(),
                        128,
                    );
                    let dn = GetDateFormatEx(
                        null(),
                        DATE_SHORTDATE,
                        null(),
                        null(),
                        date.as_mut_ptr(),
                        128,
                        null(),
                    );
                    if tn == 0 || dn == 0 {
                        return Err(os_error("Format date/time"));
                    }
                    self.replace_selection(&format!(
                        "{} {}",
                        String::from_utf16_lossy(&time[..tn as usize - 1]),
                        String::from_utf16_lossy(&date[..dn as usize - 1])
                    ))?;
                }
            }
            WRAP => {
                self.session.settings.wrap = !self.session.settings.wrap;
                for &editor in &self.editors {
                    unsafe {
                        SendMessageW(
                            editor,
                            EM_SETTARGETDEVICE_,
                            0,
                            if self.session.settings.wrap { 0 } else { 1 },
                        );
                    }
                }
                self.mark_pending();
            }
            FONT => unsafe {
                let mut lf: LOGFONTW = zeroed();
                GetObjectW(
                    self.font,
                    size_of::<LOGFONTW>() as i32,
                    &mut lf as *mut _ as *mut c_void,
                );
                let mut dialog = CHOOSEFONTW {
                    lStructSize: size_of::<CHOOSEFONTW>() as u32,
                    hwndOwner: self.hwnd,
                    lpLogFont: &mut lf,
                    Flags: CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT | CF_LIMITSIZE,
                    nSizeMin: 6,
                    nSizeMax: 72,
                    ..zeroed()
                };
                if ChooseFontW(&mut dialog) != 0 {
                    self.session.settings.font_points = dialog.iPointSize / 10;
                    self.session.settings.face = String::from_utf16_lossy(
                        &lf.lfFaceName[..lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(31)],
                    );
                    self.apply_font()?;
                    self.mark_pending();
                } else {
                    dialog_error()?;
                }
            },
            ZOOM_IN | ZOOM_OUT | ZOOM_RESET => {
                let i = self.active();
                let zoom = &mut self.session.documents[i].zoom;
                *zoom = if command == ZOOM_RESET {
                    100
                } else {
                    (*zoom + if command == ZOOM_IN { 10 } else { -10 }).clamp(10, 500)
                };
                unsafe {
                    SendMessageW(
                        self.editor(),
                        EM_SETZOOM_,
                        self.session.documents[i].zoom as usize,
                        100,
                    );
                }
                self.mark_pending();
            }
            STATUS => {
                self.session.settings.status = !self.session.settings.status;
                self.layout();
                self.mark_pending();
            }
            NEXT_TAB | PREVIOUS_TAB => {
                let count = self.editors.len();
                self.activate(
                    (self.active() + if command == NEXT_TAB { 1 } else { count - 1 }) % count,
                );
            }
            RESTORE => {
                if self.session.settings.restore
                    && ask(
                        self.hwnd,
                        "Disable restoration and remove stored recovery text?\nOpen tabs remain open; unsaved changes will require Save/Discard on exit.\nLocal recovery is not encrypted, and deletion is not secure erasure.",
                        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                    ) != IDYES
                {
                    return Ok(());
                }
                self.session.settings.restore = !self.session.settings.restore;
                self.checkpoint()?;
            }
            CLEAR_RECOVERY => {
                if ask(
                    self.hwnd,
                    "Clear recovery and disable restoration? Current tabs remain open.",
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                ) == IDYES
                {
                    self.session.settings.restore = false;
                    self.checkpoint()?;
                }
            }
            RETRY_RECOVERY => self.checkpoint()?,
            DARK_MODE => {
                let previous = self.session.settings.dark_mode;
                self.session.settings.dark_mode = !previous;
                if let Err(error) = self.apply_theme() {
                    self.session.settings.dark_mode = previous;
                    if let Err(restore) = self.apply_theme() {
                        return Err(format!("{error}\nRestoring theme: {restore}"));
                    }
                    return Err(error);
                }
                self.mark_pending();
                self.checkpoint()?;
            }
            ABOUT => {
                ask(
                    self.hwnd,
                    &format!(
                        "Rust Notepad {}\nNative plain-text editing. No AI, telemetry, or network service.\n\nRecovery text is stored locally in %LOCALAPPDATA%\\RustNotepad.\n20 MiB per file; 100 MiB decoded text across tabs.",
                        rustnotepad::VERSION
                    ),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            PRINT => self.printer.print(
                self.hwnd,
                self.editor(),
                &self.session.documents[self.active()].title(),
            )?,
            PAGE_SETUP => self.printer.setup(self.hwnd)?,
            id if (ENCODING_BASE..ENCODING_BASE + 5).contains(&id) => {
                let enc = [
                    Encoding::Utf8,
                    Encoding::Utf8Bom,
                    Encoding::Utf16Le,
                    Encoding::Utf16Be,
                    Encoding::Ansi(unsafe { GetACP() }),
                ][id - ENCODING_BASE];
                let i = self.active();
                encoding::encode(&self.session.documents[i].text.raw(), enc)?;
                self.session.documents[i].encoding = enc;
                self.session.documents[i].invalidate_savepoint();
                self.mark_pending();
            }
            id if (EOL_BASE..EOL_BASE + 3).contains(&id) => {
                let i = self.active();
                let eol = [
                    rustnotepad::document::Eol::CrLf,
                    rustnotepad::document::Eol::Lf,
                    rustnotepad::document::Eol::Cr,
                ][id - EOL_BASE];
                self.session.documents[i].convert_eol(eol);
                self.mark_pending();
            }
            _ => (),
        }
        if unsafe { IsWindow(self.hwnd) } != 0 && !self.editors.is_empty() {
            self.refresh();
        }
        Ok(())
    }
}

fn dialog_error() -> Result<()> {
    let code = unsafe { CommDlgExtendedError() };
    if code != 0 {
        Err(format!("Windows dialog failed (0x{code:08x})."))
    } else {
        Ok(())
    }
}

fn file_dialog(owner: HWND, save: bool, initial: Option<&Path>) -> Result<Option<PathBuf>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let mut path = vec![0u16; 32768];
    if let Some(initial) = initial {
        let units: Vec<_> = initial.as_os_str().encode_wide().collect();
        if units.len() < path.len() {
            path[..units.len()].copy_from_slice(&units);
        }
    }
    let filter = wide("Text files (*.txt)\0*.txt\0All files (*.*)\0*.*\0");
    let extension = wide("txt");
    unsafe {
        let mut dialog = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFilter: filter.as_ptr(),
            nFilterIndex: 1,
            lpstrFile: path.as_mut_ptr(),
            nMaxFile: path.len() as u32,
            lpstrDefExt: extension.as_ptr(),
            Flags: OFN_EXPLORER
                | OFN_NOCHANGEDIR
                | OFN_PATHMUSTEXIST
                | if save {
                    OFN_OVERWRITEPROMPT
                } else {
                    OFN_FILEMUSTEXIST
                },
            ..zeroed()
        };
        let ok = if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        };
        if ok == 0 {
            dialog_error()?;
            return Ok(None);
        }
    }
    let end = path
        .iter()
        .position(|&c| c == 0)
        .ok_or("Invalid file dialog result.")?;
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &path[..end],
    ))))
}

struct PromptResult {
    text: Vec<String>,
    case: bool,
    wrap: bool,
    all: bool,
}

unsafe extern "system" fn prompt_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if let Some(result) = crate::theme::paint_message(hwnd, msg, wp, lp) {
            return result;
        }
        match msg {
            WM_CLOSE => {
                SetWindowLongW(hwnd, GWLP_USERDATA, IDCANCEL);
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            WM_COMMAND if [IDOK as usize, IDCANCEL as usize, 3].contains(&(wp & 0xffff)) => {
                SetWindowLongW(hwnd, GWLP_USERDATA, (wp & 0xffff) as i32);
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn prompt(
    owner: HWND,
    title: &str,
    fields: &[(&str, &str)],
    options: bool,
    case: bool,
    wrap: bool,
    replace: bool,
) -> Result<Option<PromptResult>> {
    unsafe {
        let class = wide("RustNotepad.Prompt");
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(prompt_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hbrBackground: (COLOR_BTNFACE + 1) as HBRUSH,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let mut position = RECT::default();
        GetWindowRect(owner, &mut position);
        let dpi = GetDpiForWindow(owner).max(96);
        let scale = |v: i32| v * dpi as i32 / 96;
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            class.as_ptr(),
            wide(title).as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            position.left + scale(50),
            position.top + scale(70),
            scale(470),
            scale(if fields.len() == 2 { 240 } else { 195 }),
            owner,
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err(os_error("Create dialog"));
        }
        if let Err(error) = crate::theme::title_bar(hwnd) {
            DestroyWindow(hwnd);
            return Err(error);
        }
        let control = |class: &str, title: &str, style: u32, x, y, w, h, id: usize| {
            let child = CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(title).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                scale(x),
                scale(y),
                scale(w),
                scale(h),
                hwnd,
                id as HMENU,
                instance,
                null(),
            );
            SendMessageW(
                child,
                WM_SETFONT,
                GetStockObject(DEFAULT_GUI_FONT) as usize,
                1,
            );
            child
        };
        let mut edits = Vec::new();
        for (i, &(label, value)) in fields.iter().enumerate() {
            let y = 16 + i as i32 * 42;
            control("STATIC", label, 0, 12, y + 3, 115, 22, 100 + i);
            let edit = control(
                "EDIT",
                value,
                WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
                130,
                y,
                305,
                25,
                110 + i,
            );
            SendMessageW(edit, EM_LIMITTEXT, 65536, 0);
            edits.push(edit);
        }
        let y = 18 + fields.len() as i32 * 42;
        let case_box = control(
            "BUTTON",
            "Match &case",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            15,
            y,
            130,
            24,
            120,
        );
        let wrap_box = control(
            "BUTTON",
            "&Wrap search",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            155,
            y,
            140,
            24,
            121,
        );
        if !options {
            ShowWindow(case_box, SW_HIDE);
            ShowWindow(wrap_box, SW_HIDE);
        }
        SendMessageW(case_box, BM_SETCHECK, case as usize, 0);
        SendMessageW(wrap_box, BM_SETCHECK, wrap as usize, 0);
        control(
            "BUTTON",
            if replace { "&Replace" } else { "&OK" },
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            110,
            y + 34,
            100,
            28,
            IDOK as usize,
        );
        if replace {
            control(
                "BUTTON",
                "Replace &All",
                WS_TABSTOP,
                220,
                y + 34,
                100,
                28,
                3,
            );
        }
        control(
            "BUTTON",
            "Cancel",
            WS_TABSTOP,
            330,
            y + 34,
            100,
            28,
            IDCANCEL as usize,
        );
        EnableWindow(owner, 0);
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(edits[0]);
        SendMessageW(edits[0], EM_SETSEL, 0, -1);
        let mut msg: MSG = zeroed();
        while IsWindowVisible(hwnd) != 0 {
            let result = GetMessageW(&mut msg, null_mut(), 0, 0);
            if result <= 0 {
                if result == 0 {
                    PostQuitMessage(msg.wParam as i32);
                }
                break;
            }
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let id = GetWindowLongW(hwnd, GWLP_USERDATA);
        let result = if id == IDOK || id == 3 {
            let text = edits
                .iter()
                .map(|&edit| {
                    let n = GetWindowTextLengthW(edit);
                    let mut b = vec![0; n as usize + 1];
                    GetWindowTextW(edit, b.as_mut_ptr(), b.len() as i32);
                    String::from_utf16_lossy(&b[..n as usize])
                })
                .collect();
            Some(PromptResult {
                text,
                case: SendMessageW(case_box, BM_GETCHECK, 0, 0) != 0,
                wrap: SendMessageW(wrap_box, BM_GETCHECK, 0, 0) != 0,
                all: id == 3,
            })
        } else {
            None
        };
        EnableWindow(owner, 1);
        DestroyWindow(hwnd);
        SetForegroundWindow(owner);
        Ok(result)
    }
}

fn menu() -> Result<HMENU> {
    unsafe {
        let root = CreateMenu();
        if root.is_null() {
            return Err(os_error("Create menu"));
        }
        let sections: &[(&str, &[(usize, &str)])] = &[
            (
                "&File",
                &[
                    (NEW, "&New tab\tCtrl+N"),
                    (OPEN, "&Open...\tCtrl+O"),
                    (OPEN_ANSI, "Open as &ANSI..."),
                    (SAVE, "&Save\tCtrl+S"),
                    (SAVE_AS, "Save &As...\tCtrl+Shift+S"),
                    (0, ""),
                    (PAGE_SETUP, "Page Set&up..."),
                    (PRINT, "&Print...\tCtrl+P"),
                    (0, ""),
                    (CLOSE, "&Close tab\tCtrl+W"),
                    (EXIT, "E&xit"),
                ],
            ),
            (
                "&Edit",
                &[
                    (UNDO, "&Undo\tCtrl+Z"),
                    (REDO, "&Redo\tCtrl+Y"),
                    (0, ""),
                    (CUT, "Cu&t\tCtrl+X"),
                    (COPY, "&Copy\tCtrl+C"),
                    (PASTE, "&Paste\tCtrl+V"),
                    (DELETE, "&Delete\tDel"),
                    (SELECT_ALL, "Select &All\tCtrl+A"),
                    (0, ""),
                    (FIND, "&Find...\tCtrl+F"),
                    (NEXT, "Find &Next\tF3"),
                    (PREVIOUS, "Find Pre&vious\tShift+F3"),
                    (REPLACE, "R&eplace...\tCtrl+H"),
                    (GOTO, "&Go To...\tCtrl+G"),
                    (DATETIME, "Time/&Date\tF5"),
                ],
            ),
            ("F&ormat", &[(WRAP, "&Word wrap"), (FONT, "&Font...")]),
            (
                "&View",
                &[
                    (ZOOM_IN, "Zoom &in\tCtrl++"),
                    (ZOOM_OUT, "Zoom &out\tCtrl+-"),
                    (ZOOM_RESET, "&Reset zoom\tCtrl+0"),
                    (STATUS, "&Status bar"),
                    (DARK_MODE, "&Dark mode"),
                    (0, ""),
                    (RESTORE, "&Restore tabs on startup"),
                    (RETRY_RECOVERY, "Write recovery &checkpoint now"),
                    (CLEAR_RECOVERY, "&Clear stored recovery..."),
                ],
            ),
            (
                "&Encoding",
                &[
                    (600, "UTF-&8"),
                    (601, "UTF-8 with &BOM"),
                    (602, "UTF-16 &LE"),
                    (603, "UTF-16 B&E"),
                    (604, "Windows &ANSI"),
                ],
            ),
            (
                "&Line endings",
                &[
                    (700, "Convert to &CRLF"),
                    (701, "Convert to &LF"),
                    (702, "Convert to C&R"),
                ],
            ),
            ("&Help", &[(ABOUT, "&About Rust Notepad")]),
        ];
        for &(title, entries) in sections {
            let popup = CreatePopupMenu();
            if popup.is_null() {
                DestroyMenu(root);
                return Err(os_error("Create popup menu"));
            }
            for &(id, label) in entries {
                if AppendMenuW(
                    popup,
                    if id == 0 { MF_SEPARATOR } else { MF_STRING },
                    id,
                    wide(label).as_ptr(),
                ) == 0
                {
                    DestroyMenu(popup);
                    DestroyMenu(root);
                    return Err(os_error("Create menu item"));
                }
            }
            if AppendMenuW(root, MF_POPUP, popup as usize, wide(title).as_ptr()) == 0 {
                DestroyMenu(popup);
                DestroyMenu(root);
                return Err(os_error("Attach menu"));
            }
        }
        Ok(root)
    }
}

fn accelerator(msg: &MSG) -> Option<usize> {
    if msg.message != WM_KEYDOWN {
        return None;
    }
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
    let k = msg.wParam as u16;
    if ctrl {
        Some(match k {
            0x4e => NEW,
            0x4f => OPEN,
            0x53 => {
                if shift {
                    SAVE_AS
                } else {
                    SAVE
                }
            }
            0x57 => CLOSE,
            0x5a => UNDO,
            0x59 => REDO,
            0x58 => CUT,
            0x43 => COPY,
            0x56 => PASTE,
            0x41 => SELECT_ALL,
            0x46 => FIND,
            0x48 => REPLACE,
            0x47 => GOTO,
            0x50 => PRINT,
            VK_TAB => {
                if shift {
                    PREVIOUS_TAB
                } else {
                    NEXT_TAB
                }
            }
            VK_OEM_PLUS | VK_ADD => ZOOM_IN,
            VK_OEM_MINUS | VK_SUBTRACT => ZOOM_OUT,
            0x30 => ZOOM_RESET,
            _ => return None,
        })
    } else {
        match k {
            VK_F3 => Some(if shift { PREVIOUS } else { NEXT }),
            VK_F5 => Some(DATETIME),
            _ => None,
        }
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn own_session(root: &Path, paths: &[PathBuf]) -> Result<Option<Handle>> {
    let identity = file_io::hash(root.as_os_str().to_string_lossy().as_bytes());
    let name = format!("Local\\RustNotepad-{:x?}", identity);
    unsafe {
        let mutex = CreateMutexW(null(), 0, wide(&name).as_ptr());
        if mutex.is_null() {
            return Err(os_error("Create session lock"));
        }
        let guard = Handle(mutex);
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return Ok(Some(guard));
        }
        let mut hwnd = null_mut();
        for _ in 0..50 {
            hwnd = FindWindowW(wide(&window_class(root)).as_ptr(), null());
            if !hwnd.is_null() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if hwnd.is_null() {
            return Err("Another instance owns recovery but is not responding. No second writer was started.".into());
        }
        let payload = serde_json::to_vec(paths).map_err(|e| e.to_string())?;
        if payload.len() > 1024 * 1024 {
            return Err("Too many command-line file paths.".into());
        }
        let data = COPYDATASTRUCT {
            dwData: IPC_TAG,
            cbData: payload.len() as u32,
            lpData: payload.as_ptr() as *mut c_void,
        };
        let mut result = 0;
        if SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            0,
            &data as *const _ as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        ) == 0
            || result != 1
        {
            return Err("The existing instance did not accept the files.".into());
        }
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
        Ok(None)
    }
}

fn window_class(root: &Path) -> String {
    let identity = file_io::hash(root.as_os_str().to_string_lossy().as_bytes());
    format!("{CLASS}-{:x?}", identity)
}

pub fn run() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    unsafe {
        if LoadLibraryExW(
            wide("Msftedit.dll").as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .is_null()
        {
            return Err(os_error("Load system Rich Edit"));
        }
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES | ICC_BAR_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return Err(os_error("Initialize Windows controls"));
        }
    }
    if arguments.first().is_some_and(|a| a == "--self-test") {
        let report = arguments.get(1).map(PathBuf::from);
        let result = self_test(report.clone());
        if let (Err(error), Some(report)) = (&result, report) {
            std::fs::write(&report, format!("FAIL: {error}\n"))
                .map_err(|e| format!("{error}\nWriting test report: {e}"))?;
        }
        return result;
    }
    let paths: Vec<PathBuf> = arguments
        .iter()
        .map(PathBuf::from)
        .map(|p| std::path::absolute(p).map_err(|e| e.to_string()))
        .collect::<Result<_>>()?;
    let root = session::directory()?;
    let Some(_owner) = own_session(&root, &paths)? else {
        return Ok(());
    };
    let mut restored = match session::load(&root) {
        Ok(s) => s.unwrap_or_default(),
        Err(e) => {
            error(
                null_mut(),
                &format!(
                    "Current recovery could not be loaded:\n{e}\nAttempting the previous complete checkpoint."
                ),
            );
            match session::load_previous(&root) {
                Ok(s) => s,
                Err(previous) => {
                    return Err(format!(
                        "Previous recovery also failed: {previous}\nRecovery files were left untouched in {}.",
                        root.display()
                    ));
                }
            }
        }
    };
    let mut warnings = Vec::new();
    if restored.settings.restore {
        for doc in &mut restored.documents {
            if let Some(path) = doc.path.clone() {
                match file_io::read(&path) {
                    Ok(bytes) if Some(file_io::hash(&bytes)) == doc.disk_hash => (),
                    Ok(bytes) if !doc.dirty => {
                        match encoding::decode(&bytes, match doc.encoding { Encoding::Ansi(cp) => Some(cp), _ => None }) {
                            Ok((raw, enc)) => {
                                doc.text = Text::parse(&raw); doc.encoding = enc; doc.disk_hash = Some(file_io::hash(&bytes));
                            }
                            Err(e) => { doc.invalidate_savepoint(); warnings.push(format!("Recovered {} from snapshot: {e}", path.display())); }
                        }
                    }
                    Ok(_) => warnings.push(format!("{} changed on disk. Unsaved recovery was preserved; saving requires confirmation.", path.display())),
                    Err(e) => { doc.invalidate_savepoint(); warnings.push(format!("Recovered {} from snapshot: {e}", path.display())); }
                }
            }
        }
    } else {
        restored.documents.clear();
    }
    restored.validate()?;
    let docs = std::mem::take(&mut restored.documents);
    crate::theme::configure(restored.settings.dark_mode)?;
    let active = restored.active;
    restored.active = 0;
    unsafe {
        let instance = GetModuleHandleW(null());
        let class = wide(&window_class(&root));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
            hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
            ..zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(os_error("Register application window"));
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(WINDOW_TITLE).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1000,
            720,
            null_mut(),
            menu()?,
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err(os_error("Create application window"));
        }
        let tabs = CreateWindowExW(
            0,
            wide("SysTabControl32").as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | TCS_FOCUSNEVER,
            0,
            0,
            100,
            30,
            hwnd,
            1usize as HMENU,
            instance,
            null(),
        );
        let status = CreateWindowExW(
            0,
            wide("msctls_statusbar32").as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            100,
            24,
            hwnd,
            2usize as HMENU,
            instance,
            null(),
        );
        if tabs.is_null() || status.is_null() {
            DestroyWindow(hwnd);
            return Err(os_error("Create tab/status controls"));
        }
        crate::theme::install_chrome(tabs, status)?;
        SendMessageW(
            tabs,
            WM_SETFONT,
            GetStockObject(DEFAULT_GUI_FONT) as usize,
            1,
        );
        let mut app = App {
            hwnd,
            tabs,
            status,
            editors: Vec::new(),
            session: restored,
            root,
            font: null_mut(),
            find: String::new(),
            replacement: String::new(),
            match_case: false,
            search_wrap: true,
            pending: false,
            checkpoint_error: false,
            last_checkpoint: Instant::now(),
            printer: crate::printing::Printer::default(),
            revision: 0,
            worker: None,
        };
        app.apply_font()?;
        app.apply_theme()?;
        for doc in docs {
            app.add_document(doc)?;
        }
        if app.editors.is_empty() {
            app.add_document(Document::default())?;
        }
        app.activate(active.min(app.editors.len() - 1));
        for path in paths {
            if let Err(e) = app.open(&path, false) {
                warnings.push(e);
            }
        }
        DragAcceptFiles(hwnd, 1);
        if SetTimer(hwnd, 1, 1000, None) == 0 {
            return Err(os_error("Start recovery timer"));
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        for warning in warnings {
            error(hwnd, &warning);
        }
        let mut message: MSG = zeroed();
        loop {
            loop {
                let event = EVENTS.with(|q| q.borrow_mut().pop_front());
                let Some(event) = event else {
                    break;
                };
                if IsWindow(hwnd) == 0 {
                    break;
                }
                let outcome = match event {
                    Event::Command(command) => app.command(command),
                    Event::Resize => {
                        app.layout();
                        app.apply_font()
                    }
                    Event::ThemeChanged => app.apply_theme(),
                    Event::Changed(editor, hint) => app.changed(editor, hint),
                    Event::Status => {
                        app.refresh();
                        Ok(())
                    }
                    Event::SelectTab => {
                        let i = SendMessageW(tabs, TCM_GETCURSEL, 0, 0);
                        if i >= 0 {
                            app.activate(i as usize);
                        }
                        Ok(())
                    }
                    Event::Exit => app.exit(),
                    Event::Tick => {
                        let result = app.background_checkpoint();
                        if result.is_err() {
                            app.checkpoint_error = true;
                            app.refresh();
                        }
                        result
                    }
                    Event::Drop(drop) => {
                        let n = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
                        let mut paths = Vec::new();
                        for i in 0..n {
                            let len = DragQueryFileW(drop, i, null_mut(), 0);
                            let mut buffer = vec![0u16; len as usize + 1];
                            DragQueryFileW(drop, i, buffer.as_mut_ptr(), buffer.len() as u32);
                            use std::os::windows::ffi::OsStringExt;
                            paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                                &buffer[..len as usize],
                            )));
                        }
                        DragFinish(drop);
                        enqueue(Event::OpenPaths(paths));
                        Ok(())
                    }
                    Event::OpenPaths(paths) => {
                        for p in paths {
                            if let Err(e) = app.open(&p, false) {
                                error(hwnd, &e);
                            }
                        }
                        Ok(())
                    }
                    Event::Insert(text) => app.replace_selection(&text),
                    Event::Error(error) => Err(error),
                };
                if let Err(e) = outcome {
                    error(hwnd, &e);
                }
            }
            let status = GetMessageW(&mut message, null_mut(), 0, 0);
            if status == -1 {
                return Err(os_error("Get Windows message"));
            }
            if status == 0 {
                break;
            }
            if let Some(command) = accelerator(&message) {
                enqueue(Event::Command(command));
            } else {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        DeleteObject(app.font);
    }
    Ok(())
}

fn io_wait<T: Send + 'static>(
    owner: HWND,
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(operation());
    });
    unsafe {
        EnableWindow(owner, 0);
    }
    let result = loop {
        match receiver.recv_timeout(Duration::from_millis(16)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("File operation worker disconnected.".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => unsafe {
                let mut message: MSG = zeroed();
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.message == WM_QUIT {
                        PostQuitMessage(message.wParam as i32);
                        break;
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            },
        }
    };
    unsafe {
        EnableWindow(owner, 1);
    }
    result
}

fn self_test(report: Option<PathBuf>) -> Result<()> {
    let mut measurements = String::new();
    unsafe {
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Rust Notepad tests").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            800,
            600,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if parent.is_null() {
            return Err(os_error("Create test window"));
        }
        let result: Result<()> = (|| {
            for raw in [
                "",
                "no final newline",
                "a\r\nb\nc\rd",
                "\u{1f600}e\u{301}\n",
            ] {
                let doc = Document::from_text(raw, Encoding::Utf8);
                let editor = create_editor(parent, &doc)?;
                if text(editor)? != doc.text.body {
                    return Err("Rich Edit text fidelity failed.".into());
                }
                SendMessageW(editor, EM_SETTARGETDEVICE_, 0, 0);
                if text(editor)? != doc.text.body {
                    return Err("Wrapping changed text.".into());
                }
                SendMessageW(editor, EM_SETTARGETDEVICE_, 0, 1);
                if text(editor)? != doc.text.body {
                    return Err("Disabling wrapping changed text.".into());
                }
                DestroyWindow(editor);
            }
            let doc = Document::from_text("first\nsecond", Encoding::Utf8);
            let editor = create_editor(parent, &doc)?;
            select(editor, CharRange { min: 6, max: 12 });
            let s = selection(editor);
            if s.min != 6 || s.max != 12 {
                return Err("Native UTF-16 selection mapping failed.".into());
            }
            let original = text(editor)?;
            for dark in [true, false, true, false] {
                crate::theme::configure(dark)?;
                for modified in [0, 1] {
                    SendMessageW(editor, EM_SETMODIFY, modified, 0);
                    crate::theme::editor(editor)?;
                    let selected = selection(editor);
                    let expected = if crate::theme::is_dark() {
                        crate::theme::FOREGROUND
                    } else {
                        GetSysColor(COLOR_WINDOWTEXT)
                    };
                    if crate::theme::editor_foreground(editor) != expected
                        || text(editor)? != original
                        || selected.min != s.min
                        || selected.max != s.max
                        || (SendMessageW(editor, EM_GETMODIFY, 0, 0) != 0) != (modified != 0)
                    {
                        return Err(format!(
                            "Theme invariance failed: dark={dark}, color={:06x} expected={expected:06x}, selection=({}, {}) expected=({}, {}), modified={} expected={modified}, text_equal={}",
                            crate::theme::editor_foreground(editor),
                            selected.min,
                            selected.max,
                            s.min,
                            s.max,
                            SendMessageW(editor, EM_GETMODIFY, 0, 0),
                            text(editor)? == original
                        ));
                    }
                    let failure = crate::theme::with_print_colors(editor, || -> Result<()> {
                        if crate::theme::editor_foreground(editor) != 0 {
                            return Err("Printing did not use black text.".into());
                        }
                        Err("simulated print failure".into())
                    });
                    if failure != Err("simulated print failure".into())
                        || crate::theme::editor_foreground(editor) != expected
                    {
                        return Err("Print error did not restore editor colors.".into());
                    }
                }
            }
            measurements.push_str("PASS: light/dark colors, text/selection/modified-state invariance, print-error color restoration\n");
            DestroyWindow(editor);
            let raw = format!("{}\n", "a".repeat(79)).repeat(rustnotepad::FILE_LIMIT / 80);
            let mut doc = Document::from_text(&raw, Encoding::Utf8);
            let start = Instant::now();
            let editor = create_editor(parent, &doc)?;
            crate::theme::configure(true)?;
            crate::theme::editor(editor)?;
            measurements.push_str(&format!(
                "20 MiB native load: {} ms\n",
                start.elapsed().as_millis()
            ));
            let mut times = Vec::new();
            let mut native_times = Vec::new();
            let mut read_times = Vec::new();
            let mut core_times = Vec::new();
            for _ in 0..20 {
                let start = Instant::now();
                select(editor, CharRange { min: -1, max: -1 });
                let selected = selection(editor);
                let hint = Some((selected.min as usize, selected.max as usize, false));
                SendMessageW(editor, WM_CHAR, b'x' as usize, 0);
                native_times.push(start.elapsed().as_millis());
                let read_start = Instant::now();
                let body = changed_body(editor, &doc, hint)?;
                read_times.push(read_start.elapsed().as_millis());
                let core_start = Instant::now();
                doc.update_hint(body, 0, hint)?;
                search::line_column(&doc.text.body, selection(editor).max.max(0) as usize);
                core_times.push(core_start.elapsed().as_millis());
                times.push(start.elapsed().as_millis());
            }
            times.sort();
            measurements.push_str(&format!("20 MiB edit/snapshot p95: {} ms\n", times[18]));
            native_times.sort();
            read_times.sort();
            core_times.sort();
            measurements.push_str(&format!(
                "Breakdown p95: native={}ms read={}ms core={}ms\n",
                native_times[18], read_times[18], core_times[18]
            ));
            if text(editor)? != doc.text.body {
                return Err("Incremental native edit diverged from document text.".into());
            }
            DestroyWindow(editor);
            if let Some(report) = &report {
                let doc = Document::from_text(
                    &"Printing a native plain-text page.\n".repeat(200),
                    Encoding::Utf8,
                );
                let editor = create_editor(parent, &doc)?;
                let pdf = report.with_extension("pdf");
                crate::theme::editor(editor)?;
                if pdf.exists() {
                    std::fs::remove_file(&pdf).map_err(|e| e.to_string())?;
                }
                let printer = crate::printing::Printer::default();
                let result = printer.test_pdf(editor, &pdf, CharRange { min: 0, max: -1 }, None);
                if result.is_ok() {
                    measurements.push_str("PASS: multipage Microsoft Print to PDF\n");
                }
                let restored_color = crate::theme::editor_foreground(editor);
                if crate::theme::is_dark() && restored_color != crate::theme::FOREGROUND {
                    return Err("Printing did not restore the dark editor text color.".into());
                }
                DestroyWindow(editor);
                crate::theme::configure(false)?;
                result?;
                let deadline = Instant::now() + Duration::from_secs(15);
                let bytes = loop {
                    match std::fs::read(&pdf) {
                        Ok(bytes)
                            if bytes.ends_with(b"%%EOF\r\n") || bytes.ends_with(b"%%EOF\n") =>
                        {
                            break bytes;
                        }
                        Ok(_) => (),
                        Err(e)
                            if e.kind() == std::io::ErrorKind::NotFound
                                || e.kind() == std::io::ErrorKind::PermissionDenied => {}
                        Err(e) => return Err(format!("Read PDF output: {e}")),
                    }
                    if Instant::now() >= deadline {
                        return Err("Timed out waiting for the PDF print spooler.".into());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                };
                if !bytes.starts_with(b"%PDF-") {
                    return Err("Print test did not create a PDF.".into());
                }
            }
            Ok(())
        })();
        DestroyWindow(parent);
        result?;
    }
    if let Some(report) = report {
        std::fs::write(report, format!("PASS: native plain-text, Unicode, EOL normalization, wrap, and UTF-16 selection checks\n{measurements}")).map_err(|e| e.to_string())?;
    }
    Ok(())
}
