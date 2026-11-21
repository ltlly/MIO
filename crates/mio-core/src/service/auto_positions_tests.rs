use super::*;
use crate::intruder::{AutomaticPlacement, Position};

#[tokio::test]
async fn auto_positions_returns_byte_ranges_without_mutating_or_sending_the_draft() {
    let (events, mut receiver) = mpsc::channel(4);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let original = state.store.workspace().unwrap();
    let raw = b"GET /?q=a%2Fb&q=two HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let first = raw.windows(5).position(|bytes| bytes == b"a%2Fb").unwrap();
    let second = raw.windows(3).position(|bytes| bytes == b"two").unwrap();
    for placement in [AutomaticPlacement::Replace, AutomaticPlacement::Append] {
        let operation_id = Uuid::new_v4();
        state
            .command(Command::AutoIntruderPositions {
                project: state.project,
                operation_id,
                raw: raw.to_vec(),
                selection: None,
                placement,
            })
            .await
            .unwrap();
        let Event::IntruderPositions {
            project,
            operation_id: actual,
            positions,
        } = receiver.try_recv().unwrap()
        else {
            panic!("Auto must return its own operation identity");
        };
        assert_eq!(project, state.project);
        assert_eq!(actual, operation_id);
        let append = placement == AutomaticPlacement::Append;
        assert_eq!(
            positions.unwrap(),
            vec![
                Position {
                    start: if append { first + 5 } else { first },
                    end: first + 5,
                },
                Position {
                    start: if append { second + 3 } else { second },
                    end: second + 3,
                },
            ]
        );
        assert!(receiver.try_recv().is_err());
    }
    assert_eq!(state.store.workspace().unwrap(), original);
    assert!(state.repeater.sends.is_empty());
    assert!(!state.intruder.has_resources());
}

#[tokio::test]
async fn auto_positions_failures_keep_identity_and_the_next_operation_still_succeeds() {
    let (events, mut receiver) = mpsc::channel(4);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let stale_project = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    let raw = b"GET /?x=one HTTP/1.1\r\nHost: localhost\r\n\r\n";
    for (project, selection) in [
        (stale_project, None),
        (state.project, Some(raw.len()..raw.len() + 1)),
    ] {
        let operation_id = Uuid::new_v4();
        state
            .command(Command::AutoIntruderPositions {
                project,
                operation_id,
                raw: raw.to_vec(),
                selection,
                placement: AutomaticPlacement::Replace,
            })
            .await
            .expect("Auto failures must not lose identity in a generic command error");
        assert!(
            matches!(receiver.try_recv().unwrap(), Event::IntruderPositions {
            project: actual_project,
            operation_id: actual_operation,
            positions: Err(message),
        } if actual_project == project && actual_operation == operation_id && !message.is_empty())
        );
        assert!(receiver.try_recv().is_err());
    }
    let operation_id = Uuid::new_v4();
    let start = raw.windows(3).position(|bytes| bytes == b"one").unwrap();
    state
        .command(Command::AutoIntruderPositions {
            project: state.project,
            operation_id,
            raw: raw.to_vec(),
            selection: Some(start..start + 3),
            placement: AutomaticPlacement::Replace,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::IntruderPositions {
            project,
            operation_id: actual,
            positions: Ok(positions),
        } if project == state.project && actual == operation_id && positions == vec![Position {
            start, end: start + 3,
        }])
    );
    assert!(receiver.try_recv().is_err());
}
