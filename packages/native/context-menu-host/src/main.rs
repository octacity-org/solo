mod protocol;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod scripted;

fn main() {
    #[cfg(target_os = "macos")]
    {
        let result = macos::run();
        cleanup_staged_executable();
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Native context menus require macOS");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn cleanup_staged_executable() {
    // The parent may have exited before its TempDir destructor could run.
    // Remove only this running image in the private staging layout, then its
    // empty directory. Never recursively remove a path or touch shipped files.
    // macOS commonly supplies /var/... while temp_dir().canonicalize() yields
    // /private/var/.... Compare physical paths on BOTH sides. Canonicalizing
    // first also prevents a staged symlink from authorizing a shipped image.
    let Ok(executable) = std::env::current_exe().and_then(|path| path.canonicalize()) else {
        return;
    };
    if executable
        .file_name()
        .is_none_or(|name| name != "solo-context-menu")
    {
        return;
    }
    let Some(directory) = executable.parent() else {
        return;
    };
    if !directory
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("solo-context-menu-"))
    {
        return;
    }
    let Ok(temporary_root) = std::env::temp_dir().canonicalize() else {
        return;
    };
    if directory.parent() != Some(temporary_root.as_path()) {
        return;
    }
    if std::fs::remove_file(&executable).is_ok() {
        let _ = std::fs::remove_dir(directory);
    }
}
