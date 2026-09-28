#![windows_subsystem = "windows"]

#[cfg(windows)]
mod printing;
#[cfg(windows)]
mod theme;
#[cfg(windows)]
mod ui;

fn main() {
    #[cfg(windows)]
    if let Err(error) = ui::run() {
        if std::env::args_os()
            .nth(1)
            .is_some_and(|a| a == "--self-test")
        {
            eprintln!("{error}");
        } else {
            ui::error(std::ptr::null_mut(), &error);
        }
        std::process::exit(1);
    }
}
