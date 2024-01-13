use windows::{
    Win32::Foundation::*, 
    Win32::UI::WindowsAndMessaging::*,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowTextLengthW};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut title_length = GetWindowTextLengthW(hwnd) as usize;
    if title_length > 0 {
        title_length += 1; // For null terminator
        let mut title: Vec<u16> = vec![0; title_length];
        GetWindowTextW(hwnd, &mut title);

        let title_osstring = OsString::from_wide(&title[..title_length - 1]);
        if let Ok(title_string) = title_osstring.into_string() {
            let titles = &mut *(lparam.0 as *mut Vec<String>);
            if title_string.contains("Fightcade FBNeo") {
                titles.push(title_string);
            }
        }
    }
    true.into()
}

fn get_window_titles() -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_windows_callback), LPARAM(&mut titles as *mut _ as isize));
    }
    titles
}

fn main() {
    let window_titles = get_window_titles();

    //only print window titles that contain "Fightcade FBNeo"
    for title in window_titles {
        println!("{}", title);
    }
}
