use aion_types::llm::{
    AccountCreditsInfo, AccountLimitInfo, AccountLimitWindow, AccountLimitsInfo, ProviderMetadata,
};
use chrono::{DateTime, Local, Utc};

const MINUTES_PER_HOUR: u64 = 60;
const MINUTES_PER_DAY: u64 = 24 * MINUTES_PER_HOUR;
const MINUTES_PER_WEEK: u64 = 7 * MINUTES_PER_DAY;
const MINUTES_PER_MONTH: u64 = 30 * MINUTES_PER_DAY;
const ROUNDING_BIAS_MINUTES: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusTarget {
    ChatGpt,
}

impl StatusTarget {
    fn display_name(self) -> &'static str {
        match self {
            Self::ChatGpt => "ChatGPT",
        }
    }
}

pub(crate) fn parse_status_command(input: &str) -> Option<Result<StatusTarget, String>> {
    let trimmed = input.trim();
    if !trimmed.starts_with("/status") {
        return None;
    }

    let parts = trimmed.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        ["/status", "--chatgpt"] => Some(Ok(StatusTarget::ChatGpt)),
        ["/status"] => Some(Err("Usage: /status --chatgpt".to_string())),
        ["/status", flag] if flag.starts_with("--") => Some(Err(format!(
            "Unsupported status target: {flag}. Supported targets: --chatgpt"
        ))),
        ["/status", ..] => Some(Err("Usage: /status --chatgpt".to_string())),
        _ => Some(Err("Usage: /status --chatgpt".to_string())),
    }
}

pub(crate) fn render_repl_status(
    target: StatusTarget,
    current_model: &str,
    metadata: &ProviderMetadata,
) -> String {
    let mut lines = vec![
        format!("Status ({})", target.display_name()),
        format!("Model: {current_model}"),
    ];

    match metadata.account_limits.as_ref() {
        Some(account_limits) => {
            if let Some(plan_type) = account_limits.plan_type.as_deref() {
                lines.push(format!("Plan: {}", humanize_identifier(plan_type)));
            }

            let limit_lines = render_account_limit_lines(account_limits);
            if limit_lines.is_empty() {
                lines.push(format!(
                    "{} account quota: available, but no limit windows were returned.",
                    target.display_name()
                ));
            } else {
                lines.extend(limit_lines);
            }
        }
        None => lines.push(format!(
            "{} account quota: unavailable for this provider or auth mode.",
            target.display_name()
        )),
    }

    lines.join("\n")
}

fn render_account_limit_lines(account_limits: &AccountLimitsInfo) -> Vec<String> {
    let mut lines = Vec::new();

    for limit in &account_limits.limits {
        let bucket_prefix = display_bucket_prefix(limit);

        if let Some(primary) = limit.primary.as_ref() {
            let label = format_window_label(bucket_prefix.as_deref(), primary.window_minutes, "5h");
            lines.push(render_window_line(&label, primary));
        }

        if let Some(secondary) = limit.secondary.as_ref() {
            let label =
                format_window_label(bucket_prefix.as_deref(), secondary.window_minutes, "weekly");
            lines.push(render_window_line(&label, secondary));
        }

        if let Some(credits) = render_credits_line(bucket_prefix.as_deref(), limit.credits.as_ref())
        {
            lines.push(credits);
        }
    }

    lines
}

fn display_bucket_prefix(limit: &AccountLimitInfo) -> Option<String> {
    let name = limit
        .limit_name
        .as_deref()
        .or(limit.limit_id.as_deref())
        .map(|value| value.replace('_', "-"))?;

    if name.eq_ignore_ascii_case("codex") {
        None
    } else {
        Some(name)
    }
}

fn format_window_label(
    bucket_prefix: Option<&str>,
    window_minutes: Option<u64>,
    fallback: &str,
) -> String {
    let duration = window_minutes
        .map(describe_limit_window)
        .unwrap_or_else(|| fallback.to_string());
    let duration = capitalize_first(&duration);

    match bucket_prefix {
        Some(bucket_prefix) => format!("{bucket_prefix} {duration} limit"),
        None => format!("{duration} limit"),
    }
}

fn describe_limit_window(window_minutes: u64) -> String {
    let window_minutes = window_minutes.max(1);

    if window_minutes <= MINUTES_PER_DAY.saturating_add(ROUNDING_BIAS_MINUTES) {
        let adjusted = window_minutes.saturating_add(ROUNDING_BIAS_MINUTES);
        let hours = (adjusted / MINUTES_PER_HOUR).max(1);
        format!("{hours}h")
    } else if window_minutes <= MINUTES_PER_WEEK.saturating_add(ROUNDING_BIAS_MINUTES) {
        "weekly".to_string()
    } else if window_minutes <= MINUTES_PER_MONTH.saturating_add(ROUNDING_BIAS_MINUTES) {
        "monthly".to_string()
    } else {
        "annual".to_string()
    }
}

fn render_window_line(label: &str, window: &AccountLimitWindow) -> String {
    let used = window.used_percent.clamp(0.0, 100.0);
    let remaining = (100.0 - used).clamp(0.0, 100.0);
    let reset_suffix = format_reset_timestamp(window.resets_at)
        .map(|value| format!(", resets {value}"))
        .unwrap_or_default();

    format!(
        "{label}: {} left ({} used{reset_suffix})",
        format_percent(remaining),
        format_percent(used),
    )
}

