use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub struct ReceivingProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Receiver<Value>,
}

impl ReceivingProcess {
    pub fn start(mut command: Command, binary: &Path) -> Self {
        let mut child = command
            .args([
                "--exact",
                "communication_receiver::isolated_recovery_receiver_process",
                "--ignored",
                "--nocapture",
            ])
            .env("COLLAB_RECOVERY_RECEIVER_BINARY", binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if let Ok(event) = serde_json::from_str::<Value>(&line.unwrap()) {
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            }
        });
        let mut receiver = Self {
            stdin: child.stdin.take(),
            child,
            output,
        };
        let ready = receiver.event("ready");
        assert_eq!(ready["pid"], receiver.child.id());
        receiver
    }

    pub fn submit(&mut self, native: &Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{native}").unwrap();
        self.stdin.as_mut().unwrap().flush().unwrap();
    }

    pub fn event(&mut self, expected: &str) -> Value {
        let event = self
            .output
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| {
                panic!(
                    "receiver {expected}: {error}; status={:?}",
                    self.child.try_wait()
                )
            });
        assert_eq!(event["event"], expected, "{event}");
        event
    }
}

impl Drop for ReceivingProcess {
    fn drop(&mut self) {
        self.stdin.take();
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn notification_message_id(native: &Value) -> String {
    if let Some(id) = native["params"]["clientUserMessageId"].as_str() {
        return id
            .strip_prefix("collab-notification-")
            .expect("AppServer notification identity")
            .into();
    }
    if let Some(id) = native["params"]["messageId"].as_str() {
        let mut parts = id.splitn(3, ':');
        assert_eq!(parts.next(), Some("collab"));
        assert_eq!(parts.next().unwrap().len(), 16);
        return parts.next().expect("gateway mailbox identity").into();
    }
    let text = native["text"].as_str().expect("Desktop native send text");
    let ids = text
        .split_once("message_ids=")
        .expect("native batch identity list")
        .1;
    ids.split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap()
        .into()
}

#[test]
#[ignore = "independent native receiver child, invoked only by an owning public case"]
fn isolated_recovery_receiver_process() {
    let binary =
        std::env::var_os("COLLAB_RECOVERY_RECEIVER_BINARY").expect("owned receiver binary");
    let command = |args: &[&str]| {
        let output = Command::new(&binary).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "receiver {args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let dsh = std::env::var("COLLAB_RECOVERY_RECEIVER_DSH_BINDING")
        .ok()
        .map(|value| serde_json::from_str::<Value>(&value).unwrap());
    println!("{}", json!({"event":"ready","pid":std::process::id()}));
    for line in std::io::stdin().lock().lines() {
        let native: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let message_id = notification_message_id(&native);
        let receive_id = format!("recover-{message_id}");
        let received = if let Some(binding) = &dsh {
            let root = std::env::current_dir().unwrap();
            let socket = std::path::PathBuf::from(std::env::var_os("COLLAB_STATE_DIR").unwrap())
                .join("server.sock");
            let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = json!({"op":"Poll","worker_id":binding["workerId"],"token":binding["token"],
                "timeout_ms":0,"receive_id":receive_id,"project_context":{
                    "canonical_root":root,"project_scope":binding["projectScope"],
                    "app_scope_id":binding["runtimeContext"]["appserver_id"],"runtime_context":binding["runtimeContext"]}});
            writeln!(stream, "{request}").unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["ok"], true, "canonical gateway Poll must succeed");
            response
        } else {
            command(&["recv", "--receive-id", &receive_id, "--timeout", "0"])
        };
        let message = received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == message_id)
            .expect("native identity in real receive batch");
        println!(
            "{}",
            json!({"event":"received","pid":std::process::id(),"receipt":received})
        );
        if message["subject"] != "communication-recovery" {
            continue;
        }
        let identity = command(&["context"]);
        let principal = identity["result"]["snapshot"]["identity"]["worker_id"]
            .as_str()
            .unwrap();
        assert_eq!(message["to"], principal);
        let reply = command(&[
            "sendmessage",
            "--to",
            principal,
            "--subject",
            "communication-recovery-reply",
            "--type",
            "reply",
            "--in-reply-to",
            &message_id,
            "communication recovery received",
        ]);
        println!(
            "{}",
            json!({"event":"replied","pid":std::process::id(),"receipt":reply})
        );
    }
}
