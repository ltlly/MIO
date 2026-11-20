pub(crate) use std::time::Duration;

pub(crate) use mio_core::{Client, Command, Event, FlowState, HttpVersion, Runtime, Workspace};
pub(crate) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

pub(crate) async fn event(client: &mut Client, predicate: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = client
                .events
                .recv()
                .await
                .expect("service should remain connected");
            if predicate(&event) {
                return event;
            }
            assert!(
                !matches!(event, Event::Error { .. }),
                "unexpected service error: {event:?}"
            );
        }
    })
    .await
    .expect("bounded causal event")
}

pub(crate) async fn project(client: &mut Client) -> mio_core::ProjectKey {
    match event(client, |e| matches!(e, Event::ProjectOpened { .. })).await {
        Event::ProjectOpened { project, .. } => project,
        _ => unreachable!(),
    }
}

pub(crate) async fn workspace(client: &mut Client) -> Workspace {
    match event(client, |e| matches!(e, Event::RepeaterOpened { .. })).await {
        Event::RepeaterOpened { tab, .. } => Workspace {
            active: Some(tab.id),
            tabs: vec![tab],
            ..Workspace::default()
        },
        _ => unreachable!(),
    }
}

pub(crate) async fn receive_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    loop {
        let read = stream.read_buf(&mut data).await.unwrap();
        assert!(read > 0 && data.len() < 8192);
        if let Some(head_end) = data.windows(4).position(|b| b == b"\r\n\r\n") {
            let head = std::str::from_utf8(&data[..head_end]).unwrap();
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                .unwrap_or(0);
            if data.len() == head_end + 4 + length {
                return data;
            }
        }
    }
}
