use anyhow::{Result, anyhow};
use std::path::PathBuf;
use tauri::AppHandle;

#[cfg(target_os = "macos")]
fn pick(save: bool) -> Result<Option<PathBuf>> {
    use objc2::{MainThreadMarker, class, msg_send, rc::Retained, runtime::AnyObject};
    use objc2_foundation::{NSString, NSURL};
    let _main =
        MainThreadMarker::new().ok_or_else(|| anyhow!("file dialog requires main thread"))?;
    unsafe {
        let panel: Retained<AnyObject> = if save {
            msg_send![class!(NSSavePanel), savePanel]
        } else {
            msg_send![class!(NSOpenPanel), openPanel]
        };
        let _: () = msg_send![&*panel,setTitle:&*NSString::from_str(if save{"Export AirHand invitation (contains a secret)"}else{"Import AirHand pairing file"})];
        if save {
            let _: () = msg_send![&*panel,setNameFieldStringValue:&*NSString::from_str("AirHand.airhand-pair")];
        } else {
            let _: () = msg_send![&*panel,setAllowsMultipleSelection:false];
            let _: () = msg_send![&*panel,setCanChooseDirectories:false];
        }
        let response: isize = msg_send![&*panel, runModal];
        if response != 1 {
            return Ok(None);
        }
        let url: Option<Retained<NSURL>> = msg_send![&*panel, URL];
        Ok(url
            .and_then(|u| u.path())
            .map(|s| PathBuf::from(s.to_string())))
    }
}
#[cfg(not(target_os = "macos"))]
fn pick(_: bool) -> Result<Option<PathBuf>> {
    Err(anyhow!("AirDrop pairing requires macOS"))
}

pub async fn choose(app: &AppHandle, save: bool) -> Result<Option<PathBuf>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(pick(save));
    })
    .map_err(|_| anyhow!("file dialog unavailable"))?;
    rx.await.map_err(|_| anyhow!("file dialog closed"))?
}

pub async fn share(app: &AppHandle, path: PathBuf) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
            use objc2_foundation::{NSArray, NSString, NSURL};
            let result = unsafe {
                let name = NSString::from_str("com.apple.share.AirDrop");
                let service: Option<Retained<AnyObject>> =
                    msg_send![class!(NSSharingService),sharingServiceNamed:&*name];
                if let Some(service) = service {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                    let items = NSArray::from_retained_slice(&[url]);
                    let _: () = msg_send![&*service,performWithItems:&*items];
                    // NSSharingService is asynchronous; retain it through the
                    // transfer instead of releasing it as this command returns.
                    thread_local! {static SHARES: std::cell::RefCell<Vec<Retained<AnyObject>>> = const {std::cell::RefCell::new(Vec::new())};}
                    SHARES.with(|shares| {let mut shares=shares.borrow_mut();shares.push(service);if shares.len()>8{shares.remove(0);}});
                    Ok(())
                } else {
                    Err(anyhow!("AirDrop unavailable"))
                }
            };
            let _ = tx.send(result);
        })
        .map_err(|_| anyhow!("AirDrop unavailable"))?;
        rx.await.map_err(|_| anyhow!("AirDrop closed"))?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, path);
        Err(anyhow!("AirDrop requires macOS"))
    }
}

pub fn read(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|_| anyhow!("cannot open pairing file"))?;
    let info = file
        .metadata()
        .map_err(|_| anyhow!("cannot inspect pairing file"))?;
    anyhow::ensure!(
        info.is_file() && info.len() <= 65536,
        "invalid pairing file"
    );
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("cannot read pairing file"))?;
    anyhow::ensure!(bytes.len() <= 65536, "pairing file too large");
    Ok(bytes)
}
pub fn write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| anyhow!("choose a new filename; existing files are preserved"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| anyhow!("cannot export pairing file"))
}
