use crate::world::DoormanWorld;
use cucumber::then;

/// NotificationResponse body: Int32 backend pid, CString channel, CString payload.
fn parse_notification(body: &[u8]) -> Option<(i32, String, String)> {
    if body.len() < 5 {
        return None;
    }
    let pid = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    let rest = &body[4..];
    let channel_end = rest.iter().position(|&b| b == 0)?;
    let channel = String::from_utf8_lossy(&rest[..channel_end]).to_string();
    let payload = &rest[channel_end + 1..];
    let payload_end = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    let payload = String::from_utf8_lossy(&payload[..payload_end]).to_string();
    Some((pid, channel, payload))
}

fn notification_channels(world: &DoormanWorld, session: &str) -> Vec<String> {
    world
        .session_messages
        .get(session)
        .unwrap_or_else(|| panic!("Session {session}: no stored response"))
        .iter()
        .filter(|(tag, _)| *tag == 'A')
        .filter_map(|(_, body)| parse_notification(body).map(|(_, channel, _)| channel))
        .collect()
}

#[then(regex = r#"^session "([^"]+)" should receive NotificationResponse for channel "([^"]+)"$"#)]
pub async fn should_receive_notification(
    world: &mut DoormanWorld,
    session: String,
    channel: String,
) {
    let channels = notification_channels(world, &session);
    assert!(
        channels.iter().any(|c| *c == channel),
        "Session {session}: expected NotificationResponse for channel {channel:?}, got {channels:?}"
    );
}

#[then(
    regex = r#"^session "([^"]+)" should not receive NotificationResponse for channel "([^"]+)"$"#
)]
pub async fn should_not_receive_notification(
    world: &mut DoormanWorld,
    session: String,
    channel: String,
) {
    let channels = notification_channels(world, &session);
    assert!(
        !channels.iter().any(|c| *c == channel),
        "Session {session}: unexpected NotificationResponse for channel {channel:?}, got {channels:?}"
    );
}
