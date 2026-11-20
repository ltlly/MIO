use super::*;

#[tokio::test]
async fn terminal_session_retention_survives_save_as_but_not_runtime_shutdown() {
    tokio::time::timeout(Duration::from_secs(8), exercise(SessionEnd::Shutdown))
        .await
        .expect("small loopback retention lifetime is bounded");
}

#[tokio::test]
async fn terminal_session_promotion_is_durable_without_replay() {
    tokio::time::timeout(Duration::from_secs(8), exercise(SessionEnd::Promote))
        .await
        .expect("small loopback promotion is bounded");
}

#[tokio::test]
async fn terminal_session_discard_allows_project_switches_and_rejects_stale_operations() {
    tokio::time::timeout(Duration::from_secs(8), exercise(SessionEnd::Discard))
        .await
        .expect("small loopback discard, project switches, reopen and runtime joins are bounded");
}
