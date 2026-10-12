//! Drags a file out of the window with OLE (`DoDragDrop`), showing a drag picture we supply.
//!
//! The data object is the shell's own for the file (what Explorer drags: CF_HDROP, shell ID list,
//! file name formats), so DAWs and folders take the drop as they would from Explorer.
//!
//! The picture is a click-through topmost window that follows the cursor, not the shell's drag
//! image (`IDragSourceHelper`): that one is drawn by the drop target and only shows over Explorer,
//! not over this window or a DAW.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{implement, w, BOOL, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINT, SIZE, S_OK,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, DROPEFFECT, DROPEFFECT_COPY,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetCursorPos, SetWindowPos, UpdateLayeredWindow, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW, ULW_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// Where the picture's top-left corner sits relative to the cursor's hotspot (clear of the copy
/// cursor's own "+" badge).
const OFFSET: POINT = POINT { x: 32, y: 10 };

/// The drag picture: `width` x `height` pixels, premultiplied BGRA, top row first.
pub struct DragPicture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The window showing the picture, and what it needs to be freed.
struct Overlay {
    hwnd: HWND,
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
}

impl Overlay {
    /// Shows `picture` at the cursor. A layered window with per-pixel alpha, which never takes the
    /// mouse (so the drop target underneath still gets it) or the focus.
    unsafe fn new(picture: &DragPicture) -> windows::core::Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: picture.width as i32,
                // Negative: top-down rows.
                biHeight: -(picture.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        // SAFETY: on success `bits` holds width*height*4 bytes, the length `drag` checked.
        std::ptr::copy_nonoverlapping(picture.bgra.as_ptr(), bits.cast::<u8>(), picture.bgra.len());
        let dc = CreateCompatibleDC(None);
        SelectObject(dc, bitmap.into());

        // A built-in window class ("STATIC"), so none has to be registered.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("STATIC"),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            picture.width as i32,
            picture.height as i32,
            None,
            None,
            None,
            None,
        )?;
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            None,
            None,
            Some(&SIZE {
                cx: picture.width as i32,
                cy: picture.height as i32,
            }),
            Some(dc),
            Some(&POINT { x: 0, y: 0 }),
            Default::default(),
            Some(&blend),
            ULW_ALPHA,
        )?;
        let overlay = Self { hwnd, dc, bitmap };
        overlay.follow_cursor();
        Ok(overlay)
    }

    fn follow_cursor(&self) {
        let mut pt = POINT::default();
        // SAFETY: `hwnd` is our live window; `pt` outlives the calls.
        unsafe {
            if GetCursorPos(&mut pt).is_ok() {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    pt.x + OFFSET.x,
                    pt.y + OFFSET.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: the handles are ours and not used after this.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bitmap.into());
        }
    }
}

#[implement(IDropSource)]
struct DropSource {
    overlay: Option<Overlay>,
}

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        // Called on every mouse move and key change: the picture follows the cursor from here.
        if let Some(overlay) = &self.overlay {
            overlay.follow_cursor();
        }
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (grfkeystate & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Runs a copy drag of `path` with `picture` next to the cursor. Returns once the drag has ended
/// (dropped or cancelled).
pub fn drag(path: &Path, picture: &DragPicture) -> Result<(), String> {
    if picture.bgra.len() != (picture.width * picture.height * 4) as usize {
        return Err("drag picture has the wrong size".into());
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: plain COM/Win32 calls on the thread that runs the command (the main one, an STA);
    // every pointer passed lives through its call.
    unsafe {
        // Already initialised by the webview (S_FALSE); only a failure matters.
        OleInitialize(None).map_err(|e| e.to_string())?;

        let item: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(|e| e.to_string())?;
        let data: IDataObject = item
            .BindToHandler(None, &BHID_DataObject)
            .map_err(|e| e.to_string())?;

        // Without the picture the drag still works, so a failure to make it is not one.
        let source: IDropSource = DropSource {
            overlay: Overlay::new(picture).ok(),
        }
        .into();
        let mut effect = DROPEFFECT::default();
        let result = DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect);
        if result == DRAGDROP_S_DROP || result == DRAGDROP_S_CANCEL {
            Ok(())
        } else {
            Err(windows::core::Error::from(result).to_string())
        }
    }
}
