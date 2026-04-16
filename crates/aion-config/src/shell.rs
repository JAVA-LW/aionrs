use tokio::process::Command;

/// Build a cross-platform shell command runner for a raw command string.
///
/// Centralizing shell selection keeps platform-specific branching in one place.
pub fn new_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Open a URL in the user's default browser.
///
/// Platform-specific launch behavior is centralized here so callers do not need
/// to branch on the OS.
pub fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
        Ok(())
    }
}
