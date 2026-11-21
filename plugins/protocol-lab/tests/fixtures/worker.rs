//! Deliberately faulty worker, used only by process-level regression tests.
use std::io::{self, Read, Write};

use mio_plugin_api::{Uuid, panel::*, protocol::*};

fn main() -> io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--group-child") {
        let addr = std::fs::read_to_string("descendant.addr")?;
        let mut probe = std::net::TcpStream::connect(addr.trim())?;
        probe.write_all(&[1])?;
        let _ = probe.read_exact(&mut [0]);
        return Ok(());
    }
    let name = std::env::current_exe()?
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    if name.contains("multiplex-e2e") {
        return run_multiplex_e2e();
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let hello = read_frame(&mut input)?;
    if name.contains("bad-frame") {
        output.write_all(&(MAX_FRAME_BYTES as u32 + 1).to_be_bytes())?;
        return output.flush();
    }
    let identity = hello.identity().map_err(io::Error::other)?;
    let panel = PanelSnapshot {
        revision: 1,
        title: "fixture".into(),
        root: Some(Node::new(
            1,
            "test button",
            node::Kind::Button(Button {
                label: "test".into(),
                enabled: true,
                commit_editors: false,
            }),
        )),
    };
    let mut register = Envelope::new(
        identity,
        1,
        1,
        envelope::Body::Register(Register {
            plugin_id: "lab.fixture".into(),
            plugin_version: "0.1.0".into(),
            required_components: vec![Component::Button as i32],
            panel: Some(panel.clone()),
            supports_panel_multiplexing: false,
        }),
        std::time::Duration::from_secs(5),
    );
    register.in_reply_to = hello.request_id;
    write_frame(&mut output, &register)?;
    let _ready = read_frame(&mut input)?;
    if name.contains("cooperative-group") {
        let child = std::process::Command::new(std::env::current_exe()?)
            .arg("--group-child")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        write_frame(
            &mut output,
            &Envelope::new(
                identity,
                2,
                1,
                envelope::Body::HostCall(HostCall {
                    operation: Some(host_call::Operation::QueryHistory(HistoryQuery {
                        offset: 0,
                        limit: 1,
                        filter: format!("group-child:{}:{}", std::process::id(), child.id()),
                    })),
                    parent_request_id: 0,
                }),
                std::time::Duration::from_secs(5),
            ),
        )?;
        loop {
            match read_frame(&mut input)?.body {
                Some(envelope::Body::Shutdown(_)) => return Ok(()),
                Some(envelope::Body::HostResult(_)) => {}
                _ => return Err(io::Error::other("unexpected group fixture command")),
            }
        }
    }
    if name.contains("open-ack") {
        let open = read_frame(&mut input)?;
        if matches!(open.body, Some(envelope::Body::Shutdown(_))) {
            return Ok(());
        }
        if !matches!(open.body, Some(envelope::Body::OpenRequest(_))) {
            return Err(io::Error::other("expected OpenRequest"));
        }
        write_frame(
            &mut output,
            &Envelope::new(
                identity,
                2,
                1,
                envelope::Body::HostCall(HostCall {
                    operation: Some(host_call::Operation::QueryHistory(HistoryQuery {
                        offset: 0,
                        limit: 1,
                        filter: "open-received-before-ack".into(),
                    })),
                    parent_request_id: open.request_id,
                }),
                std::time::Duration::from_secs(5),
            ),
        )?;
        let release = read_frame(&mut input)?;
        if !matches!(release.body, Some(envelope::Body::HostResult(_))) || release.in_reply_to != 2
        {
            return Err(io::Error::other("expected causal open release"));
        }
        let mut panel = panel;
        panel.revision = 2;
        let mut accepted = Envelope::new(
            identity,
            3,
            panel.revision,
            envelope::Body::Snapshot(panel),
            std::time::Duration::from_secs(5),
        );
        accepted.in_reply_to = open.request_id;
        write_frame(&mut output, &accepted)?;
        return match read_frame(&mut input)?.body {
            Some(envelope::Body::Shutdown(_)) => Ok(()),
            _ => Err(io::Error::other("unexpected message after open completion")),
        };
    }
    if name.contains("forged") || name.contains("denied") {
        let mut forged = Envelope::new(
            identity,
            2,
            1,
            envelope::Body::HostCall(HostCall {
                operation: Some(host_call::Operation::QueryHistory(HistoryQuery {
                    offset: 0,
                    limit: 1,
                    filter: String::new(),
                })),
                parent_request_id: 0,
            }),
            std::time::Duration::from_secs(5),
        );
        if name.contains("forged") {
            forged.instance_id = Uuid::new_v4().as_bytes().to_vec();
        }
        write_frame(&mut output, &forged)?;
        if name.contains("denied") {
            let result = read_frame(&mut input)?;
            let Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(failure)),
            })) = result.body
            else {
                return Err(io::Error::other("unauthorized service was not rejected"));
            };
            if failure.code != ErrorCode::Denied as i32 {
                return Err(io::Error::other("wrong authorization failure"));
            }
            return match read_frame(&mut input)?.body {
                Some(envelope::Body::Shutdown(_)) => Ok(()),
                _ => Err(io::Error::other("unexpected message after denial")),
            };
        }
    }
    let event = read_frame(&mut input)?;
    if matches!(event.body, Some(envelope::Body::Shutdown(_))) {
        return Ok(());
    }
    let call = Envelope::new(
        identity,
        2,
        1,
        envelope::Body::HostCall(HostCall {
            operation: Some(host_call::Operation::QueryHistory(HistoryQuery {
                offset: 0,
                limit: 1,
                filter: format!("entered-hang:{}", std::process::id()),
            })),
            parent_request_id: 0,
        }),
        std::time::Duration::from_secs(5),
    );
    write_frame(&mut output, &call)?;
    loop {
        std::thread::park();
    }
}

