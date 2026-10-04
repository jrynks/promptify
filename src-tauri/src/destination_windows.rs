use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use promptify_core::context::WindowIdentity;
use promptify_core::delivery::Destination;
use promptify_core::pipeline::BackendError;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextEditPattern,
    IUIAutomationValuePattern, UIA_TextEditPatternId, UIA_ValuePatternId,
};

type Request = (
    WindowIdentity,
    mpsc::Sender<Result<Destination, BackendError>>,
);
static INSPECTOR: OnceLock<Result<mpsc::SyncSender<Request>, String>> = OnceLock::new();

pub fn inspect(window: &WindowIdentity) -> Result<Destination, BackendError> {
    let sender = INSPECTOR
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<Request>(1);
            std::thread::Builder::new()
                .name("destination-uia".into())
                .spawn(move || unsafe {
                    let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
                    let automation = initialized.ok().and_then(|()| {
                        CoCreateInstance::<_, IUIAutomation>(
                            &CUIAutomation,
                            None,
                            CLSCTX_INPROC_SERVER,
                        )
                    });
                    let mut previous: Option<IUIAutomationElement> = None;
                    let mut next_token = 0u64;
                    for (window, reply) in receiver {
                        let result = match &automation {
                            Ok(automation) => capture_ready(
                                || capture(automation, window, &mut previous, &mut next_token),
                                || std::thread::sleep(Duration::from_millis(50)),
                            )
                            .map_err(|error| {
                                BackendError(format!("Focused input inspection failed: {error}"))
                            }),
                            Err(error) => Err(BackendError(format!(
                                "Accessibility inspection unavailable: {error}"
                            ))),
                        };
                        let _ = reply.send(result);
                    }
                })
                .map_err(|error| error.to_string())?;
            Ok(sender)
        })
        .as_ref()
        .map_err(|error| BackendError(error.clone()))?;
    let (reply, receiver) = mpsc::channel();
    sender
        .try_send((*window, reply))
        .map_err(|error| BackendError(format!("Focused input inspection is busy: {error}")))?;
    receiver
        .recv_timeout(Duration::from_millis(500))
        .map_err(|error| {
            BackendError(format!(
                "Focused input inspection did not complete: {error}"
            ))
        })?
}

fn capture_ready(
    mut read: impl FnMut() -> windows::core::Result<Destination>,
    mut wait: impl FnMut(),
) -> windows::core::Result<Destination> {
    // Providers can initialize their editable patterns after the first UIA query.
    let mut destination = read()?;
    for _ in 0..3 {
        if destination.writable.is_some() || destination.secure != Some(false) {
            break;
        }
        wait();
        destination = read()?;
    }
    Ok(destination)
}

unsafe fn capture(
    automation: &IUIAutomation,
    window: WindowIdentity,
    previous: &mut Option<IUIAutomationElement>,
    next_token: &mut u64,
) -> windows::core::Result<Destination> {
    unsafe {
        let focused = automation.GetFocusedElement()?;
        let root =
            automation.ElementFromHandle(HWND(window.handle as usize as *mut std::ffi::c_void))?;
        let walker = automation.RawViewWalker()?;
        let mut ancestor = focused.clone();
        let mut belongs = false;
        for _ in 0..64 {
            if automation.CompareElements(&ancestor, &root)?.as_bool() {
                belongs = true;
                break;
            }

            match walker.GetParentElement(&ancestor) {
                Ok(parent) => ancestor = parent,
                Err(_) => break,
            }
        }
        if !belongs {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                "The focused control does not belong to the captured window.",
            ));
        }
        let same = match previous.as_ref() {
            Some(previous) => automation.CompareElements(previous, &focused)?.as_bool(),
            None => false,
        };
        if !same {
            *next_token += 1;
            *previous = Some(focused.clone());
        }
        let secure = focused.CurrentIsPassword()?.as_bool();
        let enabled = focused.CurrentIsEnabled()?.as_bool();
        let writable = if secure || !enabled {
            Some(false)
        } else if let Ok(value) =
            focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        {
            Some(!value.CurrentIsReadOnly()?.as_bool())
        } else if focused
            .GetCurrentPatternAs::<IUIAutomationTextEditPattern>(UIA_TextEditPatternId)
            .is_ok()
        {
            Some(true)
        } else {
            None
        };
        Ok(Destination {
            token: Some(format!("uia:{}:{next_token}", window.handle)),
            writable,
            secure: Some(secure),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_only_rechecks_unknown_metadata_and_never_infers_writability() {
        let mut reads = 0;
        let mut waits = 0;
        let destination = capture_ready(
            || {
                reads += 1;
                Ok(Destination {
                    token: Some("focused".into()),
                    writable: (reads == 3).then_some(true),
                    secure: Some(false),
                })
            },
            || waits += 1,
        )
        .unwrap();
        assert!(destination.confirmed());
        assert_eq!((reads, waits), (3, 2));
        let mut reads = 0;
        let destination = capture_ready(
            || {
                reads += 1;
                Ok(Destination {
                    token: Some("unknown".into()),
                    writable: None,
                    secure: Some(false),
                })
            },
            || {},
        )
        .unwrap();
        assert!(!destination.confirmed());
        assert_eq!(reads, 4);
    }

    #[test]
    fn protected_fields_and_inspection_errors_do_not_retry() {
        for writable in [Some(false), Some(true)] {
            capture_ready(
                || {
                    Ok(Destination {
                        token: Some("known".into()),
                        writable,
                        secure: Some(false),
                    })
                },
                || panic!("known controls need no readiness retry"),
            )
            .unwrap();
        }
        capture_ready(
            || {
                Ok(Destination {
                    token: Some("secure".into()),
                    writable: None,
                    secure: Some(true),
                })
            },
            || panic!("protected controls must not retry"),
        )
        .unwrap();
        assert!(
            capture_ready(
                || Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80004005u32 as i32
                ))),
                || panic!("errors must surface")
            )
            .is_err()
        );
    }
}
