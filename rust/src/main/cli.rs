use super::*;

pub(super) fn is_help(value: &str) -> bool {
    matches!(value, "help" | "--help" | "-h")
}

pub(super) fn print_cli_help(command: Option<&str>) {
    let usage = match command {
        Some("verify") => {
            "Usage: appsdk verify [project]\n       appsdk verify --admission [project]\n       appsdk verify --review-admission [project] --module <id>"
        }
        Some("compile") => "Usage: appsdk compile [project] [--module <id>]",
        Some("compile-module") => "Usage: appsdk compile-module [project] --module <id>",
        Some("produce-lifecycle-records") => {
            "Usage: appsdk produce-lifecycle-records [project] --module <id> --input <json>"
        }
        Some("retire-lifecycle-records") => {
            "Usage: appsdk retire-lifecycle-records [project] --module <id> --issue <id>"
        }
        Some("produce-lifecycle-chain") => {
            "Usage: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>"
        }
        Some("pin-lock") => "Usage: appsdk pin-lock [project] --binary <path>",
        Some("sdk-witness") => "Usage: appsdk sdk-witness [project] [--binary <path>]",
        Some("reset-governance") => {
            "Usage: appsdk reset-governance [project] --discard-legacy"
        }
        Some("init") => {
            "Usage: appsdk init [workspace] [--project-root <relative-path>] [--fresh --discard-legacy]"
        }
        Some("prepare") => "Usage: appsdk prepare [workspace]",
        Some("new") => "Usage: appsdk new [project]",
        Some("memory") | Some("project-memory") => {
            "Usage: appsdk memory <entry|query|get|review|promote|migrate|import|reentry|index|export|compact|verify> [project]"
        }
        Some("bug") => {
            "Usage: appsdk bug <intake|new|list|show|comment|close|webui> [options]\n       appsdk bug intake --input <json>"
        }
        Some("setup-deps") => {
            "Usage: appsdk setup-deps [--check]"
        }
        Some("goal") => {
            "Usage: appsdk goal <subscribe|status|cancel|prompt> [options]\n       appsdk goal subscribe --goal <path.md> [--interval <duration>]\n       appsdk goal prompt --goal <path.md>"
        }
        Some("task") => {
            "Usage: appsdk task <block|register|relocate|update|wait|deliver|close|status> [options]\n       appsdk task block <id> [--reason <text>]"
        }
        _ => CLI_USAGE,
    };
    println!("{usage}\n\nNo project-root environment variable is required.");
}

pub(super) fn project_root_or_cwd<I>(args: &mut std::iter::Peekable<I>) -> PathBuf
where
    I: Iterator<Item = String>,
{
    if args.peek().is_some_and(|value| value.starts_with('-')) {
        PathBuf::from(".")
    } else {
        PathBuf::from(args.next().unwrap_or_else(|| ".".into()))
    }
}

#[allow(dead_code)]
pub(super) fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}
