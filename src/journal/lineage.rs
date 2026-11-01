//! Rebind earlier evidence only through a verified inverse of our own move.
//! Cross-device moves recreate inodes; content equality alone never authorizes
//! adopting an unrelated external replacement.
use super::*;
use undo::Plan;

pub(super) fn rebase(
    connection: &Connection,
    operation: i64,
    restored: &Path,
    origin: &snapshot::Snapshot,
    actual: &snapshot::Snapshot,
) -> Result<()> {
    if !origin.relocated(actual, true) {
        return Ok(());
    }
    let ancestors: Vec<_> = restored.ancestors().map(bytes).collect();
    let mut prefix = bytes(restored);
    prefix.push(b'/');
    let indices = (0..ancestors.len())
        .map(|i| format!("?{}", i + 3))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT u.operation_id,u.sequence,u.state,CASE WHEN length(u.data)<=33554432 THEN u.data END FROM undo_items u JOIN items i ON i.operation_id=u.operation_id AND i.sequence=u.sequence WHERE u.operation_id!=?1 AND u.issue IS NULL AND u.state IN ('applied','undone') AND (i.source IN ({indices}) OR i.destination IN ({indices}) OR substr(i.source,1,length(?2))=?2 OR substr(i.destination,1,length(?2))=?2)"
    );
    let mut values = vec![
        rusqlite::types::Value::Integer(operation),
        rusqlite::types::Value::Blob(prefix),
    ];
    values.extend(ancestors.into_iter().map(rusqlite::types::Value::Blob));
    let actual_nodes: std::collections::HashMap<_, _> = actual
        .nodes
        .iter()
        .map(|node| (node.name.as_slice(), node))
        .collect();
    let mut stmt = connection.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(values))?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let sequence: i64 = row.get(1)?;
        let state: String = row.get(2)?;
        let data: Option<Vec<u8>> = row.get(3)?;
        let Some(data) = data else {
            continue;
        };
        let Some(mut plan) = serde_json::from_slice::<Option<Plan>>(&data)? else {
            continue;
        };
        let current = if state == "applied" {
            path(&plan.target)
        } else if matches!(
            plan.kind.as_str(),
            "copy" | "duplicate" | "create" | "mkdir"
        ) {
            let Some(parking) = &plan.parking else {
                continue;
            };
            path(parking)
        } else {
            path(&plan.source)
        };
        let replacement = if let Ok(relative) = restored.strip_prefix(&current) {
            let relative = bytes(relative);
            if subtree(&plan.expected, &relative).is_some_and(|previous| previous.unchanged(origin))
            {
                let mut updated = plan.expected.clone();
                for node in &mut updated.nodes {
                    let name = relative_name(&node.name, &relative);
                    if let Some(name) = name
                        && let Some(new) = actual_nodes.get(name)
                    {
                        node.identity = new.identity.clone();
                        node.hash = new.hash;
                    }
                }
                Some(updated)
            } else {
                None
            }
        } else if let Ok(relative) = current.strip_prefix(restored) {
            let relative = bytes(relative);
            if subtree(origin, &relative).is_some_and(|previous| previous.unchanged(&plan.expected))
            {
                subtree(actual, &relative)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(replacement) = replacement {
            plan.expected = replacement;
            let data = serde_json::to_vec(&plan)?;
            ensure!(
                data.len() <= 32 * 1024 * 1024,
                "Слишком большой связанный план отмены"
            );
            connection.execute(
                "UPDATE undo_items SET data=?3 WHERE operation_id=?1 AND sequence=?2",
                params![id, sequence, data],
            )?;
        }
    }
    Ok(())
}
fn relative_name<'a>(name: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if prefix.is_empty() {
        Some(name)
    } else if name == prefix {
        Some(&[])
    } else {
        name.strip_prefix(prefix).and_then(|n| n.strip_prefix(b"/"))
    }
}
fn subtree(snapshot: &snapshot::Snapshot, prefix: &[u8]) -> Option<snapshot::Snapshot> {
    let nodes: Vec<_> = snapshot
        .nodes
        .iter()
        .filter_map(|node| {
            relative_name(&node.name, prefix).map(|name| snapshot::Node {
                name: name.to_vec(),
                identity: node.identity.clone(),
                hash: node.hash,
            })
        })
        .collect();
    (!nodes.is_empty() && nodes[0].name.is_empty()).then_some(snapshot::Snapshot { nodes })
}
