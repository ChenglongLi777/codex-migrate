fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icons/CodexMigrate.ico");
        resource.set("ProductName", "Codex Migrate");
        resource.set(
            "FileDescription",
            "Migrate, repair, and export local Codex sessions",
        );
        resource.set("LegalCopyright", "Copyright (c) 2026 contributors");
        resource
            .compile()
            .expect("failed to compile Windows application resources");
    }
}
