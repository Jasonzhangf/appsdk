use serde_json::Value;

pub(super) fn help() -> Value {
    serde_json::json!({
        "harness": "appsdk-development-process-control",
        "project_root": "optional; defaults to the current working directory",
        "commands": [
            {"command": "compile", "usage": "appsdk guide compile [project]", "writes_state": true},
            {"command": "init", "usage": "appsdk guide init [project] --task <id> --mode <domain> [--module <id>]", "writes_state": false},
            {"command": "status", "usage": "appsdk guide status [project] [--task <id>] [--module <id>]", "writes_state": false},
            {"command": "<domain>", "usage": "appsdk guide <domain> [project] [--task <id>] [--module <id>]", "writes_state": false},
            {"command": "plan", "usage": "appsdk guide plan [project] --task <id> --input <plan.json>", "writes_state": true},
            {"command": "update", "usage": "appsdk guide update [project] --task <id> --input <result.json>", "writes_state": true},
            {"command": "next", "usage": "appsdk guide next [project] --task <id>", "writes_state": false},
            {"command": "close", "usage": "appsdk guide close [project] --task <id>", "writes_state": false},
            {"command": "tour", "usage": "appsdk guide tour [project] --task <id> [--mode <domain>] [--input <tour.json>]", "writes_state": true},
            {"command": "review", "usage": "appsdk guide review [project] --task <id> --input <review.json>", "writes_state": true}
        ],
        "domains": super::DOMAINS,
        "existing_project_setup": [
            "appsdk guide status",
            "appsdk guide init --task guidance-setup --mode bootstrap --module <id>",
            "read project documents and present GuidanceSetupProposal for explicit user approval",
            "after approval update project rule sources, then run appsdk guide compile"
        ],
        "existing_project_upgrade": [
            "appsdk init",
            "appsdk guide init --task guidance-upgrade --mode bootstrap --module <id>",
            "read current project rules before the installed standard template reference",
            "present retained rules, recommended differences, and declined template items for explicit user approval",
            "after approval apply only accepted differences, then compile and verify"
        ],
        "start": [
            "appsdk guide status --task <id>",
            "appsdk guide compile",
            "appsdk guide init --task <id> --mode <develop|debug> --module <id>"
        ],
        "tour_review": [
            "appsdk guide tour --task <id> --mode <domain>",
            "choose a path and submit a TourProposal with selected_path",
            "appsdk guide tour --task <id> --input tour.json",
            "submit node_review updates and accept every selected node",
            "only then submit flow_review for edges, order, or rules",
            "accepted flow patches remain staged until the declared process source is updated and compiled"
        ]
    })
}
