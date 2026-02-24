use indexmap::IndexMap;
use uuid::Uuid;

use crate::config::{SavantZoneMapping, ZoneMapping};

/// Guess RA2 OutputType from zone name.
fn guess_output_type(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.contains("fan") {
        return "NON_DIM";
    }
    if lower.contains("heater") || lower.contains("heat") {
        return "NON_DIM";
    }
    if lower.contains("hot") {
        return "NON_DIM";
    }
    "INC"
}

struct AreaOutput {
    ra2_id: u32,
    output_name: String,
}

/// Generate Lutron RadioRA 2 DbXmlInfo.xml from zone mappings (LEAP + Savant).
pub fn generate_xml(zones: &[ZoneMapping], savant_zones: &[SavantZoneMapping]) -> String {
    // Group zones by area (text before " ─ ")
    let mut areas: IndexMap<String, Vec<AreaOutput>> = IndexMap::new();
    for z in zones {
        let (area_name, output_name) = if let Some(pos) = z.name.find(" \u{2500} ") {
            (
                z.name[..pos].trim().to_string(),
                z.name[pos + " \u{2500} ".len()..].trim().to_string(),
            )
        } else {
            ("Ungrouped".to_string(), z.name.clone())
        };

        areas
            .entry(area_name)
            .or_default()
            .push(AreaOutput {
                ra2_id: z.ra2_id,
                output_name,
            });
    }

    // Add Savant zones — use room as area name
    for z in savant_zones {
        let (area_name, output_name) = if let Some(pos) = z.name.find(" \u{2500} ") {
            (
                z.name[..pos].trim().to_string(),
                z.name[pos + " \u{2500} ".len()..].trim().to_string(),
            )
        } else if !z.room.is_empty() {
            (z.room.clone(), z.name.clone())
        } else {
            ("Savant".to_string(), z.name.clone())
        };

        areas
            .entry(area_name)
            .or_default()
            .push(AreaOutput {
                ra2_id: z.ra2_id,
                output_name,
            });
    }

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n");
    xml.push_str("<Project>\n");

    // GUID
    xml.push_str(&format!("  <GUID>{}</GUID>\n", Uuid::new_v4()));

    // ProjectName
    xml.push_str("  <ProjectName ProjectName=\"RA3 Bridge Import\" />\n");

    // Empty required elements
    xml.push_str("  <Timeclocks />\n");
    xml.push_str("  <GreenMode />\n");
    xml.push_str("  <OccupancyGroups />\n");

    // Areas
    xml.push_str("  <Areas>\n");
    xml.push_str("    <Area Name=\"Root\" IntegrationID=\"1\" IsLeaf=\"false\">\n");
    xml.push_str("      <Areas>\n");

    let mut area_id: u32 = 100;
    for (area_name, outputs) in &areas {
        xml.push_str(&format!(
            "        <Area Name=\"{}\" IntegrationID=\"{}\" IsLeaf=\"true\">\n",
            xml_escape(area_name),
            area_id,
        ));
        area_id += 1;

        xml.push_str("          <Outputs>\n");
        for out in outputs {
            let output_type = guess_output_type(&out.output_name);
            xml.push_str(&format!(
                "            <Output Name=\"{}\" IntegrationID=\"{}\" OutputType=\"{}\" Wattage=\"0\" UUID=\"{}\" />\n",
                xml_escape(&out.output_name),
                out.ra2_id,
                output_type,
                Uuid::new_v4(),
            ));
        }
        xml.push_str("          </Outputs>\n");

        xml.push_str("          <DeviceGroups />\n");
        xml.push_str("          <Scenes />\n");
        xml.push_str("          <ShadeGroups />\n");
        xml.push_str("        </Area>\n");
    }

    xml.push_str("      </Areas>\n");
    xml.push_str("    </Area>\n");
    xml.push_str("  </Areas>\n");
    xml.push_str("</Project>\n");

    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guess_output_type() {
        assert_eq!(guess_output_type("CEILING FAN"), "NON_DIM");
        assert_eq!(guess_output_type("POOL HEATER"), "NON_DIM");
        assert_eq!(guess_output_type("1/2 HOT OUTLET"), "NON_DIM");
        assert_eq!(guess_output_type("KITCHEN LIGHTS"), "INC");
        assert_eq!(guess_output_type("SCONCE"), "INC");
    }

    #[test]
    fn test_generate_xml_basic() {
        let zones = vec![
            ZoneMapping {
                ra2_id: 1,
                leap_href: "/zone/100".to_string(),
                name: "KITCHEN \u{2500} CEILING LIGHTS".to_string(),
            },
            ZoneMapping {
                ra2_id: 2,
                leap_href: "/zone/101".to_string(),
                name: "KITCHEN \u{2500} EXHAUST FAN".to_string(),
            },
            ZoneMapping {
                ra2_id: 3,
                leap_href: "/zone/200".to_string(),
                name: "BEDROOM \u{2500} SCONCE".to_string(),
            },
        ];

        let xml = generate_xml(&zones, &[]);

        // Check structure
        assert!(xml.contains("<?xml version=\"1.0\""));
        assert!(xml.contains("<Project>"));
        assert!(xml.contains("<GUID>"));
        assert!(xml.contains("ProjectName=\"RA3 Bridge Import\""));
        assert!(xml.contains("Name=\"Root\""));

        // Check areas
        assert!(xml.contains("Name=\"KITCHEN\""));
        assert!(xml.contains("Name=\"BEDROOM\""));

        // Check outputs
        assert!(xml.contains("Name=\"CEILING LIGHTS\""));
        assert!(xml.contains("IntegrationID=\"1\""));
        assert!(xml.contains("OutputType=\"INC\""));

        assert!(xml.contains("Name=\"EXHAUST FAN\""));
        assert!(xml.contains("IntegrationID=\"2\""));
        assert!(xml.contains("OutputType=\"NON_DIM\""));

        assert!(xml.contains("Name=\"SCONCE\""));
        assert!(xml.contains("IntegrationID=\"3\""));
    }

    #[test]
    fn test_generate_xml_ungrouped() {
        let zones = vec![ZoneMapping {
            ra2_id: 1,
            leap_href: "/zone/100".to_string(),
            name: "STANDALONE LIGHT".to_string(),
        }];

        let xml = generate_xml(&zones, &[]);
        assert!(xml.contains("Name=\"Ungrouped\""));
        assert!(xml.contains("Name=\"STANDALONE LIGHT\""));
    }

    #[test]
    fn test_generate_xml_with_savant() {
        let zones = vec![ZoneMapping {
            ra2_id: 1,
            leap_href: "/zone/100".to_string(),
            name: "KITCHEN \u{2500} CEILING LIGHTS".to_string(),
        }];
        let savant_zones = vec![SavantZoneMapping {
            ra2_id: 200,
            address: "001".to_string(),
            load_offset: 0,
            name: "LIVING ROOM \u{2500} MAIN LIGHT".to_string(),
            room: "LIVING ROOM".to_string(),
        }];

        let xml = generate_xml(&zones, &savant_zones);
        assert!(xml.contains("Name=\"KITCHEN\""));
        assert!(xml.contains("Name=\"LIVING ROOM\""));
        assert!(xml.contains("IntegrationID=\"200\""));
        assert!(xml.contains("Name=\"MAIN LIGHT\""));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("A & B"), "A &amp; B");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("\"quoted\""), "&quot;quoted&quot;");
    }
}
