use std::collections::HashMap;

use crate::config::SavantZoneMapping;

/// Bidirectional map between RA2 integer IDs and Savant (address, load_offset) pairs.
pub struct SavantIdMap {
    ra2_to_savant: HashMap<u32, (String, usize)>,
    savant_to_ra2: HashMap<(String, usize), u32>,
}

impl SavantIdMap {
    pub fn from_zones(zones: &[SavantZoneMapping]) -> Self {
        let mut ra2_to_savant = HashMap::new();
        let mut savant_to_ra2 = HashMap::new();
        for z in zones {
            ra2_to_savant.insert(z.ra2_id, (z.address.clone(), z.load_offset));
            savant_to_ra2.insert((z.address.clone(), z.load_offset), z.ra2_id);
        }
        Self {
            ra2_to_savant,
            savant_to_ra2,
        }
    }

    pub fn ra2_to_savant(&self, id: u32) -> Option<(&str, usize)> {
        self.ra2_to_savant
            .get(&id)
            .map(|(addr, off)| (addr.as_str(), *off))
    }

    pub fn savant_to_ra2(&self, address: &str, load_offset: usize) -> Option<u32> {
        self.savant_to_ra2
            .get(&(address.to_string(), load_offset))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_zones() -> Vec<SavantZoneMapping> {
        vec![
            SavantZoneMapping {
                ra2_id: 200,
                address: "001".to_string(),
                load_offset: 0,
                name: "Kitchen Light".to_string(),
                room: "Kitchen".to_string(),
            },
            SavantZoneMapping {
                ra2_id: 201,
                address: "001".to_string(),
                load_offset: 1,
                name: "Kitchen Fan".to_string(),
                room: "Kitchen".to_string(),
            },
            SavantZoneMapping {
                ra2_id: 202,
                address: "002".to_string(),
                load_offset: 0,
                name: "Bedroom Light".to_string(),
                room: "Bedroom".to_string(),
            },
        ]
    }

    #[test]
    fn ra2_to_savant_lookup() {
        let map = SavantIdMap::from_zones(&test_zones());
        assert_eq!(map.ra2_to_savant(200), Some(("001", 0)));
        assert_eq!(map.ra2_to_savant(201), Some(("001", 1)));
        assert_eq!(map.ra2_to_savant(202), Some(("002", 0)));
        assert_eq!(map.ra2_to_savant(999), None);
    }

    #[test]
    fn savant_to_ra2_lookup() {
        let map = SavantIdMap::from_zones(&test_zones());
        assert_eq!(map.savant_to_ra2("001", 0), Some(200));
        assert_eq!(map.savant_to_ra2("001", 1), Some(201));
        assert_eq!(map.savant_to_ra2("002", 0), Some(202));
        assert_eq!(map.savant_to_ra2("003", 0), None);
    }
}
