use super::*;

#[tokio::test]
async fn settings_compare_version_broadcast_and_survive_relay_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (navidrome, _) = fake_navidrome().await;
    let relay = start(&navidrome, &dir).await;
    let (mut a, _) = login(&relay, "alice", "a").await;
    let (mut b, _) = login(&relay, "alice", "b").await;
    assert!(matches!(
        next(&mut a).await,
        ServerMessage::DevicesChanged { .. }
    ));
    let first = json!({"dark": true, "gain": "Album"});
    send(
        &mut a,
        ClientMessage::SettingsPut {
            settings: first.clone(),
            base_version: 0,
        },
    )
    .await;
    for socket in [&mut a, &mut b] {
        assert!(
            matches!(next(socket).await, ServerMessage::Settings {settings, version: 1} if settings == first)
        );
    }
    send(
        &mut b,
        ClientMessage::SettingsPut {
            settings: json!({"dark": false}),
            base_version: 0,
        },
    )
    .await;
    assert!(
        matches!(next(&mut b).await, ServerMessage::SettingsConflict {settings, version: 1} if settings == first)
    );
    send(
        &mut a,
        ClientMessage::SettingsPut {
            settings: json!({"dark": true, "lastfm_user": "local only"}),
            base_version: 1,
        },
    )
    .await;
    assert!(
        matches!(next(&mut a).await, ServerMessage::Error {code, ..} if code == "invalid_settings")
    );
    drop(relay);
    let restarted = start(&navidrome, &dir).await;
    let (_, welcome) = login(&restarted, "alice", "a").await;
    assert!(
        matches!(welcome, ServerMessage::Welcome {settings, settings_version: 1, ..} if settings == first)
    );
    let (_, welcome) = login(&restarted, "bob", "a").await;
    assert!(
        matches!(welcome, ServerMessage::Welcome {settings, settings_version: 0, ..} if settings == json!({}))
    );
}
