use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

use aion_config::shell::new_shell_command;
use aion_protocol::events::{ToolCategory, ToolOutputStream};
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const PROCESS_POLL_INTERVAL_MS: u64 = 20;
const OUTPUT_CHUNK_SIZE: usize = 8192;

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "Executes a shell command and returns its output.\n\n\
         IMPORTANT: Do NOT use Bash when a dedicated tool is available:\n\
         - File search: use Glob (not find or ls)\n\
         - Content search: use Grep (not grep or rg)\n\
         - Read files: use Read (not cat, head, or tail)\n\
         - Edit files: use Edit (not sed or awk)\n\
         - Write files: use Write (not echo or cat with heredoc)\n\n\
         # Instructions\n\
         - Use absolute paths to avoid working directory confusion.\n\
         - When issuing multiple independent commands, make parallel tool calls \
         instead of chaining them. Use `&&` only when commands depend on each other.\n\
         - You may specify an optional timeout in milliseconds (default 120000, max 600000).\n\n\
         # Git safety\n\
         - Never force push, reset --hard, or use --no-verify unless explicitly asked.\n\
         - Prefer creating new commits over amending existing ones."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000, max 600000)"
                }
            },
            "required": ["command"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        execute_bash_with_output(input, |_, _| {}).await
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn describe(&self, input: &Value) -> String {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        format!("Execute: {}", crate::truncate_utf8(cmd, 80))
    }
}

pub async fn execute_bash_with_output<F>(input: Value, mut on_output: F) -> ToolResult
where
    F: FnMut(ToolOutputStream, &str),
{
    let Some(command) = input["command"].as_str() else {
        return ToolResult {
            content: "Missing required parameter: command".to_string(),
            is_error: true,
        };
    };

    let timeout_ms = input["timeout"]
        .as_u64()
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(MAX_TIMEOUT_MS);

    let mut shell = new_shell_command(command);
    shell
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match shell.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ToolResult {
                content: format!("Failed to execute command: {}", e),
                is_error: true,
            };
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, mut rx) = mpsc::unbounded_channel::<(ToolOutputStream, Vec<u8>)>();
    let stdout_task = stdout
        .map(|pipe| tokio::spawn(read_output_pipe(pipe, ToolOutputStream::Stdout, tx.clone())));
    let stderr_task =
        stderr.map(|pipe| tokio::spawn(read_output_pipe(pipe, ToolOutputStream::Stderr, tx)));

    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to wait for command: {}", e),
                    is_error: true,
                };
            }
        }

        tokio::select! {
            _ = &mut timeout => {
                let _ = child.kill().await;
                break None;
            }
            maybe_chunk = rx.recv() => {
                if let Some((stream, bytes)) = maybe_chunk {
                    capture_output_chunk(
                        stream,
                        &bytes,
                        &mut stdout_bytes,
                        &mut stderr_bytes,
                        &mut on_output,
                    );
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(PROCESS_POLL_INTERVAL_MS)) => {}
        }
    };

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
    while let Ok((stream, bytes)) = rx.try_recv() {
        capture_output_chunk(
            stream,
            &bytes,
            &mut stdout_bytes,
            &mut stderr_bytes,
            &mut on_output,
        );
    }

    let Some(status) = status else {
        return ToolResult {
            content: format!("Command timed out after {}ms", timeout_ms),
            is_error: true,
        };
    };

    let stdout_text = String::from_utf8_lossy(&stdout_bytes);
    let stderr_text = String::from_utf8_lossy(&stderr_bytes);
    let exit_code = status.code().unwrap_or(-1);
    let content = format!(
        "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
        exit_code, stdout_text, stderr_text
    );

    ToolResult {
        content,
        is_error: exit_code != 0,
    }
}

async fn read_output_pipe<R>(
    mut pipe: R,
    stream: ToolOutputStream,
    tx: mpsc::UnboundedSender<(ToolOutputStream, Vec<u8>)>,
) where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0; OUTPUT_CHUNK_SIZE];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                if tx.send((stream, buffer[..n].to_vec())).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn capture_output_chunk<F>(
    stream: ToolOutputStream,
    bytes: &[u8],
    stdout_bytes: &mut Vec<u8>,
    stderr_bytes: &mut Vec<u8>,
    on_output: &mut F,
) where
    F: FnMut(ToolOutputStream, &str),
{
    let text = String::from_utf8_lossy(bytes);
    match stream {
        ToolOutputStream::Stdout => stdout_bytes.extend_from_slice(bytes),
        ToolOutputStream::Stderr => stderr_bytes.extend_from_slice(bytes),
    }
    on_output(stream, &text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_protocol::events::ToolOutputStream;

    #[tokio::test]
    async fn streams_stdout_chunks_to_observer() {
        let mut chunks: Vec<(ToolOutputStream, String)> = Vec::new();

        let result =
            execute_bash_with_output(json!({"command": "echo streamed"}), |stream, text| {
                chunks.push((stream, text.to_string()));
            })
            .await;

        assert!(!result.is_error);
        assert!(result.content.contains("streamed"));
        assert!(
            chunks
                .iter()
                .any(|(stream, text)| *stream == ToolOutputStream::Stdout
                    && text.contains("streamed"))
        );
    }

    #[tokio::test]
    async fn streams_stderr_chunks_to_observer() {
        let mut chunks: Vec<(ToolOutputStream, String)> = Vec::new();

        let result = execute_bash_with_output(
            json!({"command": "echo streamed-error 1>&2"}),
            |stream, text| {
                chunks.push((stream, text.to_string()));
            },
        )
        .await;

        assert!(!result.is_error);
        assert!(result.content.contains("streamed-error"));
        assert!(
            chunks
                .iter()
                .any(|(stream, text)| *stream == ToolOutputStream::Stderr
                    && text.contains("streamed-error"))
        );
    }

    #[test]
    fn final_output_preserves_utf8_split_across_chunks() {
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut observed = String::new();
        let bytes = [0xE5, 0xA5, 0xBD];

        capture_output_chunk(
            ToolOutputStream::Stdout,
            &bytes[..1],
            &mut stdout_bytes,
            &mut stderr_bytes,
            &mut |_, text| observed.push_str(text),
        );
        capture_output_chunk(
            ToolOutputStream::Stdout,
            &bytes[1..],
            &mut stdout_bytes,
            &mut stderr_bytes,
            &mut |_, text| observed.push_str(text),
        );

        let final_stdout = String::from_utf8_lossy(&stdout_bytes);
        assert_eq!(final_stdout, "\u{597d}");
        assert_ne!(observed, final_stdout);
    }
}
