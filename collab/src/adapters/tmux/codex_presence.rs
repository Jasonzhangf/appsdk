use serde::Serialize;

const INPUT_ACTIVITY_LINES_ABOVE: usize = 4;
const NO_INPUT_ACTIVITY_TAIL_LINES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPresence {
    Online,
    Offline,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivity {
    Busy,
    Idle,
    WaitingUser,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentObservation {
    pub agent_presence: AgentPresence,
    pub activity: AgentActivity,
    pub background: bool,
    pub foreground_command: Option<String>,
    pub reason: String,
}

impl AgentObservation {
    pub(super) fn unknown(reason: impl Into<String>) -> Self {
        Self {
            agent_presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            background: false,
            foreground_command: None,
            reason: reason.into(),
        }
    }
}

enum ForegroundKind {
    Codex,
    Shell,
    Unknown,
}

pub(super) fn classify(foreground_command: &str, visible_screen: &str) -> AgentObservation {
    match foreground_kind(foreground_command) {
        ForegroundKind::Shell => AgentObservation {
            agent_presence: AgentPresence::Offline,
            activity: AgentActivity::Unknown,
            background: false,
            foreground_command: Some(foreground_command.to_owned()),
            reason: "foreground_shell".into(),
        },
        ForegroundKind::Unknown => AgentObservation {
            agent_presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            background: false,
            foreground_command: Some(foreground_command.to_owned()),
            reason: "unsupported_foreground_process".into(),
        },
        ForegroundKind::Codex => classify_codex(foreground_command, visible_screen),
    }
}

pub(super) fn classify_without_screen(foreground_command: &str) -> Option<AgentObservation> {
    match foreground_kind(foreground_command) {
        ForegroundKind::Codex => None,
        ForegroundKind::Shell | ForegroundKind::Unknown => Some(classify(foreground_command, "")),
    }
}

pub(super) fn classify_codex(foreground_command: &str, visible_screen: &str) -> AgentObservation {
    let active_region = active_region(visible_screen);
    let active_text = active_region.join("\n").to_ascii_lowercase();
    let background = contains_background(&active_text);
    let current_input = active_region.iter().any(|line| is_current_input(line));

    let (activity, reason) = if contains_waiting_user(&active_text) {
        (AgentActivity::WaitingUser, "foreground_codex_waiting_user")
    } else if contains_busy(&active_text) {
        (AgentActivity::Busy, "foreground_codex_busy")
    } else if background {
        (
            AgentActivity::Unknown,
            "foreground_codex_background_unclassified",
        )
    } else if current_input {
        (AgentActivity::Idle, "foreground_codex_input_ready")
    } else {
        (
            AgentActivity::Unknown,
            "foreground_codex_activity_unclassified",
        )
    };

    AgentObservation {
        agent_presence: AgentPresence::Online,
        activity,
        background,
        foreground_command: Some(foreground_command.to_owned()),
        reason: reason.into(),
    }
}

pub(super) fn parse_foreground_command(pane_id: &str, text: &str) -> Result<String, String> {
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(session), Some(pane), Some(command)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err("TMUX_FOREGROUND_QUERY_FAILED: malformed pane metadata".into());
        };
        let session_digits = session.strip_prefix('$').unwrap_or_default();
        if session_digits.is_empty()
            || !session_digits.bytes().all(|byte| byte.is_ascii_digit())
            || validate_pane_id(pane).is_err()
        {
            return Err("TMUX_FOREGROUND_QUERY_FAILED: malformed pane metadata".into());
        }
        if pane == pane_id {
            let command = command.trim();
            if command.is_empty() || command.chars().any(char::is_control) {
                return Err("TMUX_FOREGROUND_QUERY_FAILED: empty pane command".into());
            }
            return Ok(command.to_owned());
        }
    }
    Err("TMUX_FOREGROUND_QUERY_FAILED: bound pane not returned".into())
}

fn validate_pane_id(id: &str) -> Result<(), ()> {
    let digits = id.strip_prefix('%').unwrap_or_default();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    Ok(())
}

fn foreground_kind(foreground_command: &str) -> ForegroundKind {
    let executable = foreground_command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if executable == "codex" || executable.starts_with("codex-") {
        return ForegroundKind::Codex;
    }
    if matches!(
        executable.as_str(),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "dash"
            | "ksh"
            | "csh"
            | "tcsh"
            | "nu"
            | "pwsh"
            | "powershell"
    ) {
        return ForegroundKind::Shell;
    }
    ForegroundKind::Unknown
}

fn active_region(visible_screen: &str) -> Vec<&str> {
    let lines = visible_screen.lines().collect::<Vec<_>>();
    if let Some(prompt_index) = lines.iter().rposition(|line| is_current_input(line)) {
        let start = prompt_index.saturating_sub(INPUT_ACTIVITY_LINES_ABOVE);
        return lines[start..].to_vec();
    }
    let start = lines.len().saturating_sub(NO_INPUT_ACTIVITY_TAIL_LINES);
    lines[start..].to_vec()
}

fn is_current_input(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('\u{203a}')
        || trimmed.starts_with("> ")
        || trimmed.contains("Ask Codex to do anything")
}

