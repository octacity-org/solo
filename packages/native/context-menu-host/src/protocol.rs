use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MenuItem {
    Separator {
        r#type: Separator,
    },
    Action {
        id: String,
        label: String,
        #[serde(default)]
        disabled: bool,
        #[serde(default)]
        checked: bool,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Separator {
    Separator,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MenuRequest {
    pub x: f64,
    pub y: f64,
    pub items: Vec<MenuItem>,
    /// Automation drives this menu: build it, but never open it.
    #[serde(default)]
    pub scripted: bool,
}

pub fn parse_request(json: &str) -> Result<MenuRequest, String> {
    let request: MenuRequest = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if !request.x.is_finite() || !request.y.is_finite() {
        return Err("Menu coordinates must be finite".into());
    }
    let mut ids = HashSet::new();
    for item in &request.items {
        if let MenuItem::Action { id, label, .. } = item {
            if id.len() > 1024 || label.len() > 4096 {
                return Err(
                    "Menu IDs must fit 1024 UTF-8 bytes and labels 4096 UTF-8 bytes".into(),
                );
            }
            if id.trim().is_empty()
                || label.trim().is_empty()
                || id.contains('\0')
                || label.contains('\0')
            {
                return Err("Menu IDs and labels must be non-empty text".into());
            }
            if !ids.insert(id) {
                return Err("Duplicate menu ID".into());
            }
        }
    }
    if ids.is_empty() || request.items.len() > 256 {
        return Err("Menu must contain an action and at most 256 entries".into());
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_action_states_and_screen_coordinates() {
        let request = parse_request(r#"{"x":-100,"y":350,"items":[{"id":"read","label":"Mark read","checked":true},{"type":"separator"},{"id":"delete","label":"Delete","disabled":true}]}"#).unwrap();
        assert_eq!(request.x, -100.0);
        assert!(matches!(
            &request.items[0],
            MenuItem::Action { checked: true, .. }
        ));
        assert!(matches!(&request.items[1], MenuItem::Separator { .. }));
        assert!(matches!(
            &request.items[2],
            MenuItem::Action { disabled: true, .. }
        ));
    }

    #[test]
    fn rejects_ambiguous_or_invalid_actions() {
        for items in [
            r#"[]"#,
            r#"[{"type":"separator"}]"#,
            r#"[{"id":"x","label":"One"},{"id":"x","label":"Two"}]"#,
            r#"[{"id":"x","label":" "}]"#,
            r#"[{"id":"x","label":"One","disabled":"true"}]"#,
        ] {
            assert!(parse_request(&format!(r#"{{"x":1,"y":2,"items":{items}}}"#)).is_err());
        }
    }

    #[test]
    fn bounds_menu_strings_so_results_cannot_fill_the_output_pipe() {
        for (id, label) in [
            ("x".repeat(1025), "Read".into()),
            ("read".into(), "x".repeat(4097)),
        ] {
            let json =
                serde_json::json!({ "x": 1, "y": 2, "items": [{ "id": id, "label": label }] });
            assert!(parse_request(&json.to_string()).is_err());
        }
    }
}
