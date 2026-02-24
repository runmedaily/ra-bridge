/// RadioRA 2 integration protocol types and parser.

#[derive(Debug, Clone, PartialEq)]
pub enum Ra2Command {
    /// #OUTPUT,<id>,1,<level>[,<fade>]
    SetOutput {
        id: u32,
        level: f64,
        fade: Option<f64>,
    },
    /// ?OUTPUT,<id>,1
    QueryOutput { id: u32 },
    /// #MONITORING,<type>,<action>  (action: 1=enable, 2=disable)
    Monitoring { mon_type: u32, enable: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ra2Event {
    /// ~OUTPUT,<id>,1,<level>
    OutputLevel { id: u32, level: f64 },
}

/// Parse a line from a telnet client into an RA2 command.
pub fn parse_command(line: &str) -> Option<Ra2Command> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let prefix = line.as_bytes()[0];
    let rest = &line[1..];
    let parts: Vec<&str> = rest.split(',').collect();

    match prefix {
        b'#' => parse_action(&parts),
        b'?' => parse_query(&parts),
        _ => None,
    }
}

fn parse_action(parts: &[&str]) -> Option<Ra2Command> {
    if parts.is_empty() {
        return None;
    }

    match parts[0].to_uppercase().as_str() {
        "OUTPUT" => {
            // #OUTPUT,<id>,1,<level>[,<fade>]
            if parts.len() < 4 {
                return None;
            }
            let id: u32 = parts[1].trim().parse().ok()?;
            // parts[2] is action number (1 = set level)
            let action: u32 = parts[2].trim().parse().ok()?;
            if action != 1 {
                return None; // v1: only support action 1 (set level)
            }
            let level: f64 = parts[3].trim().parse().ok()?;
            let fade = if parts.len() >= 5 {
                parts[4].trim().parse().ok()
            } else {
                None
            };
            Some(Ra2Command::SetOutput { id, level, fade })
        }
        "MONITORING" => {
            // #MONITORING,<type>,<action>
            if parts.len() < 3 {
                return None;
            }
            let mon_type: u32 = parts[1].trim().parse().ok()?;
            let action: u32 = parts[2].trim().parse().ok()?;
            Some(Ra2Command::Monitoring {
                mon_type,
                enable: action == 1,
            })
        }
        _ => None,
    }
}

fn parse_query(parts: &[&str]) -> Option<Ra2Command> {
    if parts.is_empty() {
        return None;
    }

    match parts[0].to_uppercase().as_str() {
        "OUTPUT" => {
            // ?OUTPUT,<id>,1
            if parts.len() < 3 {
                return None;
            }
            let id: u32 = parts[1].trim().parse().ok()?;
            Some(Ra2Command::QueryOutput { id })
        }
        _ => None,
    }
}

/// Format an RA2 event as a protocol line (without trailing \r\n).
pub fn format_event(event: &Ra2Event) -> String {
    match event {
        Ra2Event::OutputLevel { id, level } => {
            format!("~OUTPUT,{},1,{:.2}", id, level)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_set_output() {
        assert_eq!(
            parse_command("#OUTPUT,1,1,75.5"),
            Some(Ra2Command::SetOutput {
                id: 1,
                level: 75.5,
                fade: None,
            })
        );
    }

    #[test]
    fn parse_set_output_with_fade() {
        assert_eq!(
            parse_command("#OUTPUT,2,1,100,3.5"),
            Some(Ra2Command::SetOutput {
                id: 2,
                level: 100.0,
                fade: Some(3.5),
            })
        );
    }

    #[test]
    fn parse_query_output() {
        assert_eq!(
            parse_command("?OUTPUT,1,1"),
            Some(Ra2Command::QueryOutput { id: 1 })
        );
    }

    #[test]
    fn parse_monitoring() {
        assert_eq!(
            parse_command("#MONITORING,5,1"),
            Some(Ra2Command::Monitoring {
                mon_type: 5,
                enable: true,
            })
        );
    }

    #[test]
    fn format_output_level() {
        let event = Ra2Event::OutputLevel {
            id: 1,
            level: 100.0,
        };
        assert_eq!(format_event(&event), "~OUTPUT,1,1,100.00");
    }
}
