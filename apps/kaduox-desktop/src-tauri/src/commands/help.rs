fn help_url(page: &str) -> Result<&'static str, String> {
    match page {
        "project" => Ok("https://github.com/kaduoxzero/Kaduox-SSH"),
        "manual" => {
            Ok("https://github.com/kaduoxzero/Kaduox-SSH/blob/develop/docs/DESKTOP_GUIDE.zh-CN.md")
        }
        "releases" => Ok("https://github.com/kaduoxzero/Kaduox-SSH/releases"),
        _ => Err("不支持的帮助页面".into()),
    }
}

#[tauri::command]
pub fn open_help_link(page: String) -> Result<(), String> {
    let url = help_url(&page)?;
    // 只接受内置页面标识，不把任意 URL、路径或命令交给系统执行。
    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer.exe");
        command.creation_flags(0x08000000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_known_https_pages_are_opened() {
        for page in ["project", "manual", "releases"] {
            assert!(
                help_url(page)
                    .unwrap()
                    .starts_with("https://github.com/kaduoxzero/Kaduox-SSH")
            );
        }
        assert!(help_url("file:///C:/Windows/system32/cmd.exe").is_err());
        assert!(help_url("https://example.com").is_err());
    }
}
