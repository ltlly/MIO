use anyhow::{Result, ensure};
use rusqlite::Transaction;

use super::TABLES;

pub(in crate::store) fn validate(transaction: &Transaction<'_>, schema: i32) -> Result<()> {
    // Reject executable object kinds from the catalog before any table introspection:
    // table_list can prepare views/virtual tables merely to initialize their column counts.
    // This is not a canonical-DDL check or a general SQLite resource-limit guarantee;
    // DEFAULT/CHECK/index expressions and post-open schema changes are outside this gate.
    let tables = {
        let mut statement = transaction.prepare(
            "SELECT type,name,rootpage FROM main.sqlite_schema
             WHERE type IN ('table','view','trigger')",
        )?;
        let mut rows = statement.query([])?;
        let mut tables = Vec::<String>::new();
        while let Some(row) = rows.next()? {
            let kind: String = row.get(0)?;
            ensure!(kind == "table", "项目包含视图或数据库触发器；未打开项目");
            let root_page: i64 = row.get(2)?;
            ensure!(root_page > 0, "项目包含虚拟表或无存储根页的数据表");
            tables.push(row.get(1)?);
        }
        tables
    };

    for (since, name, _) in TABLES {
        let present = tables.iter().any(|table| table.eq_ignore_ascii_case(name));
        if schema >= *since {
            ensure!(present, "项目缺少必需数据表 {name}");
        } else {
            ensure!(!present, "项目迁移目标表 {name} 已存在；不会覆盖已有数据");
        }
    }

    // Additional ordinary stored-data tables are preserved. Do not require exact DDL
    // or a closed list of table names, and never repair a source in place.
    for table in &tables {
        let quoted = table.replace('"', "\"\"");
        let mut statement =
            transaction.prepare(&format!("PRAGMA main.table_xinfo(\"{quoted}\")"))?;
        let mut rows = statement.query([])?;
        let mut columns = Vec::<String>::new();
        while let Some(row) = rows.next()? {
            let hidden: i32 = row.get(6)?;
            ensure!(hidden == 0, "项目包含计算列或隐藏列；未打开项目");
            columns.push(row.get(1)?);
        }
        if let Some((_, _, required)) = TABLES
            .iter()
            .find(|(_, name, _)| table.eq_ignore_ascii_case(name))
        {
            ensure!(
                required.iter().all(|name| columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(name))),
                "项目数据表 {table} 缺少必需列"
            );
        }
        if table.eq_ignore_ascii_case("flows") {
            let listener_port = columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case("listener_port"));
            if schema >= 25 {
                ensure!(listener_port, "项目数据表 {table} 缺少必需列 listener_port");
            } else {
                ensure!(
                    !listener_port,
                    "项目迁移目标列 flows.listener_port 已存在；不会覆盖已有数据"
                );
            }
            for name in crate::store::FLOW_DENORM_COLUMN_NAMES {
                let present = columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(name));
                if schema >= 27 {
                    ensure!(present, "项目数据表 {table} 缺少必需列 {name}");
                } else {
                    ensure!(
                        !present,
                        "项目迁移目标列 flows.{name} 已存在；不会覆盖已有数据"
                    );
                }
            }
        }
        if table.eq_ignore_ascii_case("intruder_results") {
            for name in ["notes", "notes_revision", "highlight", "highlight_revision"] {
                let present = columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(name));
                if schema >= 25 {
                    ensure!(present, "项目数据表 {table} 缺少必需列 {name}");
                } else {
                    ensure!(
                        !present,
                        "项目迁移目标列 intruder_results.{name} 已存在；不会覆盖已有数据"
                    );
                }
            }
        }
        if table.eq_ignore_ascii_case("intruder_runs") {
            let paused = columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case("paused"));
            if schema >= 7 {
                ensure!(paused, "项目数据表 {table} 缺少必需列 paused");
            } else {
                ensure!(
                    !paused,
                    "项目迁移目标列 intruder_runs.paused 已存在；不会覆盖已有数据"
                );
            }
            for name in ["title", "title_revision"] {
                let present = columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(name));
                if schema >= 10 {
                    ensure!(present, "项目数据表 {table} 缺少必需列 {name}");
                } else {
                    ensure!(
                        !present,
                        "项目迁移目标列 intruder_runs.{name} 已存在；不会覆盖已有数据"
                    );
                }
            }
            let baseline = columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case("baseline_ordinal"));
            if schema >= 12 {
                ensure!(baseline, "项目数据表 {table} 缺少必需列 baseline_ordinal");
            } else {
                ensure!(
                    !baseline,
                    "项目迁移目标列 intruder_runs.baseline_ordinal 已存在；不会覆盖已有数据"
                );
            }
            for name in ["pool_id", "pool_revision"] {
                let present = columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(name));
                if schema >= 13 {
                    ensure!(present, "项目数据表 {table} 缺少必需列 {name}");
                } else {
                    ensure!(
                        !present,
                        "项目迁移目标列 intruder_runs.{name} 已存在；不会覆盖已有数据"
                    );
                }
            }
        }
    }
    if schema < 9 {
        let retired: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.metadata WHERE typeof(key)!='text'
             OR substr(key,1,19) COLLATE NOCASE='intruder_discarded:')",
            [],
            |row| row.get(0),
        )?;
        ensure!(!retired, "旧项目版本包含 Intruder 丢弃记录；未迁移项目");
    } else {
        crate::store::intruder::validate_retirements(transaction)?;
    }
    if schema < 13 {
        let present: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.metadata WHERE key=?1 COLLATE NOCASE)",
            [crate::store::resource_pools::KEY],
            |row| row.get(0),
        )?;
        ensure!(!present, "旧项目版本包含 Intruder 资源池目录；未迁移项目");
    } else {
        crate::store::resource_pools::read(transaction)?;
    }
    crate::store::messages::validate_all_http_history_metadata(transaction)?;
    Ok(())
}
