use crate::ra2_protocol::{Ra2Command, Ra2Event};
use crate::savant_client::{SavantEvent, SavantRequest};
use crate::savant_id_map::SavantIdMap;

/// Translate an RA2 command into a Savant request.
pub fn ra2_to_savant(cmd: &Ra2Command, map: &SavantIdMap) -> Option<SavantRequest> {
    match cmd {
        Ra2Command::SetOutput { id, level, .. } => {
            let (address, load_offset) = map.ra2_to_savant(*id)?;
            Some(SavantRequest::SetLoad {
                address: address.to_string(),
                load_offset,
                level: *level,
            })
        }
        Ra2Command::QueryOutput { id } => {
            let (address, load_offset) = map.ra2_to_savant(*id)?;
            Some(SavantRequest::QueryLoad {
                address: address.to_string(),
                load_offset,
            })
        }
        Ra2Command::Monitoring { .. } => None,
    }
}

/// Translate a Savant event into an RA2 event.
pub fn savant_to_ra2(event: &SavantEvent, map: &SavantIdMap) -> Option<Ra2Event> {
    match event {
        SavantEvent::LoadLevel {
            address,
            load_offset,
            level,
        } => {
            let ra2_id = map.savant_to_ra2(address, *load_offset)?;
            Some(Ra2Event::OutputLevel {
                id: ra2_id,
                level: *level,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SavantZoneMapping;

    fn test_map() -> SavantIdMap {
        SavantIdMap::from_zones(&[
            SavantZoneMapping {
                ra2_id: 200,
                address: "001".to_string(),
                load_offset: 0,
                name: "Kitchen Light".to_string(),
                room: "Kitchen".to_string(),
            },
            SavantZoneMapping {
                ra2_id: 201,
                address: "002".to_string(),
                load_offset: 1,
                name: "Bedroom Light".to_string(),
                room: "Bedroom".to_string(),
            },
        ])
    }

    #[test]
    fn translate_set_output() {
        let map = test_map();
        let cmd = Ra2Command::SetOutput {
            id: 200,
            level: 75.0,
            fade: None,
        };
        let req = ra2_to_savant(&cmd, &map).unwrap();
        match req {
            SavantRequest::SetLoad {
                address,
                load_offset,
                level,
            } => {
                assert_eq!(address, "001");
                assert_eq!(load_offset, 0);
                assert_eq!(level, 75.0);
            }
            _ => panic!("Expected SetLoad"),
        }
    }

    #[test]
    fn translate_query_output() {
        let map = test_map();
        let cmd = Ra2Command::QueryOutput { id: 201 };
        let req = ra2_to_savant(&cmd, &map).unwrap();
        match req {
            SavantRequest::QueryLoad {
                address,
                load_offset,
            } => {
                assert_eq!(address, "002");
                assert_eq!(load_offset, 1);
            }
            _ => panic!("Expected QueryLoad"),
        }
    }

    #[test]
    fn translate_load_level_event() {
        let map = test_map();
        let event = SavantEvent::LoadLevel {
            address: "001".to_string(),
            load_offset: 0,
            level: 50.0,
        };
        let ra2 = savant_to_ra2(&event, &map).unwrap();
        assert_eq!(
            ra2,
            Ra2Event::OutputLevel {
                id: 200,
                level: 50.0,
            }
        );
    }

    #[test]
    fn unknown_id_returns_none() {
        let map = test_map();
        let cmd = Ra2Command::SetOutput {
            id: 999,
            level: 50.0,
            fade: None,
        };
        assert!(ra2_to_savant(&cmd, &map).is_none());
    }

    #[test]
    fn monitoring_returns_none() {
        let map = test_map();
        let cmd = Ra2Command::Monitoring {
            mon_type: 5,
            enable: true,
        };
        assert!(ra2_to_savant(&cmd, &map).is_none());
    }

    #[test]
    fn unknown_savant_event_returns_none() {
        let map = test_map();
        let event = SavantEvent::LoadLevel {
            address: "099".to_string(),
            load_offset: 0,
            level: 100.0,
        };
        assert!(savant_to_ra2(&event, &map).is_none());
    }
}
