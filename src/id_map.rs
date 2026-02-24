use std::collections::HashMap;

use crate::config::ZoneMapping;

/// Bidirectional map between RA2 integer IDs and LEAP zone hrefs.
pub struct IdMap {
    ra2_to_leap: HashMap<u32, String>,
    leap_to_ra2: HashMap<String, u32>,
}

impl IdMap {
    pub fn from_zones(zones: &[ZoneMapping]) -> Self {
        let mut ra2_to_leap = HashMap::new();
        let mut leap_to_ra2 = HashMap::new();
        for z in zones {
            ra2_to_leap.insert(z.ra2_id, z.leap_href.clone());
            leap_to_ra2.insert(z.leap_href.clone(), z.ra2_id);
        }
        Self {
            ra2_to_leap,
            leap_to_ra2,
        }
    }

    pub fn ra2_to_leap(&self, id: u32) -> Option<&str> {
        self.ra2_to_leap.get(&id).map(|s| s.as_str())
    }

    pub fn leap_to_ra2(&self, href: &str) -> Option<u32> {
        self.leap_to_ra2.get(href).copied()
    }
}
