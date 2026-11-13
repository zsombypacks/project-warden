fn main() {
    #[cfg(windows)]
    {
        let mut r = winres::WindowsResource::new();
        r.set_icon("assets/icon.ico");
        r.set("ProductName", "Project Warden");
        r.compile().unwrap();
    }
}
