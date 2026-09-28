pub mod document;
pub mod encoding;
pub mod file_io;
pub mod search;
pub mod session;
pub mod version;

pub const VERSION: &str = env!("RUSTNOTEPAD_VERSION");

pub const FILE_LIMIT: usize = 20 * 1024 * 1024;
pub const TEXT_LIMIT: usize = 100 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, String>;

pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub fn utf16_len(value: &str) -> usize {
    if value.is_ascii() {
        value.len()
    } else {
        value.encode_utf16().count()
    }
}
