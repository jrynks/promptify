use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use arboard::ImageData;
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, HWND_MESSAGE, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

enum Request {
    Data {
        text: Option<String>,
        html: Option<String>,
        files: Option<Vec<PathBuf>>,
        image: Option<ImageData<'static>>,
        expected: u32,
        reply: mpsc::Sender<Result<(), String>>,
    },
    Clear {
        expected: u32,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

static RESTORER: OnceLock<Result<mpsc::SyncSender<Request>, String>> = OnceLock::new();

pub fn restore_text(text: &str, expected: Option<u32>) -> Result<(), String> {
    let expected = expected
        .ok_or("The clipboard change counter is unavailable; its contents were left untouched.")?;
    dispatch(|reply| Request::Data {
        text: Some(text.to_owned()),
        html: None,
        files: None,
        image: None,
        expected,
        reply,
    })
}

pub fn restore_html(html: &str, text: Option<&str>, expected: Option<u32>) -> Result<(), String> {
    let expected = expected
        .ok_or("The clipboard change counter is unavailable; its contents were left untouched.")?;
    dispatch(|reply| Request::Data {
        text: text.map(str::to_owned),
        html: Some(html.to_owned()),
        files: None,
        image: None,
        expected,
        reply,
    })
}

pub fn restore_files(files: &[PathBuf], expected: Option<u32>) -> Result<(), String> {
    let expected = expected
        .ok_or("The clipboard change counter is unavailable; its contents were left untouched.")?;
    dispatch(|reply| Request::Data {
        text: None,
        html: None,
        files: Some(files.to_vec()),
        image: None,
        expected,
        reply,
    })
}

pub fn restore_image(image: &ImageData<'static>, expected: Option<u32>) -> Result<(), String> {
    let expected = expected
        .ok_or("The clipboard change counter is unavailable; its contents were left untouched.")?;
    dispatch(|reply| Request::Data {
        text: None,
        html: None,
        files: None,
        image: Some(image.to_owned_img()),
        expected,
        reply,
    })
}

pub fn clear(expected: Option<u32>) -> Result<(), String> {
    let expected = expected
        .ok_or("The clipboard change counter is unavailable; its contents were left untouched.")?;
    dispatch(|reply| Request::Clear { expected, reply })
}

fn dispatch(
    request: impl FnOnce(mpsc::Sender<Result<(), String>>) -> Request,
) -> Result<(), String> {
    let sender = RESTORER.get_or_init(start).as_ref().map_err(Clone::clone)?;
    let (reply, receiver) = mpsc::channel();
    sender
        .try_send(request(reply))
        .map_err(|error| format!("Windows clipboard restoration is busy: {error}"))?;
    receiver
        .recv_timeout(Duration::from_millis(500))
        .map_err(|error| format!("Windows clipboard restoration did not complete: {error}"))?
}

fn start() -> Result<mpsc::SyncSender<Request>, String> {
    let (sender, receiver) = mpsc::sync_channel::<Request>(1);
    let (ready, initialized) = mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard-restore".into())
        .spawn(move || {
            let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Promptify Clipboard Restoration"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            if window.is_null() {
                let _ = ready.send(Err(format!(
                    "Clipboard restoration owner creation failed: {}",
                    std::io::Error::last_os_error()
                )));
                return;
            }
            let _ = ready.send(Ok(()));
            loop {
                let mut message = MSG::default();
                for _ in 0..64 {
                    if unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) }
                        == 0
                    {
                        break;
                    }
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                match receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(Request::Data {
                        text,
                        html,
                        files,
                        image,
                        expected,
                        reply,
                    }) => {
                        let _ = reply.send(write(
                            window,
                            expected,
                            text.as_deref(),
                            html.as_deref(),
                            files.as_deref(),
                            image.as_ref(),
                        ));
                    }
                    Ok(Request::Clear { expected, reply }) => {
                        let _ = reply.send(write(window, expected, None, None, None, None));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|error| format!("Clipboard restoration thread could not start: {error}"))?;
    initialized
        .recv_timeout(Duration::from_secs(1))
        .map_err(|error| format!("Clipboard restoration window did not initialize: {error}"))??;
    Ok(sender)
}

struct Allocation(HANDLE);

impl Allocation {
    fn text(text: &str) -> Result<Self, String> {
        if text.contains('\0') {
            return Err("The saved clipboard text contains an embedded null character.".into());
        }
        let bytes: Vec<u8> = text
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_ne_bytes)
            .collect();
        Self::bytes(&bytes)
    }

    fn bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("Clipboard format cannot have an empty allocation.".into());
        }
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) };
        if memory.is_null() {
            return Err(format!(
                "Saved clipboard text allocation failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let allocation = Self(memory);
        let pointer = unsafe { GlobalLock(memory) };
        if pointer.is_null() {
            return Err(format!(
                "Saved clipboard text could not be locked: {}",
                std::io::Error::last_os_error()
            ));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
            GlobalUnlock(memory);
        }
        Ok(allocation)
    }

    fn publish(mut self, format: u32) -> Result<(), String> {
        if unsafe { SetClipboardData(format, self.0) }.is_null() {
            return Err(format!(
                "Saved clipboard format {format} could not be restored: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.0 = std::ptr::null_mut();
        Ok(())
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() && !unsafe { GlobalFree(self.0) }.is_null() {
            log::error!(
                "Could not release failed clipboard restoration allocation: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        if unsafe { CloseClipboard() } == 0 {
            log::error!(
                "Could not close Windows clipboard after restoration: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

fn write(
    owner: HWND,
    expected: u32,
    text: Option<&str>,
    html: Option<&str>,
    files: Option<&[PathBuf]>,
    image: Option<&ImageData<'static>>,
) -> Result<(), String> {
    let text = text.map(Allocation::text).transpose()?;
    let html = html
        .map(html_clipboard_payload)
        .transpose()?
        .map(|payload| Allocation::bytes(&payload))
        .transpose()?;
    let files = files
        .map(file_drop_payload)
        .transpose()?
        .map(|payload| Allocation::bytes(&payload))
        .transpose()?;
    let image = image
        .map(dibv5_payload)
        .transpose()?
        .map(|payload| Allocation::bytes(&payload))
        .transpose()?;
    let html_format = if html.is_some() {
        let name: Vec<u16> = "HTML Format".encode_utf16().chain(Some(0)).collect();
        let format = unsafe {
            windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(name.as_ptr())
        };
        if format == 0 {
            return Err(format!(
                "HTML clipboard format registration failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Some(format)
    } else {
        None
    };
    for attempt in 0..5 {
        if unsafe { OpenClipboard(owner) } != 0 {
            let _open = Open;
            if unsafe { GetClipboardSequenceNumber() } != expected {
                return Err("The clipboard changed since insertion; its newer contents were left untouched.".into());
            }
            if unsafe { EmptyClipboard() } == 0 {
                return Err(format!(
                    "The clipboard could not be prepared for restoration: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if let Some(allocation) = html {
                let format = html_format.ok_or("The HTML clipboard format was not registered.")?;
                allocation.publish(format)?;
            }
            if let Some(allocation) = files {
                allocation.publish(15)?;
            }
            if let Some(allocation) = image {
                allocation.publish(17)?;
            }
            if let Some(allocation) = text {
                allocation.publish(13)?;
            }
            return Ok(());
        }

        if attempt != 4 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Err(format!(
        "Could not open the clipboard for restoration: {}",
        std::io::Error::last_os_error()
    ))
}

fn file_drop_payload(files: &[PathBuf]) -> Result<Vec<u8>, String> {
    if files.is_empty() {
        return Err("The saved file list is empty.".into());
    }
    let paths = files
        .iter()
        .flat_map(|path| path.as_os_str().encode_wide().chain(Some(0)))
        .chain(Some(0));
    let mut payload = Vec::with_capacity(
        20 + files
            .iter()
            .map(|path| path.as_os_str().len() * 2 + 2)
            .sum::<usize>()
            + 2,
    );
    payload.extend_from_slice(&20u32.to_le_bytes());
    payload.extend_from_slice(&0i32.to_le_bytes());
    payload.extend_from_slice(&0i32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&1u32.to_le_bytes());
    for unit in paths {
        payload.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(payload)
}

fn dibv5_payload(image: &ImageData<'static>) -> Result<Vec<u8>, String> {
    let width = u32::try_from(image.width).map_err(|_| "Clipboard image width is too large")?;
    let height = u32::try_from(image.height).map_err(|_| "Clipboard image height is too large")?;
    let byte_count = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Clipboard image dimensions overflow")?;
    if width == 0 || height == 0 || image.bytes.len() != byte_count {
        return Err("Clipboard image dimensions do not match its pixel data".into());
    }
    let size = u32::try_from(byte_count).map_err(|_| "Clipboard image data is too large")?;
    let mut payload = Vec::with_capacity(124 + byte_count);
    payload.extend_from_slice(&124u32.to_le_bytes());
    let width = i32::try_from(width)
        .map_err(|_| "Clipboard image width exceeds the Windows bitmap limit")?;
    let height = i32::try_from(height)
        .map_err(|_| "Clipboard image height exceeds the Windows bitmap limit")?;
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(
        &height
            .checked_neg()
            .ok_or("Clipboard image height exceeds the Windows bitmap limit")?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.extend_from_slice(&32u16.to_le_bytes());
    payload.extend_from_slice(&3u32.to_le_bytes());
    payload.extend_from_slice(&size.to_le_bytes());
    payload.extend_from_slice(&0i32.to_le_bytes());
    payload.extend_from_slice(&0i32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&0x00ff0000u32.to_le_bytes());
    payload.extend_from_slice(&0x0000ff00u32.to_le_bytes());
    payload.extend_from_slice(&0x000000ffu32.to_le_bytes());
    payload.extend_from_slice(&0xff000000u32.to_le_bytes());
    payload.extend_from_slice(&0x73524742u32.to_le_bytes());
    payload.extend_from_slice(&[0; 48]);
    payload.extend_from_slice(&4u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    for pixel in image.bytes.chunks_exact(4) {
        payload.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Ok(payload)
}

fn html_clipboard_payload(fragment: &str) -> Result<Vec<u8>, String> {
    let prefix = "<html><body><!--StartFragment-->";
    let suffix = "<!--EndFragment--></body></html>";
    let header_template = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let start_html = header_template.len();
    let start_fragment = start_html + prefix.len();
    let end_fragment = start_fragment
        .checked_add(fragment.len())
        .ok_or("HTML clipboard payload is too large")?;
    let end_html = end_fragment
        .checked_add(suffix.len())
        .ok_or("HTML clipboard payload is too large")?;
    if [start_html, start_fragment, end_fragment, end_html]
        .iter()
        .any(|offset| *offset > 9_999_999_999)
    {
        return Err("HTML clipboard payload exceeds the supported size".into());
    }
    let header = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );
    let mut payload = Vec::with_capacity(end_html);
    payload.extend_from_slice(header.as_bytes());
    payload.extend_from_slice(prefix.as_bytes());
    payload.extend_from_slice(fragment.as_bytes());
    payload.extend_from_slice(suffix.as_bytes());
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::{dibv5_payload, file_drop_payload, html_clipboard_payload};
    use arboard::ImageData;
    use std::borrow::Cow;
    use std::path::PathBuf;

    #[test]
    fn html_payload_offsets_are_utf8_byte_offsets_and_preserve_fragment() {
        let fragment = "<b>naïve 🚀</b>";
        let payload = html_clipboard_payload(fragment).unwrap();
        let header_end = payload
            .windows(6)
            .position(|window| window == b"<html>")
            .unwrap();
        let text = std::str::from_utf8(&payload).unwrap();
        assert!(text.contains(&format!("StartHTML:{header_end:010}")));
        let fragment_start = text.split("StartFragment:").nth(1).unwrap()[..10]
            .parse::<usize>()
            .unwrap();
        let fragment_end = text.split("EndFragment:").nth(1).unwrap()[..10]
            .parse::<usize>()
            .unwrap();
        assert_eq!(&payload[fragment_start..fragment_end], fragment.as_bytes());
        assert_eq!(
            text.split("EndHTML:").nth(1).unwrap()[..10]
                .parse::<usize>()
                .unwrap(),
            payload.len()
        );
    }

    #[test]
    fn file_drop_payload_uses_wide_double_terminated_path_list() {
        use std::os::windows::ffi::OsStrExt;
        let files = vec![
            PathBuf::from(r"C:\Promptify\first.txt"),
            PathBuf::from(r"C:\Promptify\second.txt"),
        ];
        let payload = file_drop_payload(&files).unwrap();
        assert_eq!(u32::from_le_bytes(payload[..4].try_into().unwrap()), 20);
        assert_eq!(u32::from_le_bytes(payload[16..20].try_into().unwrap()), 1);
        let actual: Vec<u16> = payload[20..]
            .chunks_exact(2)
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        let expected: Vec<u16> = files
            .iter()
            .flat_map(|path| path.as_os_str().encode_wide().chain(Some(0)))
            .chain(Some(0))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn dibv5_payload_preserves_rgba_pixels_as_top_down_bgra() {
        let image = ImageData {
            width: 1,
            height: 1,
            bytes: Cow::Owned(vec![1, 2, 3, 4]),
        };
        let payload = dibv5_payload(&image).unwrap();
        assert_eq!(u32::from_le_bytes(payload[..4].try_into().unwrap()), 124);
        assert_eq!(i32::from_le_bytes(payload[8..12].try_into().unwrap()), -1);
        assert_eq!(u16::from_le_bytes(payload[14..16].try_into().unwrap()), 32);
        assert_eq!(&payload[124..], &[3, 2, 1, 4]);
        let invalid = ImageData {
            width: 2,
            height: 1,
            bytes: Cow::Owned(vec![1, 2, 3, 4]),
        };
        assert!(dibv5_payload(&invalid).is_err());
    }
}