fn contains_busy(active_text: &str) -> bool {
    active_text.contains("esc to interrupt")
        || active_text.contains("waiting for background terminal")
}

fn contains_background(active_text: &str) -> bool {
    active_text.contains("background terminal")
        || active_text.contains("background task")
        || active_text.contains("background command")
}

fn contains_waiting_user(active_text: &str) -> bool {
    [
        "waiting for user",
        "waiting for approval",
        "approval required",
        "requires approval",
        "select an option",
        "choose an option",
        "press enter to confirm",
        "allow this command",
        "would you like to run",
        "do you want to proceed",
        "1. yes",
        "2. no",
    ]
    .iter()
    .any(|marker| active_text.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPT: &str = "\u{203a} Ask Codex to do anything";

    #[test]
    fn historical_busy_text_above_current_input_is_not_busy() {
        let screen = format!("esc to interrupt\nold output\n\n\n\n{PROMPT}\n\n");
        let observation = classify("codex", &screen);
        assert_eq!(observation.agent_presence, AgentPresence::Online);
        assert_eq!(observation.activity, AgentActivity::Idle);
        assert!(!observation.background);
    }

    #[test]
    fn shell_with_historical_codex_output_is_offline() {
        let screen = format!("Codex TUI\nesc to interrupt\n{PROMPT}");
        let observation = classify("zsh", &screen);
        assert_eq!(observation.agent_presence, AgentPresence::Offline);
        assert_eq!(observation.activity, AgentActivity::Unknown);
    }

    #[test]
    fn current_input_with_busy_footer_is_busy() {
        let screen = format!("output\nWaiting for background terminal\nesc to interrupt\n{PROMPT}");
        let observation = classify("codex", &screen);
        assert_eq!(observation.agent_presence, AgentPresence::Online);
        assert_eq!(observation.activity, AgentActivity::Busy);
        assert!(observation.background);
    }

    #[test]
    fn waiting_for_background_terminal_is_busy() {
        let screen = format!("Waiting for background terminal\n\n\n{PROMPT}");
        let observation = classify("codex", &screen);
        assert_eq!(observation.agent_presence, AgentPresence::Online);
        assert_eq!(observation.activity, AgentActivity::Busy);
        assert!(observation.background);
    }

    #[test]
    fn unexplained_background_keeps_activity_unknown() {
        let screen = format!("A background terminal is still open\n{PROMPT}");
        let observation = classify("codex", &screen);
        assert_eq!(observation.agent_presence, AgentPresence::Online);
        assert_eq!(observation.activity, AgentActivity::Unknown);
        assert!(observation.background);
    }

    #[test]
    fn approval_or_selection_is_waiting_user_not_idle() {
        let screen = "\
Would you like to run this command?
Select an option
1. Yes
2. No
Press Enter to confirm";
        let observation = classify("codex", screen);
        assert_eq!(observation.agent_presence, AgentPresence::Online);
        assert_eq!(observation.activity, AgentActivity::WaitingUser);
        assert!(!observation.background);
    }

    #[test]
    fn waiting_user_precedes_busy_footer() {
        let screen = format!("Approval required\nSelect an option\nesc to interrupt\n{PROMPT}");
        let observation = classify("codex", &screen);
        assert_eq!(observation.activity, AgentActivity::WaitingUser);
    }

    #[test]
    fn historical_waiting_user_text_above_current_input_is_not_waiting() {
        let screen = format!("Approval required\nSelect an option\n\n\n\n\n\n{PROMPT}");
        let observation = classify("codex", &screen);
        assert_eq!(observation.activity, AgentActivity::Idle);
    }

    #[test]
    fn unsupported_foreground_process_is_unknown() {
        let screen = format!("{PROMPT}");
        let observation = classify("vim", &screen);
        assert_eq!(observation.agent_presence, AgentPresence::Unknown);
        assert_eq!(observation.activity, AgentActivity::Unknown);
    }

    #[test]
    fn codex_without_current_input_is_not_inferred_idle() {
        let observation = classify("codex", "recent output without an input state");
        assert_eq!(observation.agent_presence, AgentPresence::Online);
        assert_eq!(observation.activity, AgentActivity::Unknown);
    }

    #[test]
    fn codex_versioned_binary_name_is_recognized() {
        let observation = classify(
            "/usr/local/bin/codex-x86_64-apple-darwin",
            "recent output without an input state",
        );
        assert_eq!(observation.agent_presence, AgentPresence::Online);
    }

    #[test]
    fn foreground_metadata_uses_the_exact_bound_pane() {
        let text = "$1\t%1\tzsh\n$2\t%2\tcodex\n";
        assert_eq!(parse_foreground_command("%2", text).unwrap(), "codex");
    }

    #[test]
    fn foreground_metadata_malformed_or_missing_is_rejected() {
        for text in [
            "broken\n",
            "session\t%2\tcodex\n",
            "$2\tpane\tcodex\n",
            "$2\t%2\t\n",
        ] {
            assert!(parse_foreground_command("%2", text).is_err(), "{text:?}");
        }
        assert!(parse_foreground_command("%3", "$2\t%2\tcodex\n").is_err());
    }
}
