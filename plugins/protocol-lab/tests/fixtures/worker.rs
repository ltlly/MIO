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
        }),
        std::time::Duration::from_secs(5),
    );
    write_frame(&mut output, &call)?;
    loop {
        std::thread::park();
    }
}
