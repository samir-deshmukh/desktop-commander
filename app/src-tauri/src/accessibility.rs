//! Read-only Linux accessibility-tree inspection over AT-SPI2.
//!
//! This module never enables the desktop accessibility bus and never invokes
//! Action, Component input, keyboard, pointer, or clipboard interfaces. Each
//! inspection is a one-shot snapshot with strict depth and node limits.

use atspi_connection::{AccessibilityConnection, read_session_accessibility};
use atspi_proxies::accessible::ObjectRefExt;
use serde::Serialize;
use std::collections::VecDeque;

const MAX_ACCESSIBLE_APPS: usize = 50;
const MAX_TREE_NODES: usize = 200;
const MAX_TREE_DEPTH: usize = 8;
const MAX_NAME_CHARS: usize = 160;

#[derive(Debug, Serialize)]
pub(crate) struct AccessibleApp {
    pub name: String,
    pub role: String,
    pub child_count: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AccessibleAppList {
    pub apps: Vec<AccessibleApp>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AccessibilityNode {
    pub depth: usize,
    pub name: String,
    pub role: String,
    pub child_count: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AccessibilityTree {
    pub app_name: String,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub nodes: Vec<AccessibilityNode>,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
struct AccessibleObjectRef {
    bus_name: String,
    object_path: String,
}

fn bounded_name(value: String) -> String {
    value.chars().take(MAX_NAME_CHARS).collect()
}

fn bounded_tree_limits(
    requested_depth: Option<usize>,
    requested_nodes: Option<usize>,
) -> (usize, usize) {
    (
        requested_depth
            .unwrap_or(MAX_TREE_DEPTH)
            .clamp(1, MAX_TREE_DEPTH),
        requested_nodes
            .unwrap_or(MAX_TREE_NODES)
            .clamp(1, MAX_TREE_NODES),
    )
}

async fn connect_to_accessibility() -> Result<AccessibilityConnection, String> {
    let enabled = read_session_accessibility()
        .await
        .map_err(|error| format!("Cannot read Linux accessibility status: {error}"))?;
    if !enabled {
        return Err(
            "Linux desktop accessibility is disabled. Enable toolkit accessibility in your desktop settings, then retry. Sam's Desktop Commander does not change this system setting.".into(),
        );
    }

    AccessibilityConnection::new()
        .await
        .map_err(|error| format!("Cannot connect to the Linux accessibility bus: {error}"))
}

pub(crate) async fn list_accessible_apps() -> Result<AccessibleAppList, String> {
    let connection = connect_to_accessibility().await?;
    let root = connection
        .root_accessible_on_registry()
        .await
        .map_err(|error| format!("Cannot read the accessibility registry: {error}"))?;
    let references = root
        .get_children()
        .await
        .map_err(|error| format!("Cannot enumerate accessible applications: {error}"))?;
    let truncated = references.len() > MAX_ACCESSIBLE_APPS;
    let mut apps = Vec::with_capacity(references.len().min(MAX_ACCESSIBLE_APPS));

    for reference in references.into_iter().take(MAX_ACCESSIBLE_APPS) {
        if reference.is_null() {
            continue;
        }
        let proxy = match reference.as_accessible_proxy(connection.connection()).await {
            Ok(proxy) => proxy,
            Err(_) => continue,
        };
        let name = proxy.name().await.unwrap_or_default();
        let role = proxy.get_role_name().await.unwrap_or_default();
        let child_count = proxy
            .child_count()
            .await
            .ok()
            .map(|count| count.max(0) as usize);
        apps.push(AccessibleApp {
            name: bounded_name(name),
            role: bounded_name(role),
            child_count,
        });
    }

    Ok(AccessibleAppList { apps, truncated })
}

pub(crate) async fn read_accessibility_tree(
    app_name: &str,
    requested_depth: Option<usize>,
    requested_nodes: Option<usize>,
) -> Result<AccessibilityTree, String> {
    let app_name = app_name.trim();
    if app_name.is_empty() {
        return Err("Application name must not be empty".into());
    }
    if app_name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "Application name exceeds {MAX_NAME_CHARS} characters"
        ));
    }

    let (max_depth, max_nodes) = bounded_tree_limits(requested_depth, requested_nodes);

    let connection = connect_to_accessibility().await?;
    let root = connection
        .root_accessible_on_registry()
        .await
        .map_err(|error| format!("Cannot read the accessibility registry: {error}"))?;
    let references = root
        .get_children()
        .await
        .map_err(|error| format!("Cannot enumerate accessible applications: {error}"))?;

    let mut matches = Vec::new();
    for reference in references {
        if reference.is_null() {
            continue;
        }
        let proxy = match reference.as_accessible_proxy(connection.connection()).await {
            Ok(proxy) => proxy,
            Err(_) => continue,
        };
        let name = proxy.name().await.unwrap_or_default();
        if bounded_name(name) == app_name {
            let bus_name = reference
                .name_as_str()
                .ok_or("Accessible application has no D-Bus name")?
                .to_owned();
            matches.push(AccessibleObjectRef {
                bus_name,
                object_path: reference.path_as_str().to_owned(),
            });
        }
    }

    if matches.is_empty() {
        return Err(format!(
            "Accessible application '{app_name}' was not found. Call list_accessible_apps first."
        ));
    }
    if matches.len() > 1 {
        return Err(format!(
            "More than one accessible application is named '{app_name}'. Use a unique application name."
        ));
    }

    let mut pending = VecDeque::from([(matches.remove(0), 0usize)]);
    let mut nodes = Vec::with_capacity(max_nodes.min(64));
    let mut truncated = false;
    let mut warnings = Vec::new();

    while let Some((reference, depth)) = pending.pop_front() {
        if nodes.len() >= max_nodes {
            truncated = true;
            break;
        }
        let proxy = match zbus::Proxy::new(
            connection.connection(),
            reference.bus_name.as_str(),
            reference.object_path.as_str(),
            "org.a11y.atspi.Accessible",
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(_) => {
                truncated = true;
                if warnings.len() < 20 {
                    warnings
                        .push("An accessible element disappeared or could not be opened.".into());
                }
                continue;
            }
        };

        let name = proxy
            .get_property::<String>("Name")
            .await
            .unwrap_or_default();
        let role = proxy
            .call::<_, _, String>("GetRoleName", &())
            .await
            .unwrap_or_default();
        let child_count = proxy
            .get_property::<i32>("ChildCount")
            .await
            .ok()
            .map(|count| count.max(0) as usize);

        nodes.push(AccessibilityNode {
            depth,
            name: bounded_name(name),
            role: bounded_name(role),
            child_count,
        });

        if depth >= max_depth {
            if child_count.unwrap_or(0) > 0 {
                truncated = true;
            }
            continue;
        }

        let Some(child_count) = child_count else {
            truncated = true;
            if warnings.len() < 20 {
                warnings.push("An element's child count could not be read.".into());
            }
            continue;
        };
        let remaining_slots = max_nodes.saturating_sub(nodes.len() + pending.len());
        let children_to_read = child_count.min(remaining_slots);
        if children_to_read < child_count {
            truncated = true;
        }
        for index in 0..children_to_read {
            let child = proxy
                .call::<_, _, (String, zbus::zvariant::OwnedObjectPath)>(
                    "GetChildAtIndex",
                    &(index as i32),
                )
                .await;
            match child {
                Ok((bus_name, object_path))
                    if !bus_name.is_empty() && object_path.as_str() != "/org/a11y/atspi/null" =>
                {
                    pending.push_back((
                        AccessibleObjectRef {
                            bus_name,
                            object_path: object_path.as_str().to_owned(),
                        },
                        depth + 1,
                    ));
                }
                Ok(_) => {
                    truncated = true;
                }
                Err(error) => {
                    truncated = true;
                    if warnings.len() < 20 {
                        warnings.push(format!("Accessible child retrieval failed: {error}"));
                    }
                }
            }
        }
    }

    if !pending.is_empty() {
        truncated = true;
    }

    Ok(AccessibilityTree {
        app_name: bounded_name(app_name.to_owned()),
        max_depth,
        max_nodes,
        nodes,
        truncated,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_names_are_bounded_without_splitting_utf8_codepoints() {
        let value = format!("{}{}", "a".repeat(MAX_NAME_CHARS - 1), "🖥️");
        let bounded = bounded_name(value);
        assert!(bounded.chars().count() <= MAX_NAME_CHARS);
        assert_eq!(bounded.chars().count(), MAX_NAME_CHARS);
        assert!(bounded.ends_with('🖥'));
    }

    #[test]
    fn requested_tree_limits_are_clamped_to_safe_bounds() {
        assert_eq!(
            bounded_tree_limits(None, None),
            (MAX_TREE_DEPTH, MAX_TREE_NODES)
        );
        assert_eq!(bounded_tree_limits(Some(0), Some(0)), (1, 1));
        assert_eq!(
            bounded_tree_limits(Some(usize::MAX), Some(usize::MAX)),
            (MAX_TREE_DEPTH, MAX_TREE_NODES)
        );
    }
}