fn multiplex_snapshot(pid: u64, rev: u64, status: &str) -> PanelSnapshot {
    let os_pid = std::process::id();
    PanelSnapshot {
        revision: rev,
        title: format!("panel-{pid}"),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "ed",
                        node::Kind::Editor(Editor {
                            document_id: 1,
                            revision: 1,
                            contents: format!("pid:{os_pid}:panel:{pid}").into_bytes(),
                            format: EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    Node::new(
                        10,
                        "open-repeater",
                        node::Kind::Button(Button {
                            label: "OpenRepeater".into(),
                            enabled: true,
                            commit_editors: false,
                        }),
                    ),
                    Node::new(
                        11,
                        "status",
                        node::Kind::Text(Text {
                            text: status.to_owned(),
                            tone: Tone::Foreground as i32,
                        }),
                    ),
                ],
            }),
        )),
    }
}

fn run_multiplex_e2e() -> io::Result<()> {
    let os_pid = std::process::id();
    let initial = PanelSnapshot {
        revision: 1,
        title: "multiplex-e2e".into(),
        root: Some(Node::new(
            1,
            "root",
            node::Kind::Text(Text {
                text: format!("pid:{os_pid}"),
                tone: Tone::Foreground as i32,
            }),
        )),
    };
    let mut session = WorkerSession::accept(
        io::stdin().lock(),
        io::stdout().lock(),
        Register {
            plugin_id: "lab.multiplex-e2e".into(),
            plugin_version: "1.5.0".into(),
            required_components: vec![Component::Text as i32, Component::Button as i32],
            panel: Some(initial),
            supports_panel_multiplexing: true,
        },
    )?;
    if session.negotiated_minor() < PROTOCOL_MINOR {
        return Err(io::Error::other("expected v1.5 negotiated minor"));
    }
    let mut revs: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    let mut deferred: std::collections::VecDeque<Envelope> = std::collections::VecDeque::new();
    loop {
        let env = match deferred.pop_front() {
            Some(env) => env,
            None => match session.receive() {
                Ok(env) => env,
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(err) => return Err(err),
            },
        };
        let reply_to = env.request_id;
        let pid = env.panel_id.max(PANEL_ID);
        let Some(body) = env.body else {
            return Err(io::Error::other("empty envelope"));
        };
        match body {
            envelope::Body::Shutdown(_) => return Ok(()),
            envelope::Body::OpenRequest(_) | envelope::Body::OpenResponse(_) => {
                let rev = revs.entry(pid).or_insert(0);
                *rev += 1;
                session.send_on_panel(
                    pid,
                    envelope::Body::Snapshot(multiplex_snapshot(pid, *rev, "ready")),
                    *rev,
                    reply_to,
                )?;
            }
            envelope::Body::UiEvent(_) => {
                let current_rev = *revs.get(&pid).unwrap_or(&0);
                let call_id = session.send_on_panel(
                    pid,
                    envelope::Body::HostCall(HostCall {
                        operation: Some(host_call::Operation::OpenRepeater(HttpRequestDto {
                            method: "GET".into(),
                            url: "http://127.0.0.1/e2e-child".into(),
                            version: HttpVersionDto::Http11 as i32,
                            headers: vec![],
                            body: b"from-child".to_vec(),
                            trailers: vec![],
                        })),
                        parent_request_id: reply_to,
                    }),
                    current_rev,
                    0,
                )?;
                let opened_id = loop {
                    let incoming = session.receive()?;
                    if incoming.in_reply_to == call_id
                        && let Some(envelope::Body::HostResult(hr)) = incoming.body
                    {
                        match hr.outcome {
                            Some(host_result::Outcome::Resource(res)) => {
                                let id = uuid(&res.resource_id).map_err(io::Error::other)?;
                                break id;
                            }
                            Some(host_result::Outcome::Error(err)) => {
                                return Err(io::Error::other(err.message));
                            }
                            _ => return Err(io::Error::other("unexpected HostResult")),
                        }
                    }
                    deferred.push_back(incoming);
                };
                let rev = revs.entry(pid).or_insert(0);
                *rev += 1;
                session.send_on_panel(
                    pid,
                    envelope::Body::Snapshot(multiplex_snapshot(
                        pid,
                        *rev,
                        &format!("opened:{opened_id}"),
                    )),
                    *rev,
                    reply_to,
                )?;
            }
            envelope::Body::CommitEditor(c) => {
                let current_rev = *revs.get(&pid).unwrap_or(&1);
                let mut req = c.request.ok_or_else(|| io::Error::other("missing request"))?;
                req.headers.push(HeaderDto {
                    name: "x-worker-pid".into(),
                    value: os_pid.to_string().into_bytes(),
                });
                req.headers.push(HeaderDto {
                    name: "x-worker-panel".into(),
                    value: pid.to_string().into_bytes(),
                });
                session.send_on_panel(
                    pid,
                    envelope::Body::Committed(Committed {
                        resource_id: c.resource_id,
                        message_revision: c.message_revision,
                        document_id: 1,
                        document_revision: 1,
                        request: Some(req),
                    }),
                    current_rev,
                    reply_to,
                )?;
            }
            envelope::Body::CloseRequest(close) => {
                if close.destroy_panel {
                    let current_rev = revs.remove(&pid).unwrap_or(1);
                    session.send_on_panel(pid, envelope::Body::Ack(Empty {}), current_rev, reply_to)?;
                } else {
                    let closed_id = uuid(&close.resource_id).map_err(io::Error::other)?;
                    let rev = revs.entry(pid).or_insert(0);
                    *rev += 1;
                    session.send_on_panel(
                        pid,
                        envelope::Body::Snapshot(multiplex_snapshot(
                            pid,
                            *rev,
                            &format!("revoked:{closed_id}"),
                        )),
                        *rev,
                        reply_to,
                    )?;
                }
            }
            _ => {}
        }
    }
}