fn render_credits_line(
    bucket_prefix: Option<&str>,
    credits: Option<&AccountCreditsInfo>,
) -> Option<String> {
    let credits = credits?;
    if !credits.has_credits {
        return None;
    }

    let label = match bucket_prefix {
        Some(bucket_prefix) => format!("{bucket_prefix} credits"),
        None => "Credits".to_string(),
    };

    let value = if credits.unlimited {
        "Unlimited".to_string()
    } else {
        let balance = credits.balance.as_deref()?.trim();
        if balance.is_empty() {
            return None;
        }
        format!("{balance} credits")
    };

    Some(format!("{label}: {value}"))
}

fn format_reset_timestamp(timestamp: Option<i64>) -> Option<String> {
    let timestamp = timestamp?;
    let dt = DateTime::<Utc>::from_timestamp(timestamp, 0)?;
    Some(
        dt.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    )
}

fn format_percent(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0}%", value.round())
    } else {
        format!("{value:.1}%")
    }
}

fn humanize_identifier(value: &str) -> String {
    value
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::llm::{AccountLimitInfo, AccountLimitsInfo, ProviderMetadata};

    #[test]
    fn parse_status_command_accepts_chatgpt_target() {
        assert_eq!(
            parse_status_command("/status --chatgpt"),
            Some(Ok(StatusTarget::ChatGpt))
        );
    }

    #[test]
    fn parse_status_command_rejects_missing_target() {
        assert_eq!(
            parse_status_command("/status"),
            Some(Err("Usage: /status --chatgpt".to_string()))
        );
    }

    #[test]
    fn parse_status_command_rejects_unknown_target() {
        assert_eq!(
            parse_status_command("/status --openai"),
            Some(Err(
                "Unsupported status target: --openai. Supported targets: --chatgpt".to_string()
            ))
        );
    }

    #[test]
    fn parse_status_command_ignores_other_commands() {
        assert_eq!(parse_status_command("/quit"), None);
    }

    #[test]
    fn render_status_shows_main_quota_windows_and_credits() {
        let status = render_repl_status(
            StatusTarget::ChatGpt,
            "gpt-5-codex",
            &ProviderMetadata {
                models: Vec::new(),
                account_limits: Some(AccountLimitsInfo {
                    plan_type: Some("pro".to_string()),
                    limits: vec![AccountLimitInfo {
                        limit_id: Some("codex".to_string()),
                        limit_name: None,
                        primary: Some(AccountLimitWindow {
                            used_percent: 45.0,
                            window_minutes: Some(300),
                            resets_at: None,
                        }),
                        secondary: Some(AccountLimitWindow {
                            used_percent: 30.0,
                            window_minutes: Some(10_080),
                            resets_at: None,
                        }),
                        credits: Some(AccountCreditsInfo {
                            has_credits: true,
                            unlimited: false,
                            balance: Some("38".to_string()),
                        }),
                    }],
                }),
            },
        );

        assert!(status.contains("Status (ChatGPT)"));
        assert!(status.contains("Model: gpt-5-codex"));
        assert!(status.contains("Plan: Pro"));
        assert!(status.contains("5h limit: 55% left (45% used)"));
        assert!(status.contains("Weekly limit: 70% left (30% used)"));
        assert!(status.contains("Credits: 38 credits"));
    }

    #[test]
    fn render_status_shows_additional_buckets_and_monthly_windows() {
        let status = render_repl_status(
            StatusTarget::ChatGpt,
            "gpt-5-codex",
            &ProviderMetadata {
                models: Vec::new(),
                account_limits: Some(AccountLimitsInfo {
                    plan_type: Some("self_serve_business_usage_based".to_string()),
                    limits: vec![AccountLimitInfo {
                        limit_id: Some("codex_other".to_string()),
                        limit_name: Some("codex_other".to_string()),
                        primary: Some(AccountLimitWindow {
                            used_percent: 12.0,
                            window_minutes: Some(30),
                            resets_at: None,
                        }),
                        secondary: Some(AccountLimitWindow {
                            used_percent: 70.0,
                            window_minutes: Some(43_200),
                            resets_at: None,
                        }),
                        credits: Some(AccountCreditsInfo {
                            has_credits: true,
                            unlimited: true,
                            balance: None,
                        }),
                    }],
                }),
            },
        );

        assert!(status.contains("Plan: Self Serve Business Usage Based"));
        assert!(status.contains("codex-other 1h limit: 88% left (12% used)"));
        assert!(status.contains("codex-other Monthly limit: 30% left (70% used)"));
        assert!(status.contains("codex-other credits: Unlimited"));
    }

    #[test]
    fn render_status_reports_unavailable_quota() {
        let status = render_repl_status(
            StatusTarget::ChatGpt,
            "gpt-4o",
            &ProviderMetadata::default(),
        );
        assert!(
            status.contains("ChatGPT account quota: unavailable for this provider or auth mode.")
        );
    }

    #[test]
    fn describe_limit_window_matches_codex_style_buckets() {
        assert_eq!(describe_limit_window(300), "5h");
        assert_eq!(describe_limit_window(10_080), "weekly");
        assert_eq!(describe_limit_window(43_200), "monthly");
        assert_eq!(describe_limit_window(43_204), "annual");
    }

    #[test]
    fn format_reset_timestamp_returns_localized_timestamp() {
        let formatted = format_reset_timestamp(Some(1_735_689_600)).expect("timestamp");
        assert_eq!(formatted.len(), 16);
        assert_eq!(&formatted[4..5], "-");
        assert_eq!(&formatted[7..8], "-");
        assert_eq!(&formatted[10..11], " ");
        assert_eq!(&formatted[13..14], ":");
    }
}
