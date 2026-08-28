use crate::LoadOrder;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::path::Path;

#[derive(Debug)]
pub struct ModsConfig {
    pub version: Option<String>,
    pub active_mods: Vec<String>,
}

impl ModsConfig {
    pub fn load_order(&self) -> LoadOrder {
        LoadOrder::new(self.active_mods.clone())
    }
}

pub fn parse_mods_config(path: &Path) -> Result<ModsConfig, Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(path)?;
    parse_mods_config_str(&xml)
}

fn parse_mods_config_str(xml: &str) -> Result<ModsConfig, Box<dyn std::error::Error>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut version: Option<String> = None;
    let mut active_mods: Vec<String> = Vec::new();

    let mut buf = Vec::new();
    let mut current_tag = String::new();
    let mut in_active_mods = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                current_tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if current_tag == "activeMods" {
                    in_active_mods = true;
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "activeMods" {
                    in_active_mods = false;
                }
                current_tag.clear();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape()?.trim().to_string();
                if text.is_empty() {
                    continue;
                }

                if current_tag == "version" {
                    version = Some(text);
                } else if in_active_mods && current_tag == "li" {
                    // Lowercase package IDs for case-insensitive matching
                    active_mods.push(text.to_lowercase());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(format!(
                    "XML parse error at position {}: {}",
                    reader.buffer_position(),
                    e
                )
                .into());
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(ModsConfig {
        version,
        active_mods,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mods_config_basic() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ModsConfigData>
    <version>1.5</version>
    <activeMods>
        <li>ludeon.rimworld</li>
        <li>test.mod.a</li>
    </activeMods>
</ModsConfigData>"#;

        let config = parse_mods_config_str(xml).unwrap();
        assert_eq!(config.version, Some("1.5".into()));
        assert_eq!(config.active_mods, vec!["ludeon.rimworld", "test.mod.a"]);
    }

    #[test]
    fn load_order_from_config() {
        let xml = r#"<ModsConfigData>
    <activeMods>
        <li>core</li><li>mod.a</li><li>mod.b</li>
    </activeMods>
</ModsConfigData>"#;

        let config = parse_mods_config_str(xml).unwrap();
        let order = config.load_order();
        assert_eq!(order.position("mod.b"), Some(2));
        assert_eq!(order.is_after("mod.b", "mod.a"), Some(true));
    }

    #[test]
    fn parse_mods_config_no_version() {
        let xml = r#"<ModsConfigData>
    <activeMods>
        <li>core</li>
    </activeMods>
</ModsConfigData>"#;

        let config = parse_mods_config_str(xml).unwrap();
        assert_eq!(config.version, None);
        assert_eq!(config.active_mods, vec!["core"]);
    }

    #[test]
    fn parse_mods_config_empty_active_mods() {
        let xml = r#"<ModsConfigData>
    <version>1.5</version>
    <activeMods>
    </activeMods>
</ModsConfigData>"#;

        let config = parse_mods_config_str(xml).unwrap();
        assert_eq!(config.version, Some("1.5".into()));
        assert_eq!(config.active_mods, Vec::<String>::new());
    }

    #[test]
    fn parse_mods_config_case_normalization() {
        let xml = r#"<ModsConfigData>
    <activeMods>
        <li>Ludeon.RimWorld</li>
        <li>Test.Mod.A</li>
    </activeMods>
</ModsConfigData>"#;

        let config = parse_mods_config_str(xml).unwrap();
        // Package IDs should be lowercased
        assert_eq!(config.active_mods, vec!["ludeon.rimworld", "test.mod.a"]);
    }
}
