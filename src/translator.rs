use crate::id_map::IdMap;
use crate::leap_client::{LeapEvent, LeapHeader, LeapRequest};
use crate::ra2_protocol::{Ra2Command, Ra2Event};

/// Translate an RA2 command into a LEAP request.
pub fn ra2_to_leap(cmd: &Ra2Command, map: &IdMap) -> Option<LeapRequest> {
    match cmd {
        Ra2Command::SetOutput { id, level, fade } => {
            let href = map.ra2_to_leap(*id)?;
            let url = format!("{}/commandprocessor", href);

            let body = if let Some(fade_time) = fade {
                let fade_str = format!("{:02}:{:02}:{:02}",
                    (*fade_time as u64) / 3600,
                    ((*fade_time as u64) % 3600) / 60,
                    (*fade_time as u64) % 60,
                );
                serde_json::json!({
                    "Command": {
                        "CommandType": "GoToDimmedLevel",
                        "DimmedLevelParameters": {
                            "Level": level,
                            "FadeTime": fade_str,
                        }
                    }
                })
            } else {
                serde_json::json!({
                    "Command": {
                        "CommandType": "GoToLevel",
                        "Parameter": [{"Type": "Level", "Value": level}]
                    }
                })
            };

            Some(LeapRequest {
                communique_type: "CreateRequest".to_string(),
                header: LeapHeader {
                    url,
                    client_tag: None,
                    extra: serde_json::Map::new(),
                },
                body: Some(body),
            })
        }
        Ra2Command::QueryOutput { id } => {
            let href = map.ra2_to_leap(*id)?;
            let url = format!("{}/status", href);

            Some(LeapRequest {
                communique_type: "ReadRequest".to_string(),
                header: LeapHeader {
                    url,
                    client_tag: None,
                    extra: serde_json::Map::new(),
                },
                body: None,
            })
        }
        Ra2Command::Monitoring { .. } => {
            // Monitoring commands are handled locally (we already subscribe to all zone events)
            None
        }
    }
}

/// Translate a LEAP event into an RA2 event.
pub fn leap_to_ra2(event: &LeapEvent, map: &IdMap) -> Option<Ra2Event> {
    let zone_status = event.body.get("ZoneStatus")?;
    let level = zone_status.get("Level")?.as_f64()?;

    // Extract zone href from ZoneStatus body or parse from header URL
    let href_owned;
    let href: &str = if let Some(h) = zone_status
        .get("Zone")
        .and_then(|z| z.get("href"))
        .and_then(|h| h.as_str())
    {
        h
    } else {
        let url = &event.header.url;
        let parts: Vec<&str> = url.split('/').collect();
        // URL like "/zone/5/status" → parts = ["", "zone", "5", "status"]
        if parts.len() >= 3 && parts[1] == "zone" {
            href_owned = format!("/{}/{}", parts[1], parts[2]);
            &href_owned
        } else {
            return None;
        }
    };

    let ra2_id = map.leap_to_ra2(href)?;

    Some(Ra2Event::OutputLevel {
        id: ra2_id,
        level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ZoneMapping;

    fn test_map() -> IdMap {
        IdMap::from_zones(&[
            ZoneMapping {
                ra2_id: 1,
                leap_href: "/zone/5".to_string(),
                name: "Kitchen".to_string(),
            },
            ZoneMapping {
                ra2_id: 2,
                leap_href: "/zone/8".to_string(),
                name: "Living Room".to_string(),
            },
        ])
    }

    #[test]
    fn translate_set_output() {
        let map = test_map();
        let cmd = Ra2Command::SetOutput {
            id: 1,
            level: 75.0,
            fade: None,
        };
        let req = ra2_to_leap(&cmd, &map).unwrap();
        assert_eq!(req.communique_type, "CreateRequest");
        assert_eq!(req.header.url, "/zone/5/commandprocessor");
        let body = req.body.unwrap();
        assert_eq!(body["Command"]["CommandType"], "GoToLevel");
        assert_eq!(body["Command"]["Parameter"][0]["Value"], 75.0);
    }

    #[test]
    fn translate_query_output() {
        let map = test_map();
        let cmd = Ra2Command::QueryOutput { id: 2 };
        let req = ra2_to_leap(&cmd, &map).unwrap();
        assert_eq!(req.communique_type, "ReadRequest");
        assert_eq!(req.header.url, "/zone/8/status");
    }

    #[test]
    fn translate_zone_status_event() {
        let map = test_map();
        let event = LeapEvent {
            communique_type: "ReadResponse".to_string(),
            header: crate::leap_client::LeapEventHeader {
                url: "/zone/5/status".to_string(),
                status_code: Some("200".to_string()),
                extra: serde_json::Map::new(),
            },
            body: serde_json::json!({
                "ZoneStatus": {
                    "Level": 100.0,
                    "Zone": {"href": "/zone/5"}
                }
            }),
        };
        let ra2 = leap_to_ra2(&event, &map).unwrap();
        assert_eq!(
            ra2,
            Ra2Event::OutputLevel {
                id: 1,
                level: 100.0
            }
        );
    }

    #[test]
    fn unknown_id_returns_none() {
        let map = test_map();
        let cmd = Ra2Command::SetOutput {
            id: 99,
            level: 50.0,
            fade: None,
        };
        assert!(ra2_to_leap(&cmd, &map).is_none());
    }
}
