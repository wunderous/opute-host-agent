//! Port of `internal/plan/graph.go`: cycle detection, topological levels,
//! and reverse-topological ordering (used for compensation) over a plan's
//! node dependency graph.

use super::schema::{Document, Node};
use std::collections::HashMap;

struct GraphData {
    nodes: HashMap<String, Node>,
    indegree: HashMap<String, i64>,
    dependents: HashMap<String, Vec<String>>,
}

fn build_graph(doc: &Document) -> Result<GraphData, String> {
    let mut nodes = HashMap::with_capacity(doc.nodes.len());
    let mut indegree = HashMap::with_capacity(doc.nodes.len());
    let mut dependents: HashMap<String, Vec<String>> = HashMap::with_capacity(doc.nodes.len());
    for node in &doc.nodes {
        if nodes.contains_key(&node.id) {
            return Err(format!("duplicate node id \"{}\"", node.id));
        }
        nodes.insert(node.id.clone(), node.clone());
        indegree.insert(node.id.clone(), 0i64);
    }
    for node in &doc.nodes {
        let mut seen = std::collections::HashSet::with_capacity(node.depends_on.len());
        for dependency in &node.depends_on {
            if !nodes.contains_key(dependency) {
                return Err(format!(
                    "node \"{}\" depends on unknown node \"{dependency}\"",
                    node.id
                ));
            }
            if !seen.insert(dependency.clone()) {
                return Err(format!(
                    "node \"{}\" repeats dependency \"{dependency}\"",
                    node.id
                ));
            }
            *indegree.get_mut(&node.id).unwrap() += 1;
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(node.id.clone());
        }
    }
    Ok(GraphData {
        nodes,
        indegree,
        dependents,
    })
}

pub fn validate_graph(doc: &Document) -> Result<(), String> {
    let graph = build_graph(doc)?;
    let mut indegree = graph.indegree.clone();
    let mut queue: Vec<String> = indegree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(id, _)| id.clone())
        .collect();
    queue.sort();
    let mut processed = 0usize;
    let mut i = 0usize;
    while i < queue.len() {
        let id = queue[i].clone();
        i += 1;
        processed += 1;
        if let Some(dependents) = graph.dependents.get(&id) {
            let mut newly_ready = Vec::new();
            for dependent in dependents {
                let degree = indegree.get_mut(dependent).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    newly_ready.push(dependent.clone());
                }
            }
            queue.extend(newly_ready);
        }
    }
    if processed != graph.nodes.len() {
        return Err("plan graph contains a cycle".to_string());
    }
    Ok(())
}

pub fn topological_levels(doc: &Document) -> Result<Vec<Vec<Node>>, String> {
    let graph = build_graph(doc)?;
    let mut indegree = graph.indegree;
    let mut levels = Vec::new();
    let mut remaining = graph.nodes.len();
    while remaining > 0 {
        let mut level_ids: Vec<String> = indegree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(id, _)| id.clone())
            .collect();
        if level_ids.is_empty() {
            return Err("plan graph contains a cycle".to_string());
        }
        level_ids.sort();
        let mut level = Vec::with_capacity(level_ids.len());
        for id in &level_ids {
            level.push(graph.nodes.get(id).unwrap().clone());
            indegree.remove(id);
            remaining -= 1;
        }
        for id in &level_ids {
            if let Some(dependents) = graph.dependents.get(id) {
                for dependent in dependents {
                    if let Some(degree) = indegree.get_mut(dependent) {
                        *degree -= 1;
                    }
                }
            }
        }
        levels.push(level);
    }
    Ok(levels)
}

pub fn reverse_topological_nodes(doc: &Document) -> Result<Vec<Node>, String> {
    let levels = topological_levels(doc)?;
    let mut result = Vec::with_capacity(doc.nodes.len());
    for level in levels.into_iter().rev() {
        for node in level.into_iter().rev() {
            result.push(node);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::{Document, Node};

    fn doc_with(nodes: Vec<(&str, Vec<&str>)>) -> Document {
        Document {
            nodes: nodes
                .into_iter()
                .map(|(id, deps)| Node {
                    id: id.to_string(),
                    depends_on: deps.into_iter().map(str::to_string).collect(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn detects_cycle() {
        let doc = doc_with(vec![("a", vec!["b"]), ("b", vec!["a"])]);
        assert!(validate_graph(&doc).is_err());
    }

    #[test]
    fn accepts_acyclic_graph() {
        let doc = doc_with(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["a", "b"])]);
        assert!(validate_graph(&doc).is_ok());
        let levels = topological_levels(&doc).unwrap();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0][0].id, "a");
    }

    #[test]
    fn rejects_duplicate_node_ids() {
        let doc = doc_with(vec![("a", vec![]), ("a", vec![])]);
        assert!(validate_graph(&doc).is_err());
    }

    #[test]
    fn rejects_unknown_dependency() {
        let doc = doc_with(vec![("a", vec!["ghost"])]);
        assert!(validate_graph(&doc).is_err());
    }

    #[test]
    fn reverse_topological_order_is_fully_reversed() {
        let doc = doc_with(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["a", "b"])]);
        let reversed = reverse_topological_nodes(&doc).unwrap();
        let ids: Vec<&str> = reversed.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }
}
