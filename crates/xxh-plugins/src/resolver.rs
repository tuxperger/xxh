//! Plugin dependency resolution and load ordering (T036).
//!
//! Detects version conflicts and missing dependencies **before** deployment
//! (§FR-021), then produces a deterministic load order: dependencies first, then
//! higher `priority`, then name (§FR-018, C-M). See contracts/plugin-manifest.md.

use std::collections::{BTreeMap, BTreeSet};

use xxh_plugin_api::{Manifest, PluginError};

/// Resolve the set of enabled plugins into a deterministic load order.
///
/// Errors (all class "plugin"): [`PluginError::MissingDependency`],
/// [`PluginError::VersionConflict`], [`PluginError::DependencyCycle`].
pub fn resolve(enabled: &[Manifest]) -> Result<Vec<String>, PluginError> {
    let by_name: BTreeMap<&str, &Manifest> = enabled.iter().map(|m| (m.name.as_str(), m)).collect();

    // 1) Validate dependencies exist and versions satisfy the requirements (§FR-021).
    for m in enabled {
        for (dep, req) in &m.dependencies {
            match by_name.get(dep.as_str()) {
                None => {
                    return Err(PluginError::MissingDependency {
                        by: m.name.clone(),
                        dep: dep.clone(),
                    });
                }
                Some(dm) if !req.matches(&dm.version) => {
                    return Err(PluginError::VersionConflict(format!(
                        "`{}` requires `{dep} {req}` but enabled version is {}",
                        m.name, dm.version
                    )));
                }
                Some(_) => {}
            }
        }
    }

    // 2) Kahn topological sort. Edge: dep -> plugin (dep loads first).
    //    indegree[p] = number of p's dependencies present in the set.
    let mut indegree: BTreeMap<&str, usize> = by_name.keys().map(|n| (*n, 0usize)).collect();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for m in enabled {
        for dep in m.dependencies.keys() {
            if by_name.contains_key(dep.as_str()) {
                *indegree.get_mut(m.name.as_str()).unwrap() += 1;
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(m.name.as_str());
            }
        }
    }

    // Ready set ordered by (priority desc, name asc) for deterministic output.
    let key = |n: &str| {
        let m = by_name[n];
        (std::cmp::Reverse(m.priority), n.to_string())
    };
    let mut ready: BTreeSet<(std::cmp::Reverse<i32>, String)> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| key(n))
        .collect();

    let mut order = Vec::with_capacity(enabled.len());
    while let Some(item) = ready.iter().next().cloned() {
        ready.remove(&item);
        let name = item.1;
        order.push(name.clone());
        if let Some(deps) = dependents.get(name.as_str()) {
            for d in deps {
                let e = indegree.get_mut(*d).unwrap();
                *e -= 1;
                if *e == 0 {
                    ready.insert(key(d));
                }
            }
        }
    }

    if order.len() != enabled.len() {
        // Whatever is left still has an unmet in-edge ⇒ a cycle.
        let stuck = indegree
            .iter()
            .find(|(_, d)| **d > 0)
            .map(|(n, _)| n.to_string())
            .unwrap_or_default();
        return Err(PluginError::DependencyCycle(stuck));
    }

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, ver: &str, prio: i32, deps: &[(&str, &str)]) -> Manifest {
        let mut toml =
            format!("name=\"{name}\"\nversion=\"{ver}\"\napi_version=\"1.0.0\"\npriority={prio}\n");
        if !deps.is_empty() {
            toml.push_str("[dependencies]\n");
            for (d, r) in deps {
                toml.push_str(&format!("{d} = \"{r}\"\n"));
            }
        }
        Manifest::parse(&toml).unwrap()
    }

    #[test]
    fn dependencies_load_before_dependents() {
        let plugins = vec![
            m("theme", "2.1.0", 0, &[]),
            m("highlight", "1.0.0", 0, &[("theme", "^2.0")]),
        ];
        let order = resolve(&plugins).unwrap();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("theme") < pos("highlight"));
    }

    #[test]
    fn missing_dependency_is_rejected() {
        let plugins = vec![m("a", "1.0.0", 0, &[("ghost", "^1")])];
        assert!(matches!(
            resolve(&plugins),
            Err(PluginError::MissingDependency { .. })
        ));
    }

    #[test]
    fn version_conflict_is_detected_before_deploy() {
        let plugins = vec![
            m("theme", "1.5.0", 0, &[]),
            m("a", "1.0.0", 0, &[("theme", "^2.0")]),
        ];
        assert!(matches!(
            resolve(&plugins),
            Err(PluginError::VersionConflict(_))
        ));
    }

    #[test]
    fn cycles_are_reported() {
        let plugins = vec![
            m("a", "1.0.0", 0, &[("b", "*")]),
            m("b", "1.0.0", 0, &[("a", "*")]),
        ];
        assert!(matches!(
            resolve(&plugins),
            Err(PluginError::DependencyCycle(_))
        ));
    }

    #[test]
    fn order_is_deterministic_by_priority_then_name() {
        // No deps: higher priority first, then name ascending.
        let plugins = vec![
            m("zeta", "1.0.0", 10, &[]),
            m("alpha", "1.0.0", 10, &[]),
            m("mid", "1.0.0", 5, &[]),
        ];
        assert_eq!(resolve(&plugins).unwrap(), vec!["alpha", "zeta", "mid"]);
    }
}
