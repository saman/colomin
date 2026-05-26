// Build script for embedding the Windows .exe icon.
//
// On Windows, the file-manager / taskbar / Alt-Tab icon for an .exe comes
// from a resource compiled into the binary at link time — *not* from any
// installer metadata. cargo-packager's WiX/MSI config only sets the
// installer/Add-Remove-Programs icon, so we need this build step to give
// the running binary its icon too.
//
// On non-Windows targets this is a no-op.
fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/Colomin.ico");
        res.compile().expect("failed to embed Windows resources");
    }
}
