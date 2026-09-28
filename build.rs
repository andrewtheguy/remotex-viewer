fn main() {
    // The icon goes into the Windows executable as a resource; macOS takes an app's
    // icon from its bundle instead.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=remotex-viewer.rc");
        println!("cargo:rerun-if-changed=icons/remotex-viewer.ico");
        embed_resource::compile("remotex-viewer.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compile remotex-viewer.rc");
    }
}
