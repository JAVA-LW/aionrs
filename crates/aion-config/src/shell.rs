use std::process::Output;

use tokio::process::Command;

pub struct ShellInfo {
    pub program: &'static str,
    pub flag: &'static str,
}

pub fn shell_info() -> ShellInfo {
    if cfg!(windows) {
        ShellInfo {
            program: "cmd",
            flag: "/C",
        }
    } else {
        ShellInfo {
            program: "sh",
            flag: "-c",
        }
    }
}

/// Build a cross-platform shell command runner for a raw command string.
pub fn shell_command_builder(command: &str) -> Command {
    let info = shell_info();
    let mut cmd = Command::new(info.program);
    cmd.arg(info.flag).arg(command);
    cmd
}

/// Backward-compatible alias for callers that build and customize the command.
pub fn new_shell_command(command: &str) -> Command {
    shell_command_builder(command)
}

pub async fn shell_command(command: &str) -> std::io::Result<Output> {
    shell_command_builder(command).output().await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_info_returns_platform_appropriate_values() {
        let info = shell_info();
        if cfg!(windows) {
            assert_eq!(info.program, "cmd");
            assert_eq!(info.flag, "/C");
        } else {
            assert_eq!(info.program, "sh");
            assert_eq!(info.flag, "-c");
        }
    }

    #[tokio::test]
    async fn shell_command_runs_echo() {
        let output = shell_command("echo hello")
            .await
            .expect("shell_command failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[tokio::test]
    async fn shell_command_builder_allows_env_and_cwd() {
        let tmp = std::env::temp_dir();
        let command = if cfg!(windows) {
            "echo %MY_VAR%"
        } else {
            "echo $MY_VAR"
        };
        let output = shell_command_builder(command)
            .env("MY_VAR", "test_value")
            .current_dir(&tmp)
            .output()
            .await
            .expect("builder failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test_value"));
    }
}
