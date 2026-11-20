use super::*;

fn rich_report() -> ExtImportReport {
    ExtImportReport {
        repeater_active_tab: Some(0),
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: 204_800,
        indexed_items: 14,
        indexed_messages: 22,
        collections: vec![
            ExtCollectionReport {
                path: "root.f1.f0".into(),
                logical_items: Some(5),
                indexed_items: 5,
                message_count: 10,
                state: "available".into(),
            },
            ExtCollectionReport {
                path: "root.f1.f2".into(),
                logical_items: Some(4),
                indexed_items: 4,
                message_count: 4,
                state: "available".into(),
            },
            ExtCollectionReport {
                path: "root.f2.f1".into(),
                logical_items: Some(3),
                indexed_items: 3,
                message_count: 6,
                state: "available".into(),
            },
            ExtCollectionReport {
                path: "root.f7.f0".into(),
                logical_items: Some(2),
                indexed_items: 2,
                message_count: 2,
                state: "failed".into(),
            },
        ],
        gaps: vec![
            "root.f4.f0 (Embedded Project Config): 已提取 Scope 规则、Repeater 设置。".into(),
        ],
        complete_archive: true,
    }
}

#[gpui_kit::test]
fn preview_and_project_reports_render_proxy_and_repeater_counts(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (job_id, _) = inspect(&mut fixture, cx);
    let inspection_id = Uuid::new_v4();
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id,
            outcome: ExtJobOutcome::Inspected(ExtInspection {
                id: inspection_id,
                source_name: "coverage-matrix.ext_archive".into(),
                report: rich_report(),
            }),
        })
        .unwrap();
    cx.run_until_parked();

    let expected_summary = "Proxy 历史抓包：5 条 · Repeater 标签：3 个 · Repeater 分组：0 个";
    let expected_rows = [
        "root.f1.f0 · available · 逻辑位置 5 · 已索引 5 · 消息 10",
        "root.f2.f1 · available · 逻辑位置 3 · 已索引 3 · 消息 6",
    ];

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("ext-coverage-ack").is_none());
        let preview = window.within("ext-preview-report");
        assert!(preview.try_find("ext-category-coverage").is_none());
        assert_eq!(
            preview.find("ext-proxy-repeater-summary").label(),
            Some(expected_summary)
        );
        for (idx, expected) in expected_rows.iter().enumerate() {
            let row = preview.find(("ext-report-row", idx));
            assert_eq!(row.label(), Some(*expected));
        }
        assert!(preview.try_find(("ext-report-row", 2_usize)).is_none());
    })
    .unwrap();

    click(&fixture, "ext-dismiss", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReleaseExtInspection {
            job_id: release_id, ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected release job");
    };
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id: *release_id,
            outcome: ExtJobOutcome::Released { inspection_id },
        })
        .unwrap();
    cx.run_until_parked();

    project_menu(&fixture, "查看 外部导入记录", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryExtReport { query_revision, .. }] = commands.as_slice() else {
        panic!("expected report query");
    };
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(Some(ExtSourceInfo {
                id: Uuid::new_v4(),
                report: rich_report(),
            })),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("ext-category-coverage").is_none());
        let persisted = window.within("ext-project-report");
        assert_eq!(
            persisted.find("ext-proxy-repeater-summary").label(),
            Some(expected_summary)
        );
        for (idx, expected) in expected_rows.iter().enumerate() {
            let row = persisted.find(("ext-report-row", idx));
            assert_eq!(row.label(), Some(*expected));
        }
        assert!(persisted.try_find(("ext-report-row", 2_usize)).is_none());
    })
    .unwrap();
}
