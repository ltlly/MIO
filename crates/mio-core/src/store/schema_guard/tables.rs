use anyhow::{Result, ensure};
use rusqlite::Transaction;

use super::TABLES;

pub(in crate::store) fn validate(transaction: &Transaction<'_>) -> Result<()> {
    let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        schema == crate::store::SCHEMA,
        "不是受支持的 MIO 项目；未修改文件"
    );
    // Reject executable object kinds from the catalog before any table introspection:
    // table_list can prepare views/virtual tables merely to initialize their column counts.
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

    for (name, _) in TABLES {
        let present = tables.iter().any(|table| table.eq_ignore_ascii_case(name));
        ensure!(present, "项目缺少必需数据表 {name}");
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
        if let Some((_, required)) = TABLES
            .iter()
            .find(|(name, _)| table.eq_ignore_ascii_case(name))
        {
            ensure!(
                required.iter().all(|name| columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(name))),
                "项目数据表 {table} 缺少必需列"
            );
        }
    }
    crate::store::intruder::validate_retirements(transaction)?;
    crate::store::resource_pools::read(transaction)?;
    Ok(())
}
